use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::db::{utc_now, Database};

static NOTIFICATION_SEQ: AtomicU64 = AtomicU64::new(1);
const MAX_RECENT_NOTIFICATIONS: usize = 50;
const SALE_DEDUP_WINDOW: Duration = Duration::from_secs(120);
const BURST_WINDOW: Duration = Duration::from_secs(8);
const NOTIFIED_EVENT_TTL: Duration = Duration::from_secs(14 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationKind {
    Message,
    Ppv,
    Tip,
}

impl NotificationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Ppv => "ppv",
            Self::Tip => "tip",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "ppv" => Self::Ppv,
            "tip" => Self::Tip,
            _ => Self::Message,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaleKind {
    Ppv,
    Tip,
}

impl SaleKind {
    pub fn into_notification_kind(self) -> NotificationKind {
        match self {
            Self::Ppv => NotificationKind::Ppv,
            Self::Tip => NotificationKind::Tip,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Ppv => "ppv",
            Self::Tip => "tip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchSource {
    /// Passive/none: no notifications or seeding.
    None,
    /// Real-time WebSocket frame from OnlyFans (`ws2.onlyfans.com`). Always live.
    WebSocket,
    /// `/api2/v2/chats` inbox list. First poll per model seeds the baseline watermark;
    /// all subsequent polls emit any message/tip newer than the stored watermark.
    Inbox { max_msg_id: u64 },
    /// `/api2/v2/chats/<fanId>/messages` (non-backfill). Emits live PPV 0->1 transitions
    /// and any messages/tips with ID strictly above the model's stored watermark.
    ChatThread { max_msg_id: u64 },
    /// `/api2/v2/payouts/transactions`. First poll per model seeds existing transactions;
    /// subsequent polls emit newly inserted PPV/Tip sales.
    Transactions,
}

#[derive(Debug, Clone)]
pub struct NotificationBatch {
    pub source: BatchSource,
    pub candidates: Vec<CandidateEvent>,
}

impl NotificationBatch {
    pub fn empty() -> Self {
        Self {
            source: BatchSource::None,
            candidates: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppNotification {
    pub id: String,
    pub kind: NotificationKind,
    pub model_id: i64,
    pub model_name: String,
    pub model_username: String,
    pub model_avatar: Option<String>,
    pub of_fan_id: String,
    pub fan_name: String,
    pub amount: Option<f64>,
    pub net_amount: Option<f64>,
    pub preview: String,
    pub created_at: String,
}

/// The conversation the chatter is looking at. That fan still gets a toast;
/// only the unread badge is skipped, because the thread is already on screen.
#[derive(Debug, Clone)]
pub struct LiveFocus {
    pub model_id: i64,
    pub of_fan_id: String,
}

#[derive(Debug, Clone)]
pub enum CandidateEvent {
    IncomingMessage {
        of_message_id: String,
        of_fan_id: String,
        fan_name: String,
        preview: String,
        created_at: String,
    },
    Sale {
        sale_kind: SaleKind,
        of_event_id: String,
        of_fan_id: String,
        fan_name: String,
        gross_amount: f64,
        net_amount: f64,
        preview: String,
        created_at: String,
        live_transition: bool,
    },
}

struct SaleFingerprint {
    model_id: i64,
    sale_kind: SaleKind,
    of_fan_id: String,
    amount_cents: i64,
    recorded_at: Instant,
}

#[derive(Debug, Clone)]
struct Burst {
    count: u32,
    generation: u64,
    model_name: String,
    fan_name: String,
    started: Instant,
}

#[derive(Debug, Clone)]
struct NotifyRow {
    last_seen_message_id: String,
    last_message_timestamp: Option<String>,
    unread_count: i64,
    inbox_seeded: bool,
    transactions_seeded: bool,
}

impl Default for NotifyRow {
    fn default() -> Self {
        Self {
            last_seen_message_id: String::new(),
            last_message_timestamp: None,
            unread_count: 0,
            inbox_seeded: false,
            transactions_seeded: false,
        }
    }
}

struct Produced {
    emitted: Vec<AppNotification>,
    unread_changed: bool,
}

pub struct NotificationBus {
    bursts: Arc<Mutex<HashMap<(i64, String), Burst>>>,
    burst_seq: Arc<AtomicU64>,
    recent_sales: Mutex<VecDeque<SaleFingerprint>>,
    messages_enabled: AtomicBool,
    sales_enabled: AtomicBool,
    os_push_enabled: AtomicBool,
}

impl NotificationBus {
    pub fn new() -> Self {
        Self {
            bursts: Arc::new(Mutex::new(HashMap::new())),
            burst_seq: Arc::new(AtomicU64::new(1)),
            recent_sales: Mutex::new(VecDeque::new()),
            messages_enabled: AtomicBool::new(true),
            sales_enabled: AtomicBool::new(true),
            os_push_enabled: AtomicBool::new(true),
        }
    }

    pub fn set_delivery(&self, messages_enabled: bool, sales_enabled: bool, os_push_enabled: bool) {
        self.messages_enabled.store(messages_enabled, Ordering::Relaxed);
        self.sales_enabled.store(sales_enabled, Ordering::Relaxed);
        self.os_push_enabled.store(os_push_enabled, Ordering::Relaxed);
    }

    pub fn recent(&self, database: &Database) -> Result<Vec<AppNotification>, String> {
        database
            .with_connection(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, kind, model_id, model_name, model_username, model_avatar,
                            of_fan_id, fan_name, amount, net_amount, preview, created_at
                     FROM notification_history
                     ORDER BY created_at DESC, rowid DESC
                     LIMIT ?1",
                )?;
                let rows = stmt.query_map([MAX_RECENT_NOTIFICATIONS as i64], |row| {
                    Ok(AppNotification {
                        id: row.get(0)?,
                        kind: NotificationKind::parse(&row.get::<_, String>(1)?),
                        model_id: row.get(2)?,
                        model_name: row.get(3)?,
                        model_username: row.get(4)?,
                        model_avatar: row.get(5)?,
                        of_fan_id: row.get(6)?,
                        fan_name: row.get(7)?,
                        amount: row.get(8)?,
                        net_amount: row.get(9)?,
                        preview: row.get(10)?,
                        created_at: row.get(11)?,
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()
            })
            .map_err(|err| err.to_string())
    }

    pub fn clear_history(&self, database: &Database) -> Result<(), String> {
        database
            .with_connection(|conn| conn.execute("DELETE FROM notification_history", []))
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    pub fn dismiss(&self, database: &Database, id: &str) -> Result<(), String> {
        database
            .with_connection(|conn| {
                conn.execute("DELETE FROM notification_history WHERE id = ?1", [id])
            })
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    pub fn unread_counts(&self, database: &Database) -> HashMap<i64, u32> {
        database
            .with_connection(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT model_id, unread_count FROM account_notify_state WHERE unread_count > 0",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })?;
                let mut counts = HashMap::new();
                for row in rows {
                    let (model_id, count) = row?;
                    if count > 0 {
                        counts.insert(model_id, count as u32);
                    }
                }
                Ok(counts)
            })
            .unwrap_or_default()
    }

    pub fn clear_unread(&self, database: &Database, model_id: i64) -> bool {
        database
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE account_notify_state SET unread_count = 0 WHERE model_id = ?1 AND unread_count > 0",
                    [model_id],
                )
            })
            .map(|changed| changed > 0)
            .unwrap_or(false)
    }

    pub fn process_batch(
        &self,
        app: Option<&AppHandle>,
        database: &Database,
        model_id: i64,
        batch: NotificationBatch,
        viewing: Option<&LiveFocus>,
    ) -> Vec<AppNotification> {
        if batch.source == BatchSource::None {
            return Vec::new();
        }
        let seeds_when_empty = matches!(batch.source, BatchSource::Inbox { .. } | BatchSource::Transactions);
        if batch.candidates.is_empty() && !seeds_when_empty {
            return Vec::new();
        }

        let Ok(model) = database.get_model(model_id) else {
            return Vec::new();
        };
        let model_name = if !model.display_name.trim().is_empty() {
            model.display_name.trim().to_owned()
        } else if !model.username.trim().is_empty() {
            format!("@{}", model.username.trim())
        } else {
            format!("Modelo #{model_id}")
        };
        let model_username = model.username.trim().to_owned();
        let model_avatar = model.avatar_url.clone();
        let now_iso = utc_now();
        let cutoff = (chrono::Utc::now() - chrono::Duration::from_std(NOTIFIED_EVENT_TTL).unwrap_or_default()).to_rfc3339();

        let produced = database.with_transaction(|tx| {
            prune_notified_events(tx, &cutoff)?;
            let mut row = load_row(tx, model_id)?;
            let mut candidates = batch.candidates.clone();

            if let BatchSource::Inbox { .. } = batch.source {
                if !row.inbox_seeded {
                    let (fresh, baseline) = partition_fresh(candidates);
                    seed_candidates(tx, model_id, &baseline, &now_iso)?;
                    raise_watermark_num(&mut row, max_candidate_id(&baseline));
                    for cand in &baseline {
                        if let CandidateEvent::IncomingMessage { of_message_id, created_at, .. } = cand {
                            raise_watermark(&mut row, of_message_id, created_at);
                        }
                    }
                    row.inbox_seeded = true;
                    save_row(tx, model_id, &row)?;
                    if fresh.is_empty() {
                        return Ok(Produced {
                            emitted: Vec::new(),
                            unread_changed: false,
                        });
                    }
                    candidates = fresh;
                }
            }
            if matches!(batch.source, BatchSource::Transactions) && !row.transactions_seeded {
                seed_candidates(tx, model_id, &candidates, &now_iso)?;
                row.transactions_seeded = true;
                save_row(tx, model_id, &row)?;
                return Ok(Produced {
                    emitted: Vec::new(),
                    unread_changed: false,
                });
            }
            if let BatchSource::ChatThread { .. } = batch.source {
                if !row.inbox_seeded {
                    let mut live = Vec::new();
                    let mut parked = Vec::new();
                    for cand in candidates.drain(..) {
                        let live_sale = matches!(cand, CandidateEvent::Sale { live_transition: true, .. });
                        if live_sale || candidate_is_fresh(&cand) {
                            live.push(cand);
                        } else {
                            parked.push(cand);
                        }
                    }
                    for cand in &parked {
                        let key = event_key(model_id, cand);
                        claim_event(tx, model_id, &key, &now_iso)?;
                    }
                    raise_watermark_num(&mut row, max_candidate_id(&parked));
                    save_row(tx, model_id, &row)?;
                    if live.is_empty() {
                        return Ok(Produced {
                            emitted: Vec::new(),
                            unread_changed: false,
                        });
                    }
                    candidates = live;
                }
            }

            let mut emitted = Vec::new();
            let mut unread_changed = false;
            for candidate in candidates {
                if let Some(notification) = self.accept_candidate(
                    tx,
                    &mut row,
                    model_id,
                    &model_name,
                    &model_username,
                    &model_avatar,
                    &now_iso,
                    batch.source,
                    candidate,
                    viewing,
                    &mut unread_changed,
                )? {
                    insert_history(tx, &notification)?;
                    trim_history(tx)?;
                    emitted.push(notification);
                }
            }
            save_row(tx, model_id, &row)?;
            Ok(Produced {
                emitted,
                unread_changed,
            })
        });

        let Ok(produced) = produced else {
            return Vec::new();
        };

        if let Some(app) = app {
            for item in &produced.emitted {
                eprintln!(
                    "creatorfloww: aviso {} cuenta {} ({}) fan {}",
                    item.kind.as_str(),
                    item.model_id,
                    item.model_name,
                    item.fan_name
                );
                let _ = app.emit("cf-notification", item);
                self.queue_os_toast(app, item);
            }
            if produced.unread_changed {
                let _ = app.emit("unread-counts-changed", self.unread_counts(database));
            }
        }
        produced.emitted
    }

    fn accept_candidate(
        &self,
        tx: &Transaction<'_>,
        row: &mut NotifyRow,
        model_id: i64,
        model_name: &str,
        model_username: &str,
        model_avatar: &Option<String>,
        now_iso: &str,
        source: BatchSource,
        candidate: CandidateEvent,
        viewing: Option<&LiveFocus>,
        unread_changed: &mut bool,
    ) -> rusqlite::Result<Option<AppNotification>> {
        match candidate {
            CandidateEvent::IncomingMessage {
                of_message_id,
                of_fan_id,
                fan_name,
                preview,
                created_at,
            } => {
                let msg_id = of_message_id.trim();
                if msg_id.is_empty() {
                    return Ok(None);
                }
                let key = format!("msg:{model_id}:{msg_id}");
                let numeric_id = parse_numeric_id(msg_id);
                let watermark = parse_numeric_id(&row.last_seen_message_id).unwrap_or(0);
                if watermark > 0 {
                    if let Some(id) = numeric_id {
                        if id <= watermark {
                            claim_event(tx, model_id, &key, now_iso)?;
                            return Ok(None);
                        }
                    }
                }
                let fan_id = of_fan_id.trim().to_owned();
                let viewing_this = is_viewing(viewing, model_id, &fan_id);
                if !claim_event(tx, model_id, &key, now_iso)? {
                    return Ok(None);
                }
                raise_watermark(row, msg_id, &created_at);
                if !viewing_this {
                    row.unread_count = row.unread_count.saturating_add(1);
                    *unread_changed = true;
                }
                let resolved_fan = resolve_fan_label_tx(tx, model_id, &fan_id, &fan_name)?;
                let clean_preview = clean_preview_or(&preview, "Nuevo mensaje en el chat");
                Ok(Some(AppNotification {
                    id: next_notification_id(),
                    kind: NotificationKind::Message,
                    model_id,
                    model_name: model_name.to_owned(),
                    model_username: model_username.to_owned(),
                    model_avatar: model_avatar.clone(),
                    of_fan_id: fan_id,
                    fan_name: resolved_fan,
                    amount: None,
                    net_amount: None,
                    preview: clean_preview,
                    created_at: timestamp_or(&created_at, now_iso),
                }))
            }
            CandidateEvent::Sale {
                sale_kind,
                of_event_id,
                of_fan_id,
                fan_name,
                gross_amount,
                net_amount,
                preview,
                created_at,
                live_transition,
            } => {
                if !gross_amount.is_finite() || gross_amount <= 0.0 {
                    return Ok(None);
                }
                let ev_id = of_event_id.trim();
                if ev_id.is_empty() {
                    return Ok(None);
                }
                let key = format!("{}:{model_id}:{ev_id}", sale_kind.key());
                let numeric_id = parse_numeric_id(ev_id);
                let watermark = parse_numeric_id(&row.last_seen_message_id).unwrap_or(0);
                let passes_watermark = if live_transition
                    || matches!(source, BatchSource::Transactions | BatchSource::WebSocket)
                {
                    true
                } else if watermark > 0 {
                    numeric_id.map(|id| id > watermark).unwrap_or(true)
                } else {
                    true
                };
                if !passes_watermark {
                    claim_event(tx, model_id, &key, now_iso)?;
                    return Ok(None);
                }
                if !claim_event(tx, model_id, &key, now_iso)? {
                    return Ok(None);
                }
                let fan_id = of_fan_id.trim().to_owned();
                if self.is_duplicate_sale(model_id, sale_kind, &fan_id, gross_amount) {
                    return Ok(None);
                }
                let resolved_fan = resolve_fan_label_tx(tx, model_id, &fan_id, &fan_name)?;
                let rounded_gross = (gross_amount * 100.0).round() / 100.0;
                let rounded_net = if net_amount.is_finite() && net_amount > 0.0 {
                    (net_amount * 100.0).round() / 100.0
                } else {
                    (rounded_gross * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0
                };
                let clean_preview = {
                    let stripped = strip_html_preview(&preview);
                    if !stripped.is_empty() {
                        stripped
                    } else {
                        match sale_kind {
                            SaleKind::Ppv => format!("PPV desbloqueado por ${rounded_gross:.2}"),
                            SaleKind::Tip => format!("Propina recibida de ${rounded_gross:.2}"),
                        }
                    }
                };
                Ok(Some(AppNotification {
                    id: next_notification_id(),
                    kind: sale_kind.into_notification_kind(),
                    model_id,
                    model_name: model_name.to_owned(),
                    model_username: model_username.to_owned(),
                    model_avatar: model_avatar.clone(),
                    of_fan_id: fan_id,
                    fan_name: resolved_fan,
                    amount: Some(rounded_gross),
                    net_amount: Some(rounded_net),
                    preview: clean_preview,
                    created_at: if live_transition || created_at.trim().is_empty() {
                        now_iso.to_owned()
                    } else {
                        created_at
                    },
                }))
            }
        }
    }

    fn is_duplicate_sale(
        &self,
        model_id: i64,
        sale_kind: SaleKind,
        of_fan_id: &str,
        gross_amount: f64,
    ) -> bool {
        let mut recent = self
            .recent_sales
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let now = Instant::now();
        while let Some(front) = recent.front() {
            if now.duration_since(front.recorded_at) > SALE_DEDUP_WINDOW {
                recent.pop_front();
            } else {
                break;
            }
        }
        let amount_cents = (gross_amount * 100.0).round() as i64;
        if amount_cents <= 0 {
            return false;
        }
        let fan_norm = of_fan_id.trim();
        let duplicate = recent.iter().any(|entry| {
            entry.model_id == model_id
                && entry.sale_kind == sale_kind
                && entry.amount_cents == amount_cents
                && (fan_norm.is_empty() || entry.of_fan_id.is_empty() || entry.of_fan_id == fan_norm)
        });
        if !duplicate {
            recent.push_back(SaleFingerprint {
                model_id,
                sale_kind,
                of_fan_id: fan_norm.to_owned(),
                amount_cents,
                recorded_at: now,
            });
        }
        duplicate
    }

    fn queue_os_toast(&self, app: &AppHandle, item: &AppNotification) {
        if !self.os_push_enabled.load(Ordering::Relaxed) {
            return;
        }
        let is_sale = matches!(item.kind, NotificationKind::Ppv | NotificationKind::Tip);
        if is_sale && !self.sales_enabled.load(Ordering::Relaxed) {
            return;
        }
        if !is_sale && !self.messages_enabled.load(Ordering::Relaxed) {
            return;
        }
        if is_sale {
            let amount = item.amount.map(|value| format!(" ${value:.2}")).unwrap_or_default();
            let title = if item.kind == NotificationKind::Ppv {
                format!("Venta PPV{amount} · {}", item.model_name)
            } else {
                format!("Nueva propina{amount} · {}", item.model_name)
            };
            let body = format!("{} — {}", item.fan_name, item.preview);
            show_os_toast(app, &title, &body, item.model_id, &item.of_fan_id);
            return;
        }

        let key = (item.model_id, item.of_fan_id.clone());
        let generation = {
            let mut bursts = self.bursts.lock().unwrap_or_else(|poison| poison.into_inner());
            let now = Instant::now();
            if let Some(burst) = bursts.get_mut(&key) {
                if now.duration_since(burst.started) < BURST_WINDOW {
                    burst.count = burst.count.saturating_add(1);
                    burst.fan_name = item.fan_name.clone();
                    if burst.count == 2 {
                        let generation = burst.generation;
                        let fan_name = burst.fan_name.clone();
                        let model_name = burst.model_name.clone();
                        drop(bursts);
                        self.schedule_burst_summary(app.clone(), key, generation, model_name, fan_name);
                    }
                    return;
                }
            }
            let generation = self.burst_seq.fetch_add(1, Ordering::Relaxed);
            bursts.insert(
                key.clone(),
                Burst {
                    count: 1,
                    generation,
                    model_name: item.model_name.clone(),
                    fan_name: item.fan_name.clone(),
                    started: now,
                },
            );
            generation
        };
        let _ = generation;
        let title = format!("Nuevo mensaje · {}", item.model_name);
        let body = format!("{}: {}", item.fan_name, item.preview);
        show_os_toast(app, &title, &body, item.model_id, &item.of_fan_id);
    }

    fn schedule_burst_summary(
        &self,
        app: AppHandle,
        key: (i64, String),
        generation: u64,
        model_name: String,
        fan_name: String,
    ) {
        let bursts = Arc::clone(&self.bursts);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(BURST_WINDOW).await;
            let summary = {
                let map = bursts.lock().unwrap_or_else(|poison| poison.into_inner());
                map.get(&key).and_then(|burst| {
                    if burst.generation == generation && burst.count > 1 {
                        Some((burst.count, burst.model_name.clone(), burst.fan_name.clone()))
                    } else {
                        None
                    }
                })
            };
            if let Some((count, stored_model, stored_fan)) = summary {
                let model_name = if stored_model.is_empty() { model_name } else { stored_model };
                let fan_name = if stored_fan.is_empty() { fan_name } else { stored_fan };
                show_os_toast(
                    &app,
                    &format!("{count} mensajes · {model_name}"),
                    &format!("{count} mensajes de {fan_name}"),
                    key.0,
                    &key.1,
                );
            }
        });
    }
}

fn show_os_toast(app: &AppHandle, title: &str, body: &str, model_id: i64, of_fan_id: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(err) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .extra("modelId", model_id)
        .extra("ofFanId", of_fan_id.to_owned())
        .show()
    {
        eprintln!("creatorfloww: no se pudo preparar el toast de Windows: {err}");
    }
}

fn is_viewing(viewing: Option<&LiveFocus>, model_id: i64, of_fan_id: &str) -> bool {
    let Some(focus) = viewing else {
        return false;
    };
    focus.model_id == model_id
        && !focus.of_fan_id.trim().is_empty()
        && focus.of_fan_id.trim() == of_fan_id.trim()
}

fn event_key(model_id: i64, cand: &CandidateEvent) -> String {
    match cand {
        CandidateEvent::IncomingMessage { of_message_id, .. } => {
            let id = of_message_id.trim();
            if id.is_empty() {
                String::new()
            } else {
                format!("msg:{model_id}:{id}")
            }
        }
        CandidateEvent::Sale {
            sale_kind,
            of_event_id,
            ..
        } => {
            let id = of_event_id.trim();
            if id.is_empty() {
                String::new()
            } else {
                format!("{}:{model_id}:{id}", sale_kind.key())
            }
        }
    }
}

const FRESH_WINDOW_SECS: i64 = 180;

fn candidate_created_at(cand: &CandidateEvent) -> &str {
    match cand {
        CandidateEvent::IncomingMessage { created_at, .. } | CandidateEvent::Sale { created_at, .. } => created_at,
    }
}

fn candidate_is_fresh(cand: &CandidateEvent) -> bool {
    let Ok(stamp) = chrono::DateTime::parse_from_rfc3339(candidate_created_at(cand).trim()) else {
        return false;
    };
    let age = chrono::Utc::now().signed_duration_since(stamp.with_timezone(&chrono::Utc));
    age < chrono::Duration::seconds(FRESH_WINDOW_SECS) && age > chrono::Duration::seconds(-600)
}

fn max_candidate_id(candidates: &[CandidateEvent]) -> u64 {
    candidates
        .iter()
        .filter_map(|cand| {
            let id = match cand {
                CandidateEvent::IncomingMessage { of_message_id, .. } => of_message_id.as_str(),
                CandidateEvent::Sale { of_event_id, .. } => of_event_id.as_str(),
            };
            parse_numeric_id(id)
        })
        .max()
        .unwrap_or(0)
}

fn partition_fresh(candidates: Vec<CandidateEvent>) -> (Vec<CandidateEvent>, Vec<CandidateEvent>) {
    let mut fresh = Vec::new();
    let mut baseline = Vec::new();
    for cand in candidates {
        if candidate_is_fresh(&cand) {
            fresh.push(cand);
        } else {
            baseline.push(cand);
        }
    }
    (fresh, baseline)
}

fn seed_candidates(
    tx: &Transaction<'_>,
    model_id: i64,
    candidates: &[CandidateEvent],
    now: &str,
) -> rusqlite::Result<()> {
    for cand in candidates {
        claim_event(tx, model_id, &event_key(model_id, cand), now)?;
    }
    Ok(())
}

fn claim_event(tx: &Transaction<'_>, model_id: i64, key: &str, now: &str) -> rusqlite::Result<bool> {
    if key.is_empty() {
        return Ok(false);
    }
    let changed = tx.execute(
        "INSERT OR IGNORE INTO notified_events (model_id, event_key, notified_at) VALUES (?1, ?2, ?3)",
        params![model_id, key, now],
    )?;
    Ok(changed == 1)
}

fn load_row(tx: &Transaction<'_>, model_id: i64) -> rusqlite::Result<NotifyRow> {
    let found = tx
        .query_row(
            "SELECT last_seen_message_id, last_message_timestamp, unread_count, inbox_seeded, transactions_seeded
             FROM account_notify_state WHERE model_id = ?1",
            [model_id],
            |row| {
                Ok(NotifyRow {
                    last_seen_message_id: row.get(0)?,
                    last_message_timestamp: row.get(1)?,
                    unread_count: row.get(2)?,
                    inbox_seeded: row.get::<_, i64>(3)? != 0,
                    transactions_seeded: row.get::<_, i64>(4)? != 0,
                })
            },
        )
        .optional()?;
    Ok(found.unwrap_or_default())
}

fn save_row(tx: &Transaction<'_>, model_id: i64, row: &NotifyRow) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO account_notify_state (
            model_id, last_seen_message_id, last_message_timestamp, unread_count, inbox_seeded, transactions_seeded
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(model_id) DO UPDATE SET
            last_seen_message_id = excluded.last_seen_message_id,
            last_message_timestamp = excluded.last_message_timestamp,
            unread_count = excluded.unread_count,
            inbox_seeded = excluded.inbox_seeded,
            transactions_seeded = excluded.transactions_seeded",
        params![
            model_id,
            row.last_seen_message_id,
            row.last_message_timestamp,
            row.unread_count,
            row.inbox_seeded as i64,
            row.transactions_seeded as i64,
        ],
    )?;
    Ok(())
}

fn prune_notified_events(tx: &Transaction<'_>, cutoff: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM notified_events WHERE notified_at < ?1", [cutoff])?;
    Ok(())
}

fn insert_history(tx: &Transaction<'_>, item: &AppNotification) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO notification_history (
            id, model_id, kind, model_name, model_username, model_avatar, of_fan_id, fan_name,
            amount, net_amount, preview, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            item.id,
            item.model_id,
            item.kind.as_str(),
            item.model_name,
            item.model_username,
            item.model_avatar,
            item.of_fan_id,
            item.fan_name,
            item.amount,
            item.net_amount,
            item.preview,
            item.created_at,
        ],
    )?;
    Ok(())
}

fn trim_history(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM notification_history
         WHERE id NOT IN (
            SELECT id FROM notification_history
            ORDER BY created_at DESC, rowid DESC
            LIMIT ?1
         )",
        [MAX_RECENT_NOTIFICATIONS as i64],
    )?;
    Ok(())
}

fn raise_watermark(row: &mut NotifyRow, message_id: &str, created_at: &str) {
    let Some(next) = parse_numeric_id(message_id) else {
        return;
    };
    raise_watermark_num(row, next);
    let created = created_at.trim();
    if !created.is_empty() && parse_numeric_id(&row.last_seen_message_id) == Some(next) {
        row.last_message_timestamp = Some(created.to_owned());
    }
}

fn raise_watermark_num(row: &mut NotifyRow, next: u64) {
    let current = parse_numeric_id(&row.last_seen_message_id).unwrap_or(0);
    if next >= current {
        row.last_seen_message_id = next.to_string();
    }
}

fn timestamp_or(created_at: &str, now_iso: &str) -> String {
    if created_at.trim().is_empty() {
        now_iso.to_owned()
    } else {
        created_at.to_owned()
    }
}

fn clean_preview_or(preview: &str, fallback: &str) -> String {
    let stripped = strip_html_preview(preview);
    if stripped.is_empty() {
        fallback.to_owned()
    } else {
        stripped
    }
}

fn resolve_fan_label_tx(
    tx: &Transaction<'_>,
    model_id: i64,
    of_fan_id: &str,
    hint: &str,
) -> rusqlite::Result<String> {
    let trimmed_hint = hint.trim();
    if !trimmed_hint.is_empty() {
        return Ok(trimmed_hint.to_owned());
    }
    let fan_id = of_fan_id.trim();
    if fan_id.is_empty() {
        return Ok("Fan".to_owned());
    }
    let found = tx
        .query_row(
            "SELECT nickname, username FROM fans WHERE of_fan_id = ?1 AND model_id = ?2",
            params![fan_id, model_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((nickname, username)) = found {
        if !nickname.trim().is_empty() {
            return Ok(nickname.trim().to_owned());
        }
        if !username.trim().is_empty() {
            return Ok(format!("@{}", username.trim()));
        }
    }
    Ok(format!("Fan #{fan_id}"))
}

pub fn parse_numeric_id(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().trim_start_matches(['u', 'U']);
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

fn next_notification_id() -> String {
    let seq = NOTIFICATION_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = chrono::Utc::now().timestamp_millis();
    format!("cf-notif-{ts}-{seq}")
}

pub fn strip_html_preview(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(raw.len().min(240));
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => {
                in_tag = true;
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            }
            '>' => {
                in_tag = false;
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut truncated = String::new();
    for (idx, ch) in collapsed.chars().enumerate() {
        if idx >= 140 {
            truncated.push('…');
            break;
        }
        truncated.push(ch);
    }
    truncated
}

/// Extracts `(of_user_id, display_label)` whether OnlyFans serialized the user
/// as a full object `{"id": 77, "name": "Alex"}` or as a bare ID `77` (e.g. `skip_users=all`).
pub fn extract_user_ref(val: Option<&Value>) -> (Option<String>, String) {
    let Some(val) = val else {
        return (None, String::new());
    };
    match val {
        Value::Object(obj) => {
            let id = json_id(obj.get("id"));
            let label = user_label(obj);
            (id, label)
        }
        Value::Number(n) => (Some(n.to_string()), String::new()),
        Value::String(s) if !s.trim().is_empty() => (Some(s.trim().to_owned()), String::new()),
        _ => (None, String::new()),
    }
}

/// Parses live WebSocket frames forwarded from OnlyFans webview into candidate notifications.
pub fn parse_live_ws_payload(creator_of_id: Option<&str>, body: &str) -> Vec<CandidateEvent> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(root) = value.as_object() else {
        return Vec::new();
    };
    let creator_id = creator_of_id.map(str::trim).filter(|s| !s.is_empty());
    let mut out = Vec::new();

    // 1. Explicit PPV unlock / payment events on OnlyFans WebSocket
    for purchase_key in [
        "pay_message",
        "message_purchased",
        "ppv_purchased",
        "purchased_message",
        "chat_message_purchased",
    ] {
        if let Some(item) = root.get(purchase_key).and_then(Value::as_object) {
            if let Some(ev) = sale_from_ws_object(item, SaleKind::Ppv) {
                out.push(ev);
            }
        }
    }

    // 2. Chat message event (`api2_chat_message`, `chat_message`, `new_message`, `message`)
    let msg_obj = root
        .get("api2_chat_message")
        .or_else(|| root.get("chat_message"))
        .or_else(|| root.get("new_message"))
        .or_else(|| root.get("message"))
        .and_then(Value::as_object)
        .or_else(|| {
            if root.contains_key("fromUser")
                && (root.contains_key("text") || root.contains_key("isTip") || root.contains_key("price"))
            {
                Some(root)
            } else {
                None
            }
        });

    if let Some(msg) = msg_obj {
        if let Some(msg_id) = json_id(msg.get("id")) {
            let (from_id, from_label) = extract_user_ref(
                msg.get("fromUser")
                    .or_else(|| msg.get("user"))
                    .or_else(|| msg.get("author")),
            );
            let from_is_creator = match (creator_id, from_id.as_deref()) {
                (Some(cid), Some(fid)) => cid == fid,
                _ => msg.get("isFromQueue").and_then(Value::as_bool) == Some(true),
            };
            let (partner_id, partner_label) = if from_is_creator {
                extract_user_ref(msg.get("toUser").or_else(|| msg.get("withUser")))
            } else {
                (from_id.clone(), from_label)
            };
            let fan_id = partner_id
                .or_else(|| json_id(msg.get("chatUserId")))
                .unwrap_or_default();
            let fan_name = partner_label;
            let text = msg
                .get("text")
                .or_else(|| msg.get("rawText"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let created_at = msg
                .get("createdAt")
                .or_else(|| msg.get("created_at"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(utc_now);

            let tip_amount = msg
                .get("tipAmount")
                .or_else(|| msg.get("tipSumm"))
                .and_then(json_num)
                .filter(|v| *v > 0.0);
            let is_tip = msg.get("isTip").and_then(json_bool) == Some(true) || tip_amount.is_some();
            let price = msg.get("price").and_then(json_num).filter(|v| *v > 0.0);
            let is_opened = msg.get("isOpened").and_then(json_bool) == Some(true);

            if is_tip && !from_is_creator {
                if let Some(amount) = tip_amount.or_else(|| money_from_text(text)) {
                    out.push(CandidateEvent::Sale {
                        sale_kind: SaleKind::Tip,
                        of_event_id: msg_id,
                        of_fan_id: fan_id,
                        fan_name,
                        gross_amount: amount,
                        net_amount: (amount * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0,
                        preview: text.to_owned(),
                        created_at,
                        live_transition: true,
                    });
                }
            } else if let Some(ppv_price) = price {
                if is_opened {
                    out.push(CandidateEvent::Sale {
                        sale_kind: SaleKind::Ppv,
                        of_event_id: msg_id,
                        of_fan_id: fan_id,
                        fan_name,
                        gross_amount: ppv_price,
                        net_amount: (ppv_price * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0,
                        preview: format!("PPV desbloqueado (${ppv_price:.2})"),
                        created_at,
                        live_transition: true,
                    });
                }
            } else if !from_is_creator && from_id.is_some() {
                out.push(CandidateEvent::IncomingMessage {
                    of_message_id: msg_id,
                    of_fan_id: fan_id,
                    fan_name,
                    preview: text.to_owned(),
                    created_at,
                });
            }
        }
    }

    // 3. Toast / notification WS frame (`new_notification`, `notification`, `toast`)
    for notif_key in ["new_notification", "notification", "toast"] {
        if let Some(notif) = root.get(notif_key).and_then(Value::as_object) {
            let kind_str = notif
                .get("type")
                .or_else(|| notif.get("subType"))
                .or_else(|| notif.get("category"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let text = notif
                .get("text")
                .or_else(|| notif.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let lower_text = text.to_lowercase();
            let sale_kind = if kind_str.contains("tip")
                || lower_text.contains("sent you a tip")
                || lower_text.contains("propina")
            {
                Some(SaleKind::Tip)
            } else if kind_str.contains("message")
                && (kind_str.contains("pay")
                    || kind_str.contains("purchas")
                    || lower_text.contains("paid")
                    || lower_text.contains("pagado")
                    || lower_text.contains("purchased"))
            {
                Some(SaleKind::Ppv)
            } else {
                None
            };
            if let Some(sale_kind) = sale_kind {
                if let Some(ev) = sale_from_ws_object(notif, sale_kind) {
                    out.push(ev);
                }
            }
        }
    }

    out
}

fn sale_from_ws_object(obj: &serde_json::Map<String, Value>, sale_kind: SaleKind) -> Option<CandidateEvent> {
    let ev_id = json_id(obj.get("id"))
        .or_else(|| json_id(obj.get("messageId")))
        .or_else(|| json_id(obj.get("transactionId")))?;
    let text = obj
        .get("text")
        .or_else(|| obj.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let amount = obj
        .get("price")
        .or_else(|| obj.get("amount"))
        .or_else(|| obj.get("tipAmount"))
        .or_else(|| obj.get("gross"))
        .and_then(json_num)
        .filter(|v| *v > 0.0)
        .or_else(|| money_from_text(text))?;
    let (user_id, fan_name) = extract_user_ref(
        obj.get("user")
            .or_else(|| obj.get("fromUser"))
            .or_else(|| obj.get("withUser")),
    );
    let fan_id = user_id
        .or_else(|| json_id(obj.get("userId")))
        .unwrap_or_default();
    Some(CandidateEvent::Sale {
        sale_kind,
        of_event_id: ev_id,
        of_fan_id: fan_id,
        fan_name,
        gross_amount: amount,
        net_amount: (amount * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0,
        preview: text.to_owned(),
        created_at: utc_now(),
        live_transition: true,
    })
}

fn user_label(user: &serde_json::Map<String, Value>) -> String {
    if let Some(name) = user.get("name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        return name.to_owned();
    }
    if let Some(username) = user.get("username").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        return format!("@{username}");
    }
    String::new()
}

fn json_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn json_num(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_start_matches('$').replace(',', "").parse().ok(),
        _ => None,
    }
}

fn json_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|v| v != 0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn money_from_text(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            let rest = text[index + 1..].trim_start();
            let cleaned: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
                .collect();
            let parsed = cleaned.trim_matches(|c| c == ',' || c == '.').replace(',', "");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn baseline_seeds_without_clock_skew_and_notifies_on_new_messages_and_sales() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-notif-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        database
            .save_onlyfans_profile(model.id, "100", "bella", "Bella", None)
            .unwrap();

        let bus = NotificationBus::new();

        // 1. Initial inbox poll seeds watermark = 5000 without emitting notifications
        let seed_batch = NotificationBatch {
            source: BatchSource::Inbox { max_msg_id: 5000 },
            candidates: vec![CandidateEvent::IncomingMessage {
                of_message_id: "4990".to_owned(),
                of_fan_id: "77".to_owned(),
                fan_name: "Alex".to_owned(),
                preview: "<p>Mensaje anterior</p>".to_owned(),
                created_at: "2020-01-01T00:00:00Z".to_owned(),
            }],
        };
        assert!(bus
            .process_batch(None, &database, model.id, seed_batch, None)
            .is_empty());

        // 2. Opening an older chat thread (id = 4500 <= 5000) does NOT emit
        let old_thread = NotificationBatch {
            source: BatchSource::ChatThread { max_msg_id: 4500 },
            candidates: vec![CandidateEvent::IncomingMessage {
                of_message_id: "4500".to_owned(),
                of_fan_id: "88".to_owned(),
                fan_name: "Marco".to_owned(),
                preview: "Viejo".to_owned(),
                created_at: "2020-01-01T00:00:00Z".to_owned(),
            }],
        };
        assert!(bus
            .process_batch(None, &database, model.id, old_thread, None)
            .is_empty());

        // 3. Subsequent background inbox poll with a new message (id = 5001 > 5000)
        // emits EVEN IF createdAt is in a totally different year from the local PC clock!
        let new_inbox = NotificationBatch {
            source: BatchSource::Inbox { max_msg_id: 5001 },
            candidates: vec![CandidateEvent::IncomingMessage {
                of_message_id: "5001".to_owned(),
                of_fan_id: "77".to_owned(),
                fan_name: "Alex".to_owned(),
                preview: "<p>Hola <b>guapa</b> &amp; feliz día</p>".to_owned(),
                created_at: "2024-03-10T12:00:00Z".to_owned(),
            }],
        };
        let emitted = bus.process_batch(None, &database, model.id, new_inbox.clone(), None);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].kind, NotificationKind::Message);
        assert_eq!(emitted[0].preview, "Hola guapa & feliz día");
        assert_eq!(bus.unread_counts(&database).get(&model.id), Some(&1));

        // Duplicate poll is suppressed
        assert!(bus
            .process_batch(None, &database, model.id, new_inbox, None)
            .is_empty());

        // 4. Bare numeric `fromUser` in WebSocket payload works (skip_users=all style)
        let ws_bare = json!({
            "message": {
                "id": 5002,
                "text": "Mensaje directo por WS",
                "fromUser": 77
            }
        });
        let parsed_ws = parse_live_ws_payload(Some("100"), &ws_bare.to_string());
        assert_eq!(parsed_ws.len(), 1);
        let ws_emitted = bus.process_batch(
            None,
            &database,
            model.id,
            NotificationBatch {
                source: BatchSource::WebSocket,
                candidates: parsed_ws,
            },
            None,
        );
        assert_eq!(ws_emitted.len(), 1);
        assert_eq!(bus.unread_counts(&database).get(&model.id), Some(&2));

        // 5. PPV unlock of an older message (id = 4200 <= 5000, live_transition = true)
        // emits green PPV sale and fuzzy-deduplicates against transactions poll
        let _ = bus.process_batch(
            None,
            &database,
            model.id,
            NotificationBatch {
                source: BatchSource::Transactions,
                candidates: Vec::new(),
            },
            None,
        );
        let ppv_unlock = NotificationBatch {
            source: BatchSource::ChatThread { max_msg_id: 4200 },
            candidates: vec![CandidateEvent::Sale {
                sale_kind: SaleKind::Ppv,
                of_event_id: "4200".to_owned(),
                of_fan_id: "77".to_owned(),
                fan_name: "Alex".to_owned(),
                gross_amount: 25.0,
                net_amount: 20.0,
                preview: String::new(),
                created_at: "2024-03-01T00:00:00Z".to_owned(),
                live_transition: true,
            }],
        };
        let sales = bus.process_batch(None, &database, model.id, ppv_unlock, None);
        assert_eq!(sales.len(), 1);
        assert_eq!(sales[0].kind, NotificationKind::Ppv);
        assert_eq!(sales[0].amount, Some(25.0));

        let dup_tx = NotificationBatch {
            source: BatchSource::Transactions,
            candidates: vec![CandidateEvent::Sale {
                sale_kind: SaleKind::Ppv,
                of_event_id: "tx-abc-999".to_owned(),
                of_fan_id: "77".to_owned(),
                fan_name: "Alex".to_owned(),
                gross_amount: 25.0,
                net_amount: 20.0,
                preview: "Payment for message".to_owned(),
                created_at: "2024-03-10T12:00:05Z".to_owned(),
                live_transition: false,
            }],
        };
        assert!(bus
            .process_batch(None, &database, model.id, dup_tx, None)
            .is_empty());

        let persisted = NotificationBus::new();
        assert!(persisted
            .process_batch(
                None,
                &database,
                model.id,
                NotificationBatch {
                    source: BatchSource::Inbox { max_msg_id: 5001 },
                    candidates: vec![CandidateEvent::IncomingMessage {
                        of_message_id: "5001".to_owned(),
                        of_fan_id: "77".to_owned(),
                        fan_name: "Alex".to_owned(),
                        preview: "otra vez".to_owned(),
                        created_at: "2024-03-10T12:00:00Z".to_owned(),
                    }],
                },
                None,
            )
            .is_empty());
        assert_eq!(persisted.unread_counts(&database).get(&model.id), Some(&2));

        assert!(bus.clear_unread(&database, model.id));
        assert!(bus.unread_counts(&database).is_empty());

        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
