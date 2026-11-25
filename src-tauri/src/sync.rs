use chrono::TimeZone;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use crate::db::{Database, DbError, EarningDraft, EarningStats, TransactionType};
use crate::notifications::{
    extract_user_ref, parse_numeric_id, BatchSource, CandidateEvent, NotificationBatch, SaleKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Profile,
    Balances,
    Transactions,
    Chat,
    Subscriber,
    LiveEvent,
}

#[derive(Debug)]
pub struct IngestOutcome {
    pub kind: CaptureKind,
    pub unauthorized: bool,
    pub earnings: Option<EarningStats>,
    pub fan_id: Option<String>,
    pub notifications: NotificationBatch,
}

pub fn classify_url(url: &str) -> Option<CaptureKind> {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return None;
    };
    if parsed.host_str() != Some("onlyfans.com") {
        return None;
    }
    let path = parsed.path();
    if path == "/api2/v2/__cf_live_event" {
        return Some(CaptureKind::LiveEvent);
    }
    if path == "/api2/v2/users/me" {
        return Some(CaptureKind::Profile);
    }
    if path == "/api2/v2/payouts/balances" {
        return Some(CaptureKind::Balances);
    }
    if path == "/api2/v2/payouts/transactions"
        || path.starts_with("/api2/v2/payouts/transactions/")
        || path.starts_with("/api2/v2/earnings/")
    {
        return Some(CaptureKind::Transactions);
    }
    if path == "/api2/v2/chats" {
        return Some(CaptureKind::Subscriber);
    }
    if path == "/api2/v2/users/list" {
        return Some(CaptureKind::Subscriber);
    }
    if path.contains("/api2/v2/chats/") && path.contains("/messages") {
        return Some(CaptureKind::Chat);
    }
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if segments.len() == 4
        && segments[0] == "api2"
        && segments[1] == "v2"
        && segments[2] == "chats"
        && segments[3].chars().all(|c| c.is_ascii_digit())
    {
        return Some(CaptureKind::Chat);
    }
    if path.starts_with("/api2/v2/subscriptions") {
        return Some(CaptureKind::Subscriber);
    }
    let user_id = path.trim_start_matches("/api2/v2/users/");
    if path.starts_with("/api2/v2/users/") && !user_id.is_empty() && user_id.chars().all(|c| c.is_ascii_digit()) {
        return Some(CaptureKind::Subscriber);
    }
    None
}

pub fn ingest_body(
    database: &Database,
    model_id: i64,
    url: &str,
    status: u16,
    body: &str,
) -> Result<Option<IngestOutcome>, DbError> {
    let Some(kind) = classify_url(url) else {
        return Ok(None);
    };
    if status == 401 || status == 403 {
        return Ok(Some(IngestOutcome {
            kind,
            unauthorized: kind == CaptureKind::Profile,
            earnings: None,
            fan_id: None,
            notifications: NotificationBatch::empty(),
        }));
    }
    if status != 200 {
        return Ok(None);
    }
    if kind == CaptureKind::LiveEvent {
        let creator_of_id = database
            .get_model(model_id)
            .ok()
            .and_then(|model| model.of_user_id);
        let candidates =
            crate::notifications::parse_live_ws_payload(creator_of_id.as_deref(), body);
        return Ok(Some(IngestOutcome {
            kind,
            unauthorized: false,
            earnings: None,
            fan_id: None,
            notifications: NotificationBatch {
                source: BatchSource::WebSocket,
                candidates,
            },
        }));
    }
    let value: Value = serde_json::from_str(body).map_err(|err| DbError::Invalid(err.to_string()))?;
    let outcome = match kind {
        CaptureKind::Profile => {
            save_profile(database, model_id, &value)?;
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id: None,
                notifications: NotificationBatch::empty(),
            }
        }
        CaptureKind::Balances => {
            save_balances(database, model_id, &value)?;
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id: None,
                notifications: NotificationBatch::empty(),
            }
        }
        CaptureKind::Transactions => {
            let is_chart = chart_earnings(database, model_id, url, &value)?;
            let is_payout_tx = is_chart.is_none();
            let earnings = match is_chart {
                Some(rows) => rows,
                None => earnings_from_payload(&value),
            };
            let (stats, newly_inserted) = database.upsert_earnings_with_new(model_id, &earnings)?;
            let notifications = if is_payout_tx {
                let candidates = newly_inserted
                    .into_iter()
                    .filter(|item| !item.of_transaction_id.starts_with("chart:"))
                    .filter_map(|item| {
                        let sale_kind = match item.kind {
                            TransactionType::Message => SaleKind::Ppv,
                            TransactionType::Tip => SaleKind::Tip,
                            _ => return None,
                        };
                        let preview = match sale_kind {
                            SaleKind::Ppv => format!("PPV vendido (${:.2})", item.gross_amount),
                            SaleKind::Tip => format!("Propina recibida (${:.2})", item.gross_amount),
                        };
                        Some(CandidateEvent::Sale {
                            sale_kind,
                            of_event_id: item.of_transaction_id,
                            of_fan_id: item.fan_of_id.unwrap_or_default(),
                            fan_name: if item.fan_username.trim().is_empty() {
                                String::new()
                            } else {
                                format!("@{}", item.fan_username.trim())
                            },
                            gross_amount: item.gross_amount,
                            net_amount: item.net_amount,
                            preview,
                            created_at: item.created_at,
                            live_transition: false,
                        })
                    })
                    .collect();
                NotificationBatch {
                    source: BatchSource::Transactions,
                    candidates,
                }
            } else {
                NotificationBatch::empty()
            };
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: Some(stats),
                fan_id: None,
                notifications,
            }
        }
        CaptureKind::Chat => {
            let is_backfill = url.contains("_cf_backfill=1");
            let fan_id = chat_fan_id(url);
            let mut fan_name = String::new();
            if let Some(user) = value.get("withUser") {
                if let Some(obj) = user.as_object() {
                    fan_name = user_display_hint(obj);
                }
                apply_one_subscriber(database, model_id, user)?;
            }
            let notifications = if let Some(fan_id_str) = fan_id.as_deref() {
                let mut candidates =
                    store_chat_ppvs(database, model_id, fan_id_str, &fan_name, &value, is_backfill)?;
                if !is_backfill {
                    let creator_of_id = database
                        .get_model(model_id)
                        .ok()
                        .and_then(|model| model.of_user_id);
                    let (max_msg_id, thread_msgs) = extract_thread_messages(
                        creator_of_id.as_deref(),
                        fan_id_str,
                        &fan_name,
                        &value,
                    );
                    candidates.extend(thread_msgs);
                    NotificationBatch {
                        source: BatchSource::ChatThread { max_msg_id },
                        candidates,
                    }
                } else {
                    NotificationBatch::empty()
                }
            } else {
                NotificationBatch::empty()
            };
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id,
                notifications,
            }
        }
        CaptureKind::Subscriber => {
            let fan_id = apply_subscriber_payload(database, model_id, url, &value)?;
            let notifications = if is_chats_inbox_url(url) {
                let creator_of_id = database
                    .get_model(model_id)
                    .ok()
                    .and_then(|model| model.of_user_id);
                let (max_msg_id, candidates) =
                    extract_inbox_events(creator_of_id.as_deref(), &value);
                NotificationBatch {
                    source: BatchSource::Inbox { max_msg_id },
                    candidates,
                }
            } else {
                NotificationBatch::empty()
            };
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id,
                notifications,
            }
        }
        CaptureKind::LiveEvent => unreachable!(),
    };
    Ok(Some(outcome))
}

fn save_profile(database: &Database, model_id: i64, value: &Value) -> Result<(), DbError> {
    let Some(object) = value.as_object() else {
        return Err(DbError::Invalid(
            "el perfil de OnlyFans no es un objeto".to_owned(),
        ));
    };
    let Some(of_user_id) = object.get("id").and_then(value_id) else {
        return Err(DbError::Invalid(
            "OnlyFans no devolvió el id de la cuenta".to_owned(),
        ));
    };
    let username = object.get("username").and_then(Value::as_str).unwrap_or("");
    let display_name = object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| object.get("displayName").and_then(Value::as_str))
        .unwrap_or("");
    let avatar_url = object
        .get("avatarThumbs")
        .and_then(|thumbs| thumbs.get("c144"))
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .or_else(|| {
            object
                .get("avatar")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
        });
    database.save_onlyfans_profile(model_id, &of_user_id, username, display_name, avatar_url)?;
    Ok(())
}

fn apply_subscriber_payload(
    database: &Database,
    model_id: i64,
    url: &str,
    value: &Value,
) -> Result<Option<String>, DbError> {
    if let Some(list) = value.get("list").and_then(Value::as_array) {
        let mut last = None;
        for item in list {
            let fan = item
                .get("withUser")
                .filter(|user| user.is_object())
                .or_else(|| {
                    if item.get("username").is_some()
                        || item.get("subscribedBy").is_some()
                        || item.get("subscribedOnData").is_some()
                        || item.get("subscribedOn").is_some()
                        || item.get("subscribedByData").is_some()
                    {
                        Some(item)
                    } else {
                        None
                    }
                });
            let Some(fan) = fan else {
                continue;
            };
            if let Some(fan_id) = apply_one_subscriber(database, model_id, fan)? {
                last = Some(fan_id);
            }
        }
        return Ok(last);
    }
    if value.get("id").is_some() {
        return apply_one_subscriber(database, model_id, value);
    }
    if let Some(object) = value.as_object() {
        let mut last = None;
        for item in object.values() {
            if item.get("id").is_none() {
                continue;
            }
            if let Some(fan_id) = apply_one_subscriber(database, model_id, item)? {
                last = Some(fan_id);
            }
        }
        if last.is_some() {
            return Ok(last);
        }
    }
    Ok(user_id_from_path(url))
}

fn apply_one_subscriber(
    database: &Database,
    model_id: i64,
    value: &Value,
) -> Result<Option<String>, DbError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(of_fan_id) = object.get("id").and_then(value_id) else {
        return Ok(None);
    };
    let username = object.get("username").and_then(Value::as_str).unwrap_or("");
    let data = subscription_blob(object);
    let status = subscription_status(object, data);
    let cost = object
        .get("currentSubscribePrice")
        .and_then(number_value)
        .or_else(|| data.and_then(|item| first_number(item, &["subscribePrice", "price", "regularPrice"])));
    let started = data.and_then(|item| {
        item.get("duration")
            .or_else(|| item.get("subscribedOnDuration"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                item.get("subscribeAt")
                    .or_else(|| item.get("subscribeDate"))
                    .or_else(|| item.get("startDate"))
                    .and_then(Value::as_str)
                    .and_then(duration_since)
            })
    });
    let duration = started.or_else(|| {
        let start = data
            .and_then(|item| item.get("subscribeDate").or_else(|| item.get("startDate")))
            .and_then(Value::as_str);
        let end = object
            .get("subscribedByExpireDate")
            .and_then(Value::as_str)
            .or_else(|| data.and_then(|item| item.get("expireDate")).and_then(Value::as_str));
        match (start, end) {
            (Some(start), Some(end)) => Some(format!("{start} – {end}")),
            _ => None,
        }
    });
    let official = official_spend_from_user(object);
    let total_spend = official.total.or_else(|| fan_spend(object, data));
    let auto_renewal = object
        .get("subscribedByAutoprolong")
        .and_then(bool_value)
        .or_else(|| data.and_then(|item| item.get("renew")).and_then(bool_value))
        .or_else(|| data.and_then(|item| item.get("autoRenew")).and_then(bool_value))
        .or_else(|| {
            data.and_then(|item| item.get("subscribedByAutoprolong"))
                .and_then(bool_value)
        });
    let mut lists = Vec::new();
    if let Some(states) = object.get("listsStates").and_then(Value::as_array) {
        for state in states {
            let checked = state.get("hasUser").and_then(Value::as_bool) == Some(true);
            if let Some(name) = state.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()) {
                lists.push(crate::fans::FanListItem {
                    name: name.to_owned(),
                    checked,
                });
            }
        }
    }
    crate::fans::apply_fan_subscription(
        database,
        model_id,
        &of_fan_id,
        username,
        status.as_deref(),
        cost,
        duration.as_deref(),
        auto_renewal,
        total_spend,
        lists,
        official.messages,
        official.tips,
        official.present,
    )?;
    Ok(Some(of_fan_id))
}

fn store_chat_ppvs(
    database: &Database,
    model_id: i64,
    fan_id: &str,
    fan_name: &str,
    value: &Value,
    is_backfill: bool,
) -> Result<Vec<CandidateEvent>, DbError> {
    let Some(list) = value.get("list").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    database.with_connection(|conn| {
        let mut events = Vec::new();
        for item in list {
            let Some(object) = item.as_object() else {
                continue;
            };
            if let Some(amount) = chat_tip_amount(object) {
                let Some(of_message_id) = object.get("id").and_then(value_id) else {
                    continue;
                };
                let created_at = message_created_at(object);
                let existing_tip: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM chat_tips WHERE model_id = ?1 AND of_message_id = ?2",
                        params![model_id, of_message_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if existing_tip.is_none() {
                    conn.execute(
                        "INSERT INTO chat_tips (model_id, of_fan_id, of_message_id, amount, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![model_id, fan_id, of_message_id, amount, created_at],
                    )?;
                    if !is_backfill {
                        let text = object
                            .get("text")
                            .or_else(|| object.get("tipText"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let sender_hint = object
                            .get("fromUser")
                            .and_then(Value::as_object)
                            .map(user_display_hint)
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| fan_name.to_owned());
                        events.push(CandidateEvent::Sale {
                            sale_kind: SaleKind::Tip,
                            of_event_id: of_message_id,
                            of_fan_id: fan_id.to_owned(),
                            fan_name: sender_hint,
                            gross_amount: amount,
                            net_amount: net_from_gross(amount),
                            preview: text.to_owned(),
                            created_at: created_at.to_owned(),
                            live_transition: false,
                        });
                    }
                }
                continue;
            }
            let Some(price) = object.get("price").and_then(number_value) else {
                continue;
            };
            if !price.is_finite() || price <= 0.0 {
                continue;
            }
            let Some(of_message_id) = object.get("id").and_then(value_id) else {
                continue;
            };
            let purchased = object.get("isOpened").and_then(bool_value) == Some(true);
            let created_at = message_created_at(object);
            let existing: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT id, purchased FROM ppv_timeline WHERE model_id = ?1 AND of_message_id = ?2",
                    params![model_id, of_message_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((id, prev_purchased)) = existing {
                conn.execute(
                    "UPDATE ppv_timeline
                     SET price = ?1,
                         purchased = CASE WHEN purchased = 1 OR ?2 = 1 THEN 1 ELSE 0 END,
                         created_at = ?3,
                         of_fan_id = ?4
                     WHERE id = ?5",
                    params![price, i64::from(purchased), created_at, fan_id, id],
                )?;
                if prev_purchased == 0 && purchased && !is_backfill {
                    events.push(CandidateEvent::Sale {
                        sale_kind: SaleKind::Ppv,
                        of_event_id: of_message_id,
                        of_fan_id: fan_id.to_owned(),
                        fan_name: fan_name.to_owned(),
                        gross_amount: price,
                        net_amount: net_from_gross(price),
                        preview: format!("PPV desbloqueado (${price:.2})"),
                        created_at: created_at.to_owned(),
                        live_transition: true,
                    });
                }
            } else {
                conn.execute(
                    "INSERT INTO ppv_timeline (
                        model_id, of_fan_id, message_id, price, purchased, created_at, of_message_id
                     ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)",
                    params![model_id, fan_id, price, i64::from(purchased), created_at, of_message_id],
                )?;
                if purchased && !is_backfill {
                    events.push(CandidateEvent::Sale {
                        sale_kind: SaleKind::Ppv,
                        of_event_id: of_message_id,
                        of_fan_id: fan_id.to_owned(),
                        fan_name: fan_name.to_owned(),
                        gross_amount: price,
                        net_amount: net_from_gross(price),
                        preview: format!("PPV comprado (${price:.2})"),
                        created_at: created_at.to_owned(),
                        live_transition: false,
                    });
                }
            }
        }
        Ok(events)
    })
}

fn is_chats_inbox_url(url: &str) -> bool {
    tauri::Url::parse(url)
        .map(|parsed| parsed.path() == "/api2/v2/chats")
        .unwrap_or(false)
}

fn user_display_hint(user: &serde_json::Map<String, Value>) -> String {
    if let Some(name) = user
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return name.to_owned();
    }
    if let Some(username) = user
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return format!("@{username}");
    }
    String::new()
}

fn extract_thread_messages(
    creator_of_id: Option<&str>,
    fan_id: &str,
    fan_name: &str,
    value: &Value,
) -> (u64, Vec<CandidateEvent>) {
    let Some(list) = value.get("list").and_then(Value::as_array) else {
        return (0, Vec::new());
    };
    let creator_id = creator_of_id.map(str::trim).filter(|s| !s.is_empty());
    let mut max_msg_id = 0_u64;
    let mut out = Vec::new();
    for item in list {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(of_message_id) = obj.get("id").and_then(value_id) else {
            continue;
        };
        if let Some(n) = parse_numeric_id(&of_message_id) {
            max_msg_id = max_msg_id.max(n);
        }
        if chat_tip_amount(obj).is_some() {
            continue;
        }
        if obj.get("price").and_then(number_value).unwrap_or(0.0) > 0.0 {
            continue;
        }
        let (from_id, from_label) = extract_user_ref(obj.get("fromUser"));
        let Some(from_id) = from_id else {
            continue;
        };
        if let Some(cid) = creator_id {
            if cid == from_id {
                continue;
            }
        } else if from_id != fan_id {
            continue;
        }
        let sender_name = if !from_label.is_empty() {
            from_label
        } else {
            fan_name.to_owned()
        };
        let text = obj
            .get("text")
            .or_else(|| obj.get("rawText"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let created_at = message_created_at(obj).to_owned();
        out.push(CandidateEvent::IncomingMessage {
            of_message_id,
            of_fan_id: fan_id.to_owned(),
            fan_name: sender_name,
            preview: text.to_owned(),
            created_at,
        });
    }
    (max_msg_id, out)
}

fn extract_inbox_events(creator_of_id: Option<&str>, value: &Value) -> (u64, Vec<CandidateEvent>) {
    let Some(list) = value.get("list").and_then(Value::as_array) else {
        return (0, Vec::new());
    };
    let creator_id = creator_of_id.map(str::trim).filter(|s| !s.is_empty());
    let mut max_msg_id = 0_u64;
    let mut out = Vec::new();
    for chat in list {
        let Some(chat_obj) = chat.as_object() else {
            continue;
        };
        let (with_id, with_label) = extract_user_ref(chat_obj.get("withUser"));
        let fan_id = with_id.unwrap_or_default();
        let Some(last_msg) = chat_obj.get("lastMessage").and_then(Value::as_object) else {
            continue;
        };
        let Some(of_message_id) = last_msg.get("id").and_then(value_id) else {
            continue;
        };
        if let Some(n) = parse_numeric_id(&of_message_id) {
            max_msg_id = max_msg_id.max(n);
        }
        let (from_id, from_label) = extract_user_ref(last_msg.get("fromUser"));
        let fan_name = if !with_label.is_empty() {
            with_label
        } else {
            from_label
        };
        let from_is_creator = match (creator_id, from_id.as_deref()) {
            (Some(cid), Some(fid)) => cid == fid,
            (None, Some(fid)) if !fan_id.is_empty() => fid != fan_id,
            _ => last_msg.get("isFromQueue").and_then(bool_value) == Some(true),
        };
        if from_is_creator {
            continue;
        }
        let created_at = message_created_at(last_msg).to_owned();
        let text = last_msg
            .get("text")
            .or_else(|| last_msg.get("rawText"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(amount) = chat_tip_amount(last_msg) {
            out.push(CandidateEvent::Sale {
                sale_kind: SaleKind::Tip,
                of_event_id: of_message_id,
                of_fan_id: fan_id,
                fan_name,
                gross_amount: amount,
                net_amount: net_from_gross(amount),
                preview: text.to_owned(),
                created_at,
                live_transition: false,
            });
        } else if from_id.is_some() {
            out.push(CandidateEvent::IncomingMessage {
                of_message_id,
                of_fan_id: fan_id,
                fan_name,
                preview: text.to_owned(),
                created_at,
            });
        }
    }
    (max_msg_id, out)
}

fn message_created_at(object: &serde_json::Map<String, Value>) -> &str {
    object
        .get("createdAt")
        .or_else(|| object.get("created_at"))
        .and_then(Value::as_str)
        .filter(|stamp| !stamp.is_empty())
        .unwrap_or("1970-01-01T00:00:00Z")
}

fn chat_tip_amount(object: &serde_json::Map<String, Value>) -> Option<f64> {
    let explicit = object
        .get("tipAmount")
        .or_else(|| object.get("tipSumm"))
        .and_then(number_value)
        .filter(|amount| amount.is_finite() && *amount > 0.0);
    let text = object
        .get("text")
        .or_else(|| object.get("tipText"))
        .or_else(|| object.get("rawText"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let phrase = tip_phrase(text);
    if object.get("isTip").and_then(bool_value) == Some(true) || phrase {
        return explicit.or_else(|| money_in_text(text));
    }
    None
}

fn tip_phrase(text: &str) -> bool {
    if text.chars().count() > 180 {
        return false;
    }
    let lower = text.to_lowercase();
    lower.contains("propina")
        || lower.contains("sent you a tip")
        || lower.contains("you sent a tip")
        || lower.contains("tip sent")
}

fn money_in_text(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            let rest = text[index + 1..].trim_start();
            let cleaned: String = rest
                .chars()
                .take_while(|character| character.is_ascii_digit() || *character == ',' || *character == '.')
                .collect();
            let parsed = cleaned.trim_matches(|character| character == ',' || character == '.').replace(',', "");
            if let Ok(amount) = parsed.parse::<f64>() {
                if amount.is_finite() && amount > 0.0 {
                    return Some(amount);
                }
            }
        }
        index += 1;
    }
    None
}

fn chat_fan_id(url: &str) -> Option<String> {
    let path = tauri::Url::parse(url).ok()?.path().to_owned();
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    if parts.next() == Some("api2") && parts.next() == Some("v2") && parts.next() == Some("chats") {
        return parts.next().and_then(numeric_id);
    }
    None
}

fn numeric_id(value: &str) -> Option<String> {
    let digits = value
        .strip_prefix('u')
        .or_else(|| value.strip_prefix('U'))
        .unwrap_or(value);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        Some(digits.to_owned())
    } else {
        None
    }
}

fn user_id_from_path(url: &str) -> Option<String> {
    let path = tauri::Url::parse(url).ok()?.path().to_owned();
    let id = path.trim_start_matches("/api2/v2/users/");
    if path.starts_with("/api2/v2/users/") && !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        Some(id.to_owned())
    } else {
        None
    }
}

fn save_balances(database: &Database, model_id: i64, value: &Value) -> Result<(), DbError> {
    let Some(object) = value.as_object() else {
        return Err(DbError::Invalid(
            "los balances de OnlyFans no son un objeto".to_owned(),
        ));
    };
    let current = first_number(object, &["current", "payoutAvailable", "available"]);
    let pending = first_number(object, &["pending", "payoutPending"]);
    database.save_onlyfans_balances(model_id, current, pending)
}

fn subscription_blob<'a>(
    object: &'a serde_json::Map<String, Value>,
) -> Option<&'a serde_json::Map<String, Value>> {
    for key in ["subscribedOnData", "subscribedOn", "subscribedByData"] {
        if let Some(data) = object.get(key).and_then(Value::as_object) {
            return Some(data);
        }
    }
    None
}

fn subscription_status(
    object: &serde_json::Map<String, Value>,
    data: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    let expired = object.get("subscribedOnExpiredNow").and_then(Value::as_bool) == Some(true)
        || object.get("subscribedIsExpiredNow").and_then(Value::as_bool) == Some(true)
        || object.get("subscribedByExpire").and_then(Value::as_bool) == Some(true);
    if expired {
        return Some("Expired".to_owned());
    }
    if let Some(status) = data.and_then(|item| item.get("status")).and_then(Value::as_str) {
        let lower = status.to_ascii_lowercase();
        if lower.contains("expir") {
            return Some("Expired".to_owned());
        }
        if lower.contains("active") {
            return Some("Active".to_owned());
        }
    }
    match object.get("subscribedBy").and_then(Value::as_bool) {
        Some(true) => Some("Active".to_owned()),
        Some(false) => Some("Inactive".to_owned()),
        None if data.is_some() => Some("Active".to_owned()),
        None => None,
    }
}

struct OfficialSpend {
    total: Option<f64>,
    messages: Option<f64>,
    tips: Option<f64>,
    present: bool,
}

fn official_spend_from_user(object: &serde_json::Map<String, Value>) -> OfficialSpend {
    let mut best = OfficialSpend {
        total: None,
        messages: None,
        tips: None,
        present: false,
    };
    for source in spend_sources(object) {
        let spend = official_spend(Some(source));
        if !spend.present {
            continue;
        }
        let candidate = spend.total.unwrap_or(0.0);
        let current = best.total.unwrap_or(0.0);
        if !best.present || candidate > current {
            best = spend;
        }
    }
    best
}

fn spend_sources<'a>(
    object: &'a serde_json::Map<String, Value>,
) -> Vec<&'a serde_json::Map<String, Value>> {
    let mut sources = Vec::new();
    for key in ["subscribedByData", "subscribedOnData", "subscribedOn"] {
        if let Some(data) = object.get(key).and_then(Value::as_object) {
            sources.push(data);
        }
    }
    sources.push(object);
    sources
}

fn official_spend(data: Option<&serde_json::Map<String, Value>>) -> OfficialSpend {
    let Some(data) = data else {
        return OfficialSpend {
            total: None,
            messages: None,
            tips: None,
            present: false,
        };
    };
    let tips = data.get("tipsSumm").and_then(number_value);
    let messages = data.get("messagesSumm").and_then(number_value);
    let direct = data.get("totalSumm").and_then(number_value);
    let present = direct.is_some() || tips.is_some() || messages.is_some();
    let total = direct.or_else(|| {
        if !present {
            return None;
        }
        let mut sum = 0.0;
        for key in ["tipsSumm", "messagesSumm", "subscribesSumm", "postsSumm", "streamsSumm"] {
            if let Some(amount) = data.get(key).and_then(number_value) {
                sum += amount;
            }
        }
        Some(sum)
    });
    OfficialSpend {
        total,
        messages,
        tips,
        present,
    }
}

fn bool_value(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number.as_i64().map(|item| item != 0),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Some(true),
            "0" | "false" | "off" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn fan_spend(
    object: &serde_json::Map<String, Value>,
    data: Option<&serde_json::Map<String, Value>>,
) -> Option<f64> {
    let direct = data
        .and_then(|item| first_number(item, &["totalSumm", "totalSpent", "spent"]))
        .or_else(|| first_number(object, &["totalSumm", "totalSpent", "spent"]));
    if direct.is_some() {
        return direct;
    }
    let parts = ["tipsSumm", "messagesSumm", "subscribesSumm", "postsSumm", "streamsSumm"];
    let source = data.unwrap_or(object);
    let mut any = false;
    let mut total = 0.0;
    for key in parts {
        if let Some(amount) = source.get(key).and_then(number_value) {
            any = true;
            total += amount;
        }
    }
    if any { Some(total) } else { None }
}

fn duration_since(value: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&chrono::Utc))
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(&value.chars().take(10).collect::<String>(), "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|date| date.and_utc())
        })?;
    let days = (chrono::Utc::now() - parsed).num_days().max(0);
    if days >= 30 {
        Some(format!("{} months", days / 30))
    } else {
        Some(format!("{days} days"))
    }
}

fn chart_earnings(
    database: &Database,
    model_id: i64,
    url: &str,
    value: &Value,
) -> Result<Option<Vec<EarningDraft>>, DbError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if object.get("list").and_then(Value::as_array).is_some() && !earnings_from_payload(value).is_empty() {
        return Ok(None);
    }
    let body = object.get("data").and_then(Value::as_object).unwrap_or(object);
    const SERIES: &[(&str, &str)] = &[
        ("subscribes", "sub"),
        ("subscriptions", "sub"),
        ("subscription", "sub"),
        ("tips", "tip"),
        ("messages", "message"),
        ("chat_messages", "message"),
        ("post", "post"),
        ("posts", "post"),
        ("ref", "referral"),
        ("referrals", "referral"),
        ("stream", "stream"),
        ("streams", "stream"),
    ];
    let mut chosen: Vec<(&str, &Value)> = Vec::new();
    let mut drafts = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (key, kind) in SERIES {
        if seen.contains(kind) {
            continue;
        }
        if let Some(series) = body.get(*key) {
            if let Some(amount) = series.as_f64() {
                if amount > 0.0 {
                    let synthetic = serde_json::json!({ "total": amount });
                    drafts.extend(store_chart_series(database, model_id, url, kind, &synthetic)?);
                }
                seen.insert(*kind);
                continue;
            }
            if series.as_object().is_some() {
                seen.insert(*kind);
                chosen.push((kind, series));
            }
        }
    }
    if chosen.is_empty() && drafts.is_empty() {
        let by = query_value(url, "by").unwrap_or_else(|| "total".to_owned());
        let kind = match by.as_str() {
            "subscribes" => "sub",
            "tips" | "tips_chat" | "tips_profile" | "tips_post" | "tips_stream" | "tips_story" => "tip",
            "messages" | "chat_messages" => "message",
            "post" | "posts" => "post",
            "stream" | "streams" => "stream",
            "ref" | "referrals" => "referral",
            _ => return Ok(None),
        };
        let series = if body.get("chartAmount").is_some() {
            value
        } else {
            body.get("total").unwrap_or(value)
        };
        chosen.push((kind, series));
    }
    for (kind, series) in chosen {
        drafts.extend(store_chart_series(database, model_id, url, kind, series)?);
    }
    Ok(Some(drafts))
}

fn store_chart_series(
    database: &Database,
    model_id: i64,
    url: &str,
    kind: &str,
    series: &Value,
) -> Result<Vec<EarningDraft>, DbError> {
    let Some(object) = series.as_object() else {
        return Ok(Vec::new());
    };
    let net_total = first_number(object, &["total", "net"]);
    let gross_total = first_number(object, &["gross"]);
    let points = object
        .get("chartAmount")
        .or_else(|| object.get("chartGross"))
        .and_then(Value::as_array);
    let mut dated = Vec::new();
    if let Some(points) = points {
        for point in points {
            let Some(point) = point.as_object() else { continue };
            let Some(date) = point.get("date").and_then(Value::as_str) else { continue };
            let amount = first_number(point, &["count", "amount", "sum"]).unwrap_or(0.0);
            if let Some(stamp) = bucket_noon(date) {
                dated.push((date_key(date), stamp, amount));
            }
        }
    }
    if dated.is_empty() {
        let amount = net_total.filter(|value| *value > 0.0).or_else(|| gross_total.filter(|value| *value > 0.0));
        if let Some(amount) = amount {
            let date = query_value(url, "endDate").unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
            if let Some(stamp) = bucket_noon(&date) {
                dated.push((date_key(&date), stamp, amount));
            }
        }
    }
    let sum: f64 = dated.iter().map(|(_, _, amount)| *amount).sum();
    let (net_factor, gross_factor) = chart_factors(sum, net_total, gross_total);
    let mut stamps = Vec::new();
    let mut rows = Vec::new();
    for (key, stamp, amount) in &dated {
        stamps.push(stamp.clone());
        if *amount == 0.0 {
            continue;
        }
        let net = amount * net_factor;
        let gross = amount * gross_factor;
        rows.push(EarningDraft {
            of_transaction_id: format!("chart:{model_id}:{kind}:{key}"),
            fan_of_id: None,
            fan_username: String::new(),
            kind: chart_type(kind),
            gross_amount: gross,
            net_amount: net,
            created_at: stamp.clone(),
        });
    }
    if let Some((start, end)) = span_unix(&stamps) {
        database.delete_chart_span(model_id, kind, start, end.saturating_add(1))?;
    }
    Ok(rows)
}

fn chart_type(kind: &str) -> TransactionType {
    match kind {
        "tip" => TransactionType::Tip,
        "message" => TransactionType::Message,
        "post" => TransactionType::Post,
        "stream" => TransactionType::Stream,
        "referral" => TransactionType::Referral,
        _ => TransactionType::Sub,
    }
}

fn chart_factors(sum: f64, net_total: Option<f64>, gross_total: Option<f64>) -> (f64, f64) {
    if sum <= 0.0 {
        return (1.0, 1.0);
    }
    let close = |left: f64, right: f64| (left - right).abs() <= right.abs() * 0.02 + 0.05;
    let distinct = match (net_total, gross_total) {
        (Some(net), Some(gross)) => (gross - net).abs() > 0.009,
        _ => false,
    };
    if let (Some(net), Some(gross)) = (net_total, gross_total) {
        if distinct {
            if close(sum, net) {
                return (1.0, gross / sum);
            }
            if close(sum, gross) {
                return (net / sum, 1.0);
            }
            return (net / sum, gross / sum);
        }
    }
    let net = net_total
        .filter(|value| *value > 0.0)
        .or_else(|| gross_total.map(|gross| gross * crate::db::ONLYFANS_NET_RATIO));
    match net {
        Some(net) if close(sum, net) => (1.0, 1.0 / crate::db::ONLYFANS_NET_RATIO),
        Some(net) if net_total.is_none() => (
            (net / sum).min(1.0),
            1.0,
        ),
        Some(net) => (net / sum, (net / crate::db::ONLYFANS_NET_RATIO) / sum),
        None => (1.0, 1.0 / crate::db::ONLYFANS_NET_RATIO),
    }
}

fn span_unix(stamps: &[String]) -> Option<(i64, i64)> {
    let mut values = stamps.iter().filter_map(|stamp| {
        chrono::DateTime::parse_from_rfc3339(stamp).ok().map(|date| date.timestamp())
    });
    let first = values.next()?;
    let mut min = first;
    let mut max = first;
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    Some((min, max))
}

fn date_key(value: &str) -> String {
    let text = value.trim();
    if text.len() >= 10 && text.as_bytes()[4] == b'-' {
        text.chars().take(10).collect()
    } else {
        text.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect()
    }
}

fn bucket_noon(value: &str) -> Option<String> {
    let day = value.trim().chars().take(10).collect::<String>();
    let date = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").ok()?;
    let naive = date.and_hms_opt(12, 0, 0)?;
    let local = match chrono::Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(value) => value,
        chrono::LocalResult::Ambiguous(earliest, _) => earliest,
        chrono::LocalResult::None => return None,
    };
    Some(local.to_rfc3339())
}

fn query_value(url: &str, key: &str) -> Option<String> {
    tauri::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

pub struct ObservedProfile {
    pub status: Option<String>,
    pub duration: Option<String>,
    pub cost: Option<f64>,
    pub total_spend: Option<f64>,
    pub last_spend_at: Option<String>,
    pub highest_spend: Option<f64>,
}

pub fn ingest_observation(
    database: &Database,
    model_id: i64,
    fan_id: &str,
    profile: Option<&ObservedProfile>,
) -> Result<(), DbError> {
    let fan_id = numeric_id(fan_id).unwrap_or_else(|| fan_id.trim().to_owned());
    if fan_id.is_empty() {
        return Ok(());
    }
    if let Some(profile) = profile {
        let official = profile.total_spend.is_some();
        crate::fans::apply_fan_subscription_with_spend_meta(
            database,
            model_id,
            &fan_id,
            "",
            profile.status.as_deref(),
            profile.cost,
            profile.duration.as_deref(),
            None,
            profile.total_spend,
            Vec::new(),
            None,
            None,
            official,
            profile.last_spend_at.as_deref(),
            profile.highest_spend,
        )?;
    }
    Ok(())
}

fn earnings_from_payload(value: &Value) -> Vec<EarningDraft> {
    let list = value
        .get("list")
        .or_else(|| value.get("items"))
        .or_else(|| value.get("transactions"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    let Some(list) = list else {
        return Vec::new();
    };
    list.iter().filter_map(earning_from_item).collect()
}

fn earning_from_item(value: &Value) -> Option<EarningDraft> {
    let object = value.as_object()?;
    let of_transaction_id = object.get("id").and_then(value_id)?;
    let created_at = object
        .get("createdAt")
        .or_else(|| object.get("created_at"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    let amounts_object = object.get("amounts").and_then(Value::as_object);
    let kind = transaction_kind(
        object
            .get("type")
            .or_else(|| object.get("paymentType"))
            .or_else(|| object.get("transactionType"))
            .or_else(|| object.get("category"))
            .and_then(Value::as_str),
        object
            .get("description")
            .or_else(|| object.get("descriptionText"))
            .and_then(Value::as_str),
    )?;
    let amount = object
        .get("amount")
        .and_then(number_value)
        .or_else(|| amounts_object.and_then(|item| item.get("gross")).and_then(number_value))
        .or_else(|| object.get("gross").and_then(number_value));
    let net = object
        .get("net")
        .and_then(number_value)
        .or_else(|| amounts_object.and_then(|item| item.get("net")).and_then(number_value));
    let fee = object
        .get("fee")
        .and_then(number_value)
        .or_else(|| amounts_object.and_then(|item| item.get("fee")).and_then(number_value));
    let (gross_amount, net_amount) = amounts(amount, net, fee)?;
    let user = object.get("user").and_then(Value::as_object);
    let fan_of_id = user.and_then(|user| user.get("id")).and_then(value_id);
    let fan_username = user
        .and_then(|user| user.get("username"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(EarningDraft {
        of_transaction_id,
        fan_of_id,
        fan_username,
        kind,
        gross_amount,
        net_amount,
        created_at: created_at.to_owned(),
    })
}

fn net_from_gross(gross: f64) -> f64 {
    (gross * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0
}

fn gross_from_net(net: f64) -> f64 {
    (net / crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0
}

fn amounts(amount: Option<f64>, net: Option<f64>, fee: Option<f64>) -> Option<(f64, f64)> {
    match (amount, net, fee) {
        (Some(gross), Some(net), _) => Some((gross, net)),
        (Some(gross), None, Some(fee)) => Some((gross, (gross - fee).max(0.0))),
        (Some(gross), None, None) => Some((gross, net_from_gross(gross))),
        (None, Some(net), Some(fee)) => Some((net + fee, net)),
        (None, Some(net), None) => Some((gross_from_net(net), net)),
        _ => None,
    }
}

fn transaction_kind(type_name: Option<&str>, description: Option<&str>) -> Option<TransactionType> {
    if let Some(kind) = type_name.and_then(kind_from_text) {
        return Some(kind);
    }
    description.and_then(kind_from_text)
}

fn kind_from_text(value: &str) -> Option<TransactionType> {
    let value = value.to_ascii_lowercase();
    if value.contains("tip") || value.contains("propina") {
        Some(TransactionType::Tip)
    } else if value.contains("message") || value.contains("chat") || value.contains("mensaje") || value.contains("ppv") {
        Some(TransactionType::Message)
    } else if value.contains("subscri") || value.contains("suscrip") {
        Some(TransactionType::Sub)
    } else if value.contains("post") || value.contains("public") {
        Some(TransactionType::Post)
    } else if value.contains("stream") || value.contains("transmis") {
        Some(TransactionType::Stream)
    } else if value.contains("referral") || value.contains("referid") || value == "ref" {
        Some(TransactionType::Referral)
    } else {
        None
    }
}

fn first_number(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(number_value))
}

fn number_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let cleaned = text.trim().trim_start_matches('$').replace(',', "");
            cleaned.parse().ok()
        }
        _ => None,
    }
}

fn value_id(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, EmployeeRole, TransactionType};
    use chrono::{Duration, Local, Utc};
    use serde_json::json;

    #[tokio::test]
    async fn repeated_sync_does_not_inflate_the_open_shift() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-sync-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        database
            .save_onlyfans_profile(model.id, "88", "creator", "Creator", Some("https://cdn.example/a.jpg"))
            .unwrap();
        let employee = database
            .add_employee("Ana", EmployeeRole::Chatter, &json!({}))
            .unwrap();
        database.toggle_clock_in(employee.id).unwrap();
        let created_at = (Utc::now() + Duration::seconds(30)).to_rfc3339();
        let body = json!({
            "list": [{
                "id": 501,
                "amount": 10.0,
                "net": 8.0,
                "createdAt": created_at,
                "description": "Payment for message from fan",
                "user": { "id": 77, "username": "fan" }
            }, {
                "id": "sub-1",
                "amount": 5.0,
                "net": 4.0,
                "createdAt": created_at,
                "description": "Subscription from fan"
            }]
        });
        let url = "https://onlyfans.com/api2/v2/payouts/transactions?limit=10";
        let first = ingest_body(&database, model.id, url, 200, &body.to_string()).unwrap();
        let second = ingest_body(&database, model.id, url, 200, &body.to_string()).unwrap();
        let first_stats = first.unwrap().earnings.unwrap();
        let second_stats = second.unwrap().earnings.unwrap();
        assert_eq!(first_stats.upserted, 2);
        assert_eq!(first_stats.linked, 1);
        assert_eq!(second_stats.upserted, 2);
        assert_eq!(second_stats.linked, 0);

        database
            .with_connection(|conn| {
                let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;
                let gross: f64 = conn.query_row(
                    "SELECT total_sales_gross FROM shifts WHERE employee_id = ?1",
                    [employee.id],
                    |row| row.get(0),
                )?;
                let linked_type: String = conn.query_row(
                    "SELECT type FROM transactions WHERE employee_id = ?1",
                    [employee.id],
                    |row| row.get(0),
                )?;
                assert_eq!(count, 2);
                assert_eq!(gross, 10.0);
                assert_eq!(linked_type, TransactionType::Message.as_str());
                Ok(())
            })
            .unwrap();
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn chat_messages_and_subscription_update_the_fan() {
        let dir = std::env::temp_dir().join(format!("creatorfloww-chat-{}", std::process::id()));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        database
            .save_onlyfans_profile(model.id, "88", "creator", "Creator", None)
            .unwrap();
        let chat = json!({
            "list": [
                {"id": 9, "price": 10, "isOpened": true, "createdAt": "2026-02-01T00:00:00Z", "fromUser": {"id": 88}},
                {"id": 10, "price": 10, "isOpened": false, "createdAt": "2026-02-01T00:00:00Z", "fromUser": {"id": 88}},
                {"id": 11, "isTip": true, "tipAmount": 4, "createdAt": "2026-02-02T00:00:00Z", "fromUser": {"id": 77}}
            ],
            "withUser": {
                "id": 77,
                "username": "fan",
                "subscribedBy": true,
                "currentSubscribePrice": 9.99,
                "subscribedByAutoprolong": true,
                "subscribedByData": {"duration": "3 months"},
                "listsStates": [{"name": "Muted", "hasUser": true}]
            }
        });
        let url = "https://onlyfans.com/api2/v2/chats/77/messages?limit=10";
        ingest_body(&database, model.id, url, 200, &chat.to_string()).unwrap();
        ingest_body(&database, model.id, url, 200, &chat.to_string()).unwrap();
        let from_thread = crate::fans::fan_insights(&database, model.id, "77").unwrap();
        assert_eq!(from_thread.sub_status, "Active");
        assert_eq!(from_thread.tip_total, 4.0);
        assert_eq!(from_thread.tip_count, 1);
        assert_eq!(from_thread.ppv_total, 10.0);
        assert_eq!(from_thread.ppv_count, 1);
        assert_eq!(from_thread.total_spend, 14.0);
        database
            .with_connection(|conn| {
                let rows: i64 = conn.query_row("SELECT COUNT(*) FROM ppv_timeline", [], |row| row.get(0))?;
                let paid: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM ppv_timeline WHERE purchased = 1",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(rows, 2);
                assert_eq!(paid, 1);
                let tips: i64 = conn.query_row("SELECT COUNT(*) FROM chat_tips", [], |row| row.get(0))?;
                assert_eq!(tips, 1);
                Ok(())
            })
            .unwrap();
        let subscriber = json!({
            "id": 77,
            "username": "fan",
            "subscribedBy": true,
            "currentSubscribePrice": 9.99,
            "subscribedByAutoprolong": true,
            "subscribedByData": {"duration": "3 months"},
            "listsStates": [{"name": "Muted", "hasUser": true}]
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/users/77",
            200,
            &subscriber.to_string(),
        )
        .unwrap();
        let insights = crate::fans::fan_insights(&database, model.id, "77").unwrap();
        assert_eq!(insights.ppv_count, 1);
        assert_eq!(insights.ppv_total, 10.0);
        assert_eq!(insights.tip_count, 1);
        assert_eq!(insights.tip_total, 4.0);
        assert_eq!(insights.total_spend, 14.0);
        assert_eq!(insights.sub_status, "Active");
        assert_eq!(insights.sub_cost, Some(9.99));
        assert_eq!(insights.sub_duration, "3 months");
        assert_eq!(insights.auto_renewal, "On");
        assert_eq!(insights.lists.fans.len(), 1);
        database
            .with_connection(|conn| {
                let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn earnings_chart_and_visible_chat_sales_are_stored() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-chart-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        database
            .save_onlyfans_profile(model.id, "88", "creator", "Creator", None)
            .unwrap();
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let chart = json!({
            "subscribes": {"total": 8.0, "gross": 10.0, "chartAmount": [{"date": today, "count": 8.0}]},
            "tips": {"total": 36.0, "gross": 45.0, "chartAmount": [{"date": today, "count": 36.0}]},
            "messages": {"total": 18.0, "gross": 22.5, "chartAmount": [{"date": today, "count": 18.0}]},
            "post": {"total": 0, "chartAmount": []},
            "ref": {"total": 0, "chartAmount": []},
            "stream": {"total": 0, "chartAmount": []}
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/earnings/chart?startDate=2026-01-01&endDate=2026-11-07&withTotal=true",
            200,
            &chart.to_string(),
        )
        .unwrap();
        let overview = crate::dashboard::earnings_overview(
            &database,
            crate::dashboard::DashboardRange::Today,
            crate::dashboard::EarningsBasis::Net,
        )
        .unwrap();
        assert_eq!(overview.total, 62.0);
        let gross = crate::dashboard::earnings_overview(
            &database,
            crate::dashboard::DashboardRange::Today,
            crate::dashboard::EarningsBasis::Gross,
        )
        .unwrap();
        assert_eq!(gross.total, 77.5);

        let chat = json!({
            "list": [{
                "id": 45,
                "createdAt": "2026-11-07T18:00:00Z",
                "fromUser": {"id": 77},
                "text": "Te envié una $45.00 propina"
            }, {
                "id": 46,
                "createdAt": "2026-11-07T18:05:00Z",
                "fromUser": {"id": 88},
                "text": "$22.50 pagado"
            }]
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/chats/u77/messages?limit=10",
            200,
            &chat.to_string(),
        )
        .unwrap();
        let subscriber = json!({
            "id": 77,
            "username": "joseph",
            "subscribedBy": true,
            "currentSubscribePrice": 0,
            "subscribedByData": {"totalSumm": 466, "subscribeAt": "2026-07-07T00:00:00Z"}
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/users/77",
            200,
            &subscriber.to_string(),
        )
        .unwrap();
        let insights = crate::fans::fan_insights(&database, model.id, "77").unwrap();
        assert_eq!(insights.total_spend, 466.0);
        assert_eq!(insights.tip_total, 45.0);
        assert_eq!(insights.tip_count, 1);
        assert_eq!(insights.ppv_total, 0.0);
        assert_eq!(insights.spend_source, "onlyfans");
        assert_eq!(insights.sub_status, "Active");
        assert!(insights.sub_duration.contains("months") || insights.sub_duration.contains("days"));
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn chat_list_and_users_list_keep_historical_spend() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-sums-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        let chats = json!({
            "list": [{
                "withUser": {
                    "id": 405580496,
                    "username": "realgg",
                    "name": "Mike 28 (RealG)",
                    "subscribedBy": true,
                    "subscribedByAutoprolong": true,
                    "subscribedOnExpiredNow": false,
                    "subscribedOnData": {
                        "subscribePrice": 0,
                        "duration": "5 months",
                        "tipsSumm": "40",
                        "messagesSumm": "165.51",
                        "totalSumm": "205.51"
                    }
                }
            }]
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/chats?limit=10&offset=0&skip_users=all",
            200,
            &chats.to_string(),
        )
        .unwrap();
        let users = json!({
            "405580496": {
                "id": 405580496,
                "username": "realgg",
                "subscribedBy": true,
                "subscribedByAutoprolong": true,
                "subscribedOnData": {
                    "subscribePrice": 0,
                    "duration": "5 months",
                    "tipsSumm": 40,
                    "messagesSumm": 165.51,
                    "subscribesSumm": 0,
                    "totalSumm": 205.51
                }
            }
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/users/list?m[]=405580496",
            200,
            &users.to_string(),
        )
        .unwrap();
        let insights = crate::fans::fan_insights(&database, model.id, "405580496").unwrap();
        assert_eq!(insights.total_spend, 205.51);
        assert_eq!(insights.ppv_total, 165.51);
        assert_eq!(insights.tip_total, 40.0);
        assert_eq!(insights.spend_source, "onlyfans");
        assert_eq!(insights.sub_status, "Active");
        assert_eq!(insights.sub_cost, Some(0.0));
        assert_eq!(insights.sub_duration, "5 months");
        assert_eq!(insights.auto_renewal, "On");
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn subscribed_by_data_keeps_spend_when_subscribed_on_data_has_no_sums() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-bydata-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO fans (of_fan_id, model_id, username) VALUES ('180845750', ?1, 'king')",
                    [model.id],
                )?;
                let fan_id: i64 = conn.query_row(
                    "SELECT id FROM fans WHERE of_fan_id = '180845750'",
                    [],
                    |row| row.get(0),
                )?;
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, fan_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('b94402399292a0fb3238407260660357', ?1, ?2, 'message', 20, 16, '2026-10-23T06:04:06+00:00')",
                    rusqlite::params![model.id, fan_id],
                )?;
                Ok(())
            })
            .unwrap();
        let user = json!({
            "id": 180845750,
            "username": "a80974685",
            "subscribedBy": true,
            "subscribedByAutoprolong": true,
            "currentSubscribePrice": 0,
            "subscribedOnData": {
                "subscribePrice": 0,
                "duration": "6 months"
            },
            "subscribedByData": {
                "subscribePrice": 0,
                "duration": "6 months",
                "tipsSumm": 0,
                "messagesSumm": 367.23,
                "subscribesSumm": 0,
                "postsSumm": 0,
                "streamsSumm": 0,
                "totalSumm": 367.23
            }
        });
        ingest_body(
            &database,
            model.id,
            "https://onlyfans.com/api2/v2/users/180845750",
            200,
            &user.to_string(),
        )
        .unwrap();
        let insights = crate::fans::fan_insights(&database, model.id, "180845750").unwrap();
        assert_eq!(insights.total_spend, 367.23);
        assert_eq!(insights.ppv_total, 367.23);
        assert_eq!(insights.tip_total, 0.0);
        assert_eq!(insights.spend_source, "onlyfans");
        // Official totalSumm keeps the lifetime total, but known sales still expose last/highest.
        assert_eq!(
            insights.last_spend.as_deref(),
            Some("2026-10-23T06:04:06+00:00")
        );
        assert_eq!(insights.highest_spend, Some(16.0));
        assert_eq!(insights.sub_status, "Active");
        assert_eq!(insights.sub_cost, Some(0.0));
        assert_eq!(insights.sub_duration, "6 months");
        assert_eq!(insights.auto_renewal, "On");
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn today_scalar_chart_splits_net_and_gross() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-today-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let chart = json!({ "messages": 28.0 });
        ingest_body(
            &database,
            model.id,
            &format!("https://onlyfans.com/api2/v2/earnings/chart?startDate={today}&endDate={today}"),
            200,
            &chart.to_string(),
        )
        .unwrap();
        let net = crate::dashboard::earnings_overview(
            &database,
            crate::dashboard::DashboardRange::Today,
            crate::dashboard::EarningsBasis::Net,
        )
        .unwrap();
        let gross = crate::dashboard::earnings_overview(
            &database,
            crate::dashboard::DashboardRange::Today,
            crate::dashboard::EarningsBasis::Gross,
        )
        .unwrap();
        assert_eq!(net.total, 28.0);
        assert_eq!(gross.total, 35.0);
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
