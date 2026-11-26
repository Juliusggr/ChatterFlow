use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::supabase::{utc_now, NotificationRow, NotifyStateRow, SupabaseStore, ONLYFANS_NET_RATIO};

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
    None,
    WebSocket,
    Inbox { max_msg_id: u64 },
    ChatThread { max_msg_id: u64 },
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
    pub model_id: String,
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

impl From<NotificationRow> for AppNotification {
    fn from(row: NotificationRow) -> Self {
        Self {
            id: row.id,
            kind: NotificationKind::parse(&row.kind),
            model_id: row.model_id,
            model_name: row.model_name,
            model_username: row.model_username,
            model_avatar: row.model_avatar,
            of_fan_id: row.of_fan_id,
            fan_name: row.fan_name,
            amount: row.amount,
            net_amount: row.net_amount,
            preview: row.preview,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveFocus {
    pub model_id: String,
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
    model_id: String,
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

pub struct NotificationBus {
    bursts: Arc<Mutex<HashMap<(String, String), Burst>>>,
    burst_seq: Arc<AtomicU64>,
    recent_sales: Mutex<VecDeque<SaleFingerprint>>,
    notify_states: Mutex<HashMap<String, NotifyStateRow>>,
    unread_cache: Mutex<Option<HashMap<String, u32>>>,
    messages_enabled: AtomicBool,
    sales_enabled: AtomicBool,
    os_push_enabled: AtomicBool,
    process_gate: tokio::sync::Mutex<()>,
}

impl NotificationBus {
    pub fn new() -> Self {
        Self {
            bursts: Arc::new(Mutex::new(HashMap::new())),
            burst_seq: Arc::new(AtomicU64::new(1)),
            recent_sales: Mutex::new(VecDeque::new()),
            notify_states: Mutex::new(HashMap::new()),
            unread_cache: Mutex::new(None),
            messages_enabled: AtomicBool::new(true),
            sales_enabled: AtomicBool::new(true),
            os_push_enabled: AtomicBool::new(true),
            process_gate: tokio::sync::Mutex::new(()),
        }
    }

    pub fn reset_caches(&self) {
        self.notify_states
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        *self
            .unread_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }

    pub fn set_delivery(&self, messages_enabled: bool, sales_enabled: bool, os_push_enabled: bool) {
        self.messages_enabled.store(messages_enabled, Ordering::Relaxed);
        self.sales_enabled.store(sales_enabled, Ordering::Relaxed);
        self.os_push_enabled.store(os_push_enabled, Ordering::Relaxed);
    }

    pub async fn recent(&self, store: &SupabaseStore) -> Result<Vec<AppNotification>, String> {
        store
            .list_recent_notifications(MAX_RECENT_NOTIFICATIONS)
            .await
            .map(|rows| rows.into_iter().map(AppNotification::from).collect())
            .map_err(|err| err.to_string())
    }

    pub async fn clear_history(&self, store: &SupabaseStore) -> Result<(), String> {
        store
            .clear_notification_history()
            .await
            .map_err(|err| err.to_string())
    }

    pub async fn dismiss(&self, store: &SupabaseStore, id: &str) -> Result<(), String> {
        store
            .dismiss_notification(id)
            .await
            .map_err(|err| err.to_string())
    }

    pub async fn unread_counts(&self, store: &SupabaseStore) -> HashMap<String, u32> {
        if let Some(cached) = self
            .unread_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
        {
            return cached;
        }
        let fetched = store.unread_counts().await.unwrap_or_default();
        *self
            .unread_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(fetched.clone());
        fetched
    }

    pub async fn clear_unread(&self, store: &SupabaseStore, model_id: &str) -> bool {
        let had_cached_unread = self
            .unread_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_none_or(|map| map.get(model_id).copied().unwrap_or(0) > 0);
        if !had_cached_unread {
            return false;
        }
        let cleared = store.clear_unread(model_id).await.unwrap_or(false);
        {
            let mut states = self
                .notify_states
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(row) = states.get_mut(model_id) {
                row.unread_count = 0;
            }
        }
        if let Some(map) = self
            .unread_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_mut()
        {
            map.remove(model_id);
        }
        cleared
    }

    pub async fn process_batch(
        &self,
        app: Option<&AppHandle>,
        store: &SupabaseStore,
        model_id: &str,
        batch: NotificationBatch,
        viewing: Option<&LiveFocus>,
    ) -> Vec<AppNotification> {
        if batch.source == BatchSource::None {
            return Vec::new();
        }
        let seeds_when_empty =
            matches!(batch.source, BatchSource::Inbox { .. } | BatchSource::Transactions);
        if batch.candidates.is_empty() && !seeds_when_empty {
            return Vec::new();
        }

        let _guard = self.process_gate.lock().await;

        let Ok(model) = store.get_model(model_id).await else {
            return Vec::new();
        };
        let session_started = model
            .of_user_id
            .as_ref()
            .is_some_and(|id| !id.trim().is_empty());
        if !session_started {
            return Vec::new();
        }
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
        let cutoff = (chrono::Utc::now()
            - chrono::Duration::from_std(NOTIFIED_EVENT_TTL).unwrap_or_default())
        .to_rfc3339();

        store.prune_notified_events_throttled(&cutoff).await;
        let cached_row = self
            .notify_states
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(model_id)
            .cloned();
        let mut row = match cached_row {
            Some(existing) => existing,
            None => {
                let Ok(loaded) = store.load_notify_state(model_id).await else {
                    return Vec::new();
                };
                self.notify_states
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(model_id.to_owned(), loaded.clone());
                loaded
            }
        };
        let initial_row = row.clone();
        let mut candidates = batch.candidates;

        if let BatchSource::Inbox { .. } = batch.source {
            if !row.inbox_seeded {
                let (fresh, baseline) = partition_fresh(candidates);
                let keys: Vec<String> = baseline
                    .iter()
                    .map(|cand| event_key(model_id, cand))
                    .collect();
                let _ = store
                    .claim_notified_events_batch(model_id, &keys, &now_iso)
                    .await;
                raise_watermark_num(&mut row, max_candidate_id(&baseline));
                for cand in &baseline {
                    if let CandidateEvent::IncomingMessage {
                        of_message_id,
                        created_at,
                        ..
                    } = cand
                    {
                        raise_watermark(&mut row, of_message_id, created_at);
                    }
                }
                row.inbox_seeded = true;
                self.persist_notify_row_if_changed(store, model_id, &initial_row, &row)
                    .await;
                if fresh.is_empty() {
                    return Vec::new();
                }
                candidates = fresh;
            }
        }
        if matches!(batch.source, BatchSource::Transactions) && !row.transactions_seeded {
            let keys: Vec<String> = candidates
                .iter()
                .map(|cand| event_key(model_id, cand))
                .collect();
            let _ = store
                .claim_notified_events_batch(model_id, &keys, &now_iso)
                .await;
            row.transactions_seeded = true;
            self.persist_notify_row_if_changed(store, model_id, &initial_row, &row)
                .await;
            return Vec::new();
        }
        if let BatchSource::ChatThread { .. } = batch.source {
            if !row.inbox_seeded {
                let mut live = Vec::new();
                let mut parked = Vec::new();
                for cand in candidates.drain(..) {
                    let live_sale =
                        matches!(cand, CandidateEvent::Sale { live_transition: true, .. });
                    if live_sale || candidate_is_fresh(&cand) {
                        live.push(cand);
                    } else {
                        parked.push(cand);
                    }
                }
                let keys: Vec<String> = parked
                    .iter()
                    .map(|cand| event_key(model_id, cand))
                    .collect();
                let _ = store
                    .claim_notified_events_batch(model_id, &keys, &now_iso)
                    .await;
                raise_watermark_num(&mut row, max_candidate_id(&parked));
                self.persist_notify_row_if_changed(store, model_id, &initial_row, &row)
                    .await;
                if live.is_empty() {
                    return Vec::new();
                }
                candidates = live;
            }
        }

        let mut emitted = Vec::new();
        let mut history_rows = Vec::new();
        let mut unread_changed = false;
        let pre_loop_row = row.clone();
        for candidate in candidates {
            if let Some(notification) = self
                .accept_candidate(
                    store,
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
                )
                .await
            {
                history_rows.push(NotificationRow {
                    id: notification.id.clone(),
                    model_id: notification.model_id.clone(),
                    kind: notification.kind.as_str().to_owned(),
                    model_name: notification.model_name.clone(),
                    model_username: notification.model_username.clone(),
                    model_avatar: notification.model_avatar.clone(),
                    of_fan_id: notification.of_fan_id.clone(),
                    fan_name: notification.fan_name.clone(),
                    amount: notification.amount,
                    net_amount: notification.net_amount,
                    preview: notification.preview.clone(),
                    created_at: notification.created_at.clone(),
                });
                emitted.push(notification);
            }
        }
        let _ = store.insert_notification_history_batch(&history_rows).await;
        self.persist_notify_row_if_changed(store, model_id, &pre_loop_row, &row)
            .await;

        if unread_changed {
            if let Some(map) = self
                .unread_cache
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .as_mut()
            {
                if row.unread_count > 0 {
                    map.insert(model_id.to_owned(), row.unread_count as u32);
                } else {
                    map.remove(model_id);
                }
            }
        }

        if let Some(app) = app {
            for item in &emitted {
                let _ = app.emit("cf-notification", item);
                self.queue_os_toast(app, item);
            }
            if unread_changed {
                let counts = self.unread_counts(store).await;
                let _ = app.emit("unread-counts-changed", counts);
            }
        }
        emitted
    }

    async fn persist_notify_row_if_changed(
        &self,
        store: &SupabaseStore,
        model_id: &str,
        before: &NotifyStateRow,
        after: &NotifyStateRow,
    ) {
        if before == after {
            return;
        }
        self.notify_states
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(model_id.to_owned(), after.clone());
        let _ = store.save_notify_state(model_id, after).await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept_candidate(
        &self,
        store: &SupabaseStore,
        row: &mut NotifyStateRow,
        model_id: &str,
        model_name: &str,
        model_username: &str,
        model_avatar: &Option<String>,
        now_iso: &str,
        source: BatchSource,
        candidate: CandidateEvent,
        viewing: Option<&LiveFocus>,
        unread_changed: &mut bool,
    ) -> Option<AppNotification> {
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
                    return None;
                }
                let key = format!("msg:{model_id}:{msg_id}");
                let numeric_id = parse_numeric_id(msg_id);
                let watermark = parse_numeric_id(&row.last_seen_message_id).unwrap_or(0);
                if watermark > 0 {
                    if let Some(id) = numeric_id {
                        if id <= watermark {
                            let _ = store.claim_notified_event(model_id, &key, now_iso).await;
                            return None;
                        }
                    }
                }
                let fan_id = of_fan_id.trim().to_owned();
                let viewing_this = is_viewing(viewing, model_id, &fan_id);
                if !store
                    .claim_notified_event(model_id, &key, now_iso)
                    .await
                    .unwrap_or(false)
                {
                    return None;
                }
                raise_watermark(row, msg_id, &created_at);
                if !viewing_this {
                    row.unread_count = row.unread_count.saturating_add(1);
                    *unread_changed = true;
                }
                let resolved_fan = resolve_fan_label(store, model_id, &fan_id, &fan_name).await;
                let clean_preview = clean_preview_or(&preview, "Nuevo mensaje en el chat");
                Some(AppNotification {
                    id: next_notification_id(),
                    kind: NotificationKind::Message,
                    model_id: model_id.to_owned(),
                    model_name: model_name.to_owned(),
                    model_username: model_username.to_owned(),
                    model_avatar: model_avatar.clone(),
                    of_fan_id: fan_id,
                    fan_name: resolved_fan,
                    amount: None,
                    net_amount: None,
                    preview: clean_preview,
                    created_at: timestamp_or(&created_at, now_iso),
                })
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
                    return None;
                }
                let ev_id = of_event_id.trim();
                if ev_id.is_empty() {
                    return None;
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
                    let _ = store.claim_notified_event(model_id, &key, now_iso).await;
                    return None;
                }
                if !store
                    .claim_notified_event(model_id, &key, now_iso)
                    .await
                    .unwrap_or(false)
                {
                    return None;
                }
                let fan_id = of_fan_id.trim().to_owned();
                if self.is_duplicate_sale(model_id, sale_kind, &fan_id, gross_amount) {
                    return None;
                }
                let resolved_fan = resolve_fan_label(store, model_id, &fan_id, &fan_name).await;
                let rounded_gross = (gross_amount * 100.0).round() / 100.0;
                let rounded_net = if net_amount.is_finite() && net_amount > 0.0 {
                    (net_amount * 100.0).round() / 100.0
                } else {
                    (rounded_gross * ONLYFANS_NET_RATIO * 100.0).round() / 100.0
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
                Some(AppNotification {
                    id: next_notification_id(),
                    kind: sale_kind.into_notification_kind(),
                    model_id: model_id.to_owned(),
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
                })
            }
        }
    }

    fn is_duplicate_sale(
        &self,
        model_id: &str,
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
                model_id: model_id.to_owned(),
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
            show_os_toast(app, &title, &body, &item.model_id, &item.of_fan_id);
            return;
        }

        let key = (item.model_id.clone(), item.of_fan_id.clone());
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
        show_os_toast(app, &title, &body, &item.model_id, &item.of_fan_id);
    }

    fn schedule_burst_summary(
        &self,
        app: AppHandle,
        key: (String, String),
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
                    &key.0,
                    &key.1,
                );
            }
        });
    }
}

fn show_os_toast(app: &AppHandle, title: &str, body: &str, model_id: &str, of_fan_id: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(err) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .extra("modelId", model_id.to_owned())
        .extra("ofFanId", of_fan_id.to_owned())
        .show()
    {
        eprintln!("creatorfloww: no se pudo preparar el toast de Windows: {err}");
    }
}

fn is_viewing(viewing: Option<&LiveFocus>, model_id: &str, of_fan_id: &str) -> bool {
    let Some(focus) = viewing else {
        return false;
    };
    focus.model_id == model_id
        && !focus.of_fan_id.trim().is_empty()
        && focus.of_fan_id.trim() == of_fan_id.trim()
}

fn event_key(model_id: &str, cand: &CandidateEvent) -> String {
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
        CandidateEvent::IncomingMessage { created_at, .. }
        | CandidateEvent::Sale { created_at, .. } => created_at,
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

fn raise_watermark(row: &mut NotifyStateRow, message_id: &str, created_at: &str) {
    let Some(next) = parse_numeric_id(message_id) else {
        return;
    };
    raise_watermark_num(row, next);
    let created = created_at.trim();
    if !created.is_empty() && parse_numeric_id(&row.last_seen_message_id) == Some(next) {
        row.last_message_timestamp = Some(created.to_owned());
    }
}

fn raise_watermark_num(row: &mut NotifyStateRow, next: u64) {
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

async fn resolve_fan_label(
    store: &SupabaseStore,
    model_id: &str,
    of_fan_id: &str,
    hint: &str,
) -> String {
    let trimmed_hint = hint.trim();
    if !trimmed_hint.is_empty() {
        return trimmed_hint.to_owned();
    }
    let fan_id = of_fan_id.trim();
    if fan_id.is_empty() {
        return "Fan".to_owned();
    }
    if let Ok(Some((nickname, username))) = store.lookup_fan_label(model_id, fan_id).await {
        if !nickname.trim().is_empty() {
            return nickname.trim().to_owned();
        }
        if !username.trim().is_empty() {
            return format!("@{}", username.trim());
        }
    }
    format!("Fan #{fan_id}")
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

pub fn parse_live_ws_payload(creator_of_id: Option<&str>, body: &str) -> Vec<CandidateEvent> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(root) = value.as_object() else {
        return Vec::new();
    };
    let creator_id = creator_of_id.map(str::trim).filter(|s| !s.is_empty());
    let mut out = Vec::new();

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
                        net_amount: (amount * ONLYFANS_NET_RATIO * 100.0).round() / 100.0,
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
                        net_amount: (ppv_price * ONLYFANS_NET_RATIO * 100.0).round() / 100.0,
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
        net_amount: (amount * ONLYFANS_NET_RATIO * 100.0).round() / 100.0,
        preview: text.to_owned(),
        created_at: utc_now(),
        live_transition: true,
    })
}

fn user_label(user: &serde_json::Map<String, Value>) -> String {
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

    #[test]
    fn parses_live_ws_message_and_sale_frames() {
        let msg = json!({
            "api2_chat_message": {
                "id": 9001,
                "fromUser": { "id": 77, "name": "Alex" },
                "text": "<p>Hola &amp; gracias</p>",
                "createdAt": "2026-11-26T06:00:00Z"
            }
        });
        let events = parse_live_ws_payload(Some("100"), &msg.to_string());
        assert_eq!(events.len(), 1);
        assert_eq!(strip_html_preview("<p>Hola &amp; gracias</p>"), "Hola & gracias");

        let ppv = json!({
            "pay_message": {
                "id": 9002,
                "price": 25.0,
                "user": { "id": 77, "username": "alex" }
            }
        });
        let sale_events = parse_live_ws_payload(Some("100"), &ppv.to_string());
        assert_eq!(sale_events.len(), 1);
    }
}
