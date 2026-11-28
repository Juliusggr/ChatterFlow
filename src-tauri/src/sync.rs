use chrono::TimeZone;
use serde_json::Value;

use crate::notifications::{
    extract_user_ref, parse_numeric_id, BatchSource, CandidateEvent, NotificationBatch, SaleKind,
};
use crate::supabase::{
    normalize_timestamp_rfc3339, DbError, EarningDraft, EarningStats, FanListItem, FanUpsertOwned,
    SupabaseStore, TransactionType, ONLYFANS_NET_RATIO,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Profile,
    Balances,
    Transactions,
    Chat,
    ChatMute,
    Subscriber,
    Notifications,
    LiveEvent,
    Publication,
    CreatorPosts,
}

#[derive(Debug)]
pub struct IngestOutcome {
    pub kind: CaptureKind,
    pub unauthorized: bool,
    pub earnings: Option<EarningStats>,
    pub fan_id: Option<String>,
    pub notifications: NotificationBatch,
    pub dropper_activity_changed: bool,
    pub model_data_changed: bool,
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
    if path == "/api2/v2/users/notifications"
        || path.starts_with("/api2/v2/users/notifications/")
    {
        return Some(CaptureKind::Notifications);
    }
    if path.contains("/api2/v2/chats/") && path.contains("/messages") {
        return Some(CaptureKind::Chat);
    }
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if segments.len() == 5
        && segments[0] == "api2"
        && segments[1] == "v2"
        && (segments[2] == "chats" || segments[2] == "users")
        && numeric_id(segments[3]).is_some()
        && (segments[4].eq_ignore_ascii_case("mute") || segments[4].eq_ignore_ascii_case("unmute"))
    {
        return Some(CaptureKind::ChatMute);
    }
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
    if (path == "/api2/v2/posts" || path.starts_with("/api2/v2/posts/"))
        && parsed.query().unwrap_or("").contains("_cf_publish=1")
    {
        return Some(CaptureKind::Publication);
    }
    if segments.len() == 5
        && segments[0] == "api2"
        && segments[1] == "v2"
        && segments[2] == "users"
        && segments[3].chars().all(|c| c.is_ascii_digit())
        && segments[4] == "posts"
    {
        return Some(CaptureKind::CreatorPosts);
    }
    let user_id = path.trim_start_matches("/api2/v2/users/");
    if path.starts_with("/api2/v2/users/")
        && !user_id.is_empty()
        && user_id.chars().all(|c| c.is_ascii_digit())
    {
        return Some(CaptureKind::Subscriber);
    }
    None
}

pub async fn ingest_body(
    store: &SupabaseStore,
    model_id: &str,
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
            dropper_activity_changed: false,
            model_data_changed: false,
        }));
    }
    if kind == CaptureKind::ChatMute {
        if !(200..300).contains(&status) {
            return Ok(None);
        }
        let fan_id = mute_target_fan_id(url);
        if let Some(ref fid) = fan_id {
            let body_mute = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| v.as_object().and_then(extract_mute_flag));
            let muted = body_mute.unwrap_or_else(|| infer_mute_from_url(url));
            store.set_fan_muted_state(model_id, fid, muted).await;
        }
        return Ok(Some(IngestOutcome {
            kind,
            unauthorized: false,
            earnings: None,
            fan_id,
            notifications: NotificationBatch::empty(),
            dropper_activity_changed: false,
            model_data_changed: false,
        }));
    }
    if kind == CaptureKind::Publication {
        if !(200..300).contains(&status) {
            return Ok(None);
        }
        let value: Value =
            serde_json::from_str(body).map_err(|err| DbError::Invalid(err.to_string()))?;
        let changed =
            save_publication(store, model_id, &value, store.active_dropper_id()).await?;
        return Ok(Some(IngestOutcome {
            kind,
            unauthorized: false,
            earnings: None,
            fan_id: None,
            notifications: NotificationBatch::empty(),
            dropper_activity_changed: changed,
            model_data_changed: false,
        }));
    }
    if status != 200 {
        return Ok(None);
    }
    if kind == CaptureKind::CreatorPosts {
        let value: Value =
            serde_json::from_str(body).map_err(|err| DbError::Invalid(err.to_string()))?;
        let changed = save_creator_posts(store, model_id, url, &value).await?;
        return Ok(Some(IngestOutcome {
            kind,
            unauthorized: false,
            earnings: None,
            fan_id: None,
            notifications: NotificationBatch::empty(),
            dropper_activity_changed: changed,
            model_data_changed: false,
        }));
    }
    if kind == CaptureKind::LiveEvent {
        let creator_of_id = store
            .get_model(model_id)
            .await
            .ok()
            .and_then(|model| model.of_user_id);
        let candidates =
            crate::notifications::parse_live_ws_payload(creator_of_id.as_deref(), body);
        let ws_sub_arrivals = extract_live_ws_sub_arrivals(creator_of_id.as_deref(), body);
        let arrival_added = if ws_sub_arrivals.is_empty() {
            false
        } else {
            store
                .record_fan_arrivals_batch(model_id, &ws_sub_arrivals)
                .await
                .unwrap_or(false)
        };
        return Ok(Some(IngestOutcome {
            kind,
            unauthorized: false,
            earnings: None,
            fan_id: None,
            notifications: NotificationBatch {
                source: BatchSource::WebSocket,
                candidates,
            },
            dropper_activity_changed: arrival_added,
            model_data_changed: false,
        }));
    }
    let value: Value =
        serde_json::from_str(body).map_err(|err| DbError::Invalid(err.to_string()))?;
    let outcome = match kind {
        CaptureKind::Profile => {
            let model_data_changed = save_profile(store, model_id, &value).await?;
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id: None,
                notifications: NotificationBatch::empty(),
                dropper_activity_changed: false,
                model_data_changed,
            }
        }
        CaptureKind::Balances => {
            let model_data_changed = save_balances(store, model_id, &value).await?;
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id: None,
                notifications: NotificationBatch::empty(),
                dropper_activity_changed: false,
                model_data_changed,
            }
        }
        CaptureKind::Transactions => {
            let is_chart = chart_earnings(store, model_id, url, &value).await?;
            let is_payout_tx = is_chart.is_none();
            let earnings = match is_chart {
                Some(rows) => rows,
                None => earnings_from_payload(&value),
            };
            let mut sub_arrivals = Vec::new();
            if is_payout_tx {
                for item in &earnings {
                    if item.kind == TransactionType::Sub
                        && !item.of_transaction_id.starts_with("chart:")
                    {
                        if let Some(fan_id) =
                            item.fan_of_id.as_deref().filter(|id| !id.is_empty())
                        {
                            let arrived = normalize_timestamp_rfc3339(&item.created_at)
                                .unwrap_or_else(|| item.created_at.clone());
                            sub_arrivals.push((fan_id.to_owned(), arrived));
                        }
                    }
                }
            }
            let (arrival_res, earnings_res) = tokio::join!(
                store.record_fan_arrivals_batch(model_id, &sub_arrivals),
                store.upsert_earnings_with_new(model_id, &earnings)
            );
            let sub_arrival_added = arrival_res.unwrap_or(false);
            let (stats, newly_inserted) = earnings_res?;
            let model_data_changed = stats.linked > 0;
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
                            SaleKind::Tip => {
                                format!("Propina recibida (${:.2})", item.gross_amount)
                            }
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
                dropper_activity_changed: sub_arrival_added,
                model_data_changed,
            }
        }
        CaptureKind::Chat => {
            let is_backfill = url.contains("_cf_backfill=1");
            let fan_id = chat_fan_id(url);
            let mut fan_name = String::new();
            let mut arrival_added = false;
            let chat_mute = value
                .as_object()
                .and_then(extract_mute_flag)
                .or_else(|| {
                    value
                        .get("withUser")
                        .and_then(Value::as_object)
                        .and_then(extract_mute_flag)
                });
            if let (Some(fid), Some(muted)) = (fan_id.as_deref(), chat_mute) {
                store.set_fan_muted_state(model_id, fid, muted).await;
            }
            if let Some(user) = value.get("withUser") {
                if let Some(obj) = user.as_object() {
                    fan_name = user_display_hint(obj);
                }
                let (_, added) =
                    apply_subscribers_batch(store, model_id, &[(user, chat_mute)]).await?;
                arrival_added = added;
            }
            let notifications = if let Some(fan_id_str) = fan_id.as_deref() {
                let mut candidates = store_chat_ppvs(
                    store,
                    model_id,
                    fan_id_str,
                    &fan_name,
                    &value,
                    is_backfill,
                )
                .await?;
                if !is_backfill {
                    let is_muted = chat_mute == Some(true)
                        || store.is_fan_muted(model_id, fan_id_str).await;
                    let max_msg_id = if is_muted {
                        0
                    } else {
                        let creator_of_id = store
                            .get_model(model_id)
                            .await
                            .ok()
                            .and_then(|model| model.of_user_id);
                        let (max_id, thread_msgs) = extract_thread_messages(
                            creator_of_id.as_deref(),
                            fan_id_str,
                            &fan_name,
                            &value,
                        );
                        candidates.extend(thread_msgs);
                        max_id
                    };
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
                dropper_activity_changed: arrival_added,
                model_data_changed: false,
            }
        }
        CaptureKind::Subscriber => {
            let (fan_id, arrival_added) =
                apply_subscriber_payload(store, model_id, url, &value).await?;
            let notifications = if is_chats_inbox_url(url) {
                let creator_of_id = store
                    .get_model(model_id)
                    .await
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
                dropper_activity_changed: arrival_added,
                model_data_changed: false,
            }
        }
        CaptureKind::Notifications => {
            let arrival_added =
                save_subscription_notifications(store, model_id, url, &value).await?;
            let sale_candidates = crate::notifications::extract_sales_from_notifications_payload(&value);
            let notifications = if sale_candidates.is_empty() {
                NotificationBatch::empty()
            } else {
                NotificationBatch {
                    source: BatchSource::WebSocket,
                    candidates: sale_candidates,
                }
            };
            IngestOutcome {
                kind,
                unauthorized: false,
                earnings: None,
                fan_id: None,
                notifications,
                dropper_activity_changed: arrival_added,
                model_data_changed: false,
            }
        }
        CaptureKind::Publication
        | CaptureKind::CreatorPosts
        | CaptureKind::LiveEvent
        | CaptureKind::ChatMute => {
            unreachable!()
        }
    };
    Ok(Some(outcome))
}

async fn save_profile(store: &SupabaseStore, model_id: &str, value: &Value) -> Result<bool, DbError> {
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
    let fans = object
        .get("subscribersCount")
        .or_else(|| object.get("fansCount"))
        .and_then(number_value)
        .map(|value| value.round() as i64)
        .filter(|value| *value >= 0);
    let (_, changed) = store
        .save_onlyfans_profile(model_id, &of_user_id, username, display_name, avatar_url, fans)
        .await?;
    Ok(changed)
}

async fn apply_subscriber_payload(
    store: &SupabaseStore,
    model_id: &str,
    url: &str,
    value: &Value,
) -> Result<(Option<String>, bool), DbError> {
    let from_muted_filter = url.contains("_cf_muted_list=1");
    if let Some(list) = value
        .get("list")
        .or_else(|| value.get("users"))
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
    {
        let mut items: Vec<(&Value, Option<bool>)> = Vec::with_capacity(list.len());
        for item in list {
            let item_mute = item
                .as_object()
                .and_then(extract_mute_flag)
                .or(if from_muted_filter { Some(true) } else { None });
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
            if let Some(fan) = fan {
                items.push((fan, item_mute));
            }
        }
        return apply_subscribers_batch(store, model_id, &items).await;
    }
    if value.get("id").is_some() {
        let mute = value.as_object().and_then(extract_mute_flag);
        return apply_subscribers_batch(store, model_id, &[(value, mute)]).await;
    }
    if let Some(object) = value.as_object() {
        let items: Vec<(&Value, Option<bool>)> = object
            .values()
            .filter(|item| item.get("id").is_some())
            .map(|item| (item, item.as_object().and_then(extract_mute_flag)))
            .collect();
        if !items.is_empty() {
            let (last, added) = apply_subscribers_batch(store, model_id, &items).await?;
            if last.is_some() {
                return Ok((last, added));
            }
        }
    }
    Ok((user_id_from_path(url), false))
}

async fn apply_subscribers_batch(
    store: &SupabaseStore,
    model_id: &str,
    values: &[(&Value, Option<bool>)],
) -> Result<(Option<String>, bool), DbError> {
    let mut fans = Vec::with_capacity(values.len());
    let mut arrivals = Vec::new();
    let mut last_id = None;

    let creator_of_id = store
        .get_model(model_id)
        .await
        .ok()
        .and_then(|model| model.of_user_id)
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());

    for (value, outer_mute) in values {
        let Some((mut fan, arrived)) = extract_subscriber_upsert(value) else {
            continue;
        };
        if outer_mute.is_some() {
            fan.is_muted = *outer_mute;
        }
        if let Some(arrived_at) = arrived {
            if creator_of_id.as_deref() != Some(fan.of_fan_id.as_str()) {
                arrivals.push((fan.of_fan_id.clone(), arrived_at));
            }
        }
        last_id = Some(fan.of_fan_id.clone());
        fans.push(fan);
    }

    if fans.is_empty() {
        return Ok((None, false));
    }

    let (arrival_res, fan_res) = tokio::join!(
        store.record_fan_arrivals_batch(model_id, &arrivals),
        store.upsert_fans_batch(model_id, &fans)
    );
    let _ = fan_res;
    Ok((last_id, arrival_res.unwrap_or(false)))
}

fn extract_subscriber_upsert(value: &Value) -> Option<(FanUpsertOwned, Option<String>)> {
    let object = value.as_object()?;
    let of_fan_id = object.get("id").and_then(value_id)?;
    let username = object
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let data = subscription_blob(object);
    let status = subscription_status(object, data);
    let cost = object
        .get("currentSubscribePrice")
        .and_then(number_value)
        .or_else(|| {
            data.and_then(|item| first_number(item, &["subscribePrice", "price", "regularPrice"]))
        });
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
            .or_else(|| {
                data.and_then(|item| item.get("expireDate"))
                    .and_then(Value::as_str)
            });
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
        .or_else(|| {
            data.and_then(|item| item.get("autoRenew"))
                .and_then(bool_value)
        })
        .or_else(|| {
            data.and_then(|item| item.get("subscribedByAutoprolong"))
                .and_then(bool_value)
        });
    let mut lists = Vec::new();
    if let Some(states) = object.get("listsStates").and_then(Value::as_array) {
        for state in states {
            let checked = state.get("hasUser").and_then(Value::as_bool) == Some(true);
            if let Some(name) = state
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                lists.push(FanListItem {
                    name: name.to_owned(),
                    checked,
                });
            }
        }
    }
    let arrived = fan_arrived_at(object);
    Some((
        FanUpsertOwned {
            of_fan_id,
            username,
            sub_status: status,
            sub_cost: cost,
            sub_duration: duration,
            auto_renewal,
            total_spend,
            lists,
            messages_summ: official.messages,
            tips_summ: official.tips,
            official_spend: official.present,
            last_spend_at: spend_stamp(object, data),
            highest_spend: data.and_then(|item| {
                first_number(item, &["highestSumm", "maxSumm", "biggestSumm", "highestPurchase"])
            }),
            is_muted: extract_mute_flag(object),
        },
        arrived,
    ))
}

pub(crate) fn extract_mute_flag(object: &serde_json::Map<String, Value>) -> Option<bool> {
    for key in [
        "isMutedNotifications",
        "isMuted",
        "is_muted",
        "muteNotifications",
        "notificationsMuted",
        "isChatMuted",
    ] {
        if let Some(flag) = object.get(key).and_then(bool_value) {
            return Some(flag);
        }
    }
    if let Some(states) = object.get("listsStates").and_then(Value::as_array) {
        for state in states {
            let Some(name) = state.get("name").and_then(Value::as_str) else {
                continue;
            };
            let norm = name.trim().to_lowercase();
            let is_mute_list = matches!(
                norm.as_str(),
                "muted"
                    | "mute"
                    | "silenciados"
                    | "silenciado"
                    | "silenciadas"
                    | "muted notifications"
            );
            if is_mute_list && state.get("hasUser").and_then(bool_value) == Some(true) {
                return Some(true);
            }
        }
    }
    None
}

fn mute_target_fan_id(url: &str) -> Option<String> {
    let parsed = tauri::Url::parse(url).ok()?;
    let segments: Vec<&str> = parsed.path().split('/').filter(|part| !part.is_empty()).collect();
    if segments.len() >= 5
        && segments[0] == "api2"
        && segments[1] == "v2"
        && (segments[2] == "chats" || segments[2] == "users")
    {
        return numeric_id(segments[3]);
    }
    None
}

fn infer_mute_from_url(url: &str) -> bool {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return true;
    };
    let path_lower = parsed.path().to_ascii_lowercase();
    if path_lower.ends_with("/unmute") {
        return false;
    }
    if let Some(method) = query_value(url, "_cf_method") {
        if method.eq_ignore_ascii_case("DELETE") {
            return false;
        }
    }
    true
}

async fn store_chat_ppvs(
    store: &SupabaseStore,
    model_id: &str,
    fan_id: &str,
    fan_name: &str,
    value: &Value,
    is_backfill: bool,
) -> Result<Vec<CandidateEvent>, DbError> {
    let Some(list) = value.get("list").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    struct PendingTip {
        msg_id: String,
        amount: f64,
        created_at: String,
        text: String,
        sender_hint: String,
    }
    struct PendingPpv {
        msg_id: String,
        price: f64,
        purchased: bool,
        created_at: String,
    }

    let mut pending_tips = Vec::new();
    let mut tip_db_rows = Vec::new();
    let mut pending_ppvs = Vec::new();
    let mut ppv_db_rows = Vec::new();

    for item in list {
        let Some(object) = item.as_object() else {
            continue;
        };
        if let Some(amount) = chat_tip_amount(object) {
            let Some(of_message_id) = object.get("id").and_then(value_id) else {
                continue;
            };
            let created_at = message_created_at(object).to_owned();
            let text = object
                .get("text")
                .or_else(|| object.get("tipText"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let sender_hint = object
                .get("fromUser")
                .and_then(Value::as_object)
                .map(user_display_hint)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fan_name.to_owned());
            tip_db_rows.push((
                fan_id.to_owned(),
                of_message_id.clone(),
                amount,
                created_at.clone(),
            ));
            pending_tips.push(PendingTip {
                msg_id: of_message_id,
                amount,
                created_at,
                text,
                sender_hint,
            });
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
        let created_at = message_created_at(object).to_owned();
        ppv_db_rows.push((
            fan_id.to_owned(),
            of_message_id.clone(),
            price,
            purchased,
            created_at.clone(),
        ));
        pending_ppvs.push(PendingPpv {
            msg_id: of_message_id,
            price,
            purchased,
            created_at,
        });
    }

    let (inserted_tips_res, ppv_res) = tokio::join!(
        store.insert_chat_tips_batch(model_id, &tip_db_rows),
        store.upsert_ppv_timeline_batch(model_id, &ppv_db_rows)
    );
    let inserted_tips = inserted_tips_res.unwrap_or_default();
    let ppv_states = ppv_res.unwrap_or_default();

    let mut events = Vec::new();
    if !is_backfill {
        for tip in pending_tips {
            if inserted_tips.contains(&tip.msg_id) {
                events.push(CandidateEvent::Sale {
                    sale_kind: SaleKind::Tip,
                    of_event_id: tip.msg_id,
                    of_fan_id: fan_id.to_owned(),
                    fan_name: tip.sender_hint,
                    gross_amount: tip.amount,
                    net_amount: net_from_gross(tip.amount),
                    preview: tip.text,
                    created_at: tip.created_at,
                    live_transition: false,
                });
            }
        }
        for ppv in pending_ppvs {
            let Some(res) = ppv_states.get(&ppv.msg_id) else {
                continue;
            };
            if res.existed {
                if !res.prev_purchased && ppv.purchased {
                    events.push(CandidateEvent::Sale {
                        sale_kind: SaleKind::Ppv,
                        of_event_id: ppv.msg_id,
                        of_fan_id: fan_id.to_owned(),
                        fan_name: fan_name.to_owned(),
                        gross_amount: ppv.price,
                        net_amount: net_from_gross(ppv.price),
                        preview: format!("PPV desbloqueado (${:.2})", ppv.price),
                        created_at: ppv.created_at,
                        live_transition: true,
                    });
                }
            } else if ppv.purchased {
                events.push(CandidateEvent::Sale {
                    sale_kind: SaleKind::Ppv,
                    of_event_id: ppv.msg_id,
                    of_fan_id: fan_id.to_owned(),
                    fan_name: fan_name.to_owned(),
                    gross_amount: ppv.price,
                    net_amount: net_from_gross(ppv.price),
                    preview: format!("PPV comprado (${:.2})", ppv.price),
                    created_at: ppv.created_at,
                    live_transition: false,
                });
            }
        }
    }
    Ok(events)
}

fn is_chats_inbox_url(url: &str) -> bool {
    if url.contains("_cf_muted_list=1") {
        return false;
    }
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
        let chat_muted = extract_mute_flag(chat_obj) == Some(true)
            || chat_obj
                .get("withUser")
                .and_then(Value::as_object)
                .and_then(extract_mute_flag)
                == Some(true);
        let Some(last_msg) = chat_obj.get("lastMessage").and_then(Value::as_object) else {
            continue;
        };
        let Some(of_message_id) = last_msg.get("id").and_then(value_id) else {
            continue;
        };
        if !chat_muted {
            if let Some(n) = parse_numeric_id(&of_message_id) {
                max_msg_id = max_msg_id.max(n);
            }
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
        } else if !chat_muted && from_id.is_some() {
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
                .take_while(|character| {
                    character.is_ascii_digit() || *character == ',' || *character == '.'
                })
                .collect();
            let parsed = cleaned
                .trim_matches(|character| character == ',' || character == '.')
                .replace(',', "");
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

async fn save_balances(store: &SupabaseStore, model_id: &str, value: &Value) -> Result<bool, DbError> {
    let Some(object) = value.as_object() else {
        return Err(DbError::Invalid(
            "los balances de OnlyFans no son un objeto".to_owned(),
        ));
    };
    let current = first_number(object, &["current", "payoutAvailable", "available"]);
    let pending = first_number(object, &["pending", "payoutPending"]);
    store.save_onlyfans_balances(model_id, current, pending).await
}

fn timestamp_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() || trimmed.starts_with("1970-01-01") {
                return None;
            }
            normalize_timestamp_rfc3339(trimmed)
        }
        Value::Number(number) => {
            let raw = number.to_string();
            normalize_timestamp_rfc3339(&raw)
        }
        _ => None,
    }
}

fn fan_arrived_at(object: &serde_json::Map<String, Value>) -> Option<String> {
    const KEYS: &[&str] = &[
        "subscribeAt",
        "subscribedAt",
        "subscribeDate",
        "startDate",
        "subscribedOnDate",
        "renewedAt",
    ];
    let stamp = |source: &serde_json::Map<String, Value>| {
        KEYS.iter()
            .find_map(|key| source.get(*key).and_then(timestamp_from_value))
    };
    let entry_stamp = |source: &serde_json::Map<String, Value>| {
        stamp(source)
            .or_else(|| source.get("date").and_then(timestamp_from_value))
            .or_else(|| source.get("createdAt").and_then(timestamp_from_value))
            .or_else(|| source.get("created_at").and_then(timestamp_from_value))
    };
    for blob_key in ["subscribedByData", "subscribedOnData", "subscribedOn"] {
        if let Some(blob) = object.get(blob_key).and_then(Value::as_object) {
            if let Some(found) = stamp(blob) {
                return Some(found);
            }
            if let Some(subs) = blob.get("subscribes").and_then(Value::as_array) {
                if let Some(current) = subs.iter().filter_map(Value::as_object).find_map(|entry| {
                    if entry.get("isCurrent").and_then(bool_value) == Some(true) {
                        entry_stamp(entry)
                    } else {
                        None
                    }
                }) {
                    return Some(current);
                }
                for entry in subs {
                    if let Some(entry_obj) = entry.as_object() {
                        if let Some(found) = entry_stamp(entry_obj) {
                            return Some(found);
                        }
                    }
                }
            }
        }
    }
    stamp(object)
}

fn is_subscription_notification(
    item: &serde_json::Map<String, Value>,
    subscribed_tab: bool,
) -> bool {
    let mut kind_parts = Vec::new();
    for key in ["type", "subType", "category"] {
        if let Some(text) = item.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                kind_parts.push(trimmed.to_ascii_lowercase());
            }
        }
    }
    let kind_str = kind_parts.join(" ");
    let raw_text = item
        .get("text")
        .or_else(|| item.get("replacedText"))
        .or_else(|| item.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let text_str = crate::notifications::strip_html_preview(raw_text).to_lowercase();

    let is_negative = kind_str.contains("unsubscrib")
        || kind_str.contains("expir")
        || kind_str.contains("cancel")
        || kind_str.contains("renew_off")
        || text_str.contains("unsubscribed")
        || text_str.contains("expired")
        || text_str.contains("expirad")
        || text_str.contains("cancel");
    if is_negative {
        return false;
    }

    if kind_str.contains("subscrib") || kind_str.contains("suscri") {
        return true;
    }
    if text_str.contains("subscribed to your")
        || text_str.contains("has subscribed")
        || text_str.contains("resubscribed")
        || text_str.contains("suscrito a tu")
        || text_str.contains("suscritos a su")
        || text_str.contains("se ha suscrito")
        || text_str.contains("se suscribió")
        || text_str.contains("se suscribio")
    {
        return true;
    }
    if subscribed_tab {
        let clearly_other = kind_str.contains("like")
            || kind_str.contains("comment")
            || kind_str.contains("mention")
            || kind_str.contains("favorite")
            || kind_str.contains("tip")
            || kind_str.contains("message")
            || kind_str.contains("purchase")
            || kind_str.contains("stream");
        return !clearly_other;
    }
    false
}

fn notification_fan_id(item: &serde_json::Map<String, Value>) -> Option<String> {
    let raw = item
        .get("user")
        .and_then(Value::as_object)
        .and_then(|user| user.get("id"))
        .and_then(value_id)
        .or_else(|| {
            item.get("fromUser")
                .and_then(Value::as_object)
                .and_then(|user| user.get("id"))
                .and_then(value_id)
        })
        .or_else(|| {
            item.get("subscriber")
                .and_then(Value::as_object)
                .and_then(|user| user.get("id"))
                .and_then(value_id)
        })
        .or_else(|| item.get("user_id").and_then(value_id))
        .or_else(|| item.get("userId").and_then(value_id))
        .or_else(|| item.get("fromUserId").and_then(value_id))
        .or_else(|| item.get("subscriberId").and_then(value_id))?;
    Some(numeric_id(&raw).unwrap_or(raw))
}

fn notification_arrived_at(item: &serde_json::Map<String, Value>) -> Option<String> {
    ["createdAt", "created_at", "date", "sentAt"]
        .iter()
        .find_map(|key| item.get(*key).and_then(timestamp_from_value))
        .or_else(|| {
            item.get("user")
                .and_then(Value::as_object)
                .and_then(fan_arrived_at)
        })
}

fn extract_subscription_notification_arrivals(
    creator_of_id: Option<&str>,
    url: &str,
    value: &Value,
) -> (Vec<(String, String)>, Vec<FanUpsertOwned>) {
    let subscribed_tab = query_value(url, "type")
        .map(|kind| {
            let lower = kind.to_ascii_lowercase();
            lower == "subscribed" || lower == "subscribes" || lower == "subscriptions"
        })
        .unwrap_or(false);
    let Some(list) = value
        .get("list")
        .or_else(|| value.get("items"))
        .or_else(|| value.get("notifications"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
    else {
        return (Vec::new(), Vec::new());
    };
    let creator_id = creator_of_id.map(str::trim).filter(|id| !id.is_empty());
    let mut arrivals = Vec::new();
    let mut fans = Vec::new();
    for entry in list {
        let Some(item) = entry.as_object() else {
            continue;
        };
        if !is_subscription_notification(item, subscribed_tab) {
            continue;
        }
        let Some(fan_id) = notification_fan_id(item) else {
            continue;
        };
        if creator_id == Some(fan_id.as_str()) {
            continue;
        }
        let Some(arrived_at) = notification_arrived_at(item) else {
            continue;
        };
        arrivals.push((fan_id, arrived_at));
        if let Some(user_val) = item.get("user").filter(|u| u.is_object()) {
            if let Some((fan, _)) = extract_subscriber_upsert(user_val) {
                if creator_id != Some(fan.of_fan_id.as_str()) {
                    fans.push(fan);
                }
            }
        }
    }
    (arrivals, fans)
}

async fn save_subscription_notifications(
    store: &SupabaseStore,
    model_id: &str,
    url: &str,
    value: &Value,
) -> Result<bool, DbError> {
    let creator_of_id = store
        .get_model(model_id)
        .await
        .ok()
        .and_then(|model| model.of_user_id);
    let (arrivals, fans) =
        extract_subscription_notification_arrivals(creator_of_id.as_deref(), url, value);
    if arrivals.is_empty() && fans.is_empty() {
        return Ok(false);
    }
    let (arrival_res, fan_res) = tokio::join!(
        store.record_fan_arrivals_batch(model_id, &arrivals),
        store.upsert_fans_batch(model_id, &fans)
    );
    let _ = fan_res;
    Ok(arrival_res.unwrap_or(false))
}

fn extract_live_ws_sub_arrivals(
    creator_of_id: Option<&str>,
    body: &str,
) -> Vec<(String, String)> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(root) = value.as_object() else {
        return Vec::new();
    };
    let creator_id = creator_of_id.map(str::trim).filter(|id| !id.is_empty());
    let mut out = Vec::new();
    let inspect = |obj: &serde_json::Map<String, Value>, force_sub: bool| -> Option<(String, String)> {
        if !force_sub && !is_subscription_notification(obj, false) {
            return None;
        }
        let fan_id = notification_fan_id(obj).or_else(|| {
            if force_sub {
                obj.get("id")
                    .and_then(value_id)
                    .map(|id| numeric_id(&id).unwrap_or(id))
            } else {
                None
            }
        })?;
        if creator_id == Some(fan_id.as_str()) {
            return None;
        }
        let arrived_at =
            notification_arrived_at(obj).unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        Some((fan_id, arrived_at))
    };

    for key in ["new_notification", "notification", "toast"] {
        if let Some(obj) = root.get(key).and_then(Value::as_object) {
            if let Some(found) = inspect(obj, false) {
                out.push(found);
            }
        }
    }
    for key in ["new_subscriber", "subscribed"] {
        if let Some(obj) = root.get(key).and_then(Value::as_object) {
            if let Some(found) = inspect(obj, true) {
                out.push(found);
            }
        }
    }
    if out.is_empty() && is_subscription_notification(root, false) {
        if let Some(found) = inspect(root, false) {
            out.push(found);
        }
    }
    out
}

fn spend_stamp(
    object: &serde_json::Map<String, Value>,
    data: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    const KEYS: &[&str] = &["lastSpendAt", "lastPurchaseAt", "lastTransactionAt"];
    for source in [Some(object), data] {
        let Some(source) = source else { continue };
        if let Some(found) = KEYS
            .iter()
            .find_map(|key| source.get(*key).and_then(timestamp_from_value))
        {
            return Some(found);
        }
    }
    None
}

fn publication_objects(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    if let Some(list) = value
        .get("list")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
    {
        return list
            .iter()
            .filter_map(Value::as_object)
            .filter(|item| looks_like_publication(item))
            .collect();
    }
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    if looks_like_publication(object) {
        return vec![object];
    }
    for nested_key in ["post", "data"] {
        if let Some(inner) = object.get(nested_key).and_then(Value::as_object) {
            if looks_like_publication(inner) {
                return vec![inner];
            }
        }
    }
    Vec::new()
}

fn looks_like_publication(object: &serde_json::Map<String, Value>) -> bool {
    object.get("id").is_some()
        && (object.get("postedAt").is_some()
            || object.get("postedAtPrecise").is_some()
            || object.get("createdAt").is_some()
            || object.get("postedDate").is_some()
            || object.get("rawText").is_some()
            || object.get("text").is_some()
            || object.get("isPublished").is_some()
            || object.get("author").is_some()
            || object.get("responseType").and_then(Value::as_str) == Some("post"))
}

fn publication_created_at(object: &serde_json::Map<String, Value>) -> String {
    ["postedAt", "createdAt", "postedDate", "postedAtPrecise"]
        .iter()
        .find_map(|key| object.get(*key).and_then(timestamp_from_value))
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
}

async fn save_publication(
    store: &SupabaseStore,
    model_id: &str,
    value: &Value,
    author: Option<String>,
) -> Result<bool, DbError> {
    let objects = publication_objects(value);
    let mut items = Vec::with_capacity(objects.len());
    for object in objects {
        let Some(of_post_id) = object.get("id").and_then(value_id) else {
            continue;
        };
        items.push((of_post_id, publication_created_at(object)));
    }
    store
        .record_publications_batch(model_id, &items, author.as_deref())
        .await
}

async fn save_creator_posts(
    store: &SupabaseStore,
    model_id: &str,
    url: &str,
    value: &Value,
) -> Result<bool, DbError> {
    let Some(expected_creator) = store
        .get_model(model_id)
        .await
        .ok()
        .and_then(|model| model.of_user_id)
        .filter(|id| !id.trim().is_empty())
    else {
        return Ok(false);
    };
    let path_user_id = tauri::Url::parse(url).ok().and_then(|parsed| {
        let parts: Vec<&str> = parsed.path().split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() == 5 && parts[2] == "users" && parts[4] == "posts" {
            Some(parts[3].to_owned())
        } else {
            None
        }
    });
    if path_user_id.as_deref() != Some(expected_creator.as_str()) {
        return Ok(false);
    }
    let objects = publication_objects(value);
    let mut items = Vec::with_capacity(objects.len());
    for object in objects {
        if let Some(post_author_id) = object
            .get("author")
            .and_then(Value::as_object)
            .and_then(|author| author.get("id"))
            .and_then(value_id)
        {
            if post_author_id != expected_creator {
                continue;
            }
        }
        let Some(of_post_id) = object.get("id").and_then(value_id) else {
            continue;
        };
        items.push((of_post_id, publication_created_at(object)));
    }
    store.record_publications_batch(model_id, &items, None).await
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
    let expired = object
        .get("subscribedOnExpiredNow")
        .and_then(Value::as_bool)
        == Some(true)
        || object
            .get("subscribedIsExpiredNow")
            .and_then(Value::as_bool)
            == Some(true)
        || object.get("subscribedByExpire").and_then(Value::as_bool) == Some(true);
    if expired {
        return Some("Expired".to_owned());
    }
    if let Some(status) = data
        .and_then(|item| item.get("status"))
        .and_then(Value::as_str)
    {
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
        for key in [
            "tipsSumm",
            "messagesSumm",
            "subscribesSumm",
            "postsSumm",
            "streamsSumm",
        ] {
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
    let parts = [
        "tipsSumm",
        "messagesSumm",
        "subscribesSumm",
        "postsSumm",
        "streamsSumm",
    ];
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

async fn chart_earnings(
    store: &SupabaseStore,
    model_id: &str,
    url: &str,
    value: &Value,
) -> Result<Option<Vec<EarningDraft>>, DbError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if object.get("list").and_then(Value::as_array).is_some()
        && !earnings_from_payload(value).is_empty()
    {
        return Ok(None);
    }
    let body = object
        .get("data")
        .and_then(Value::as_object)
        .unwrap_or(object);
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
                    drafts.extend(
                        store_chart_series(store, model_id, url, kind, &synthetic).await?,
                    );
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
            "tips" | "tips_chat" | "tips_profile" | "tips_post" | "tips_stream" | "tips_story" => {
                "tip"
            }
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
        drafts.extend(store_chart_series(store, model_id, url, kind, series).await?);
    }
    Ok(Some(drafts))
}

async fn store_chart_series(
    store: &SupabaseStore,
    model_id: &str,
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
            let Some(date) = point.get("date").and_then(Value::as_str) else {
                continue;
            };
            let amount = first_number(point, &["count", "amount", "sum"]).unwrap_or(0.0);
            if let Some(stamp) = bucket_noon(date) {
                dated.push((date_key(date), stamp, amount));
            }
        }
    }
    if dated.is_empty() {
        let amount = net_total
            .filter(|value| *value > 0.0)
            .or_else(|| gross_total.filter(|value| *value > 0.0));
        if let Some(amount) = amount {
            let date = query_value(url, "endDate")
                .unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
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
        store
            .delete_chart_span(model_id, kind, start, end.saturating_add(1))
            .await?;
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
        .or_else(|| gross_total.map(|gross| gross * ONLYFANS_NET_RATIO));
    match net {
        Some(net) if close(sum, net) => (1.0, 1.0 / ONLYFANS_NET_RATIO),
        Some(net) if net_total.is_none() => ((net / sum).min(1.0), 1.0),
        Some(net) => (net / sum, (net / ONLYFANS_NET_RATIO) / sum),
        None => (1.0, 1.0 / ONLYFANS_NET_RATIO),
    }
}

fn span_unix(stamps: &[String]) -> Option<(i64, i64)> {
    let mut values = stamps.iter().filter_map(|stamp| {
        chrono::DateTime::parse_from_rfc3339(stamp)
            .ok()
            .map(|date| date.timestamp())
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
        text.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(16)
            .collect()
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
    Some(local.with_timezone(&chrono::Utc).to_rfc3339())
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

pub async fn ingest_observation(
    store: &SupabaseStore,
    model_id: &str,
    fan_id: &str,
    profile: Option<&ObservedProfile>,
) -> Result<(), DbError> {
    let fan_id = numeric_id(fan_id).unwrap_or_else(|| fan_id.trim().to_owned());
    if fan_id.is_empty() {
        return Ok(());
    }
    if let Some(profile) = profile {
        let official = profile.total_spend.is_some();
        store
            .upsert_fans_batch(
                model_id,
                &[FanUpsertOwned {
                    of_fan_id: fan_id,
                    username: String::new(),
                    sub_status: profile.status.clone(),
                    sub_cost: profile.cost,
                    sub_duration: profile.duration.clone(),
                    auto_renewal: None,
                    total_spend: profile.total_spend,
                    lists: Vec::new(),
                    messages_summ: None,
                    tips_summ: None,
                    official_spend: official,
                    last_spend_at: profile.last_spend_at.clone(),
                    highest_spend: profile.highest_spend,
                    is_muted: None,
                }],
            )
            .await?;
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
    let type_text = object
        .get("type")
        .or_else(|| object.get("paymentType"))
        .or_else(|| object.get("transactionType"))
        .or_else(|| object.get("category"))
        .and_then(Value::as_str);
    let desc_text = object
        .get("description")
        .or_else(|| object.get("descriptionText"))
        .and_then(Value::as_str);
    if is_non_earning_transaction(type_text, desc_text) {
        return None;
    }
    let amounts_object = object.get("amounts").and_then(Value::as_object);
    let amount = object
        .get("amount")
        .and_then(number_value)
        .or_else(|| {
            amounts_object
                .and_then(|item| item.get("gross"))
                .and_then(number_value)
        })
        .or_else(|| object.get("gross").and_then(number_value));
    let net = object.get("net").and_then(number_value).or_else(|| {
        amounts_object
            .and_then(|item| item.get("net"))
            .and_then(number_value)
    });
    let fee = object.get("fee").and_then(number_value).or_else(|| {
        amounts_object
            .and_then(|item| item.get("fee"))
            .and_then(number_value)
    });
    let (gross_amount, net_amount) = amounts(amount, net, fee)?;
    if !gross_amount.is_finite() || gross_amount <= 0.0 || net_amount < 0.0 {
        return None;
    }
    let kind = transaction_kind(type_text, desc_text).unwrap_or_else(|| {
        let lower_desc = desc_text.unwrap_or("").to_lowercase();
        if lower_desc.contains("/chats/") || lower_desc.contains("chat") {
            TransactionType::Message
        } else {
            TransactionType::Post
        }
    });
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

fn is_non_earning_transaction(type_name: Option<&str>, description: Option<&str>) -> bool {
    for candidate in [type_name, description].into_iter().flatten() {
        let lower = candidate.to_lowercase();
        if lower.contains("chargeback")
            || lower.contains("refund")
            || lower.contains("reversal")
            || lower.contains("withdrawal")
            || lower.contains("payout")
            || lower.contains("reembolso")
            || lower.contains("retiro")
            || lower.contains("contracargo")
            || lower.contains("devolucion")
            || lower.contains("devolución")
            || lower.contains("reverso")
            || lower.contains("canceled")
            || lower.contains("cancelled")
        {
            return true;
        }
    }
    false
}

fn net_from_gross(gross: f64) -> f64 {
    (gross * ONLYFANS_NET_RATIO * 100.0).round() / 100.0
}

fn gross_from_net(net: f64) -> f64 {
    (net / ONLYFANS_NET_RATIO * 100.0).round() / 100.0
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
    let value = value.to_lowercase();
    if value.contains("tip") || value.contains("propina") {
        Some(TransactionType::Tip)
    } else if value.contains("message")
        || value.contains("chat")
        || value.contains("mensaje")
        || value.contains("ppv")
        || value.contains("/my/chats/")
    {
        Some(TransactionType::Message)
    } else if value.contains("subscri")
        || value.contains("suscrip")
        || value.contains("rebill")
        || value.contains("renovaci")
        || value.contains("trial")
    {
        Some(TransactionType::Sub)
    } else if value.contains("post")
        || value.contains("public")
        || value.contains("story")
        || value.contains("historia")
        || value.contains("bundle")
        || value.contains("paquete")
    {
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
    use serde_json::json;

    #[test]
    fn classifies_publications_creator_posts_and_subscribers() {
        assert_eq!(
            classify_url("https://onlyfans.com/api2/v2/posts?_cf_publish=1"),
            Some(CaptureKind::Publication)
        );
        assert_eq!(
            classify_url("https://onlyfans.com/api2/v2/users/88/posts?limit=10"),
            Some(CaptureKind::CreatorPosts)
        );
        assert_eq!(
            classify_url("https://onlyfans.com/api2/v2/subscriptions/subscribers?limit=20"),
            Some(CaptureKind::Subscriber)
        );
    }

    #[test]
    fn normalizes_publication_posted_at_and_posted_at_precise() {
        let with_both = json!({
            "id": 99001,
            "postedAt": "2026-11-26T06:36:52+00:00",
            "postedAtPrecise": "1795588612.000000",
            "text": "post de prueba"
        });
        let objects = publication_objects(&with_both);
        assert_eq!(objects.len(), 1);
        assert_eq!(
            publication_created_at(objects[0]),
            "2026-11-26T06:36:52+00:00"
        );

        let precise_only = json!({
            "id": 99002,
            "postedAtPrecise": "1795588612.000000",
            "rawText": "solo epoch"
        });
        let objs = publication_objects(&precise_only);
        assert_eq!(objs.len(), 1);
        let stamp = publication_created_at(objs[0]);
        assert!(chrono::DateTime::parse_from_rfc3339(&stamp).is_ok());
    }

    #[test]
    fn extracts_fan_arrival_from_subscribed_by_data_even_when_subscribed_on_data_exists() {
        let fan = json!({
            "id": 55501,
            "username": "newfan",
            "subscribedBy": true,
            "subscribedOnData": { "subscribePrice": 0 },
            "subscribedByData": {
                "subscribeAt": "2026-11-26T06:40:00+00:00"
            }
        });
        assert_eq!(
            fan_arrived_at(fan.as_object().unwrap()).as_deref(),
            Some("2026-11-26T06:40:00+00:00")
        );
    }

    #[test]
    fn classifies_and_extracts_subscription_notifications_and_ws_events() {
        assert_eq!(
            classify_url("https://onlyfans.com/api2/v2/users/notifications?limit=50&type=subscribed"),
            Some(CaptureKind::Notifications)
        );
        let payload = json!({
            "list": [
                {
                    "id": "900001",
                    "type": "subscribed",
                    "subType": "subscribed",
                    "text": "suscritos a su perfil!",
                    "createdAt": "2026-11-27T04:30:00+00:00",
                    "user": {
                        "id": 404982216,
                        "name": "John99",
                        "username": "u404982216"
                    }
                },
                {
                    "id": "900002",
                    "type": "favorited",
                    "text": "me ha gustado tu post",
                    "createdAt": "2026-11-27T04:20:00+00:00",
                    "user": {
                        "id": 604129243,
                        "name": "Raul",
                        "username": "u604129243"
                    }
                },
                {
                    "id": "900003",
                    "type": "subscribed",
                    "createdAt": "2026-11-27T00:15:00+00:00",
                    "user_id": 777001
                }
            ]
        });
        let (arrivals, fans) = extract_subscription_notification_arrivals(
            Some("100"),
            "https://onlyfans.com/api2/v2/users/notifications?limit=50&type=all",
            &payload,
        );
        assert_eq!(arrivals.len(), 2);
        assert_eq!(arrivals[0].0, "404982216");
        assert_eq!(arrivals[0].1, "2026-11-27T04:30:00+00:00");
        assert_eq!(arrivals[1].0, "777001");
        assert_eq!(arrivals[1].1, "2026-11-27T00:15:00+00:00");
        assert_eq!(fans.len(), 1);
        assert_eq!(fans[0].of_fan_id, "404982216");

        let ws_body = json!({
            "new_notification": {
                "id": "900099",
                "type": "subscribed",
                "text": "subscribed to your profile!",
                "createdAt": "2026-11-27T04:31:00+00:00",
                "user": { "id": 404982216, "username": "u404982216" }
            }
        })
        .to_string();
        let ws_arrivals = extract_live_ws_sub_arrivals(Some("100"), &ws_body);
        assert_eq!(ws_arrivals.len(), 1);
        assert_eq!(ws_arrivals[0].0, "404982216");
        assert_eq!(ws_arrivals[0].1, "2026-11-27T04:31:00+00:00");
    }

    #[test]
    fn extracts_fan_arrival_from_subscribes_date_field() {
        let fan = json!({
            "id": 404982216,
            "username": "u404982216",
            "subscribedOnData": {
                "subscribes": [
                    {
                        "date": "2026-11-27T04:30:00+00:00",
                        "isCurrent": true
                    }
                ]
            }
        });
        assert_eq!(
            fan_arrived_at(fan.as_object().unwrap()).as_deref(),
            Some("2026-11-27T04:30:00+00:00")
        );
    }

    #[test]
    fn suppresses_inbox_messages_for_muted_fans_and_classifies_mute_urls() {
        assert_eq!(
            classify_url("https://onlyfans.com/api2/v2/chats/884422/mute?_cf_method=POST"),
            Some(CaptureKind::ChatMute)
        );
        assert_eq!(
            classify_url("https://onlyfans.com/api2/v2/chats/884422/unmute?_cf_method=POST"),
            Some(CaptureKind::ChatMute)
        );
        assert!(infer_mute_from_url(
            "https://onlyfans.com/api2/v2/chats/884422/mute?_cf_method=POST"
        ));
        assert!(!infer_mute_from_url(
            "https://onlyfans.com/api2/v2/chats/884422/mute?_cf_method=DELETE"
        ));
        assert!(!infer_mute_from_url(
            "https://onlyfans.com/api2/v2/chats/884422/unmute?_cf_method=POST"
        ));

        let inbox = json!({
            "list": [
                {
                    "isMutedNotifications": true,
                    "withUser": { "id": 884422, "name": "Muted Fan", "username": "mutedfan" },
                    "lastMessage": {
                        "id": 9001,
                        "text": "hola",
                        "createdAt": "2026-11-27T20:00:00+00:00",
                        "fromUser": { "id": 884422, "username": "mutedfan" }
                    }
                },
                {
                    "isMutedNotifications": false,
                    "withUser": {
                        "id": 884423,
                        "name": "Silenciado Lista",
                        "username": "listmuted",
                        "listsStates": [
                            { "name": "Silenciados", "hasUser": true }
                        ]
                    },
                    "lastMessage": {
                        "id": 9002,
                        "text": "mensaje silenciado por lista",
                        "createdAt": "2026-11-27T20:01:00+00:00",
                        "fromUser": { "id": 884423, "username": "listmuted" }
                    }
                },
                {
                    "isMutedNotifications": false,
                    "withUser": { "id": 884424, "name": "Active Fan", "username": "activefan" },
                    "lastMessage": {
                        "id": 9003,
                        "text": "hola activa",
                        "createdAt": "2026-11-27T20:02:00+00:00",
                        "fromUser": { "id": 884424, "username": "activefan" }
                    }
                }
            ]
        });
        // Note: chat 2 has explicit isMutedNotifications=false on chat_obj, while withUser has listsStates Silenciados=true!
        let (max_id, events) = extract_inbox_events(Some("100"), &inbox);
        assert_eq!(max_id, 9003);
        assert_eq!(events.len(), 1);
        match &events[0] {
            CandidateEvent::IncomingMessage { of_fan_id, .. } => {
                assert_eq!(of_fan_id, "884424");
            }
            _ => panic!("expected incoming message from active fan only"),
        }
    }
}
