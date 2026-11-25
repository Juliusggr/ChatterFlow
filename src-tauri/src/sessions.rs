use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::db::{Database, Model};
use crate::sync::{ingest_body, IngestOutcome};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelAccount {
    #[serde(flatten)]
    pub model: Model,
    pub window_open: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct SyncReport {
    pub model: ModelAccount,
    pub transactions_upserted: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageBarPlacement {
    #[default]
    Top,
    Sidebar,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageTabsState {
    pub open_model_ids: Vec<i64>,
    pub active_model_id: Option<i64>,
    pub bar_placement: MessageBarPlacement,
    pub unread_counts: std::collections::HashMap<i64, u32>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiPreferencesFile {
    bar_placement: MessageBarPlacement,
}

const MESSAGES_WINDOW_LABEL: &str = "messages-hub";
const MESSAGES_TABS_LABEL: &str = "messages-tabs";
const TAB_BAR_HEIGHT: f64 = 44.0;
const SIDEBAR_WIDTH: f64 = 196.0;
const UI_PREFS_FILE: &str = "ui_preferences.json";
static HUB_VISIBLE: AtomicBool = AtomicBool::new(false);
static HUB_GATE: Mutex<()> = Mutex::new(());

struct HubRest {
    x: i32,
    y: i32,
    parked: bool,
}

fn hub_rest() -> &'static Mutex<HubRest> {
    static REST: std::sync::OnceLock<Mutex<HubRest>> = std::sync::OnceLock::new();
    REST.get_or_init(|| Mutex::new(HubRest { x: 120, y: 80, parked: false }))
}

fn viewing_fans() -> &'static Mutex<std::collections::HashMap<i64, String>> {
    static FANS: std::sync::OnceLock<Mutex<std::collections::HashMap<i64, String>>> =
        std::sync::OnceLock::new();
    FANS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub fn note_viewing_fan(model_id: i64, of_fan_id: &str) {
    let fan = of_fan_id.trim().trim_start_matches(['u', 'U']);
    if fan.is_empty() {
        return;
    }
    viewing_fans()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(model_id, fan.to_owned());
}

fn current_live_focus() -> Option<crate::notifications::LiveFocus> {
    if !HUB_VISIBLE.load(Ordering::SeqCst) {
        return None;
    }
    let active = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .active_model_id?;
    let fan = viewing_fans()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&active)
        .cloned()
        .unwrap_or_default();
    Some(crate::notifications::LiveFocus {
        model_id: active,
        of_fan_id: fan,
    })
}

fn place_webview(view: &tauri::Webview, x: f64, y: f64, width: f64, height: f64) {
    let _ = view.set_position(tauri::LogicalPosition::new(x, y));
    let _ = view.set_size(tauri::LogicalSize::new(width, height));
    let _ = view.show();
}

fn park_of_webview(view: &tauri::Webview) {
    // Real viewport inside the parked hub. A 4×4 webview never boots OnlyFans,
    // so the signed client stays null and no account is polled.
    place_webview(view, 0.0, 0.0, 1100.0, 760.0);
}

fn park_every_of_webview(app: &AppHandle) {
    let Some(database) = app.try_state::<Database>() else {
        return;
    };
    let Ok(models) = database.list_models() else {
        return;
    };
    for model in models {
        if let Some(view) = app.get_webview(&messages_of_label(model.id)) {
            park_of_webview(&view);
        }
    }
}

fn remember_hub_origin(window: &tauri::Window) {
    if let Ok(pos) = window.outer_position() {
        if pos.x > -16000 && pos.y > -16000 {
            let mut rest = hub_rest().lock().unwrap_or_else(|poison| poison.into_inner());
            rest.x = pos.x;
            rest.y = pos.y;
        }
    }
}

fn park_messages_hub(window: &tauri::Window) {
    remember_hub_origin(window);
    {
        let mut rest = hub_rest().lock().unwrap_or_else(|poison| poison.into_inner());
        rest.parked = true;
    }
    let _ = window.set_skip_taskbar(true);
    let _ = window.set_position(tauri::PhysicalPosition::new(-32000, -32000));
    let _ = window.show();
}

fn reveal_messages_hub(window: &tauri::Window) {
    let mut rest = hub_rest().lock().unwrap_or_else(|poison| poison.into_inner());
    if rest.parked {
        let _ = window.set_position(tauri::PhysicalPosition::new(rest.x, rest.y));
        rest.parked = false;
    }
    let _ = window.set_skip_taskbar(false);
    let _ = window.show();
}
#[cfg(target_os = "windows")]
const FAST_WEBVIEW_ARGS: &str =
    "--no-first-run --no-default-browser-check --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows --disable-features=TranslateUI,msSmartScreenProtection,IntensiveWakeUpThrottling,CalculateNativeWinOcclusion";

fn messages_hub_state() -> &'static Mutex<MessageTabsState> {
    static STATE: std::sync::OnceLock<Mutex<MessageTabsState>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Mutex::new(MessageTabsState::default()))
}

fn model_aliases() -> &'static Mutex<std::collections::HashMap<i64, i64>> {
    static ALIASES: std::sync::OnceLock<Mutex<std::collections::HashMap<i64, i64>>> =
        std::sync::OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn register_model_alias(from_id: i64, to_id: i64) {
    if from_id == to_id {
        return;
    }
    model_aliases()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(from_id, to_id);
}

fn resolve_model_alias(model_id: i64) -> i64 {
    model_aliases()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&model_id)
        .copied()
        .unwrap_or(model_id)
}

pub fn warm_session_environment(app: &AppHandle) {
    if let Ok(app_dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(app_dir.join("partitions"));
        let prefs_path = app_dir.join(UI_PREFS_FILE);
        if let Ok(raw) = std::fs::read_to_string(prefs_path) {
            if let Ok(parsed) = serde_json::from_str::<UiPreferencesFile>(&raw) {
                let mut state = messages_hub_state()
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                state.bar_placement = parsed.bar_placement;
            }
        }
    }
}

fn persist_bar_placement(app: &AppHandle, placement: MessageBarPlacement) {
    let Ok(app_dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&app_dir);
    let payload = UiPreferencesFile {
        bar_placement: placement,
    };
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        let _ = std::fs::write(app_dir.join(UI_PREFS_FILE), json);
    }
}

pub fn set_message_bar_placement(
    app: &AppHandle,
    placement: MessageBarPlacement,
) -> Result<MessageTabsState, String> {
    {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.bar_placement = placement;
    }
    persist_bar_placement(app, placement);
    layout_messages_hub(app);
    emit_tabs_changed(app);
    Ok(get_message_tabs(app))
}

fn messages_insights_label(model_id: i64) -> String {
    format!("messages-{model_id}-insights")
}

fn tabs_script() -> &'static str {
    "window.__CF_VIEW__ = \"messages-tabs\";"
}

fn emit_tabs_changed(app: &AppHandle) {
    let snapshot = get_message_tabs(app);
    let _ = app.emit("message-tabs-changed", snapshot);
}

fn update_hub_title(app: &AppHandle, database: &Database, active_model_id: Option<i64>) {
    let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) else {
        return;
    };
    let title = active_model_id
        .and_then(|id| database.get_model(id).ok())
        .map(|model| {
            if model.display_name.trim().is_empty() {
                "Mensajes · CreatorFloww".to_owned()
            } else {
                format!("Mensajes · {}", model.display_name)
            }
        })
        .unwrap_or_else(|| "Mensajes · CreatorFloww".to_owned());
    let _ = window.set_title(&title);
}

struct Slot {
    unauthorized: AtomicBool,
    profile: AtomicBool,
    balances: AtomicBool,
    transactions: AtomicBool,
    widened: AtomicBool,
    upserted: AtomicU64,
}

impl Slot {
    fn new() -> Self {
        Self {
            unauthorized: AtomicBool::new(false),
            profile: AtomicBool::new(false),
            balances: AtomicBool::new(false),
            transactions: AtomicBool::new(false),
            widened: AtomicBool::new(false),
            upserted: AtomicU64::new(0),
        }
    }
}

pub struct SyncBus {
    slots: Mutex<std::collections::HashMap<i64, Arc<Slot>>>,
}

impl SyncBus {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn begin(&self, model_id: i64) -> Arc<Slot> {
        let slot = Arc::new(Slot::new());
        self.slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(model_id, slot.clone());
        slot
    }

    fn mark_widened(&self, model_id: i64) {
        if let Some(slot) = self.slot(model_id) {
            slot.widened.store(true, Ordering::Relaxed);
        }
    }

    fn slot(&self, model_id: i64) -> Option<Arc<Slot>> {
        self.slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&model_id)
            .cloned()
    }
}

pub fn window_label(model_id: i64) -> String {
    format!("model-{model_id}")
}

pub fn list_accounts(app: &AppHandle, database: &Database) -> Result<Vec<ModelAccount>, String> {
    let models = database.list_models().map_err(|err| err.to_string())?;
    Ok(models
        .into_iter()
        .map(|model| account(app, model))
        .collect())
}

pub fn open_model_session(
    app: &AppHandle,
    database: &Database,
    model_id: Option<i64>,
) -> Result<ModelAccount, String> {
    let model = match model_id {
        Some(id) => database.get_model(id).map_err(|err| err.to_string())?,
        None => {
            let open_ids = currently_open_model_ids(app, database);
            match database
                .find_reusable_empty_model(&open_ids)
                .map_err(|err| err.to_string())?
            {
                Some(existing_empty) => existing_empty,
                None => database
                    .create_model_partition()
                    .map_err(|err| err.to_string())?,
            }
        }
    };
    let window = open_or_focus(app, &model)?;
    spawn_reader(app.clone(), window.label().to_string(), model.id);
    Ok(account(app, model))
}

fn currently_open_model_ids(app: &AppHandle, database: &Database) -> Vec<i64> {
    let Ok(models) = database.list_models() else {
        return Vec::new();
    };
    models
        .into_iter()
        .filter(|m| {
            app.get_webview_window(&window_label(m.id)).is_some()
                || app.get_webview(&messages_of_label(m.id)).is_some()
        })
        .map(|m| m.id)
        .collect()
}

pub fn delete_model_session(
    app: &AppHandle,
    database: &Database,
    model_id: i64,
) -> Result<(), String> {
    let model = database.get_model(model_id).map_err(|err| err.to_string())?;
    let _ = detach_model_from_hub(app, Some(database), model_id);
    if let Some(window) = app.get_webview_window(&window_label(model_id)) {
        let _ = window.destroy();
    }
    database
        .delete_model(model_id)
        .map_err(|err| err.to_string())?;
    if let Ok(directory) = partition_dir(app, &model.partition_id) {
        let _ = std::fs::remove_dir_all(directory);
    }
    let _ = app.emit("models-changed", ());
    Ok(())
}

/// Starts background OnlyFans sessions for all authenticated models so incoming messages,
/// PPV unlocks, and tips trigger notifications no matter which view or chat is active.
pub fn start_background_monitors(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let models = {
            let Some(db) = app.try_state::<Database>() else {
                return;
            };
            db.list_models().unwrap_or_default()
        };
        for model in models {
            if model.of_user_id.is_none() {
                continue;
            }
            schedule_background_monitor(&app, model.id);
            tokio::time::sleep(Duration::from_millis(450)).await;
        }
    });
}

pub fn schedule_background_monitor(app: &AppHandle, model_id: i64) {
    let app_for_main = app.clone();
    let app_inner = app.clone();
    let _ = app_for_main.run_on_main_thread(move || {
        let Some(db) = app_inner.try_state::<Database>() else {
            return;
        };
        let Ok(model) = db.get_model(model_id) else {
            return;
        };
        if model.of_user_id.is_none() {
            return;
        }
        let _ = ensure_background_monitor_on_main(&app_inner, &model);
    });
}

fn ensure_background_monitor_on_main(app: &AppHandle, model: &Model) -> Result<(), String> {
    let _gate = HUB_GATE.lock().unwrap_or_else(|poison| poison.into_inner());
    if model.of_user_id.is_none() {
        return Ok(());
    }
    if app.get_webview_window(&window_label(model.id)).is_some() {
        return Ok(());
    }
    let of_label = messages_of_label(model.id);
    if app.get_webview(&of_label).is_some() {
        spawn_reader(app.clone(), of_label, model.id);
        return Ok(());
    }
    let placement = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .bar_placement;
    let initial_panes = default_message_panes(placement);
    let window = ensure_hub_window(app, &model.display_name, HUB_VISIBLE.load(Ordering::SeqCst), &initial_panes)?;
    let directory = partition_dir(app, &model.partition_id)?;
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let model_id = model.id;
    let builder = tauri::webview::WebviewBuilder::new(
        of_label.clone(),
        WebviewUrl::External(chats_url()?),
    )
    .background_color(tauri::window::Color(15, 23, 42, 255))
    .data_directory(directory)
    .initialization_script(capture_script(model_id, true))
    .on_navigation(move |url| navigation_allowed(model_id, url))
    .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
    .focused(false);
    #[cfg(target_os = "windows")]
    let builder = builder.additional_browser_args(FAST_WEBVIEW_ARGS);
    let created = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(initial_panes.chat_x, initial_panes.chat_y),
            tauri::LogicalSize::new(initial_panes.chat_width, initial_panes.chat_height),
        )
        .map_err(|err| err.to_string())?;
    let is_active_visible = HUB_VISIBLE.load(Ordering::SeqCst)
        && messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .active_model_id
            == Some(model.id);
    if !is_active_visible {
        park_of_webview(&created);
    }
    spawn_reader(app.clone(), of_label, model.id);
    Ok(())
}

pub async fn sync_model_data(
    app: &AppHandle,
    database: &Database,
    bus: &SyncBus,
    model_id: i64,
) -> Result<SyncReport, String> {
    let model = database.get_model(model_id).map_err(|err| err.to_string())?;
    let slot = bus.begin(model_id);
    // The messages webview already owns this account's WebView2 profile.
    // Destroying it and opening another window on the same data directory
    // starts a blank profile and OnlyFans shows the login page.
    let in_hub_tabs = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .open_model_ids
        .contains(&model_id);
    let messages_sync = if let Some(existing) = app.get_webview(&messages_of_label(model_id)) {
        Some(begin_messages_sync(&existing, model_id)?)
    } else if in_hub_tabs && app.get_window(MESSAGES_WINDOW_LABEL).is_some() {
        return Err(
            "La ventana de Messages aún no está lista. Espera un momento y sincroniza de nuevo."
                .to_owned(),
        );
    } else {
        let opened = open_model_session(app, database, Some(model_id))?;
        let window = app
            .get_webview_window(&window_label(model_id))
            .ok_or_else(|| "no se pudo abrir la ventana de OnlyFans".to_owned())?;
        prime_window(&window, opened.model.of_user_id.is_some())?;
        None
    };
    let webview = if let Some(guard) = &messages_sync {
        guard.webview.clone()
    } else {
        app.get_webview_window(&window_label(model_id))
            .ok_or_else(|| "no se pudo abrir la ventana de OnlyFans".to_owned())?
            .as_ref()
            .clone()
    };
    if messages_sync.is_some() {
        spawn_reader(app.clone(), messages_of_label(model_id), model_id);
    }
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(80);
    let mut asked_for_transactions = messages_sync.is_some() || model.of_user_id.is_some();
    loop {
        if slot.unauthorized.load(Ordering::Relaxed) && !slot.profile.load(Ordering::Relaxed) {
            return Err(
                "La sesión de OnlyFans no está autenticada. Inicia sesión en la ventana de la modelo."
                    .to_owned(),
            );
        }
        if slot.profile.load(Ordering::Relaxed)
            && !slot.transactions.load(Ordering::Relaxed)
            && !asked_for_transactions
        {
            asked_for_transactions = true;
            let _ = webview.navigate(statements_url()?);
        }
        if slot.profile.load(Ordering::Relaxed)
            && slot.balances.load(Ordering::Relaxed)
            && (slot.transactions.load(Ordering::Relaxed) || slot.widened.load(Ordering::Relaxed))
            && slot.widened.load(Ordering::Relaxed)
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !slot.profile.load(Ordering::Relaxed) {
        if slot.unauthorized.load(Ordering::Relaxed) {
            return Err(
                "La sesión de OnlyFans no está autenticada. Inicia sesión en la ventana de la modelo."
                    .to_owned(),
            );
        }
        return Err(
            "No se recibieron datos de OnlyFans. Abre la ventana, inicia sesión y sincroniza de nuevo."
                .to_owned(),
        );
    }
    let model = database.get_model(model_id).map_err(|err| err.to_string())?;
    Ok(SyncReport {
        transactions_upserted: slot.upserted.load(Ordering::Relaxed),
        model: account(app, model),
    })
}

pub fn record_capture(
    app: &AppHandle,
    database: &Database,
    bus: &SyncBus,
    window_label_value: &str,
    model_id: i64,
    url: &str,
    status: u16,
    body: &str,
) -> Result<(), String> {
    if !capture_window_matches(window_label_value, model_id) {
        return Err("la ventana no corresponde a la modelo".to_owned());
    }
    if body.len() > 1_500_000 {
        return Err("la respuesta de OnlyFans es demasiado grande".to_owned());
    }
    let effective_model_id = resolve_model_alias(model_id);
    let was_unauthenticated = database
        .get_model(effective_model_id)
        .map(|m| m.of_user_id.is_none())
        .unwrap_or(false);

    let outcome = match ingest_body(database, effective_model_id, url, status, body) {
        Ok(outcome) => outcome,
        Err(err) => {
            let msg = err.to_string();
            if was_unauthenticated && msg.contains("ya está agregada") {
                if try_adopt_duplicate_profile(app, database, model_id, effective_model_id, body)
                    .is_some()
                {
                    let _ = app.emit("models-changed", ());
                    return Ok(());
                }
            }
            return Err(msg);
        }
    };
    let Some(outcome) = outcome else {
        return Ok(());
    };
    note(bus, effective_model_id, &outcome);

    if outcome.kind == crate::sync::CaptureKind::Profile && !outcome.unauthorized && was_unauthenticated {
        if let Ok(updated) = database.get_model(effective_model_id) {
            if updated.of_user_id.is_some() {
                on_model_first_authenticated(app, model_id, &updated);
            }
        }
    }

    if outcome.notifications.source != crate::notifications::BatchSource::None {
        if let Some(notif_bus) = app.try_state::<crate::notifications::NotificationBus>() {
            let focus = current_live_focus();
            let emitted = notif_bus.process_batch(
                Some(app),
                database,
                effective_model_id,
                outcome.notifications,
                focus.as_ref(),
            );
            if !emitted.is_empty() {
                emit_tabs_changed(app);
            }
        }
    }

    if !outcome.unauthorized && outcome.kind != crate::sync::CaptureKind::LiveEvent {
        let _ = app.emit("models-changed", ());
        if let Some(fan_id) = &outcome.fan_id {
            let _ = app.emit(
                "fan-insights-changed",
                serde_json::json!({ "modelId": effective_model_id, "ofFanId": fan_id }),
            );
        }
    }
    Ok(())
}

fn on_model_first_authenticated(app: &AppHandle, raw_window_model_id: i64, updated: &Model) {
    let label_name = if !updated.display_name.trim().is_empty() {
        updated.display_name.trim().to_owned()
    } else if !updated.username.trim().is_empty() {
        format!("@{}", updated.username.trim())
    } else {
        "OnlyFans".to_owned()
    };
    let title = if !updated.username.trim().is_empty() && !updated.display_name.trim().is_empty() {
        format!("{} (@{}) · CreatorFloww", updated.display_name.trim(), updated.username.trim())
    } else {
        format!("{label_name} · CreatorFloww")
    };
    if let Some(win) = app.get_webview_window(&window_label(raw_window_model_id)) {
        let _ = win.set_title(&title);
        let banner_js = format!(
            "window.__CF_LOGIN_READY && window.__CF_LOGIN_READY({});",
            js_string(&label_name)
        );
        let _ = win.eval(&banner_js);
    }
    let _ = app.emit("model-login-completed", account(app, updated.clone()));
    schedule_background_monitor(app, updated.id);
}

fn try_adopt_duplicate_profile(
    app: &AppHandle,
    database: &Database,
    raw_window_model_id: i64,
    placeholder_id: i64,
    body: &str,
) -> Option<Model> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = parsed.as_object()?;
    let of_user_id = match obj.get("id")? {
        serde_json::Value::String(s) if !s.trim().is_empty() => s.trim().to_owned(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let username = obj.get("username").and_then(|v| v.as_str()).unwrap_or("");
    let display_name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("displayName").and_then(|v| v.as_str()))
        .unwrap_or("");
    let avatar_url = obj
        .get("avatarThumbs")
        .and_then(|t| t.get("c144"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| obj.get("avatar").and_then(|v| v.as_str()).filter(|s| !s.is_empty()));

    let (adopted, old_partition) = database
        .adopt_partition_into_existing_model(
            placeholder_id,
            &of_user_id,
            username,
            display_name,
            avatar_url,
        )
        .ok()??;

    register_model_alias(raw_window_model_id, adopted.id);
    register_model_alias(placeholder_id, adopted.id);

    if app.get_webview_window(&window_label(adopted.id)).is_none()
        && app.get_webview(&messages_of_label(adopted.id)).is_none()
    {
        if let Ok(old_dir) = partition_dir(app, &old_partition) {
            let _ = std::fs::remove_dir_all(old_dir);
        }
    }
    on_model_first_authenticated(app, raw_window_model_id, &adopted);
    Some(adopted)
}

fn capture_window_matches(label: &str, model_id: i64) -> bool {
    label == window_label(model_id) || label == format!("messages-{model_id}-of")
}

fn note(bus: &SyncBus, model_id: i64, outcome: &IngestOutcome) {
    let Some(slot) = bus.slot(model_id) else {
        return;
    };
    match outcome.kind {
        crate::sync::CaptureKind::Profile if outcome.unauthorized => {
            slot.unauthorized.store(true, Ordering::Relaxed);
        }
        crate::sync::CaptureKind::Profile => {
            slot.profile.store(true, Ordering::Relaxed);
        }
        crate::sync::CaptureKind::Balances => {
            slot.balances.store(true, Ordering::Relaxed);
        }
        crate::sync::CaptureKind::Transactions | crate::sync::CaptureKind::Chat => {
            slot.transactions.store(true, Ordering::Relaxed);
            if let Some(stats) = outcome.earnings {
                slot.upserted.fetch_add(stats.upserted, Ordering::Relaxed);
            }
        }
        crate::sync::CaptureKind::Subscriber | crate::sync::CaptureKind::LiveEvent => {}
    }
}

pub fn get_message_tabs(app: &AppHandle) -> MessageTabsState {
    let unread_counts = app
        .try_state::<crate::notifications::NotificationBus>()
        .and_then(|bus| {
            app.try_state::<Database>()
                .map(|database| bus.unread_counts(&database))
        })
        .unwrap_or_default();
    let mut state = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    state.unread_counts = unread_counts;
    if app.get_window(MESSAGES_WINDOW_LABEL).is_none() || !HUB_VISIBLE.load(Ordering::SeqCst) {
        state.open_model_ids.clear();
        state.active_model_id = None;
        return state.clone();
    }
    state
        .open_model_ids
        .retain(|id| app.get_webview(&messages_of_label(*id)).is_some());
    if let Some(active) = state.active_model_id {
        if !state.open_model_ids.contains(&active) {
            state.active_model_id = state.open_model_ids.first().copied();
        }
    } else {
        state.active_model_id = state.open_model_ids.first().copied();
    }
    state.clone()
}

pub fn select_message_tab(
    app: &AppHandle,
    database: &Database,
    model_id: i64,
) -> Result<(), String> {
    let already_open = {
        let state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        HUB_VISIBLE.load(Ordering::SeqCst)
            && state.open_model_ids.contains(&model_id)
            && app.get_window(MESSAGES_WINDOW_LABEL).is_some()
            && app.get_webview(&messages_of_label(model_id)).is_some()
    };
    if !already_open {
        return open_messages(app, database, model_id);
    }
    {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.active_model_id = Some(model_id);
    }
    if let Some(nb) = app.try_state::<crate::notifications::NotificationBus>() {
        if nb.clear_unread(database, model_id) {
            let _ = app.emit("unread-counts-changed", nb.unread_counts(database));
        }
    }
    update_hub_title(app, database, Some(model_id));
    layout_messages_hub(app);
    if let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) {
        let _ = focus_window(&window);
    }
    if let Some(of_view) = app.get_webview(&messages_of_label(model_id)) {
        let _ = of_view.set_focus();
    }
    emit_tabs_changed(app);
    Ok(())
}

/// Closes a visible tab in `MessagesTabBar` while keeping the model's background OnlyFans
/// webview alive so incoming messages and sales continue triggering notifications.
pub fn close_message_tab(
    app: &AppHandle,
    database: Option<&Database>,
    model_id: i64,
) -> Result<(), String> {
    if let Some(of_view) = app.get_webview(&messages_of_label(model_id)) {
        park_of_webview(&of_view);
    }
    if let Some(insights) = app.get_webview(&messages_insights_label(model_id)) {
        let _ = insights.hide();
    }
    finish_removing_tab(app, database, model_id)
}

/// Truly destroys the `messages-<id>-of` webview so its WebView2 `data_directory` lock is
/// released (used when opening standalone `model-<id>` or deleting the model).
fn detach_model_from_hub(
    app: &AppHandle,
    database: Option<&Database>,
    model_id: i64,
) -> Result<(), String> {
    if let Some(of_view) = app.get_webview(&messages_of_label(model_id)) {
        let _ = of_view.close();
    }
    if let Some(insights) = app.get_webview(&messages_insights_label(model_id)) {
        let _ = insights.close();
    }
    finish_removing_tab(app, database, model_id)
}

fn finish_removing_tab(
    app: &AppHandle,
    database: Option<&Database>,
    model_id: i64,
) -> Result<(), String> {
    let next_active = {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let removed_idx = state.open_model_ids.iter().position(|id| *id == model_id);
        state.open_model_ids.retain(|id| *id != model_id);
        if state.open_model_ids.is_empty() {
            state.active_model_id = None;
        } else if state.active_model_id == Some(model_id) {
            let pick = removed_idx
                .map(|idx| idx.saturating_sub(1).min(state.open_model_ids.len() - 1))
                .unwrap_or(0);
            state.active_model_id = state.open_model_ids.get(pick).copied();
        }
        state.active_model_id
    };
    if next_active.is_none() {
        HUB_VISIBLE.store(false, Ordering::SeqCst);
        if let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) {
            park_every_of_webview(app);
            park_messages_hub(&window);
        }
    } else {
        if let Some(db) = database {
            update_hub_title(app, db, next_active);
        }
        layout_messages_hub(app);
        if let Some(active_id) = next_active {
            if let Some(of_view) = app.get_webview(&messages_of_label(active_id)) {
                let _ = of_view.set_focus();
            }
        }
    }
    emit_tabs_changed(app);
    let _ = app.emit("models-changed", ());
    Ok(())
}

fn default_message_panes(placement: MessageBarPlacement) -> MessagePanes {
    message_panes(1280.0, 800.0, placement).unwrap_or(MessagePanes {
        tabs_x: 0.0,
        tabs_y: 0.0,
        tabs_width: 1280.0,
        tabs_height: TAB_BAR_HEIGHT,
        chat_x: 0.0,
        chat_y: TAB_BAR_HEIGHT,
        chat_width: 900.0,
        chat_height: 800.0 - TAB_BAR_HEIGHT,
        insights_x: 900.0,
        insights_y: TAB_BAR_HEIGHT,
        insights_width: 380.0,
        insights_height: 800.0 - TAB_BAR_HEIGHT,
    })
}

fn ensure_hub_window(
    app: &AppHandle,
    display_name: &str,
    visible: bool,
    initial_panes: &MessagePanes,
) -> Result<tauri::Window, String> {
    if let Some(existing) = app.get_window(MESSAGES_WINDOW_LABEL) {
        return Ok(existing);
    }
    let title = if display_name.trim().is_empty() {
        "Mensajes · CreatorFloww".to_owned()
    } else {
        format!("Mensajes · {}", display_name.trim())
    };
    let created = tauri::window::WindowBuilder::new(app, MESSAGES_WINDOW_LABEL)
        .title(title)
        .inner_size(1280.0, 800.0)
        .min_inner_size(980.0, 640.0)
        .center()
        .visible(visible)
        .focused(visible)
        .resizable(true)
        .closable(true)
        .build()
        .map_err(|err| err.to_string())?;
    created
        .add_child(
            tauri::webview::WebviewBuilder::new(
                MESSAGES_TABS_LABEL,
                WebviewUrl::App("index.html".into()),
            )
            .background_color(tauri::window::Color(15, 23, 42, 255))
            .initialization_script(tabs_script()),
            tauri::LogicalPosition::new(initial_panes.tabs_x, initial_panes.tabs_y),
            tauri::LogicalSize::new(initial_panes.tabs_width, initial_panes.tabs_height),
        )
        .map_err(|err| err.to_string())?;

    let host = created.clone();
    let closing = created.clone();
    let app_for_events = app.clone();
    let layout_queued = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    created.on_window_event(move |event| {
        let relayout = match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                HUB_VISIBLE.store(false, Ordering::SeqCst);
                let (removed_ids, snapshot) = {
                    let mut state = messages_hub_state()
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    let prev = state.open_model_ids.clone();
                    state.open_model_ids.clear();
                    state.active_model_id = None;
                    (prev, state.clone())
                };
                for id in removed_ids {
                    if let Some(of_view) = app_for_events.get_webview(&messages_of_label(id)) {
                        park_of_webview(&of_view);
                    }
                    if let Some(ins) = app_for_events.get_webview(&messages_insights_label(id)) {
                        let _ = ins.hide();
                    }
                }
                park_every_of_webview(&app_for_events);
                park_messages_hub(&closing);
                let _ = app_for_events.emit("message-tabs-changed", snapshot);
                let _ = app_for_events.emit("models-changed", ());
                false
            }
            tauri::WindowEvent::Resized(size) => size.width > 0 && size.height > 0,
            tauri::WindowEvent::Moved(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => true,
            _ => false,
        };
        if relayout && !layout_queued.swap(true, Ordering::AcqRel) {
            let queued = layout_queued.clone();
            let app_layout = app_for_events.clone();
            let posted = host.run_on_main_thread(move || {
                queued.store(false, Ordering::Release);
                layout_messages_hub(&app_layout);
            });
            if posted.is_err() {
                layout_queued.store(false, Ordering::Release);
                layout_messages_hub(&app_for_events);
            }
        }
    });
    if !visible {
        park_messages_hub(&created);
    }
    Ok(created)
}

pub fn open_messages(app: &AppHandle, database: &Database, model_id: i64) -> Result<(), String> {
    let _gate = HUB_GATE.lock().unwrap_or_else(|poison| poison.into_inner());
    let model = database.get_model(model_id).map_err(|err| err.to_string())?;
    close_label(app, &window_label(model_id));

    let placement = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .bar_placement;
    let initial_panes = default_message_panes(placement);
    let window = ensure_hub_window(app, &model.display_name, true, &initial_panes)?;
    // The hub may be parked at -32000. WebView2 refuses to create a controller there (0x8007139F).
    reveal_messages_hub(&window);

    let of_label = messages_of_label(model_id);
    let insights_label = messages_insights_label(model_id);
    if app.get_webview(&of_label).is_none() {
        let directory = partition_dir(app, &model.partition_id)?;
        std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
        let builder = tauri::webview::WebviewBuilder::new(
            of_label.clone(),
            WebviewUrl::External(chats_url()?),
        )
        .background_color(tauri::window::Color(15, 23, 42, 255))
        .data_directory(directory)
        .initialization_script(capture_script(model.id, true))
        .on_navigation(move |url| navigation_allowed(model.id, url))
        .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
        .focused(true);
        #[cfg(target_os = "windows")]
        let builder = builder.additional_browser_args(FAST_WEBVIEW_ARGS);
        window
            .add_child(
                builder,
                tauri::LogicalPosition::new(initial_panes.chat_x, initial_panes.chat_y),
                tauri::LogicalSize::new(initial_panes.chat_width, initial_panes.chat_height),
            )
            .map_err(|err| err.to_string())?;
    }
    if app.get_webview(&insights_label).is_none() {
        window
            .add_child(
                tauri::webview::WebviewBuilder::new(
                    insights_label,
                    WebviewUrl::App("index.html".into()),
                )
                .initialization_script(insights_script(model.id)),
                tauri::LogicalPosition::new(initial_panes.insights_x, initial_panes.insights_y),
                tauri::LogicalSize::new(initial_panes.insights_width, initial_panes.insights_height),
            )
            .map_err(|err| err.to_string())?;
    }

    HUB_VISIBLE.store(true, Ordering::SeqCst);
    {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !state.open_model_ids.contains(&model_id) {
            state.open_model_ids.push(model_id);
        }
        state.active_model_id = Some(model_id);
    }
    if let Some(nb) = app.try_state::<crate::notifications::NotificationBus>() {
        if nb.clear_unread(database, model_id) {
            let _ = app.emit("unread-counts-changed", nb.unread_counts(database));
        }
    }

    update_hub_title(app, database, Some(model_id));
    reveal_messages_hub(&window);
    focus_window(&window)?;
    layout_messages_hub(app);
    spawn_reader(app.clone(), of_label.clone(), model_id);
    if let Some(of_view) = app.get_webview(&of_label) {
        let _ = of_view.set_focus();
    }
    emit_tabs_changed(app);
    let _ = app.emit("models-changed", ());
    Ok(())
}

pub fn open_incoming_chat(
    app: &AppHandle,
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
) -> Result<(), String> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
    }
    open_messages(app, database, model_id)?;
    let fan = of_fan_id.trim().trim_start_matches(['u', 'U']).to_owned();
    if fan.is_empty() {
        return Ok(());
    }
    note_viewing_fan(model_id, &fan);
    let app_nav = app.clone();
    let label = messages_of_label(model_id);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(450)).await;
        let Some(view) = app_nav.get_webview(&label) else {
            return;
        };
        let url = format!("https://onlyfans.com/my/chats/chat/{fan}");
        let _ = view.eval(&format!("location.assign({});", js_string(&url)));
    });
    Ok(())
}

fn account(app: &AppHandle, model: Model) -> ModelAccount {
    let hub_open = HUB_VISIBLE.load(Ordering::SeqCst)
        && messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .open_model_ids
            .contains(&model.id);
    let window_open = app.get_webview_window(&window_label(model.id)).is_some() || hub_open;
    ModelAccount { model, window_open }
}

fn open_or_focus(app: &AppHandle, model: &Model) -> Result<WebviewWindow, String> {
    let label = window_label(model.id);
    if let Some(window) = app.get_webview_window(&label) {
        allow_small_onlyfans(&window);
        focus(&window)?;
        return Ok(window);
    }
    let _ = detach_model_from_hub(app, None, model.id);
    let directory = partition_dir(app, &model.partition_id)?;
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let title = if !model.display_name.trim().is_empty() {
        format!("{} · CreatorFloww", model.display_name.trim())
    } else if !model.username.trim().is_empty() {
        format!("@{} · CreatorFloww", model.username.trim())
    } else {
        "Iniciar sesión en OnlyFans · CreatorFloww".to_owned()
    };
    // The background monitor may still be releasing this partition. Same HRESULT as a bad WebView2 state.
    let mut last_err = String::new();
    for attempt in 0..6 {
        let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(home_url()?))
            .title(title.clone())
            .inner_size(1100.0, 760.0)
            .min_inner_size(320.0, 400.0)
            .resizable(true)
            .closable(true)
            .center()
            .focused(true)
            .background_color(tauri::window::Color(15, 23, 42, 255))
            .data_directory(directory.clone())
            .initialization_script(capture_script(model.id, false));
        #[cfg(target_os = "windows")]
        let builder = builder.additional_browser_args(FAST_WEBVIEW_ARGS);
        match builder.build() {
            Ok(window) => {
                bind_close(&window, model.id);
                allow_small_onlyfans(&window);
                return Ok(window);
            }
            Err(err) => {
                last_err = err.to_string();
                let retry = last_err.contains("8007139F")
                    || last_err.contains("estado correcto")
                    || last_err.contains("correct state");
                if !retry || attempt == 5 {
                    return Err(last_err);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    Err(last_err)
}

fn allow_small_onlyfans(window: &WebviewWindow) {
    let _ = window.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(320.0, 400.0))));
}

fn bind_close(window: &WebviewWindow, raw_model_id: i64) {
    let target = window.clone();
    let app_handle = window.app_handle().clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = target.destroy();
            let _ = app_handle.emit("models-changed", ());
            // Resume silent background monitoring after WebView2 releases the partition directory
            let app_resume = app_handle.clone();
            let effective_id = resolve_model_alias(raw_model_id);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(900)).await;
                schedule_background_monitor(&app_resume, effective_id);
            });
        }
    });
}

fn close_label(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.destroy();
    }
    if let Some(window) = app.get_window(label) {
        let _ = window.destroy();
    }
}

struct MessagePanes {
    tabs_x: f64,
    tabs_y: f64,
    tabs_width: f64,
    tabs_height: f64,
    chat_x: f64,
    chat_y: f64,
    chat_width: f64,
    chat_height: f64,
    insights_x: f64,
    insights_y: f64,
    insights_width: f64,
    insights_height: f64,
}

fn message_panes(
    width: f64,
    height: f64,
    placement: MessageBarPlacement,
) -> Option<MessagePanes> {
    if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
        return None;
    }
    match placement {
        MessageBarPlacement::Top => {
            let tab_bar_height = TAB_BAR_HEIGHT.min(height);
            let content_height = (height - tab_bar_height).max(0.0);
            let insights_width = 380.0_f64.min(width * 0.42);
            let chat_width = (width - insights_width).max(0.0);
            Some(MessagePanes {
                tabs_x: 0.0,
                tabs_y: 0.0,
                tabs_width: width,
                tabs_height: tab_bar_height,
                chat_x: 0.0,
                chat_y: tab_bar_height,
                chat_width,
                chat_height: content_height,
                insights_x: chat_width,
                insights_y: tab_bar_height,
                insights_width,
                insights_height: content_height,
            })
        }
        MessageBarPlacement::Sidebar => {
            let sidebar_width = SIDEBAR_WIDTH.min((width * 0.22).max(140.0)).min(width);
            let remaining_width = (width - sidebar_width).max(0.0);
            let insights_width = 370.0_f64.min(remaining_width * 0.40);
            let chat_width = (remaining_width - insights_width).max(0.0);
            Some(MessagePanes {
                tabs_x: 0.0,
                tabs_y: 0.0,
                tabs_width: sidebar_width,
                tabs_height: height,
                chat_x: sidebar_width,
                chat_y: 0.0,
                chat_width,
                chat_height: height,
                insights_x: sidebar_width + chat_width,
                insights_y: 0.0,
                insights_width,
                insights_height: height,
            })
        }
    }
}

fn layout_messages_hub(app: &AppHandle) {
    let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    if size.width == 0 || size.height == 0 {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return;
    }
    let logical_width = size.width as f64 / scale;
    let logical_height = size.height as f64 / scale;
    let state = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let Some(panes) = message_panes(logical_width, logical_height, state.bar_placement) else {
        return;
    };
    if let Some(tabs) = app.get_webview(MESSAGES_TABS_LABEL) {
        let _ = tabs.set_position(tauri::LogicalPosition::new(panes.tabs_x, panes.tabs_y));
        let _ = tabs.set_size(tauri::LogicalSize::new(panes.tabs_width, panes.tabs_height));
        let _ = tabs.show();
    }
    let mut all_ids = state.open_model_ids.clone();
    if let Some(db) = app.try_state::<Database>() {
        if let Ok(models) = db.list_models() {
            for m in models {
                if !all_ids.contains(&m.id) {
                    all_ids.push(m.id);
                }
            }
        }
    }
    let hub_visible = HUB_VISIBLE.load(Ordering::SeqCst);
    let mut active_of: Option<tauri::Webview> = None;
    for id in &all_ids {
        let is_active = hub_visible && state.active_model_id == Some(*id) && state.open_model_ids.contains(id);
        if let Some(of_view) = app.get_webview(&messages_of_label(*id)) {
            if is_active {
                active_of = Some(of_view);
            } else if hub_visible {
                // Off the visible hub, still 1100×760. A later webview would otherwise cover the open chat.
                place_webview(&of_view, -1400.0, 0.0, 1100.0, 760.0);
            } else {
                park_of_webview(&of_view);
            }
        }
        if let Some(insights) = app.get_webview(&messages_insights_label(*id)) {
            if is_active {
                place_webview(
                    &insights,
                    panes.insights_x,
                    panes.insights_y,
                    panes.insights_width,
                    panes.insights_height,
                );
            } else {
                let _ = insights.hide();
            }
        }
    }
    if let Some(of_view) = active_of {
        place_webview(&of_view, panes.chat_x, panes.chat_y, panes.chat_width, panes.chat_height);
    }
    if let Some(tabs) = app.get_webview(MESSAGES_TABS_LABEL) {
        let _ = tabs.show();
    }
}

fn focus_window(window: &tauri::Window) -> Result<(), String> {
    let _ = window.unminimize();
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    Ok(())
}

fn focus(window: &WebviewWindow) -> Result<(), String> {
    let _ = window.unminimize();
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    Ok(())
}

fn prime_window(window: &WebviewWindow, known_account: bool) -> Result<(), String> {
    if known_account {
        window
            .navigate(statements_url()?)
            .map_err(|err| err.to_string())?;
        return Ok(());
    }
    match window.url() {
        Ok(url) if url.host_str() == Some("onlyfans.com") => {
            window.reload().map_err(|err| err.to_string())
        }
        _ => window
            .navigate(home_url()?)
            .map_err(|err| err.to_string()),
    }
}

fn partition_dir(app: &AppHandle, partition_id: &str) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())?
        .join("partitions")
        .join(partition_id))
}

fn chats_url() -> Result<Url, String> {
    "https://onlyfans.com/my/chats"
        .parse::<Url>()
        .map_err(|err| err.to_string())
}

pub async fn insert_chat_text(app: &AppHandle, model_id: i64, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("el script no tiene texto para insertar".to_owned());
    }
    if text.chars().count() > 10_000 {
        return Err("el mensaje supera los 10000 caracteres".to_owned());
    }
    let label = format!("messages-{model_id}-of");
    let webview = app.get_webview(&label).ok_or_else(|| {
        "la ventana de OnlyFans de esta modelo no está abierta".to_owned()
    })?;
    let literal = serde_json::to_string(text).map_err(|err| err.to_string())?;
    let script = INSERT_CHAT_JS.replace("__CF_TEXT__", &literal);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let slot = Mutex::new(Some(sender));
    webview
        .eval_with_callback(&script, move |value| {
            if let Some(sender) = slot.lock().unwrap_or_else(|poison| poison.into_inner()).take() {
                let _ = sender.send(value);
            }
        })
        .map_err(|err| err.to_string())?;
    let value = tokio::time::timeout(Duration::from_secs(3), receiver)
        .await
        .map_err(|_| "el chat de OnlyFans no respondió".to_owned())?
        .map_err(|_| "el chat de OnlyFans se cerró antes de insertar el texto".to_owned())?;
    let status = value.trim().trim_matches('"');
    Ok(if status == "inserted" {
        "inserted".to_owned()
    } else {
        "missing".to_owned()
    })
}

pub fn onlyfans_webview(app: &AppHandle, model_id: i64) -> Result<tauri::Webview, String> {
    for label in [window_label(model_id), format!("messages-{model_id}-of")] {
        if let Some(webview) = app.get_webview(&label) {
            return Ok(webview);
        }
    }
    Err("Abre la ventana de la modelo e inicia sesión en OnlyFans.".to_owned())
}

pub async fn fetch_onlyfans_json(app: &AppHandle, model_id: i64, url: &str) -> Result<String, String> {
    let webview = onlyfans_webview(app, model_id)?;
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let url_literal = serde_json::to_string(url).map_err(|err| err.to_string())?;
    let start = vault_request_script(token, &url_literal);
    eval_text(&webview, &start).await?;
    let read = format!(r#"(window["__CF_VAULT_{token}"] || "")"#);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let value = eval_text(&webview, &read).await?;
        let decoded = decode_eval_string(&value);
        if !decoded.is_empty() {
            return vault_fetch_body(&decoded);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("OnlyFans no respondió el vault.".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn eval_text(webview: &tauri::Webview, script: &str) -> Result<String, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let slot = Mutex::new(Some(sender));
    webview
        .eval_with_callback(script, move |value| {
            if let Some(sender) = slot.lock().unwrap_or_else(|poison| poison.into_inner()).take() {
                let _ = sender.send(value);
            }
        })
        .map_err(|err| err.to_string())?;
    tokio::time::timeout(Duration::from_secs(8), receiver)
        .await
        .map_err(|_| "OnlyFans no respondió.".to_owned())?
        .map_err(|_| "la ventana de OnlyFans se cerró".to_owned())
}

fn decode_eval_string(value: &str) -> String {
    serde_json::from_str::<String>(value).unwrap_or_else(|_| value.trim().trim_matches('"').to_owned())
}

fn vault_request_script(token: u128, url_literal: &str) -> String {
    format!(
        r#"(async () => {{
          const key = "__CF_VAULT_{token}";
          const target = {url_literal};
          window[key] = "";
          function finish(status, body, error) {{
            window[key] = JSON.stringify({{ status: status, body: String(body || "").slice(0, 2000000), error: error || "" }});
          }}
          function webpackRequire() {{
            const names = Object.getOwnPropertyNames(window).filter((name) => name.indexOf("webpackChunk") === 0);
            for (let index = 0; index < names.length; index += 1) {{
              const chunk = window[names[index]];
              if (!chunk || typeof chunk.push !== "function") continue;
              const id = "cfvault" + Date.now() + index;
              let req = null;
              try {{
                const modules = {{}};
                modules[id] = function (module, exports, require) {{ req = require; }};
                chunk.push([[id], modules, [[id]]]);
              }} catch (error) {{}}
              if (!req) {{
                try {{
                  chunk.push([[id + "b"], {{}}, function (require) {{ req = require; }}]);
                }} catch (error) {{}}
              }}
              if (req) return req;
            }}
            return null;
          }}
          function isClient(value) {{
            return !!(value && typeof value.get === "function" && value.interceptors && value.interceptors.request);
          }}
          function findClient() {{
            const req = webpackRequire();
            const cache = req && req.c;
            if (!cache) return null;
            let best = null;
            let bestScore = 0;
            Object.keys(cache).forEach((id) => {{
              const exp = cache[id] && cache[id].exports;
              [exp, exp && exp.default].forEach((value) => {{
                if (!isClient(value)) return;
                let score = 1;
                try {{
                  const headers = value.defaults && value.defaults.headers;
                  const bag = headers && (headers.common || headers);
                  if (bag && (bag["app-token"] || bag["App-Token"])) score += 3;
                  const base = String((value.defaults && value.defaults.baseURL) || "");
                  if (base.indexOf("onlyfans.com") !== -1) score += 2;
                }} catch (error) {{}}
                if (score > bestScore) {{
                  best = value;
                  bestScore = score;
                }}
              }});
            }});
            return best;
          }}
          try {{
            const client = findClient();
            if (!client) {{
              const cached = window.__CF_VAULT_ITEMS || {{}};
              const items = Object.keys(cached).map((id) => cached[id]).filter((item) => item && (item.type === "photo" || item.type === "video" || item.type === "gif"));
              if (items.length) {{
                finish(200, JSON.stringify({{ list: items, hasMore: false }}), "");
                return;
              }}
              finish(0, "", "Abre la bóveda en la ventana de la modelo. No pedí el vault sin la firma de OnlyFans.");
              return;
            }}
            const response = await client.get(target);
            const data = response && response.data;
            const body = typeof data === "string" ? data : JSON.stringify(data || {{}});
            finish((response && response.status) || 200, body, "");
          }} catch (error) {{
            const status = error && error.response ? error.response.status : 0;
            const data = error && error.response ? error.response.data : null;
            const body = data ? (typeof data === "string" ? data : JSON.stringify(data)) : "";
            finish(status, body, status ? "" : String(error && error.message || error));
          }}
        }})();
        "started""#
    )
}

fn vault_fetch_body(text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    if let Some(error) = value.get("error").and_then(|item| item.as_str()) {
        if !error.is_empty() {
            return Err(error.to_owned());
        }
    }
    let status = value.get("status").and_then(|item| item.as_u64()).unwrap_or(0);
    if status == 401 || status == 403 {
        return Err(
            "La sesión de OnlyFans no está autenticada. Inicia sesión en la ventana de la modelo."
                .to_owned(),
        );
    }
    if status != 200 {
        return Err(format!("OnlyFans respondió {status} al pedir el vault."));
    }
    value
        .get("body")
        .and_then(|item| item.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "OnlyFans devolvió un vault vacío.".to_owned())
}

const INSERT_CHAT_JS: &str = r#"(() => {
  const text = __CF_TEXT__;
  function visible(node) {
    const rect = node.getBoundingClientRect();
    return rect.width > 40 && rect.height > 8;
  }
  const preferred = [
    ".b-chat__footer [contenteditable='true']",
    ".b-chat__input [contenteditable='true']",
    ".b-chat__message-input [contenteditable='true']",
    ".b-chat__input-message [contenteditable='true']",
    ".b-text-editor[contenteditable='true']",
    ".ProseMirror",
    "textarea.b-chat__message-input",
    ".b-make-post [contenteditable='true']"
  ];
  let node = null;
  for (const selector of preferred) {
    const found = document.querySelector(selector);
    if (found && visible(found)) {
      node = found;
      break;
    }
  }
  if (!node) {
    const nodes = Array.from(document.querySelectorAll("[contenteditable='true'], textarea"));
    node = nodes.find((item) => visible(item) && item.getBoundingClientRect().top > window.innerHeight * 0.45) || null;
  }
  if (!node) return "missing";
  node.focus();
  if (node instanceof HTMLTextAreaElement || node instanceof HTMLInputElement) {
    const proto = node instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
    if (setter) setter.call(node, text);
    else node.value = text;
    node.dispatchEvent(new Event("input", { bubbles: true }));
    return "inserted";
  }
  const selection = window.getSelection();
  if (selection) {
    const range = document.createRange();
    range.selectNodeContents(node);
    selection.removeAllRanges();
    selection.addRange(range);
  }
  const inserted = document.execCommand("insertText", false, text);
  if (!inserted) {
    node.textContent = text;
    node.dispatchEvent(new InputEvent("input", { bubbles: true, data: text, inputType: "insertText" }));
  }
  return "inserted";
})()"#;

fn insights_script(model_id: i64) -> String {
    format!(
        "window.__CF_VIEW__ = \"insights\"; window.__CF_MODEL_ID__ = {model_id};"
    )
}

fn home_url() -> Result<Url, String> {
    "https://onlyfans.com"
        .parse::<Url>()
        .map_err(|err| err.to_string())
}

#[allow(dead_code)]
fn statistics_url() -> Result<Url, String> {
    "https://onlyfans.com/my/statistics"
        .parse::<Url>()
        .map_err(|err| err.to_string())
}

fn statements_url() -> Result<Url, String> {
    "https://onlyfans.com/my/statements/earnings"
        .parse::<Url>()
        .map_err(|err| err.to_string())
}

fn chats_navigation(url: &Url) -> bool {
    let host_ok = matches!(url.host_str(), Some("onlyfans.com") | Some("www.onlyfans.com"));
    if !host_ok {
        return false;
    }
    let path = url.path();
    path.starts_with("/my/chats") || path.starts_with("/auth") || path.starts_with("/login") || path == "/"
}

fn earnings_page(url: &Url) -> bool {
    let host_ok = matches!(url.host_str(), Some("onlyfans.com") | Some("www.onlyfans.com"));
    if !host_ok {
        return false;
    }
    let path = url.path();
    path.starts_with("/my/statements") || path.starts_with("/my/statistics")
}

fn navigation_allowed(model_id: i64, url: &Url) -> bool {
    chats_navigation(url) || (earnings_gate(model_id).load(Ordering::SeqCst) && earnings_page(url))
}

fn earnings_gate(model_id: i64) -> Arc<AtomicBool> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static GATES: OnceLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = OnceLock::new();
    let gates = GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut gates = gates.lock().unwrap_or_else(|poison| poison.into_inner());
    gates
        .entry(model_id)
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

fn messages_of_label(model_id: i64) -> String {
    format!("messages-{model_id}-of")
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

fn restore_target(url: Option<Url>) -> String {
    let Some(url) = url else {
        return "https://onlyfans.com/my/chats".to_owned();
    };
    let host_ok = matches!(url.host_str(), Some("onlyfans.com") | Some("www.onlyfans.com"));
    if host_ok && (url.path().starts_with("/my/chats") || url.path() == "/") {
        return url.to_string();
    }
    "https://onlyfans.com/my/chats".to_owned()
}

fn open_earnings_script() -> Result<String, String> {
    let url = js_string(statements_url()?.as_str());
    Ok(format!(
        "try{{sessionStorage.setItem('cf-allow-earnings','1')}}catch(e){{}};location.assign({url});"
    ))
}

fn restore_chat_script(url: &str) -> String {
    format!(
        "try{{sessionStorage.removeItem('cf-allow-earnings')}}catch(e){{}};location.assign({});",
        js_string(url)
    )
}

struct MessagesSyncGuard {
    model_id: i64,
    webview: tauri::Webview,
    restore_url: String,
}

impl Drop for MessagesSyncGuard {
    fn drop(&mut self) {
        earnings_gate(self.model_id).store(false, Ordering::SeqCst);
        let _ = self.webview.eval(restore_chat_script(&self.restore_url));
    }
}

fn begin_messages_sync(webview: &tauri::Webview, model_id: i64) -> Result<MessagesSyncGuard, String> {
    let restore_url = restore_target(webview.url().ok());
    let script = open_earnings_script()?;
    earnings_gate(model_id).store(true, Ordering::SeqCst);
    let guard = MessagesSyncGuard {
        model_id,
        webview: webview.clone(),
        restore_url,
    };
    webview.eval(script).map_err(|err| err.to_string())?;
    Ok(guard)
}

fn capture_script(model_id: i64, stay_in_chats: bool) -> String {
    format!(
        r##"
(() => {{
  const modelId = {model_id};
  const stayInChats = {stay_in_chats};
  const paths = new Set([
    "/api2/v2/users/me",
    "/api2/v2/payouts/balances",
    "/api2/v2/payouts/transactions"
  ]);
  function absolute(url) {{
    try {{
      return new URL(url, location.href).href;
    }} catch (error) {{
      return "";
    }}
  }}
  function watched(url) {{
    try {{
      const parsed = new URL(url, location.href);
      if (parsed.hostname !== "onlyfans.com") return false;
      const path = parsed.pathname;
      if (paths.has(path)) return true;
      if (path.indexOf("/api2/v2/earnings/") === 0) return true;
      if (path.indexOf("/api2/v2/payouts/") === 0) return true;
      if (path === "/api2/v2/chats") return true;
      if (path === "/api2/v2/users/list") return true;
      if (path.indexOf("/api2/v2/chats/") !== -1 && path.indexOf("/messages") !== -1) return true;
      if (/^\/api2\/v2\/chats\/\d+$/.test(path)) return true;
      if (path.indexOf("/api2/v2/subscriptions") === 0) return true;
      if (path.indexOf("/api2/v2/vault/") === 0) return true;
      return /^\/api2\/v2\/users\/\d+$/.test(path);
    }} catch (error) {{
      return false;
    }}
  }}
  function isLoginOrAuthSurface() {{
    if (window.__CF_CREATOR_ID) return false;
    const p = location.pathname;
    if (p.indexOf("/auth") === 0 || p.indexOf("/login") === 0) return true;
    return !!(document.body && document.querySelector("input[type='password'], .b-loginreg__form"));
  }}
  const queue = [];
  function hydrateInitialBalances() {{
    if (window.__CF_BALANCES_HYDRATED) return;
    const attempt = (tries) => {{
      if (window.__CF_BALANCES_HYDRATED) return;
      const client = findSignedClient();
      if (!client) {{
        if (tries < 5) setTimeout(() => attempt(tries + 1), 650);
        return;
      }}
      window.__CF_BALANCES_HYDRATED = true;
      client.get("https://onlyfans.com/api2/v2/payouts/balances").then((response) => {{
        const data = response && response.data;
        const text = typeof data === "string" ? data : JSON.stringify(data || {{}});
        deliver("https://onlyfans.com/api2/v2/payouts/balances", (response && response.status) || 200, text);
      }}).catch(() => {{}});
    }};
    setTimeout(() => attempt(0), 300);
  }}
  function rememberCreator(url, body) {{
    try {{
      const parsed = new URL(url, location.href);
      if (parsed.pathname !== "/api2/v2/users/me") return;
      const data = JSON.parse(body);
      if (data && data.id != null) {{
        const hadCreator = !!window.__CF_CREATOR_ID;
        window.__CF_CREATOR_ID = String(data.id);
        if (!hadCreator) hydrateInitialBalances();
      }}
      const joined = data && (data.joinDate || data.createdAt || data.joinDateTime);
      if (joined) window.__CF_JOIN = String(joined).slice(0, 10);
    }} catch (error) {{}}
  }}
  const spendCache = window.__CF_SPEND || (window.__CF_SPEND = {{}});
  let spendRaf = 0;
  function schedulePaintSpendBadges() {{
    if (spendRaf || isLoginOrAuthSurface()) return;
    spendRaf = requestAnimationFrame(() => {{
      spendRaf = 0;
      paintSpendBadges();
    }});
  }}
  function moneyOf(value) {{
    if (value == null || value === "") return null;
    const amount = Number(value);
    return Number.isFinite(amount) ? amount : null;
  }}
  function spendOf(user) {{
    if (!user || typeof user !== "object") return null;
    const blobs = [];
    if (user.subscribedByData && typeof user.subscribedByData === "object") blobs.push(user.subscribedByData);
    if (user.subscribedOnData && typeof user.subscribedOnData === "object") blobs.push(user.subscribedOnData);
    if (user.subscribedOn && typeof user.subscribedOn === "object") blobs.push(user.subscribedOn);
    blobs.push(user);
    let best = null;
    for (const data of blobs) {{
      let amount = moneyOf(data.totalSumm);
      if (amount == null) {{
        const keys = ["tipsSumm", "messagesSumm", "subscribesSumm", "postsSumm", "streamsSumm"];
        let any = false;
        let sum = 0;
        for (const key of keys) {{
          const part = moneyOf(data[key]);
          if (part != null) {{
            any = true;
            sum += part;
          }}
        }}
        if (any) amount = sum;
      }}
      if (amount != null && (best == null || amount > best)) best = amount;
    }}
    return best;
  }}
  function paintSpendBadges() {{
    const ids = Object.keys(spendCache);
    if (!ids.length || isLoginOrAuthSurface()) return;
    for (const id of ids) {{
      const amount = spendCache[id];
      if (!(amount > 0)) continue;
      const root = document.getElementById(String(id));
      if (!root) continue;
      const avatar = root.querySelector(".g-avatar") || root.querySelector("[class*='avatar']");
      if (!avatar) continue;
      if (window.getComputedStyle(avatar).position === "static") avatar.style.position = "relative";
      let badge = avatar.querySelector(".cf-spend-badge");
      if (!badge) {{
        badge = document.createElement("span");
        badge.className = "cf-spend-badge";
        badge.style.cssText = "position:absolute;left:50%;bottom:0;transform:translate(-50%,35%);background:#f97316;color:#fff;font-size:10px;line-height:1;font-weight:700;padding:2px 4px;border-radius:999px;white-space:nowrap;z-index:3;pointer-events:none;";
        avatar.appendChild(badge);
      }}
      const label = "$" + Number(amount).toFixed(2);
      if (badge.textContent !== label) badge.textContent = label;
    }}
  }}
  function rememberUser(user) {{
    if (!user || user.id == null) return;
    const amount = spendOf(user);
    if (amount == null) return;
    spendCache[String(user.id)] = amount;
  }}
  function rememberFans(url, body) {{
    let data = null;
    try {{
      data = JSON.parse(body);
    }} catch (error) {{
      return;
    }}
    let path = "";
    try {{
      path = new URL(url, location.href).pathname;
    }} catch (error) {{
      return;
    }}
    if (path.endsWith("/users/list") && data && typeof data === "object") {{
      const values = Array.isArray(data) ? data : (Array.isArray(data.list) ? data.list : Object.values(data));
      values.forEach(rememberUser);
    }} else if (path === "/api2/v2/chats" && data && Array.isArray(data.list)) {{
      data.list.forEach((chat) => rememberUser(chat && chat.withUser));
    }} else if (/^\/api2\/v2\/users\/\d+$/.test(path)) {{
      rememberUser(data);
    }} else if (data && data.withUser) {{
      rememberUser(data.withUser);
    }}
    schedulePaintSpendBadges();
  }}
  if (!window.__CF_SPEND_STYLE) {{
    window.__CF_SPEND_STYLE = true;
    const startWatch = () => {{
      if (!document.body || window.__CF_SPEND_WATCH) return;
      window.__CF_SPEND_WATCH = true;
      const observer = new MutationObserver(() => schedulePaintSpendBadges());
      observer.observe(document.body, {{ childList: true, subtree: true }});
    }};
    if (document.body) startWatch();
    else document.addEventListener("DOMContentLoaded", startWatch);
  }}
  const vaultItems = window.__CF_VAULT_ITEMS || (window.__CF_VAULT_ITEMS = {{}});
  function mediaUrl(value) {{
    if (!value) return "";
    if (typeof value === "string") return value;
    return value.url || value.src || "";
  }}
  function directFile(url) {{
    if (!url || url.indexOf("https://") !== 0) return false;
    try {{
      const parsed = new URL(url);
      const host = parsed.hostname.toLowerCase();
      if (host !== "onlyfans.com" && !host.endsWith(".onlyfans.com")) return false;
      const path = parsed.pathname.toLowerCase();
      if (path.indexOf(".m3u8") !== -1 || path.indexOf(".mpd") !== -1) return false;
      return true;
    }} catch (error) {{
      return false;
    }}
  }}
  function bestVaultUrl(item) {{
    if (!item) return "";
    let best = "";
    let rank = -1;
    const files = item.files || {{}};
    const named = {{ source: 1000000, original: 1000000, full: 900000 }};
    Object.keys(files).forEach((key) => {{
      const score = named[String(key).toLowerCase()];
      if (score == null) return;
      const candidates = [files[key]];
      const nested = files[key] && files[key].sources;
      if (Array.isArray(nested)) candidates.push.apply(candidates, nested);
      candidates.forEach((candidate) => {{
        const url = mediaUrl(candidate);
        if (!directFile(url) || (item.type === "video" && /\.(jpg|jpeg|png|webp|gif)(\?|$)/i.test(url.split("?")[0]))) return;
        if (score > rank) {{
          best = url;
          rank = score;
        }}
      }});
    }});
    const sources = item.videoSources || item.video_sources;
    const list = !sources ? [] : Array.isArray(sources) ? sources : Object.keys(sources).map((key) => ({{ key: key, value: sources[key] }}));
    list.forEach((entry) => {{
      const key = String(entry.key || "");
      const url = mediaUrl(entry.value || entry);
      if (item.type === "video" && /\.(jpg|jpeg|png|webp|gif)(\?|$)/i.test(String(url).split("?")[0])) return;
      let score = 1;
      if (/source|original/i.test(key)) score = 1000000;
      else if (/^\d+$/.test(key)) score = Number(key);
      else if (entry.value && entry.value.height) score = Number(entry.value.height) || 1;
      else if (entry.height) score = Number(entry.height) || 1;
      if (directFile(url) && score > rank) {{
        best = url;
        rank = score;
      }}
    }});
    return best;
  }}
  function urlsOf(item) {{
    const found = [];
    const push = (value) => {{
      const url = mediaUrl(value);
      if (url) found.push(url);
      const nested = value && value.sources;
      if (Array.isArray(nested)) nested.forEach(push);
    }};
    const files = (item && item.files) || {{}};
    Object.keys(files).forEach((key) => push(files[key]));
    const sources = item && (item.videoSources || item.video_sources);
    const list = !sources ? [] : Array.isArray(sources) ? sources : Object.values(sources);
    list.forEach(push);
    return found;
  }}
  function stem(url) {{
    try {{
      const file = new URL(url, location.href).pathname.split("/").pop() || "";
      return file.replace(/\.[a-z0-9]+$/i, "");
    }} catch (error) {{
      return "";
    }}
  }}
  function normalizeVaultUrl(url) {{
    try {{
      const parsed = new URL(url, location.href);
      return parsed.origin + parsed.pathname;
    }} catch (error) {{
      return String(url || "").split("?")[0];
    }}
  }}
  function mediaIdFromNode(node) {{
    if (!node || !node.getAttribute) return "";
    const raw = [
      node.getAttribute("data-media-id"),
      node.getAttribute("data-id"),
      node.getAttribute("data-mediaid"),
      node.id,
    ].filter(Boolean).join(" ");
    const match = String(raw).match(/\d{{5,}}/);
    return match ? match[0] : "";
  }}
  function itemForSrc(src) {{
    const exact = normalizeVaultUrl(src);
    const needle = stem(src);
    if (!exact && needle.length < 6) return null;
    const ids = Object.keys(vaultItems);
    let exactHit = null;
    for (let index = 0; index < ids.length; index += 1) {{
      const item = vaultItems[ids[index]];
      if (!bestVaultUrl(item)) continue;
      const urls = urlsOf(item);
      if (exact && urls.some((url) => normalizeVaultUrl(url) === exact)) {{
        if (exactHit && String(exactHit.id) !== String(item.id)) return null;
        exactHit = item;
      }}
    }}
    if (exactHit) return exactHit;
    if (needle.length < 8) return null;
    let stemHit = null;
    for (let index = 0; index < ids.length; index += 1) {{
      const item = vaultItems[ids[index]];
      if (!bestVaultUrl(item)) continue;
      const urls = urlsOf(item);
      if (urls.some((url) => stem(url) === needle)) {{
        if (stemHit && String(stemHit.id) !== String(item.id)) return null;
        stemHit = item;
      }}
    }}
    return stemHit;
  }}
  function mediaInActiveSlide(node) {{
    const slide = node.closest && node.closest(".swiper-slide, [class*='carousel-item'], [class*='slider__item'], [class*='MediaViewer']");
    if (!slide) return 0;
    if (slide.classList.contains("swiper-slide-active") || /active|current|selected/i.test(slide.className)) return 2;
    if (slide.classList.contains("swiper-slide-duplicate-active")) return 1;
    return -2;
  }}
  function rankedVisibleMedia(root) {{
    const scope = root || document;
    const vx = window.innerWidth / 2;
    const vy = window.innerHeight / 2;
    const nodes = Array.from(scope.querySelectorAll("img, video"));
    return nodes.map((node) => {{
      const rect = node.getBoundingClientRect();
      const url = node.currentSrc || node.src || "";
      const area = rect.width * rect.height;
      const cx = rect.left + rect.width / 2;
      const cy = rect.top + rect.height / 2;
      const onScreen = rect.right > 40 && rect.bottom > 40 && rect.left < window.innerWidth - 40 && rect.top < window.innerHeight - 40;
      const centered = Math.hypot(cx - vx, cy - vy);
      const slideBonus = mediaInActiveSlide(node);
      const score = onScreen ? area - centered * 25 + slideBonus * 250000 : -1;
      return {{ node: node, url: url, width: rect.width, height: rect.height, area: area, score: score, onScreen: onScreen }};
    }}).filter((item) => item.onScreen && item.width > 220 && item.height > 220 && item.url && item.score > 0).sort((left, right) => right.score - left.score);
  }}
  function viewerOpen() {{
    return !!document.querySelector("[role='dialog'], [class*='modal'], [class*='lightbox'], [class*='MediaViewer'], .swiper-slide-active img, .swiper-slide-active video");
  }}
  function viewedVaultSrc() {{
    const activeSlide = document.querySelector(".swiper-slide-active img, .swiper-slide-active video, [class*='swiper-slide-active'] img, [class*='swiper-slide-active'] video");
    if (activeSlide) {{
      const rect = activeSlide.getBoundingClientRect();
      const url = activeSlide.currentSrc || activeSlide.src || "";
      if (url && directFile(url) && rect.width > 220 && rect.height > 220) return url;
    }}
    const roots = Array.from(document.querySelectorAll("[role='dialog'], [class*='modal'], [class*='lightbox'], [class*='MediaViewer']"));
    for (let index = 0; index < roots.length; index += 1) {{
      const ranked = rankedVisibleMedia(roots[index]).filter((item) => directFile(item.url));
      if (ranked[0]) return ranked[0].url;
    }}
    const ranked = rankedVisibleMedia(document).filter((item) => directFile(item.url));
    return ranked[0] ? ranked[0].url : "";
  }}
  function checkedVaultItems() {{
    const found = [];
    const seen = {{}};
    const pushId = (id) => {{
      if (!id || seen[id] || !vaultItems[id] || !bestVaultUrl(vaultItems[id])) return;
      seen[id] = true;
      found.push(vaultItems[id]);
    }};
    document.querySelectorAll("input[type='checkbox']:checked").forEach((input) => {{
      let node = input;
      for (let depth = 0; depth < 6 && node; depth += 1) {{
        pushId(mediaIdFromNode(node));
        const img = node.querySelector && node.querySelector("img, video");
        if (img) {{
          const matched = itemForSrc(img.currentSrc || img.src || "");
          if (matched) pushId(String(matched.id));
        }}
        node = node.parentElement;
      }}
    }});
    return found;
  }}
  function itemFromActiveSlide() {{
    const slides = document.querySelectorAll(".swiper-slide-active, [class*='swiper-slide-active'], [class*='carousel-item'].active, [class*='is-current']");
    for (let index = 0; index < slides.length; index += 1) {{
      let node = slides[index];
      for (let depth = 0; depth < 8 && node; depth += 1) {{
        const id = mediaIdFromNode(node);
        if (id && vaultItems[id] && bestVaultUrl(vaultItems[id])) return vaultItems[id];
        node = node.parentElement;
      }}
    }}
    return null;
  }}
  function selectedVaultItems() {{
    const fromSlide = itemFromActiveSlide();
    if (fromSlide) return [fromSlide];
    const src = viewedVaultSrc();
    if (src) {{
      const matched = itemForSrc(src);
      if (matched) return [matched];
      return [];
    }}
    if (viewerOpen()) return [];
    return checkedVaultItems();
  }}
  function visibleVaultFile() {{
    const src = viewedVaultSrc();
    if (!src) return null;
    const path = src.split("?")[0];
    const kind = /\.(mp4|mov|m4v|webm)$/i.test(path) ? "video" : "photo";
    return {{ url: src, kind: kind }};
  }}
  function rememberVault(url, body) {{
    try {{
      const path = new URL(url, location.href).pathname;
      if (path.indexOf("/api2/v2/vault/") !== 0) return;
      const data = JSON.parse(body);
      const list = Array.isArray(data) ? data : ((data && (data.list || data.media)) || []);
      if (!Array.isArray(list)) return;
      list.forEach((item) => {{
        if (!item || item.id == null) return;
        const kind = String(item.type || "");
        if (kind !== "photo" && kind !== "video" && kind !== "gif") return;
        vaultItems[String(item.id)] = item;
      }});
    }} catch (error) {{}}
  }}
  function vaultNotice(text) {{
    const node = document.getElementById("cf-vault-notice");
    if (node) node.textContent = text;
  }}
  async function downloadVaultItems(items) {{
    const files = [];
    items.forEach((item) => {{
      const url = bestVaultUrl(item);
      if (!url) return;
      files.push({{ id: String(item.id), kind: String(item.type || "photo"), url: url }});
    }});
    if (!files.length) {{
      vaultNotice("Sin archivo original");
      return;
    }}
    const limited = files.slice(0, 400);
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") {{
      vaultNotice("Cierra y vuelve a abrir la ventana de la modelo");
      return;
    }}
    const bar = document.getElementById("cf-vault-download");
    if (bar) bar.dataset.busy = "1";
    vaultNotice(limited.length === 1 ? "Elige la carpeta…" : "Elige la carpeta… " + limited.length + " archivos");
    try {{
      const report = await internals.invoke("download_vault_media", {{ modelId: modelId, files: limited }});
      const failed = report && report.failed ? report.failed.length : 0;
      vaultNotice("Guardados " + (report && report.saved) + (failed ? " · fallaron " + failed : ""));
    }} catch (error) {{
      const message = String((error && (error.message || error)) || "No se pudo descargar");
      vaultNotice(message.indexOf("cancelada") !== -1 ? "Descarga cancelada" : message);
    }}
    if (bar) bar.dataset.busy = "0";
  }}
  function onVaultPage() {{
    return location.pathname.indexOf("/my/vault") === 0;
  }}
  function installVaultButton() {{
    const existing = document.getElementById("cf-vault-download");
    if (!onVaultPage()) {{
      if (existing) existing.remove();
      return;
    }}
    if (!document.body) return;
    const loaded = Object.keys(vaultItems).length;
    if (existing) {{
      const count = document.getElementById("cf-vault-count");
      if (count && existing.dataset.busy !== "1") count.textContent = "Descargar cargadas (" + loaded + ")";
      return;
    }}
    const bar = document.createElement("div");
    bar.id = "cf-vault-download";
    bar.dataset.busy = "0";
    bar.style.cssText = "position:fixed;left:50%;bottom:18px;transform:translateX(-50%);z-index:2147483646;display:flex;gap:8px;align-items:center;padding:8px;border-radius:16px;background:rgba(15,23,42,.92);box-shadow:0 10px 30px rgba(0,0,0,.35);";
    const one = document.createElement("button");
    one.type = "button";
    one.textContent = "Descargar";
    one.style.cssText = "background:#1d4ed8;color:#fff;border:0;border-radius:999px;padding:10px 16px;font:600 14px/1 sans-serif;cursor:pointer;";
    one.addEventListener("click", () => {{
      if (bar.dataset.busy === "1") return;
      const items = selectedVaultItems();
      if (items.length) {{
        downloadVaultItems(items);
        return;
      }}
      const visible = visibleVaultFile();
      if (!visible) {{
        vaultNotice(loaded ? "Abre la foto o el video y pulsa Descargar" : "Espera a que cargue la bóveda");
        return;
      }}
      downloadVaultItems([{{ id: "vista", type: visible.kind, files: {{ full: {{ url: visible.url }} }} }}]);
    }});
    const all = document.createElement("button");
    all.id = "cf-vault-count";
    all.type = "button";
    all.textContent = "Descargar cargadas (" + loaded + ")";
    all.style.cssText = "background:#0f172a;color:#fff;border:1px solid rgba(255,255,255,.16);border-radius:999px;padding:10px 16px;font:600 14px/1 sans-serif;cursor:pointer;";
    all.addEventListener("click", () => {{
      if (bar.dataset.busy === "1") return;
      const items = Object.keys(vaultItems).map((id) => vaultItems[id]);
      if (!items.length) {{
        vaultNotice("Espera a que cargue la bóveda");
        return;
      }}
      downloadVaultItems(items);
    }});
    const notice = document.createElement("span");
    notice.id = "cf-vault-notice";
    notice.style.cssText = "color:#e2e8f0;font:500 12px/1.3 sans-serif;max-width:220px;";
    bar.appendChild(one);
    bar.appendChild(all);
    bar.appendChild(notice);
    document.body.appendChild(bar);
  }}
  function deliver(url, status, body) {{
    const text = String(body || "");
    rememberCreator(url, text);
    rememberFans(url, text);
    rememberVault(url, text);
    try {{
      if (new URL(url, location.href).pathname.indexOf("/api2/v2/vault/") === 0) return;
    }} catch (error) {{}}
    if (url.indexOf("/api2/v2/earnings/chart") !== -1 || (url.indexOf("/api2/v2/earnings/") !== -1 && url.indexOf("chart") !== -1)) {{
      window.__CF_CHART_RECEIVED = true;
    }}
    if (text.length < 800000) {{
      const inbox = String(url).indexOf("/api2/v2/chats") !== -1 && String(url).indexOf("/messages") === -1;
      if (queue.length >= 20) {{
        const dropAt = inbox ? 0 : queue.findIndex((entry) => String(entry.url).indexOf("/api2/v2/chats") === -1 || String(entry.url).indexOf("/messages") !== -1);
        if (dropAt >= 0) queue.splice(dropAt, 1);
      }}
      if (queue.length < 20) queue.push({{ url: absolute(url), status: status, body: text }});
    }}
    let payload = {{ modelId: modelId, url: absolute(url), status: status, body: text }};
    try {{
      const p = new URL(url, location.href).pathname;
      if (p === "/api2/v2/chats" && String(url).indexOf("_cf_backfill") === -1) {{
        window.__CF_LAST_INBOX_AT = Date.now();
      }} else if (p === "/api2/v2/payouts/transactions") {{
        window.__CF_LAST_TX_AT = Date.now();
      }}
    }} catch (e) {{}}
    const send = () => {{
      const internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== "function") {{
        setTimeout(send, 50);
        return;
      }}
      internals.invoke("ingest_of_response", payload).catch(() => {{}});
    }};
    send();
    observeMessages(url, text);
  }}
  let emitFan = (id) => {{
    if (!id) return;
    const sendFan = () => {{
      const internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== "function") {{
        setTimeout(sendFan, 50);
        return;
      }}
      internals.invoke("fan_selected", {{ modelId: modelId, ofFanId: String(id) }}).catch(() => {{}});
    }};
    sendFan();
  }};
  function usableFan(id) {{
    if (!id) return null;
    const text = String(id);
    if (window.__CF_CREATOR_ID && text === String(window.__CF_CREATOR_ID)) return null;
    return text;
  }}
  const fanFromUrl = () => {{
    const match = location.pathname.match(/\/my\/chats\/chat\/u?(\d+)/i);
    const fanId = usableFan(match && match[1]);
    if (fanId) emitFan(fanId);
  }};
  function earningsAllowed() {{
    try {{
      return sessionStorage.getItem("cf-allow-earnings") === "1";
    }} catch (error) {{
      return false;
    }}
  }}
  function staysInChats(url) {{
    if (!stayInChats || earningsAllowed() || !url) return true;
    try {{
      return new URL(url, location.href).pathname.indexOf("/my/chats") === 0;
    }} catch (error) {{
      return false;
    }}
  }}
  const pushState = history.pushState;
  const replaceState = history.replaceState;
  history.pushState = function (state, title, url) {{
    if (!staysInChats(url)) return;
    const result = pushState.apply(this, arguments);
    fanFromUrl();
    installVaultButton();
    return result;
  }};
  history.replaceState = function (state, title, url) {{
    if (!staysInChats(url)) return;
    const result = replaceState.apply(this, arguments);
    fanFromUrl();
    installVaultButton();
    return result;
  }};
  if (stayInChats) window.open = function () {{ return null; }};
  window.addEventListener("popstate", () => {{
    fanFromUrl();
    installVaultButton();
  }});
  fanFromUrl();
  let bgPollBusy = false;
  let lastWsRefreshAt = 0;
  async function runBgPoll(forceInbox) {{
    if (bgPollBusy || isLoginOrAuthSurface()) return;
    if (/statements|statistics/i.test(location.pathname)) return;
    const client = findSignedClient();
    if (!client) return;
    const now = Date.now();
    const lastInbox = window.__CF_LAST_INBOX_AT || 0;
    const lastTx = window.__CF_LAST_TX_AT || 0;
    const needInbox = forceInbox || (now - lastInbox >= 1500);
    const needTx = (now - lastTx >= 14000);
    if (!needInbox && !needTx) return;
    bgPollBusy = true;
    try {{
      if (!window.__CF_CREATOR_ID) {{
        try {{
          const meRes = await client.get("https://onlyfans.com/api2/v2/users/me");
          if (meRes && meRes.data) {{
            const meText = typeof meRes.data === "string" ? meRes.data : JSON.stringify(meRes.data);
            deliver("https://onlyfans.com/api2/v2/users/me", meRes.status || 200, meText);
          }}
        }} catch (e) {{}}
      }}
      if (needInbox) {{
        window.__CF_LAST_INBOX_AT = Date.now();
        const chatUrl = "https://onlyfans.com/api2/v2/chats?limit=10&offset=0&order=recent";
        const res = await client.get(chatUrl);
        if (res && res.data) {{
          const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
          deliver(chatUrl, res.status || 200, text);
        }}
      }}
      if (needTx) {{
        window.__CF_LAST_TX_AT = Date.now();
        const txUrl = "https://onlyfans.com/api2/v2/payouts/transactions?limit=10";
        const res = await client.get(txUrl);
        if (res && res.data) {{
          const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
          deliver(txUrl, res.status || 200, text);
        }}
      }}
    }} catch (err) {{
    }} finally {{
      bgPollBusy = false;
    }}
  }}
  function forwardLiveWsFrame(rawText) {{
    if (!rawText || typeof rawText !== "string" || rawText.length < 4 || rawText.length > 400000) return;
    const trimmed = rawText.trim();
    if (trimmed === "pong" || trimmed === "ping" || trimmed.indexOf('"connected":true') !== -1) return;
    if (
      rawText.indexOf("message") !== -1 ||
      rawText.indexOf("chat") !== -1 ||
      rawText.indexOf("unread") !== -1 ||
      rawText.indexOf("pay") !== -1 ||
      rawText.indexOf("purchas") !== -1 ||
      rawText.indexOf("Tip") !== -1 ||
      rawText.indexOf("tip") !== -1 ||
      rawText.indexOf("isOpened") !== -1 ||
      rawText.indexOf("notification") !== -1 ||
      rawText.indexOf("toast") !== -1
    ) {{
      const payload = {{
        modelId: modelId,
        url: "https://onlyfans.com/api2/v2/__cf_live_event",
        status: 200,
        body: rawText
      }};
      const sendLive = () => {{
        const internals = window.__TAURI_INTERNALS__;
        if (!internals || typeof internals.invoke !== "function") {{
          setTimeout(sendLive, 80);
          return;
        }}
        internals.invoke("ingest_of_response", payload).catch(() => {{}});
      }};
      sendLive();
      const now = Date.now();
      if (now - lastWsRefreshAt >= 1200) {{
        lastWsRefreshAt = now;
        void runBgPoll(true);
      }}
    }}
  }}
  if (typeof window.WebSocket === "function" && !window.__CF_WS_HOOKED) {{
    window.__CF_WS_HOOKED = true;
    try {{
      window.WebSocket = new Proxy(window.WebSocket, {{
        construct(Target, args, newTarget) {{
          const ws = Reflect.construct(Target, args, newTarget);
          try {{
            const wsUrl = String((args && args[0]) || "");
            if (!wsUrl || wsUrl.indexOf("onlyfans.com") !== -1) {{
              ws.addEventListener("message", (event) => {{
                if (!event || !event.data) return;
                if (typeof event.data === "string") {{
                  forwardLiveWsFrame(event.data);
                }} else if (typeof Blob !== "undefined" && event.data instanceof Blob) {{
                  event.data.text().then(forwardLiveWsFrame).catch(() => {{}});
                }}
              }});
            }}
          }} catch (err) {{}}
          return ws;
        }}
      }});
    }} catch (err) {{}}
  }}
  window.__CF_LOGIN_READY = (nameLabel) => {{
    try {{
      if (!document.body) return;
      const existing = document.getElementById("cf-login-ready-pill");
      if (existing) existing.remove();
      const pill = document.createElement("div");
      pill.id = "cf-login-ready-pill";
      pill.style.cssText = "position:fixed;right:18px;bottom:18px;z-index:2147483647;display:flex;align-items:center;gap:8px;padding:10px 16px;border-radius:999px;background:linear-gradient(135deg,#059669,#047857);border:1px solid #34d399;color:#ecfdf5;font:600 13px/1.2 system-ui,sans-serif;box-shadow:0 12px 28px rgba(5,150,105,.45);pointer-events:none;transition:opacity .3s ease;";
      pill.textContent = "✓ Sesión conectada: " + String(nameLabel || "OnlyFans");
      document.body.appendChild(pill);
      setTimeout(() => {{
        pill.style.opacity = "0";
        setTimeout(() => pill.remove(), 350);
      }}, 4200);
    }} catch (e) {{}}
  }};
  const originalFetch = window.fetch;
  if (typeof originalFetch === "function") {{
    window.fetch = function () {{
      const input = arguments[0];
      const url = typeof input === "string" ? input : (input && input.url) || "";
      const pending = originalFetch.apply(this, arguments);
      if (!watched(url)) {{
        return pending;
      }}
      return pending.then((response) => {{
        response.clone().text().then((body) => deliver(url, response.status, body)).catch(() => {{}});
        return response;
      }});
    }};
  }}
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {{
    this.__cfUrl = url;
    return originalOpen.apply(this, arguments);
  }};
  XMLHttpRequest.prototype.send = function () {{
    this.addEventListener("load", function () {{
      const url = this.__cfUrl || "";
      if (!watched(url)) {{
        return;
      }}
      let body = "";
      try {{
        body = typeof this.responseText === "string" ? this.responseText : JSON.stringify(this.response);
      }} catch (error) {{
        body = "";
      }}
      deliver(url, this.status, body);
    }});
    return originalSend.apply(this, arguments);
  }};
  const fanPattern = /\/(?:chats\/chat|chats|users)\/u?(\d+)/i;
  function fanFromLocation() {{
    const match = location.pathname.match(/\/my\/chats\/chat\/u?(\d+)/i);
    return usableFan(match && match[1]);
  }}
  function moneyFromText(text) {{
    if (!text) return null;
    const match = String(text).replace(/\s+/g, " ").match(/(?:USD\s*)?\$?\s*([0-9]{{1,3}}(?:,[0-9]{{3}})*(?:\.[0-9]{{1,2}})?|[0-9]+(?:\.[0-9]{{1,2}})?)/i);
    if (!match) return null;
    const amount = Number(match[1].replace(/,/g, ""));
    return Number.isFinite(amount) ? amount : null;
  }}
  function stampFromText(text) {{
    if (!text) return null;
    const iso = String(text).match(/(\d{{4}}-\d{{2}}-\d{{2}}(?:[T\s]\d{{2}}:\d{{2}}(?::\d{{2}})?(?:Z|[+-]\d{{2}}:?\d{{2}})?)?)/);
    if (iso) return iso[1].replace(" ", "T");
    return null;
  }}
  function readChat() {{
    let profile = null;
    const panels = [...document.querySelectorAll("aside, section, div")].filter((node) => {{
      const text = node.innerText || "";
      return text.length > 20 && text.length < 2500 && /ESTAD[IÃ]STICAS DE FANS|FAN STATISTICS|Gastado|Spent|Last paid|Último pago|Ultimo pago/i.test(text);
    }}).sort((left, right) => left.innerText.length - right.innerText.length);
    if (panels[0]) {{
      const text = panels[0].innerText;
      const duration = text.match(/suscrito para\s+([^\n]{{1,40}})/i) || text.match(/subscribed for\s+([^\n]{{1,40}})/i);
      const spentLine = text.match(/(?:Gastado|Spent|Total spent)\s*[:\n]?\s*([^\n]{{1,40}})/i);
      const lastLine = text.match(/(?:Último pago|Ultimo pago|Last paid|Last purchase)\s*[:\n]?\s*([^\n]{{1,60}})/i);
      const highLine = text.match(/(?:Gasto m[aá]s alto|Highest purchase|Highest spend)\s*[:\n]?\s*([^\n]{{1,40}})/i);
      profile = {{
        status: /SUSCRITO|SUBSCRIBED/i.test(text) ? "Active" : (/caduc|expir/i.test(text) ? "Inactive" : null),
        duration: duration ? duration[1].trim() : null,
        cost: /GRATIS|\bFREE\b/i.test(text) ? 0 : null,
        totalSpend: spentLine ? moneyFromText(spentLine[1]) : null,
        lastSpendAt: lastLine ? stampFromText(lastLine[1]) : null,
        highestSpend: highLine ? moneyFromText(highLine[1]) : null
      }};
    }}
    return {{ profile: profile }};
  }}
  function findStatsTab() {{
    const tabs = [...document.querySelectorAll("a, button, [role='tab'], .b-tabs__nav__item, .b-tabs__nav a")];
    return tabs.find((el) => {{
      const text = (el.innerText || el.textContent || "").toLowerCase().trim();
      return (
        text.includes("estadísticas de ganancias") ||
        text.includes("estadisticas de ganancias") ||
        text.includes("estadísticas de ingresos") ||
        text.includes("estadisticas de ingresos") ||
        text.includes("earnings statistics") ||
        (text.includes("estadísticas") && el.closest(".b-tabs, nav, [role='tablist']"))
      );
    }});
  }}
  function findTodoElTiempoOption() {{
    const options = [...document.querySelectorAll("button, a, [role='menuitem'], [role='option'], .dropdown-item, li")];
    return options.find((el) => {{
      const text = (el.innerText || el.textContent || "").toLowerCase().trim();
      return (
        text === "todo el tiempo" ||
        text === "all time" ||
        text === "all period" ||
        text === "desde el inicio" ||
        text === "máximo" ||
        text === "maximo"
      );
    }});
  }}
  function findRangeDropdownToggle() {{
    const toggles = [...document.querySelectorAll("button, [data-toggle='dropdown'], .b-dropdown button, .b-dropdown__toggle, [role='button']")];
    return toggles.find((el) => {{
      if (el.closest(".b-make-post") || el.closest("form") || el.closest(".b-post")) return false;
      const text = (el.innerText || el.textContent || "").toLowerCase().trim();
      return (
        text.includes("últim") ||
        text.includes("ultim") ||
        text.includes("last ") ||
        text.includes("días") ||
        text.includes("dias") ||
        text.includes("days") ||
        text.includes("horas") ||
        text.includes("hours") ||
        text.includes("todo el tiempo") ||
        text.includes("all time") ||
        /^\d{{4}}$/.test(text)
      );
    }});
  }}
  function collectEarnings() {{
    const path = location.pathname;
    if (!/statements/i.test(path)) {{
      if (/statistics/i.test(path)) return true;
      return false;
    }}
    if (window.__CF_CHART_RECEIVED) {{
      return true;
    }}
    const now = Date.now();
    if (window.__CF_EARN_AT && now - window.__CF_EARN_AT < 1200) {{
      return window.__CF_CHART_RECEIVED || false;
    }}
    window.__CF_EARN_AT = now;

    const statsTab = findStatsTab();
    if (statsTab) {{
      const isActive = statsTab.classList.contains("active") ||
                       statsTab.getAttribute("aria-selected") === "true" ||
                       statsTab.closest(".active") != null;
      if (!isActive) {{
        statsTab.click();
        return false;
      }}
    }}

    const allTimeOption = findTodoElTiempoOption();
    if (allTimeOption) {{
      allTimeOption.click();
      window.__CF_ALL_TIME_CLICKED = true;
      return false;
    }}

    const rangeToggle = findRangeDropdownToggle();
    if (rangeToggle) {{
      const text = (rangeToggle.innerText || "").toLowerCase().trim();
      if (text.includes("todo el tiempo") || text.includes("all time")) {{
        window.__CF_ALL_TIME_CLICKED = true;
      }} else {{
        rangeToggle.click();
        return false;
      }}
    }}

    if (window.__CF_ALL_TIME_CLICKED && !window.__CF_ALL_TIME_AT) {{
      window.__CF_ALL_TIME_AT = Date.now();
    }}
    if (window.__CF_ALL_TIME_CLICKED && Date.now() - (window.__CF_ALL_TIME_AT || 0) > 4000) {{
      return true;
    }}

    return window.__CF_CHART_RECEIVED || false;
  }}
  function webpackRequire() {{
    const names = Object.getOwnPropertyNames(window).filter((name) => name.indexOf("webpackChunk") === 0);
    for (let index = 0; index < names.length; index += 1) {{
      const chunk = window[names[index]];
      if (!chunk || typeof chunk.push !== "function") continue;
      const id = "cfreq" + Date.now() + index;
      let req = null;
      try {{
        const modules = {{}};
        modules[id] = function (module, exports, require) {{ req = require; }};
        chunk.push([[id], modules, [[id]]]);
      }} catch (error) {{}}
      if (!req) {{
        try {{
          chunk.push([[id + "b"], {{}}, function (require) {{ req = require; }}]);
        }} catch (error) {{}}
      }}
      if (req) return req;
    }}
    return null;
  }}
  function scoreClient(value) {{
    if (!value || typeof value.get !== "function" || !value.interceptors || !value.interceptors.request) return 0;
    let score = 1;
    if (typeof value.post === "function") score += 1;
    try {{
      const headers = value.defaults && value.defaults.headers;
      const bag = headers && (headers.common || headers);
      const keys = bag ? Object.keys(bag).map((key) => String(key).toLowerCase()) : [];
      if (keys.indexOf("app-token") !== -1 || keys.indexOf("user-id") !== -1) score += 4;
      const base = String((value.defaults && value.defaults.baseURL) || "");
      if (base.indexOf("onlyfans.com") !== -1) score += 3;
    }} catch (error) {{}}
    return score;
  }}
  function findSignedClient() {{
    if (scoreClient(window.__CF_HTTP) > 0) return window.__CF_HTTP;
    const req = webpackRequire();
    const cache = req && (req.c || req.cache);
    if (!cache) return null;
    let best = null;
    let bestScore = 0;
    Object.keys(cache).forEach((id) => {{
      const exp = cache[id] && cache[id].exports;
      [exp, exp && exp.default, exp && exp.axios, exp && exp.api].forEach((value) => {{
        const score = scoreClient(value);
        if (score > bestScore) {{
          best = value;
          bestScore = score;
        }}
      }});
    }});
    if (best) window.__CF_HTTP = best;
    return best;
  }}
  function purchaseSlot(fanId) {{
    const state = window.__CF_PURCHASE_SYNC || (window.__CF_PURCHASE_SYNC = {{}});
    if (!state[fanId]) state[fanId] = {{ seen: {{}}, before: "", running: false, done: false, triedMax: false, retries: 0, error: "" }};
    return state[fanId];
  }}
  function notePurchases(fanId) {{
    const slot = fanId && window.__CF_PURCHASE_SYNC && window.__CF_PURCHASE_SYNC[fanId];
    const node = document.getElementById("cf-msg-note");
    if (!node || !slot) return;
    if (slot.error) node.textContent = slot.error;
    else if (slot.done) node.textContent = "Ventas cargadas";
    else if (slot.running) node.textContent = "Cargando ventas…";
  }}
  function observeMessages(url, body) {{
    if (String(url).indexOf("/messages") === -1) return;
    let data = null;
    try {{ data = JSON.parse(body); }} catch (error) {{ return; }}
    const list = data && Array.isArray(data.list) ? data.list : null;
    if (!list || !list.length) return;
    const match = String(url).match(/\/api2\/v2\/chats\/u?(\d+)\/messages/i);
    const fanId = usableFan(match && match[1]);
    if (!fanId) return;
    const slot = purchaseSlot(fanId);
    list.forEach((item) => {{
      if (item && item.id != null) slot.seen[String(item.id)] = true;
    }});
    const oldest = list[list.length - 1];
    if (oldest && oldest.id != null) slot.before = String(oldest.id);
    if (data.hasMore === false || (list.length < 10 && data.hasMore !== true)) {{
      slot.done = true;
      slot.running = false;
      notePurchases(fanId);
      return;
    }}
    if (!slot.running && !slot.done) pullPurchases(fanId);
  }}
  async function pullPurchases(fanId) {{
    const slot = purchaseSlot(fanId);
    if (!fanId || slot.running || slot.done) return;
    const client = findSignedClient();
    if (!client) return;
    slot.running = true;
    slot.error = "";
    notePurchases(fanId);
    let pages = 0;
    while (pages < 300 && !slot.done) {{
      pages += 1;
      const param = slot.triedMax ? "maxId" : "id";
      let path = "/api2/v2/chats/" + fanId + "/messages?limit=10&order=desc&skip_users=all";
      if (slot.before) path += "&" + param + "=" + encodeURIComponent(slot.before);
      try {{
        const response = await client.get("https://onlyfans.com" + path);
        const data = response && response.data ? response.data : {{}};
        const list = Array.isArray(data.list) ? data.list : [];
        const fresh = list.filter((item) => item && item.id != null && !slot.seen[String(item.id)]);
        if (!fresh.length) {{
          if (!slot.triedMax && slot.before) {{
            slot.triedMax = true;
            continue;
          }}
          slot.done = true;
          break;
        }}
        fresh.forEach((item) => {{ slot.seen[String(item.id)] = true; }});
        const oldest = list[list.length - 1];
        if (oldest && oldest.id != null) slot.before = String(oldest.id);
        deliver("https://onlyfans.com" + path + "&_cf_backfill=1", (response && response.status) || 200, JSON.stringify(data));
        if (data.hasMore === false || (list.length < 10 && data.hasMore !== true)) {{
          slot.done = true;
          break;
        }}
        await new Promise((resolve) => setTimeout(resolve, 150));
      }} catch (error) {{
        const status = error && error.response ? error.response.status : 0;
        slot.retries += 1;
        slot.error = status ? "OnlyFans respondió " + status + " al cargar ventas" : "No se pudo cargar el historial";
        slot.running = false;
        notePurchases(fanId);
        if (slot.retries < 4) setTimeout(() => pullPurchases(fanId), 800);
        else slot.done = true;
        return;
      }}
    }}
    slot.running = false;
    notePurchases(fanId);
  }}
  window.__CF_TICK = () => {{
    const api = queue.splice(0, queue.length);
    let fanId = fanFromLocation();
    for (const item of api) {{
      rememberCreator(item.url, item.body);
      const match = String(item.url || "").match(/\/api2\/v2\/chats\/u?(\d+)\/messages/i);
      const nextFan = usableFan(match && match[1]);
      if (nextFan) fanId = nextFan;
    }}
    const onChat = location.pathname.indexOf("/my/chats/chat/") === 0;
    const onEarnings = (!stayInChats || earningsAllowed()) && /statements|statistics/i.test(location.pathname);
    const chat = onChat ? readChat() : {{ profile: null }};
    if (onChat && fanId) {{
      const slot = purchaseSlot(fanId);
      if (!slot.running && !slot.done) pullPurchases(fanId);
      notePurchases(fanId);
    }}
    const widened = onEarnings ? collectEarnings() : false;
    const authReady = !!window.__CF_CREATOR_ID;
    if (authReady && !onChat) cleanComposerButtons();
    if (!onEarnings) {{
      void runBgPoll(false);
    }}
    return {{ fanId: onChat ? fanId : null, widened: widened, active: (onChat || onEarnings), creator: !!window.__CF_CREATOR_ID, ready: !!window.__CF_CREATOR_ID && !!window.__CF_HTTP, api: api, profile: onChat ? chat.profile : null }};
  }};

  function injectPostStyles() {{
    if (document.getElementById("cf-post-composer-styles")) return;
    const style = document.createElement("style");
    style.id = "cf-post-composer-styles";
    style.textContent = `
      .cf-hidden-composer-item {{
        display: none !important;
      }}
    `;
    const target = document.head || document.documentElement || document.body;
    if (target) target.appendChild(style);
  }}

  function controlSignal(node) {{
    if (!node || !node.getAttribute) return "";
    const parts = [
      node.getAttribute("aria-label"),
      node.getAttribute("title"),
      node.getAttribute("mattooltip"),
      node.getAttribute("data-original-title"),
      node.getAttribute("data-name"),
      node.getAttribute("data-icon-name"),
      node.getAttribute("data-icon")
    ];
    const className = typeof node.className === "string" ? node.className : "";
    const classTokens = className.split(/\s+/).filter((token) => /icon-|hourglass|expire|quiz|puzzle|clock|calendar|schedule|datepicker|poll|vault/i.test(token));
    let signal = parts.filter(Boolean).concat(classTokens).join(" ");
    const ownsNestedButton = node.querySelector && node.querySelector("button, [role='button']");
    const ownText = ownsNestedButton ? "" : (node.textContent || "");
    if (ownText.trim().length > 0 && ownText.trim().length < 80) signal += " " + ownText;
    const icons = node.querySelectorAll ? node.querySelectorAll("svg, use") : [];
    for (const icon of icons) {{
      const owner = icon.closest ? icon.closest("button, [role='button']") : null;
      if (owner && owner !== node) continue;
      signal += " " + (icon.getAttribute("href") || "") + " " + (icon.getAttribute("xlink:href") || "") + " " + (icon.getAttribute("data-icon-name") || "");
      const iconClass = icon.getAttribute("class") || "";
      signal += " " + iconClass.split(/\s+/).filter((token) => /icon-|hourglass|expire|quiz|puzzle|clock|calendar|schedule|date/i.test(token)).join(" ");
    }}
    return signal.toLowerCase();
  }}

  function isQuizButton(node) {{
    return /quiz|cuestionario|questionnaire|puzzle/.test(controlSignal(node));
  }}

  function leafText(node) {{
    return ((node && (node.innerText || node.textContent)) || "").replace(/\s+/g, " ").trim();
  }}

  function isIconOnlyControl(node) {{
    const hasIcon = node && node.querySelector && node.querySelector("svg, use");
    return !!hasIcon && leafText(node).length <= 1;
  }}

  function isHourglassButton(node) {{
    if (!node || isQuizButton(node)) return false;
    const signal = controlSignal(node);
    if (/vencimiento|expir|hourglass|sandglass|hour-glass|reloj de arena|tiempo l[ií]mite|time limit|icon-clock|\bclock\b|icon-timer|icon-time\b|sand-glass/.test(signal)) {{
      return true;
    }}
    const attr = (node.getAttribute && (node.getAttribute("at-attr") || node.getAttribute("data-attr") || "")) || "";
    return /expire|hourglass|clock/i.test(attr);
  }}

  function isChatPage() {{
    return location.pathname.indexOf("/my/chats") === 0;
  }}

  function isForbiddenComposerButton(node) {{
    if (!node || isHourglassButton(node)) return false;
    if (isChatPage()) return false;
    if (node.closest && node.closest(".b-chat, [class*='b-chat']")) return false;
    if (isQuizButton(node)) return true;

    // NUNCA ocultar nada que esté dentro de una publicación existente en el feed
    if (node.closest(".b-post, article, [data-post-id], .b-feed-item, .b-post__header, .b-comments, .b-chat__message")) {{
      return false;
    }}
    if (node.closest(".b-make-post__media, .b-dropzone, [class*='media-file'], [class*='preview']")) {{
      return false;
    }}

    const hint = controlSignal(node);

    // NUNCA ocultar los botones de 3 puntos o menú de opciones de posts
    if (hint.includes("more") || hint.includes("puntos") || hint.includes("dots") || hint.includes("opciones") || hint.includes("eliminar") || hint.includes("delete") || hint.includes("remove")) {{
      return false;
    }}

    const rawText = (node.textContent || "").trim().toLowerCase();
    if (rawText === "publicar" || rawText === "publish" || rawText === "guardar" || rawText === "save" || rawText.includes("transmisi") || rawText.includes("live")) {{
      return false;
    }}
    if (node.tagName === "TEXTAREA" || node.tagName === "INPUT" || node.getAttribute("contenteditable") === "true") {{
      return false;
    }}

    // Solo considerar elementos dentro del compositor de nueva publicación
    const inComposer = node.closest(".b-make-post, [class*='make-post'], .b-post-form, [class*='composer']");
    if (!inComposer) {{
      return false;
    }}

    if (isIconOnlyControl(node) && !/quiz|puzzle|poll|vault|price|photo|image|video|camera|\bmic\b|voice|mention|icon-at|\bgoal\b|fundrais|cuestionario|encuesta|etiqueta|\btag\b/.test(hint)) {{
      return false;
    }}

    return /(icon-photo|\bphoto\b|icon-media|\bmedios\b|icon-image|\bimagen\b|\bpicture\b|icon-video|\bvideo\b|icon-camera|\bcamera\b|icon-voice|\bvoice\b|icon-audio|\baudio\b|icon-mic|\bmic\b|icon-vault|\bvault\b|b[oó]veda|icon-album|\balbum\b|icon-gallery|\bgallery\b|icon-poll|\bpoll\b|icon-vote|\bvotaci\b|\bsurvey\b|encuesta|cuestionario|questionnaire|\bquiz\b|puzzle|icon-question|\bpregunta\b|trivia|programar|icon-schedule|\bschedule\b|calendario|icon-goal|\bgoal\b|fundrais|icon-price|\bprice\b|\btarifa\b|precio|icon-mention|\bmention\b|icon-at|icon-tag|etiqueta|icon-format|formato|text format|typography|\baa\b)/i.test(hint);
  }}

  function containsHourglass(node) {{
    if (!node || !node.querySelectorAll) return false;
    if (isHourglassButton(node)) return true;
    const nested = node.querySelectorAll("button, [role='button'], .b-make-post__actions-btn, .b-make-post__tools-item");
    for (const child of nested) {{
      if (child !== node && isHourglassButton(child)) return true;
    }}
    return false;
  }}

  function hideComposerControl(node) {{
    if (!node || isHourglassButton(node) || containsHourglass(node)) return;
    const rect = node.getBoundingClientRect ? node.getBoundingClientRect() : null;
    if (rect && (rect.width > 220 || rect.height > 80)) return;
    if (!node.classList.contains("cf-hidden-composer-item")) {{
      node.classList.add("cf-hidden-composer-item");
      node.style.setProperty("display", "none", "important");
    }}
  }}

  function showComposerControl(node) {{
    if (!node) return;
    node.classList.remove("cf-hidden-composer-item");
    node.style.removeProperty("display");
    node.style.removeProperty("visibility");
    node.style.removeProperty("opacity");
    if (node.removeAttribute) node.removeAttribute("hidden");
  }}

  function forceShowHourglass(node) {{
    if (!node || node === document.body || node === document.documentElement) return;
    const tag = node.tagName;
    if (tag !== "BUTTON" && node.getAttribute("role") !== "button") return;
    const rect = node.getBoundingClientRect ? node.getBoundingClientRect() : null;
    if (rect && (rect.width > 180 || rect.height > 180)) return;
    showComposerControl(node);
    let parent = node.parentElement;
    let depth = 0;
    while (parent && parent !== document.body && depth < 6) {{
      if (parent.classList && parent.classList.contains("cf-hidden-composer-item")) {{
        showComposerControl(parent);
      }}
      parent = parent.parentElement;
      depth += 1;
    }}
  }}

  function cleanComposerButtons() {{
    injectPostStyles();

    // 1. Restaurar visibilidad de cualquier botón dentro de posts existentes, chats o menús de 3 puntos
    const hiddenItems = document.querySelectorAll(".cf-hidden-composer-item");
    for (const item of hiddenItems) {{
      if (
        isChatPage() ||
        item.closest(".b-chat, [class*='b-chat'], .b-post, article, [data-post-id], .b-feed-item, .b-post__header, .b-comments") ||
        !item.closest(".b-make-post, [class*='make-post'], .b-post-form")
      ) {{
        item.classList.remove("cf-hidden-composer-item");
        item.style.removeProperty("display");
      }}
    }}

    if (isChatPage()) return;

    // 2. Solo buscar candidatos dentro de contenedores de nueva publicación
    const composerRoots = document.querySelectorAll(".b-make-post, [class*='make-post'], .b-post-form");
    for (const root of composerRoots) {{
      if (root.closest(".b-post, article, [data-post-id], .b-chat, [class*='b-chat']")) continue;

      const candidates = root.querySelectorAll("button, label.b-make-post__upload-btn, [role='button'], div.b-make-post__actions-btn, .b-make-post__tools-item");
      for (const btn of candidates) {{
        const nestedControl = btn.querySelector && btn.querySelector("button, [role='button']");
        if (nestedControl && nestedControl !== btn) continue;
        if (isHourglassButton(btn)) {{
          forceShowHourglass(btn);
          if (!btn.dataset.cfHooked) {{
            btn.dataset.cfHooked = "true";
            btn.addEventListener("click", () => {{
              triggerAutoExpiry();
            }});
          }}
          continue;
        }}
        if (isForbiddenComposerButton(btn)) {{
          hideComposerControl(btn);
        }}
      }}
    }}
  }}

  let isAutoExpiryRunning = false;

  function safeClick(element) {{
    if (!element) return;
    try {{
      element.dispatchEvent(new MouseEvent("mousedown", {{ bubbles: true, cancelable: true, view: window }}));
      element.dispatchEvent(new MouseEvent("mouseup", {{ bubbles: true, cancelable: true, view: window }}));
    }} catch (e) {{}}
    element.click();
  }}

  function find1DayButton() {{
    const btnById = document.getElementById("1-día") || 
                    document.getElementById("1-dia") || 
                    document.querySelector('button[id*="1-d"]');
    if (btnById) return btnById;

    const candidates = Array.from(document.querySelectorAll("button, [role='button'], span, div, label"));
    return candidates.find((btn) => {{
      const text = (btn.innerText || btn.textContent || "").trim().toLowerCase();
      return text === "1 día" || text === "1 dia" || text === "1 day";
    }});
  }}

  function findSaveButton(context) {{
    const scope = context || document;
    return Array.from(scope.querySelectorAll("button, [role='button']")).find((btn) => {{
      const text = (btn.innerText || btn.textContent || "").trim().toLowerCase();
      return text === "guardar" || text === "save";
    }});
  }}

  function findConfirmDeleteButton() {{
    return Array.from(document.querySelectorAll("button, [role='button']")).find((btn) => {{
      const text = (btn.innerText || btn.textContent || "").trim().toUpperCase();
      return text === "SÍ, ELIMINAR" || text === "SI, ELIMINAR" || text === "YES, DELETE" || text === "YES, REMOVE";
    }});
  }}

  function findPublishButton() {{
    return Array.from(document.querySelectorAll("button, [role='button'], .b-btn")).find((btn) => {{
      if (btn.closest('#of-debug-panel')) return false;
      const text = (btn.innerText || btn.textContent || "").trim().toLowerCase();
      return text === "publicar" || text === "publish";
    }});
  }}

  function isRateLimitText(text) {{
    if (!text) return false;
    const lowercase = text.toLowerCase();
    const hasKeyword = /(wait|allow|espera|espere|limit|slow|delay|again|vuelve|intente|intentar)/.test(lowercase);
    const hasTime = /(\d+\s*(second|sec|s\b|segundo|seg)s?)/.test(lowercase);
    return hasKeyword && hasTime;
  }}

  function clearRateLimitErrors() {{
    try {{
      function walk(node) {{
        if (!node) return;
        if (node.nodeType === Node.ELEMENT_NODE) {{
          const tagName = node.tagName;
          const isEditable = 
            node.getAttribute("contenteditable") === "true" ||
            tagName === "TEXTAREA" ||
            tagName === "INPUT" ||
            node.classList?.contains("ql-editor") ||
            node.classList?.contains("b-chat__message-input");
          if (isEditable) return;
        }}
        if (node.nodeType === Node.TEXT_NODE) {{
          if (isRateLimitText(node.textContent)) {{
            node.textContent = "";
          }}
        }} else if (node.childNodes) {{
          const children = Array.from(node.childNodes);
          for (let i = 0; i < children.length; i++) {{
            walk(children[i]);
          }}
        }}
      }}
      walk(document.body);
    }} catch (e) {{}}
  }}

  function detectRateLimitError() {{
    let found = false;
    try {{
      function walk(node) {{
        if (found) return;
        if (node.nodeType === Node.ELEMENT_NODE) {{
          const tagName = node.tagName;
          const isEditable = 
            node.getAttribute("contenteditable") === "true" ||
            tagName === "TEXTAREA" ||
            tagName === "INPUT" ||
            node.classList?.contains("ql-editor") ||
            node.classList?.contains("b-chat__message-input");
          if (isEditable) return;
        }}
        if (node.nodeType === Node.TEXT_NODE) {{
          if (isRateLimitText(node.textContent)) {{
            found = true;
          }}
        }} else if (node.childNodes) {{
          const children = Array.from(node.childNodes);
          for (let i = 0; i < children.length; i++) {{
            walk(children[i]);
          }}
        }}
      }}
      walk(document.body);
    }} catch (e) {{}}
    return found;
  }}

  function monitorPublishStatus(retryCount) {{
    const currentRetry = retryCount || 0;
    const maxRetries = 15;
    if (currentRetry >= maxRetries) {{
      isAutoExpiryRunning = false;
      return;
    }}

    const startTime = Date.now();
    let loopsWithoutError = 0;

    const interval = setInterval(() => {{
      const publishBtn = findPublishButton();
      if (!publishBtn) {{
        clearInterval(interval);
        isAutoExpiryRunning = false;
        return;
      }}

      if (detectRateLimitError()) {{
        clearInterval(interval);
        clearRateLimitErrors();
        setTimeout(() => {{
          const btn = findPublishButton();
          if (btn) {{
            if (btn.hasAttribute("disabled") || btn.disabled) {{
              btn.removeAttribute("disabled");
              btn.disabled = false;
            }}
            safeClick(btn);
            monitorPublishStatus(currentRetry + 1);
          }} else {{
            isAutoExpiryRunning = false;
          }}
        }}, 1000);
        return;
      }}

      loopsWithoutError++;
      if (loopsWithoutError >= 8) {{
        loopsWithoutError = 0;
        if (publishBtn.hasAttribute("disabled") || publishBtn.disabled) {{
          publishBtn.removeAttribute("disabled");
          publishBtn.disabled = false;
        }}
        safeClick(publishBtn);
      }}

      if (Date.now() - startTime > 15000) {{
        clearInterval(interval);
        isAutoExpiryRunning = false;
      }}
    }}, 250);
  }}

  function triggerAutoExpiry() {{
    let count = 0;
    const poll = setInterval(() => {{
      count++;
      const oneDayBtn = find1DayButton();
      if (oneDayBtn) {{
        clearInterval(poll);
        executeAutoExpiry(oneDayBtn);
      }} else if (count > 40) {{
        clearInterval(poll);
      }}
    }}, 100);
  }}

  function executeAutoExpiry(oneDayBtn) {{
    if (isAutoExpiryRunning) return;
    isAutoExpiryRunning = true;

    setTimeout(() => {{
      safeClick(oneDayBtn);

      setTimeout(() => {{
        const modalContainer = 
          oneDayBtn.closest(".g-popup") || 
          oneDayBtn.closest("[role='dialog']") || 
          oneDayBtn.closest(".modal") || 
          document.body;
        const saveBtn = findSaveButton(modalContainer);

        if (saveBtn) {{
          safeClick(saveBtn);

          setTimeout(() => {{
            const publishBtn = findPublishButton();
            if (publishBtn) {{
              if (publishBtn.hasAttribute("disabled") || publishBtn.disabled) {{
                publishBtn.removeAttribute("disabled");
                publishBtn.disabled = false;
              }}
              safeClick(publishBtn);
              monitorPublishStatus(0);
            }} else {{
              isAutoExpiryRunning = false;
            }}
          }}, 350);
        }} else {{
          isAutoExpiryRunning = false;
        }}
      }}, 150);
    }}, 100);
  }}

  let cleanQueued = false;
  const runComposerPass = () => {{
    if (isLoginOrAuthSurface()) return;
    cleanComposerButtons();

    const confirmDeleteBtn = findConfirmDeleteButton();
    if (confirmDeleteBtn && !confirmDeleteBtn.dataset.cfClicked) {{
      confirmDeleteBtn.dataset.cfClicked = "true";
      setTimeout(() => {{
        safeClick(confirmDeleteBtn);
      }}, 100);
      return;
    }}

    if (!isAutoExpiryRunning) {{
      const oneDayBtn = find1DayButton();
      const modal = oneDayBtn && oneDayBtn.closest(".g-popup, [role='dialog'], .modal");
      if (oneDayBtn && modal && !oneDayBtn.dataset.cfProcessed) {{
        const text = (modal.textContent || "").toLowerCase();
        if (text.includes("vencimiento") || text.includes("expir") || text.includes("sin l") || text.includes("no limit")) {{
          oneDayBtn.dataset.cfProcessed = "true";
          executeAutoExpiry(oneDayBtn);
        }}
      }}
    }}
  }};
  const scheduleClean = () => {{
    if (cleanQueued) return;
    cleanQueued = true;
    setTimeout(() => {{
      cleanQueued = false;
      runComposerPass();
    }}, 250);
  }};
  const domObserver = new MutationObserver(() => {{
    scheduleClean();
  }});

  const startObserver = () => {{
    runComposerPass();
    if (document.body) {{
      domObserver.observe(document.body, {{
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["title", "aria-label", "data-original-title"]
      }});
    }} else {{
      setTimeout(startObserver, 50);
    }}
  }};
  startObserver();
  setInterval(() => {{
    if (!isLoginOrAuthSurface() && !isChatPage()) runComposerPass();
  }}, 1200);
  setInterval(() => {{
    if (onVaultPage()) installVaultButton();
  }}, 1200);
  installVaultButton();
}})();
"##
    )
}

fn spawn_reader(app: AppHandle, webview_label: String, model_id: i64) {
    use std::collections::HashSet;
    use std::sync::Mutex as StdMutex;
    static STARTED: std::sync::OnceLock<StdMutex<HashSet<String>>> = std::sync::OnceLock::new();
    let started = STARTED.get_or_init(|| StdMutex::new(HashSet::new()));
    if !started
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(webview_label.clone())
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut poll_ms = 900_u64;
        let mut misses = 0_u32;
        loop {
            tokio::time::sleep(Duration::from_millis(poll_ms)).await;
            let Some(webview) = app.get_webview(&webview_label) else {
                if let Some(set) = STARTED.get() {
                    set.lock().unwrap_or_else(|poison| poison.into_inner()).remove(&webview_label);
                }
                break;
            };
            let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
            if webview
                .eval_with_callback(
                    "(() => { try { return window.__CF_TICK ? window.__CF_TICK() : null; } catch (error) { return { error: String(error) }; } })()",
                    move |value| {
                        let _ = sender.try_send(value);
                    },
                )
                .is_err()
            {
                if let Some(set) = STARTED.get() {
                    set.lock().unwrap_or_else(|poison| poison.into_inner()).remove(&webview_label);
                }
                break;
            }
            let Ok(Some(raw)) = tokio::time::timeout(Duration::from_secs(4), receiver.recv()).await else {
                continue;
            };
            if raw.contains("\"creator\":true") {
                misses = 0;
            } else if webview_label.ends_with("-of") {
                misses = misses.saturating_add(1);
                if misses == 12 || misses == 30 {
                    let _ = webview.eval("location.assign('https://onlyfans.com/my/chats')");
                }
            }
            poll_ms = if raw.contains("\"active\":true") { 800 } else { 1000 };
            let Some(database) = app.try_state::<Database>() else {
                continue;
            };
            let bus = app.try_state::<SyncBus>();
            apply_tick(
                &app,
                database.inner(),
                bus.as_deref(),
                model_id,
                &webview_label,
                &raw,
            );
        }
    });
}

fn apply_tick(
    app: &AppHandle,
    database: &Database,
    bus: Option<&SyncBus>,
    model_id: i64,
    label: &str,
    raw: &str,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let fan_id = value.get("fanId").and_then(|item| item.as_str()).unwrap_or("").trim().to_owned();
    if let Some(items) = value.get("api").and_then(|item| item.as_array()) {
        for item in items {
            let url = item.get("url").and_then(|entry| entry.as_str()).unwrap_or("");
            let status = item.get("status").and_then(|entry| entry.as_u64()).unwrap_or(0) as u16;
            let body = item.get("body").and_then(|entry| entry.as_str()).unwrap_or("");
            if let Some(bus) = bus {
                let _ = record_capture(app, database, bus, label, model_id, url, status, body);
            }
        }
    }
    if value.get("widened").and_then(|item| item.as_bool()) == Some(true) {
        if let Some(bus) = bus {
            bus.mark_widened(model_id);
        }
    }
    if fan_id.is_empty() {
        return;
    }
    let profile = value.get("profile").and_then(|item| {
        if item.is_null() {
            return None;
        }
        let observed = crate::sync::ObservedProfile {
            status: item.get("status").and_then(|entry| entry.as_str()).map(str::to_owned),
            duration: item.get("duration").and_then(|entry| entry.as_str()).map(str::to_owned),
            cost: item.get("cost").and_then(|entry| entry.as_f64()),
            total_spend: item
                .get("totalSpend")
                .and_then(|entry| entry.as_f64())
                .or_else(|| item.get("total_spend").and_then(|entry| entry.as_f64())),
            last_spend_at: item
                .get("lastSpendAt")
                .and_then(|entry| entry.as_str())
                .map(str::to_owned)
                .or_else(|| {
                    item.get("last_spend_at")
                        .and_then(|entry| entry.as_str())
                        .map(str::to_owned)
                }),
            highest_spend: item
                .get("highestSpend")
                .and_then(|entry| entry.as_f64())
                .or_else(|| item.get("highest_spend").and_then(|entry| entry.as_f64())),
        };
        if observed.status.is_none()
            && observed.duration.is_none()
            && observed.cost.is_none()
            && observed.total_spend.is_none()
            && observed.last_spend_at.is_none()
            && observed.highest_spend.is_none()
        {
            None
        } else {
            Some(observed)
        }
    });
    // Always notify the insights pane when the active chat fan changes.
    let fan_signature = format!("fan:{fan_id}");
    if observation_changed(model_id, "__active_fan__", &fan_signature) {
        let _ = app.emit(
            "fan-selected",
            serde_json::json!({ "modelId": model_id, "ofFanId": fan_id }),
        );
    }
    let Some(profile) = profile else {
        return;
    };
    let signature = format!(
        "{:?}|{}|{:?}|{:?}|{:?}",
        profile.cost,
        profile.duration.clone().unwrap_or_default(),
        profile.total_spend,
        profile.last_spend_at,
        profile.highest_spend
    );
    if !observation_changed(model_id, &fan_id, &signature) {
        return;
    }
    if crate::sync::ingest_observation(database, model_id, &fan_id, Some(&profile)).is_ok() {
        let _ = app.emit(
            "fan-insights-changed",
            serde_json::json!({ "modelId": model_id, "ofFanId": fan_id }),
        );
        let _ = app.emit("models-changed", ());
    }
}

fn observation_changed(model_id: i64, fan_id: &str, signature: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    static LAST: std::sync::OnceLock<StdMutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    let map = LAST.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(|poison| poison.into_inner());
    let key = format!("{model_id}:{fan_id}");
    if map.get(&key).map(String::as_str) == Some(signature) {
        return false;
    }
    map.insert(key, signature.to_owned());
    true
}

#[cfg(test)]
mod tests {
    use super::{message_panes, MessageBarPlacement};

    #[test]
    fn minimized_panes_are_left_alone_and_a_normal_window_splits() {
        assert!(message_panes(0.0, 0.0, MessageBarPlacement::Top).is_none());
        assert!(message_panes(1280.0, 0.0, MessageBarPlacement::Sidebar).is_none());

        let top = message_panes(1280.0, 800.0, MessageBarPlacement::Top).unwrap();
        assert_eq!(top.tabs_x, 0.0);
        assert_eq!(top.tabs_y, 0.0);
        assert_eq!(top.tabs_width, 1280.0);
        assert_eq!(top.tabs_height, 44.0);
        assert_eq!(top.chat_x, 0.0);
        assert_eq!(top.chat_y, 44.0);
        assert_eq!(top.chat_height, 756.0);
        assert_eq!(top.chat_width, 900.0);
        assert_eq!(top.insights_x, 900.0);
        assert_eq!(top.insights_width, 380.0);

        let side = message_panes(1280.0, 800.0, MessageBarPlacement::Sidebar).unwrap();
        assert_eq!(side.tabs_x, 0.0);
        assert_eq!(side.tabs_y, 0.0);
        assert_eq!(side.tabs_width, 196.0);
        assert_eq!(side.tabs_height, 800.0);
        assert_eq!(side.chat_x, 196.0);
        assert_eq!(side.chat_y, 0.0);
        assert_eq!(side.chat_height, 800.0);
        assert!(side.chat_width > 600.0);
        assert_eq!(side.insights_x, side.chat_x + side.chat_width);
        assert_eq!(side.insights_height, 800.0);
    }

    #[test]
    fn capture_script_includes_clean_composer_and_auto_expiry() {
        let script = super::capture_script(42, false);
        assert!(script.contains("cleanComposerButtons"));
        assert!(!script.contains("mountChatComposer"));
        assert!(script.contains("pullPurchases"));
        assert!(script.contains("observeMessages"));
        assert!(!script.contains("cf-msg-tools"));
        assert!(script.contains("/api2/v2/chats/"));
        assert!(script.contains("isHourglassButton"));
        assert!(script.contains("executeAutoExpiry"));
        assert!(script.contains("monitorPublishStatus"));
        assert!(script.contains("find1DayButton"));
        assert!(script.contains("findStatsTab"));
        assert!(script.contains("findTodoElTiempoOption"));
        assert!(script.contains("isQuizButton"));
        assert!(script.contains("puzzle"));
        assert!(script.contains("icon-clock"));
        assert!(script.contains("forceShowHourglass"));
        assert!(script.contains("formato"));
        assert!(script.contains("runComposerPass"));
        assert!(script.contains("scheduleClean"));
        assert!(!script.contains("\"class\", \"hidden\""));
        assert!(!script.contains("|at|"));
        assert!(script.contains("cf-allow-earnings"));
        assert!(script.contains("cf-vault-download"));
        assert!(script.contains("selectedVaultItems"));
        assert!(script.contains("normalizeVaultUrl"));
        assert!(script.contains("viewedVaultSrc"));
        assert!(script.contains("swiper-slide-active"));
        assert!(!script.contains("[class*='active'], [class*='selected'], [class*='checked']"));
        assert!(!script.contains("needle.indexOf(candidate)"));
        assert!(script.contains("endsWith(\".onlyfans.com\")"));
        let request = super::vault_request_script(7, "\"https://onlyfans.com/api2/v2/vault/media\"");
        assert!(request.contains("interceptors"));
        assert!(!request.contains("credentials"));
        assert!(!script.contains("__CF_FETCH_ATTEMPTED"));
        assert!(!script.contains("earnings/chart?startDate"));
    }

    #[test]
    fn statements_stay_inside_chats_until_sync_opens_them() {
        let blocked: tauri::Url = "https://onlyfans.com/my/statements/earnings".parse().unwrap();
        let chats: tauri::Url = "https://onlyfans.com/my/chats/chat/42".parse().unwrap();
        let model_id = 9_000_001;
        super::earnings_gate(model_id).store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!super::navigation_allowed(model_id, &blocked));
        assert!(super::navigation_allowed(model_id, &chats));
        super::earnings_gate(model_id).store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(super::navigation_allowed(model_id, &blocked));
        super::earnings_gate(model_id).store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!super::navigation_allowed(model_id, &blocked));
        assert_eq!(
            super::restore_target(Some(chats)),
            "https://onlyfans.com/my/chats/chat/42"
        );
        assert_eq!(
            super::restore_target(Some(blocked)),
            "https://onlyfans.com/my/chats"
        );
    }

    #[test]
    fn chat_composer_keeps_native_ppv_controls_without_custom_bar() {
        let script = super::capture_script(42, true);
        assert!(script.contains("function isChatPage()"));
        assert!(script.contains("if (isChatPage()) return false;"));
        assert!(script.contains("if (isChatPage()) return;"));
        assert!(!script.contains("createChatTools"));
        assert!(!script.contains("cf-msg-tools"));
        assert!(!script.contains("cf-msg-strip"));
    }
}
