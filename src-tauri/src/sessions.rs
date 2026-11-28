use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::supabase::{Model, SupabaseStore};
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageTabsState {
    pub open_model_ids: Vec<String>,
    pub active_model_id: Option<String>,
    pub bar_placement: MessageBarPlacement,
    pub insights_open: bool,
    pub unread_counts: HashMap<String, u32>,
}

impl Default for MessageTabsState {
    fn default() -> Self {
        Self {
            open_model_ids: Vec::new(),
            active_model_id: None,
            bar_placement: MessageBarPlacement::default(),
            insights_open: true,
            unread_counts: HashMap::new(),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiPreferencesFile {
    #[serde(default)]
    bar_placement: MessageBarPlacement,
    #[serde(default = "default_insights_open")]
    insights_open: bool,
}

fn default_insights_open() -> bool {
    true
}

const MESSAGES_WINDOW_LABEL: &str = "messages-hub";
const MESSAGES_TABS_LABEL: &str = "messages-tabs";
const TAB_BAR_HEIGHT: f64 = 44.0;
const SIDEBAR_WIDTH: f64 = 60.0;
const INSIGHTS_WIDTH: f64 = 300.0;
static APP_DARK: AtomicBool = AtomicBool::new(false);
const UI_PREFS_FILE: &str = "ui_preferences.json";
static HUB_VISIBLE: AtomicBool = AtomicBool::new(false);
static APP_QUITTING: AtomicBool = AtomicBool::new(false);
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

fn known_model_ids() -> &'static Mutex<HashSet<String>> {
    static KNOWN: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(HashSet::new()))
}

fn remember_model_id(model_id: &str) {
    if !model_id.trim().is_empty() {
        known_model_ids()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(model_id.to_owned());
    }
}

fn forget_model_id(model_id: &str) {
    known_model_ids()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .remove(model_id);
}

fn viewing_fans() -> &'static Mutex<HashMap<String, String>> {
    static FANS: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    FANS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn note_viewing_fan(model_id: &str, of_fan_id: &str) {
    let fan = of_fan_id.trim().trim_start_matches(['u', 'U']);
    if fan.is_empty() || model_id.trim().is_empty() {
        return;
    }
    viewing_fans()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(model_id.to_owned(), fan.to_owned());
}

fn current_live_focus() -> Option<crate::notifications::LiveFocus> {
    if !HUB_VISIBLE.load(Ordering::SeqCst) {
        return None;
    }
    let active = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .active_model_id
        .clone()?;
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
    place_webview(view, 0.0, 0.0, 1100.0, 760.0);
}

fn park_every_of_webview(app: &AppHandle) {
    let ids: Vec<String> = known_model_ids()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .cloned()
        .collect();
    for id in ids {
        if let Some(view) = app.get_webview(&messages_of_label(&id)) {
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

fn model_aliases() -> &'static Mutex<HashMap<String, String>> {
    static ALIASES: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_model_alias(from_id: &str, to_id: &str) {
    if from_id == to_id {
        return;
    }
    model_aliases()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(from_id.to_owned(), to_id.to_owned());
}

fn resolve_model_alias(model_id: &str) -> String {
    model_aliases()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(model_id)
        .cloned()
        .unwrap_or_else(|| model_id.to_owned())
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
                state.insights_open = parsed.insights_open;
            }
        }
    }
}

fn persist_ui_preferences(app: &AppHandle) {
    let Ok(app_dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&app_dir);
    let state = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let payload = UiPreferencesFile {
        bar_placement: state.bar_placement,
        insights_open: state.insights_open,
    };
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        let _ = std::fs::write(app_dir.join(UI_PREFS_FILE), json);
    }
}

pub async fn set_message_bar_placement(
    app: &AppHandle,
    placement: MessageBarPlacement,
) -> Result<MessageTabsState, String> {
    {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.bar_placement = placement;
    }
    persist_ui_preferences(app);
    layout_messages_hub(app);
    let snapshot = get_message_tabs(app).await;
    let _ = app.emit("message-tabs-changed", &snapshot);
    Ok(snapshot)
}

pub async fn set_insights_open(app: &AppHandle, open: bool) -> Result<MessageTabsState, String> {
    {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.insights_open = open;
    }
    persist_ui_preferences(app);
    layout_messages_hub(app);
    let snapshot = get_message_tabs(app).await;
    let _ = app.emit("message-tabs-changed", &snapshot);
    Ok(snapshot)
}

fn messages_insights_label(model_id: &str) -> String {
    format!("messages-{model_id}-insights")
}

fn app_is_dark() -> bool {
    APP_DARK.load(Ordering::Relaxed)
}

fn current_theme() -> tauri::Theme {
    if app_is_dark() {
        tauri::Theme::Dark
    } else {
        tauri::Theme::Light
    }
}

pub fn apply_app_theme(app: &AppHandle, dark: bool) {
    APP_DARK.store(dark, Ordering::Relaxed);
    if let Some(window) = app.get_window("main") {
        let _ = window.set_theme(Some(current_theme()));
    }
    let script = format!("window.__CF_APPLY_THEME && window.__CF_APPLY_THEME({dark});");
    for (label, webview) in app.webviews() {
        if label.contains("-of") || label.starts_with("model-") {
            let _ = webview.eval(&script);
        }
    }
    let _ = app.emit("theme-changed", dark);
}

fn theme_bootstrap() -> String {
    let dark = app_is_dark();
    let theme = if dark { "dark" } else { "light" };
    let flag = if dark { "true" } else { "false" };
    format!(
        "try{{localStorage.setItem('cf-theme','{theme}');document.documentElement.classList.toggle('dark',{flag});}}catch(e){{}}"
    )
}

fn tabs_script() -> String {
    format!(
        "window.__CF_VIEW__ = \"messages-tabs\";{}",
        theme_bootstrap()
    )
}

async fn emit_tabs_changed(app: &AppHandle) {
    let snapshot = get_message_tabs(app).await;
    let _ = app.emit("message-tabs-changed", snapshot);
}

async fn update_hub_title(app: &AppHandle, store: &SupabaseStore, active_model_id: Option<&str>) {
    let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) else {
        return;
    };
    let title = match active_model_id {
        Some(id) => match store.get_model(id).await {
            Ok(model) if !model.display_name.trim().is_empty() => {
                format!("Mensajes · {}", model.display_name)
            }
            _ => "Mensajes · CreatorFloww".to_owned(),
        },
        None => "Mensajes · CreatorFloww".to_owned(),
    };
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
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl SyncBus {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    fn begin(&self, model_id: &str) -> Arc<Slot> {
        let slot = Arc::new(Slot::new());
        self.slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(model_id.to_owned(), slot.clone());
        slot
    }

    fn mark_widened(&self, model_id: &str) {
        if let Some(slot) = self.slot(model_id) {
            slot.widened.store(true, Ordering::Relaxed);
        }
    }

    fn slot(&self, model_id: &str) -> Option<Arc<Slot>> {
        self.slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(model_id)
            .cloned()
    }
}

pub fn window_label(model_id: &str) -> String {
    format!("model-{model_id}")
}

pub async fn list_accounts(app: &AppHandle, store: &SupabaseStore) -> Result<Vec<ModelAccount>, String> {
    let models = store.list_models().await.map_err(|err| err.to_string())?;
    for model in &models {
        remember_model_id(&model.id);
    }
    Ok(models
        .into_iter()
        .map(|model| account(app, model))
        .collect())
}

pub async fn open_model_session(
    app: &AppHandle,
    store: &SupabaseStore,
    model_id: Option<String>,
) -> Result<ModelAccount, String> {
    let model = match model_id {
        Some(id) => store.get_model(&id).await.map_err(|err| err.to_string())?,
        None => {
            let open_ids = currently_open_model_ids(app);
            match store
                .find_reusable_empty_model(&open_ids)
                .await
                .map_err(|err| err.to_string())?
            {
                Some(existing_empty) => existing_empty,
                None => store
                    .create_model_partition()
                    .await
                    .map_err(|err| err.to_string())?,
            }
        }
    };
    remember_model_id(&model.id);
    let _ = detach_model_from_hub(app, None, &model.id).await;
    let remote_ua = if model.of_user_id.is_some() {
        store
            .fetch_model_session(&model.id)
            .await
            .ok()
            .map(|s| s.user_agent)
            .filter(|ua| !ua.is_empty())
    } else {
        None
    };
    let window = open_or_focus(app, &model, remote_ua.as_deref())?;
    let label = window.label().to_string();
    spawn_reader(app.clone(), label, model.id.clone());
    Ok(account(app, model))
}

fn currently_open_model_ids(app: &AppHandle) -> Vec<String> {
    let ids: Vec<String> = known_model_ids()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .cloned()
        .collect();
    ids.into_iter()
        .filter(|id| {
            app.get_webview_window(&window_label(id)).is_some()
                || app.get_webview(&messages_of_label(id)).is_some()
        })
        .collect()
}

pub async fn delete_model_session(
    app: &AppHandle,
    store: &SupabaseStore,
    model_id: &str,
) -> Result<(), String> {
    let model = store.get_model(model_id).await.map_err(|err| err.to_string())?;
    let _ = detach_model_from_hub(app, Some(store), model_id).await;
    if let Some(window) = app.get_webview_window(&window_label(model_id)) {
        let _ = window.destroy();
    }
    store.delete_model(model_id).await.map_err(|err| err.to_string())?;
    forget_model_id(model_id);
    if let Ok(directory) = partition_dir(app, &model.partition_id) {
        let _ = std::fs::remove_dir_all(directory);
    }
    let _ = app.emit("models-changed", ());
    Ok(())
}

pub async fn revoke_model_access(app: &AppHandle, model_id: &str) {
    let _ = detach_model_from_hub(app, None, model_id).await;
    if let Some(window) = app.get_webview_window(&window_label(model_id)) {
        let _ = window.destroy();
    }
    forget_model_id(model_id);
}

pub fn start_background_monitors(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let models = {
            let Some(store) = app.try_state::<SupabaseStore>() else {
                return;
            };
            if !store.has_session() {
                return;
            }
            store.list_models().await.unwrap_or_default()
        };
        for model in models {
            remember_model_id(&model.id);
            if model.of_user_id.is_none() {
                continue;
            }
            schedule_background_monitor(&app, model.id);
            tokio::time::sleep(Duration::from_millis(450)).await;
        }
    });
}

pub fn schedule_background_monitor(app: &AppHandle, model_id: String) {
    if APP_QUITTING.load(Ordering::SeqCst) {
        return;
    }
    let app_async = app.clone();
    tauri::async_runtime::spawn(async move {
        let (model, remote_ua) = {
            let Some(store) = app_async.try_state::<SupabaseStore>() else {
                return;
            };
            if !store.has_session() {
                return;
            }
            let Ok(model) = store.get_model(&model_id).await else {
                return;
            };
            if model.of_user_id.is_none() {
                return;
            }
            let ua = store
                .fetch_model_session(&model.id)
                .await
                .ok()
                .map(|s| s.user_agent)
                .filter(|ua| !ua.is_empty());
            (model, ua)
        };
        let app_inner = app_async.clone();
        let _ = app_async.run_on_main_thread(move || {
            let _ = ensure_background_monitor_on_main(&app_inner, &model, remote_ua.as_deref());
        });
    });
}

fn ensure_background_monitor_on_main(
    app: &AppHandle,
    model: &Model,
    user_agent: Option<&str>,
) -> Result<(), String> {
    if APP_QUITTING.load(Ordering::SeqCst) {
        return Ok(());
    }
    let _gate = HUB_GATE.lock().unwrap_or_else(|poison| poison.into_inner());
    if model.of_user_id.is_none() {
        return Ok(());
    }
    remember_model_id(&model.id);
    if app.get_webview_window(&window_label(&model.id)).is_some() {
        return Ok(());
    }
    let of_label = messages_of_label(&model.id);
    if app.get_webview(&of_label).is_some() {
        spawn_reader(app.clone(), of_label, model.id.clone());
        return Ok(());
    }
    let placement = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .bar_placement;
    let initial_panes = default_message_panes(placement);
    let window = ensure_hub_window(
        app,
        &model.display_name,
        HUB_VISIBLE.load(Ordering::SeqCst),
        &initial_panes,
    )?;
    let directory = partition_dir(app, &model.partition_id)?;
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let nav_model_id = model.id.clone();
    let builder = tauri::webview::WebviewBuilder::new(
        of_label.clone(),
        WebviewUrl::External(chats_url()?),
    )
    .background_color(tauri::window::Color(15, 23, 42, 255))
    .data_directory(directory)
    .initialization_script(capture_script(&model.id, true))
    .on_navigation(move |url| navigation_allowed(&nav_model_id, url))
    .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
    .focused(false);
    let builder = if let Some(ua) = user_agent.filter(|ua| !ua.trim().is_empty()) {
        builder.user_agent(ua)
    } else {
        builder
    };
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
            .as_deref()
            == Some(model.id.as_str());
    if !is_active_visible {
        park_of_webview(&created);
    }
    spawn_reader(app.clone(), of_label.clone(), model.id.clone());
    schedule_session_sync(app, model.id.clone(), of_label);
    Ok(())
}

pub async fn sync_model_data(
    app: &AppHandle,
    store: &SupabaseStore,
    bus: &SyncBus,
    model_id: &str,
) -> Result<SyncReport, String> {
    store.get_model(model_id).await.map_err(|err| err.to_string())?;
    let slot = bus.begin(model_id);
    let in_hub_tabs = messages_hub_state()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .open_model_ids
        .iter()
        .any(|id| id == model_id);
    let messages_sync = if let Some(existing) = app.get_webview(&messages_of_label(model_id)) {
        Some(begin_messages_sync(&existing, model_id)?)
    } else if in_hub_tabs && app.get_window(MESSAGES_WINDOW_LABEL).is_some() {
        return Err(
            "La ventana de Messages aún no está lista. Espera un momento y sincroniza de nuevo."
                .to_owned(),
        );
    } else {
        let opened = open_model_session(app, store, Some(model_id.to_owned())).await?;
        let window = app
            .get_webview_window(&window_label(model_id))
            .ok_or_else(|| "no se pudo abrir la ventana de OnlyFans".to_owned())?;
        if opened.model.of_user_id.is_some() {
            prime_window(&window, true)?;
        }
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
        spawn_reader(app.clone(), messages_of_label(model_id), model_id.to_owned());
    }
    let capture_label = if messages_sync.is_some() {
        messages_of_label(model_id)
    } else {
        window_label(model_id)
    };
    let _ = webview.eval("window.__CF_FORCE_LINK && window.__CF_FORCE_LINK(true)");
    let mut last_err = String::new();
    for attempt in 0..4 {
        match pull_model_api(app, store, bus, model_id, &capture_label).await {
            Ok(()) => {
                last_err.clear();
                break;
            }
            Err(err) if retryable_sync_error(&err) && attempt < 3 => {
                last_err = err;
                tokio::time::sleep(Duration::from_millis(800)).await;
                let _ = webview.eval("window.__CF_FORCE_LINK && window.__CF_FORCE_LINK(true)");
            }
            Err(err) => {
                last_err = err;
                break;
            }
        }
    }
    if !last_err.is_empty() {
        let linked = store
            .get_model(model_id)
            .await
            .map(|model| model.of_user_id.is_some())
            .unwrap_or(false);
        if !linked {
            return Err(friendly_sync_error(&last_err));
        }
    }
    let _ = store.refresh_fan_spend_peaks(model_id).await;
    let model = store.get_model(model_id).await.map_err(|err| err.to_string())?;
    Ok(SyncReport {
        transactions_upserted: slot.upserted.load(Ordering::Relaxed),
        model: account(app, model),
    })
}

fn retryable_sync_error(err: &str) -> bool {
    err.contains("CLIENT_NOT_READY") || err.contains("Timeout") || err.contains("no está lista")
}

fn friendly_sync_error(err: &str) -> String {
    if err.contains("CLIENT_NOT_READY") || err.contains("Timeout") {
        return "OnlyFans todavía no entregó la sesión. Deja la ventana abierta en el inicio de la cuenta y pulsa Sincronizar otra vez.".to_owned();
    }
    if err.contains("Sesión expirada") || err.contains("no autenticada") {
        return "La ventana no tiene la sesión iniciada. Entra a la cuenta de OnlyFans y pulsa Sincronizar.".to_owned();
    }
    err.to_owned()
}

async fn pull_model_api(
    app: &AppHandle,
    store: &SupabaseStore,
    bus: &SyncBus,
    model_id: &str,
    capture_label: &str,
) -> Result<(), String> {
    let me = crate::of_api::get_me(app, model_id).await?;
    let me_body = serde_json::to_string(&me).map_err(|err| err.to_string())?;
    record_capture(
        app,
        store,
        bus,
        capture_label,
        model_id,
        "https://onlyfans.com/api2/v2/users/me",
        200,
        &me_body,
    )
    .await?;
    let balances = crate::of_api::get_balances(app, model_id).await?;
    let balances_body = serde_json::to_string(&balances).map_err(|err| err.to_string())?;
    record_capture(
        app,
        store,
        bus,
        capture_label,
        model_id,
        "https://onlyfans.com/api2/v2/payouts/balances",
        200,
        &balances_body,
    )
    .await?;

    let mut marker: Option<String> = None;
    for _ in 0..40 {
        let page = match crate::of_api::list_transactions(app, model_id, 50, marker.as_deref()).await {
            Ok(page) => page,
            Err(err) if crate::of_api::stop_sync(&err) => return Err(err),
            Err(err) => return Err(err),
        };
        let empty = crate::of_api::page_list(&page).map(|list| list.is_empty()).unwrap_or(true);
        let body = serde_json::to_string(&page).map_err(|err| err.to_string())?;
        record_capture(
            app,
            store,
            bus,
            capture_label,
            model_id,
            "https://onlyfans.com/api2/v2/payouts/transactions",
            200,
            &body,
        )
        .await?;
        if empty {
            break;
        }
        match crate::of_api::next_marker(&page) {
            Some(next) => marker = Some(next),
            None => break,
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let mut offset = 0_u32;
    for _ in 0..40 {
        let page = match crate::of_api::list_subscribers(app, model_id, "all", 50, offset).await {
            Ok(page) => page,
            Err(err) if crate::of_api::stop_sync(&err) => return Err(err),
            Err(err) => return Err(err),
        };
        let count = crate::of_api::page_list(&page).map(|list| list.len()).unwrap_or(0);
        let body = serde_json::to_string(&page).map_err(|err| err.to_string())?;
        record_capture(
            app,
            store,
            bus,
            capture_label,
            model_id,
            "https://onlyfans.com/api2/v2/subscriptions/subscribers?type=all",
            200,
            &body,
        )
        .await?;
        if count == 0 {
            break;
        }
        offset += 50;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let mut from_id: Option<String> = None;
    for _ in 0..5 {
        let page = match crate::of_api::list_subscription_notifications(
            app,
            model_id,
            50,
            from_id.as_deref(),
        )
        .await
        {
            Ok(page) => page,
            Err(err) if crate::of_api::stop_sync(&err) => return Err(err),
            Err(_) => break,
        };
        let list = crate::of_api::page_list(&page);
        let count = list.map(|items| items.len()).unwrap_or(0);
        let last_id = list.and_then(|items| {
            items.last().and_then(|entry| {
                entry.get("id").and_then(|val| match val {
                    serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
            })
        });
        let has_more = page
            .get("hasMore")
            .and_then(|val| val.as_bool())
            .unwrap_or(false);
        let body = serde_json::to_string(&page).map_err(|err| err.to_string())?;
        record_capture(
            app,
            store,
            bus,
            capture_label,
            model_id,
            "https://onlyfans.com/api2/v2/users/notifications?limit=50&type=subscribed",
            200,
            &body,
        )
        .await?;
        if count == 0 || !has_more {
            break;
        }
        match last_id {
            Some(next) if from_id.as_deref() != Some(next.as_str()) => from_id = Some(next),
            _ => break,
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    for notif_type in ["purchases", "tips"] {
        let page = match crate::of_api::list_user_notifications(app, model_id, notif_type, 50, None).await
        {
            Ok(page) => page,
            Err(err) if crate::of_api::stop_sync(&err) => return Err(err),
            Err(_) => continue,
        };
        let body = serde_json::to_string(&page).map_err(|err| err.to_string())?;
        let url = format!(
            "https://onlyfans.com/api2/v2/users/notifications?limit=50&type={notif_type}"
        );
        record_capture(
            app,
            store,
            bus,
            capture_label,
            model_id,
            &url,
            200,
            &body,
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn record_capture(
    app: &AppHandle,
    store: &SupabaseStore,
    bus: &SyncBus,
    window_label_value: &str,
    model_id: &str,
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
    if !store.has_session() {
        return Ok(());
    }
    let effective_model_id = resolve_model_alias(model_id);
    let was_unauthenticated = store
        .get_model(&effective_model_id)
        .await
        .map(|m| m.of_user_id.is_none())
        .unwrap_or(false);

    let outcome = match ingest_body(store, &effective_model_id, url, status, body).await {
        Ok(outcome) => outcome,
        Err(err) => {
            let msg = err.to_string();
            if was_unauthenticated && msg.contains("ya está agregada") {
                if try_adopt_duplicate_profile(app, store, model_id, &effective_model_id, body)
                    .await
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
    note(bus, &effective_model_id, &outcome);

    if outcome.kind == crate::sync::CaptureKind::Profile && !outcome.unauthorized && was_unauthenticated {
        if let Ok(updated) = store.get_model(&effective_model_id).await {
            if updated.of_user_id.is_some() {
                on_model_first_authenticated(app, model_id, &updated);
            }
        }
    }

    if outcome.notifications.source != crate::notifications::BatchSource::None {
        if let Some(notif_bus) = app.try_state::<crate::notifications::NotificationBus>() {
            let focus = current_live_focus();
            let emitted = notif_bus
                .process_batch(
                    Some(app),
                    store,
                    &effective_model_id,
                    outcome.notifications,
                    focus.as_ref(),
                )
                .await;
            if !emitted.is_empty() {
                emit_tabs_changed(app).await;
            }
        }
    }

    if !outcome.unauthorized {
        if outcome.dropper_activity_changed {
            let _ = app.emit("dropper-activity-changed", effective_model_id.clone());
        }
        if outcome.kind != crate::sync::CaptureKind::LiveEvent {
            if outcome.model_data_changed {
                let _ = app.emit("models-changed", ());
            }
            if let Some(fan_id) = &outcome.fan_id {
                let _ = app.emit(
                    "fan-insights-changed",
                    serde_json::json!({ "modelId": effective_model_id, "ofFanId": fan_id }),
                );
            }
        }
    }
    Ok(())
}

fn on_model_first_authenticated(app: &AppHandle, raw_window_model_id: &str, updated: &Model) {
    let label_name = if !updated.display_name.trim().is_empty() {
        updated.display_name.trim().to_owned()
    } else if !updated.username.trim().is_empty() {
        format!("@{}", updated.username.trim())
    } else {
        "OnlyFans".to_owned()
    };
    let title = if !updated.username.trim().is_empty() && !updated.display_name.trim().is_empty() {
        format!(
            "{} (@{}) · CreatorFloww",
            updated.display_name.trim(),
            updated.username.trim()
        )
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
    schedule_session_sync(app, updated.id.clone(), messages_of_label(&updated.id));
    schedule_session_sync(app, updated.id.clone(), window_label(raw_window_model_id));
    schedule_background_monitor(app, updated.id.clone());
}

async fn try_adopt_duplicate_profile(
    app: &AppHandle,
    store: &SupabaseStore,
    raw_window_model_id: &str,
    placeholder_id: &str,
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

    let (adopted, old_partition) = store
        .adopt_partition_into_existing_model(
            placeholder_id,
            &of_user_id,
            username,
            display_name,
            avatar_url,
        )
        .await
        .ok()??;

    register_model_alias(raw_window_model_id, &adopted.id);
    register_model_alias(placeholder_id, &adopted.id);
    remember_model_id(&adopted.id);

    if app.get_webview_window(&window_label(&adopted.id)).is_none()
        && app.get_webview(&messages_of_label(&adopted.id)).is_none()
    {
        if !old_partition.is_empty() {
            if let Ok(old_dir) = partition_dir(app, &old_partition) {
                let _ = std::fs::remove_dir_all(old_dir);
            }
        }
    }
    on_model_first_authenticated(app, raw_window_model_id, &adopted);
    Some(adopted)
}

fn capture_window_matches(label: &str, model_id: &str) -> bool {
    label == window_label(model_id) || label == format!("messages-{model_id}-of")
}

fn note(bus: &SyncBus, model_id: &str, outcome: &IngestOutcome) {
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
        crate::sync::CaptureKind::Subscriber
        | crate::sync::CaptureKind::ChatMute
        | crate::sync::CaptureKind::Notifications
        | crate::sync::CaptureKind::LiveEvent
        | crate::sync::CaptureKind::Publication
        | crate::sync::CaptureKind::CreatorPosts => {}
    }
}

pub async fn get_message_tabs(app: &AppHandle) -> MessageTabsState {
    let unread_counts = if let (Some(bus), Some(store)) = (
        app.try_state::<crate::notifications::NotificationBus>(),
        app.try_state::<SupabaseStore>(),
    ) {
        if store.has_session() {
            bus.unread_counts(&store).await
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };
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
        .retain(|id| app.get_webview(&messages_of_label(id)).is_some());
    if let Some(active) = state.active_model_id.clone() {
        if !state.open_model_ids.contains(&active) {
            state.active_model_id = state.open_model_ids.first().cloned();
        }
    } else {
        state.active_model_id = state.open_model_ids.first().cloned();
    }
    state.clone()
}

pub async fn select_message_tab(
    app: &AppHandle,
    store: &SupabaseStore,
    model_id: &str,
) -> Result<(), String> {
    let already_open = {
        let state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        HUB_VISIBLE.load(Ordering::SeqCst)
            && state.open_model_ids.iter().any(|id| id == model_id)
            && app.get_window(MESSAGES_WINDOW_LABEL).is_some()
            && app.get_webview(&messages_of_label(model_id)).is_some()
    };
    if !already_open {
        return open_messages(app, store, model_id).await;
    }
    {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.active_model_id = Some(model_id.to_owned());
    }
    if let Some(nb) = app.try_state::<crate::notifications::NotificationBus>() {
        if nb.clear_unread(store, model_id).await {
            let counts = nb.unread_counts(store).await;
            let _ = app.emit("unread-counts-changed", counts);
        }
    }
    update_hub_title(app, store, Some(model_id)).await;
    layout_messages_hub(app);
    if let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) {
        let _ = focus_window(&window);
    }
    if let Some(of_view) = app.get_webview(&messages_of_label(model_id)) {
        let _ = of_view.set_focus();
    }
    emit_tabs_changed(app).await;
    Ok(())
}

pub async fn close_message_tab(
    app: &AppHandle,
    store: Option<&SupabaseStore>,
    model_id: &str,
) -> Result<(), String> {
    if let Some(of_view) = app.get_webview(&messages_of_label(model_id)) {
        park_of_webview(&of_view);
    }
    if let Some(insights) = app.get_webview(&messages_insights_label(model_id)) {
        let _ = insights.hide();
    }
    finish_removing_tab(app, store, model_id).await
}

async fn detach_model_from_hub(
    app: &AppHandle,
    store: Option<&SupabaseStore>,
    model_id: &str,
) -> Result<(), String> {
    if let Some(of_view) = app.get_webview(&messages_of_label(model_id)) {
        let _ = of_view.close();
    }
    if let Some(insights) = app.get_webview(&messages_insights_label(model_id)) {
        let _ = insights.close();
    }
    finish_removing_tab(app, store, model_id).await
}

async fn finish_removing_tab(
    app: &AppHandle,
    store: Option<&SupabaseStore>,
    model_id: &str,
) -> Result<(), String> {
    let next_active = {
        let mut state = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let removed_idx = state.open_model_ids.iter().position(|id| id == model_id);
        state.open_model_ids.retain(|id| id != model_id);
        if state.open_model_ids.is_empty() {
            state.active_model_id = None;
        } else if state.active_model_id.as_deref() == Some(model_id) {
            let pick = removed_idx
                .map(|idx| idx.saturating_sub(1).min(state.open_model_ids.len() - 1))
                .unwrap_or(0);
            state.active_model_id = state.open_model_ids.get(pick).cloned();
        }
        state.active_model_id.clone()
    };
    if next_active.is_none() {
        HUB_VISIBLE.store(false, Ordering::SeqCst);
        if let Some(window) = app.get_window(MESSAGES_WINDOW_LABEL) {
            park_every_of_webview(app);
            park_messages_hub(&window);
        }
    } else {
        if let Some(store) = store {
            update_hub_title(app, store, next_active.as_deref()).await;
        }
        layout_messages_hub(app);
        if let Some(active_id) = &next_active {
            if let Some(of_view) = app.get_webview(&messages_of_label(active_id)) {
                let _ = of_view.set_focus();
            }
        }
    }
    emit_tabs_changed(app).await;
    let _ = app.emit("models-changed", ());
    Ok(())
}

fn default_message_panes(placement: MessageBarPlacement) -> MessagePanes {
    message_panes(1280.0, 800.0, placement, true).unwrap_or(MessagePanes {
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
    let layout_queued = Arc::new(AtomicBool::new(false));
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
                    if let Some(of_view) = app_for_events.get_webview(&messages_of_label(&id)) {
                        park_of_webview(&of_view);
                    }
                    if let Some(ins) = app_for_events.get_webview(&messages_insights_label(&id)) {
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

pub async fn open_messages(
    app: &AppHandle,
    store: &SupabaseStore,
    model_id: &str,
) -> Result<(), String> {
    let model = store.get_model(model_id).await.map_err(|err| err.to_string())?;
    remember_model_id(&model.id);
    let remote_ua = store
        .fetch_model_session(&model.id)
        .await
        .ok()
        .map(|s| s.user_agent)
        .filter(|ua| !ua.is_empty());

    let of_label = messages_of_label(model_id);
    let insights_label = messages_insights_label(model_id);
    let standalone = window_label(model_id);
    let closed_standalone = app.get_webview_window(&standalone).is_some();
    close_label(app, &standalone);
    if closed_standalone {
        tokio::time::sleep(Duration::from_millis(450)).await;
    }

    {
        let _gate = HUB_GATE.lock().unwrap_or_else(|poison| poison.into_inner());
        let placement = messages_hub_state()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .bar_placement;
        let initial_panes = default_message_panes(placement);
        let window = ensure_hub_window(app, &model.display_name, true, &initial_panes)?;
        reveal_messages_hub(&window);

        if app.get_webview(&of_label).is_none() {
            let directory = partition_dir(app, &model.partition_id)?;
            std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
            let mut last_err = String::new();
            let mut attached = false;
            for attempt in 0..8 {
                let nav_id = model.id.clone();
                let builder = tauri::webview::WebviewBuilder::new(
                    of_label.clone(),
                    WebviewUrl::External(chats_url()?),
                )
                .background_color(tauri::window::Color(15, 23, 42, 255))
                .data_directory(directory.clone())
                .initialization_script(capture_script(&model.id, true))
                .on_navigation(move |url| navigation_allowed(&nav_id, url))
                .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
                .focused(true);
                let builder = if let Some(ua) = remote_ua.as_deref() {
                    builder.user_agent(ua)
                } else {
                    builder
                };
                #[cfg(target_os = "windows")]
                let builder = builder.additional_browser_args(FAST_WEBVIEW_ARGS);
                match window.add_child(
                    builder,
                    tauri::LogicalPosition::new(initial_panes.chat_x, initial_panes.chat_y),
                    tauri::LogicalSize::new(initial_panes.chat_width, initial_panes.chat_height),
                ) {
                    Ok(_) => {
                        attached = true;
                        break;
                    }
                    Err(err) => {
                        last_err = err.to_string();
                        let retry = last_err.contains("8007139F")
                            || last_err.contains("estado correcto")
                            || last_err.contains("correct state");
                        if !retry || attempt == 7 {
                            return Err(last_err);
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            }
            if !attached {
                return Err(last_err);
            }
        }
        if app.get_webview(&insights_label).is_none() {
            window
                .add_child(
                    tauri::webview::WebviewBuilder::new(
                        insights_label,
                        WebviewUrl::App("index.html".into()),
                    )
                    .initialization_script(insights_script(&model.id, &model.partition_id)),
                    tauri::LogicalPosition::new(initial_panes.insights_x, initial_panes.insights_y),
                    tauri::LogicalSize::new(
                        initial_panes.insights_width,
                        initial_panes.insights_height,
                    ),
                )
                .map_err(|err| err.to_string())?;
        }

        HUB_VISIBLE.store(true, Ordering::SeqCst);
        {
            let mut state = messages_hub_state()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !state.open_model_ids.iter().any(|id| id == model_id) {
                state.open_model_ids.push(model_id.to_owned());
            }
            state.active_model_id = Some(model_id.to_owned());
        }
        reveal_messages_hub(&window);
        focus_window(&window)?;
    }

    if let Some(nb) = app.try_state::<crate::notifications::NotificationBus>() {
        if nb.clear_unread(store, model_id).await {
            let counts = nb.unread_counts(store).await;
            let _ = app.emit("unread-counts-changed", counts);
        }
    }

    update_hub_title(app, store, Some(model_id)).await;
    layout_messages_hub(app);
    spawn_reader(app.clone(), of_label.clone(), model_id.to_owned());
    schedule_session_sync(app, model_id.to_owned(), of_label.clone());
    if let Some(of_view) = app.get_webview(&of_label) {
        let _ = of_view.set_focus();
    }
    emit_tabs_changed(app).await;
    let _ = app.emit("models-changed", ());
    Ok(())
}

pub async fn open_incoming_chat(
    app: &AppHandle,
    store: &SupabaseStore,
    model_id: &str,
    of_fan_id: &str,
) -> Result<(), String> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
    }
    open_messages(app, store, model_id).await?;
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
    let window_open = app.get_webview_window(&window_label(&model.id)).is_some() || hub_open;
    ModelAccount { model, window_open }
}

fn open_or_focus(
    app: &AppHandle,
    model: &Model,
    user_agent: Option<&str>,
) -> Result<WebviewWindow, String> {
    let label = window_label(&model.id);
    if let Some(window) = app.get_webview_window(&label) {
        allow_small_onlyfans(&window);
        focus(&window)?;
        schedule_session_sync(app, model.id.clone(), label.clone());
        if model.of_user_id.is_none() {
            schedule_profile_link(app, model.id.clone(), label);
            let _ = window.eval("window.__CF_FORCE_LINK && window.__CF_FORCE_LINK(true)");
        }
        return Ok(window);
    }
    let directory = partition_dir(app, &model.partition_id)?;
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let title = if !model.display_name.trim().is_empty() {
        format!("{} · CreatorFloww", model.display_name.trim())
    } else if !model.username.trim().is_empty() {
        format!("@{} · CreatorFloww", model.username.trim())
    } else {
        "Iniciar sesión en OnlyFans · CreatorFloww".to_owned()
    };
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
            .initialization_script(capture_script(&model.id, false));
        let builder = if let Some(ua) = user_agent.filter(|ua| !ua.trim().is_empty()) {
            builder.user_agent(ua)
        } else {
            builder
        };
        #[cfg(target_os = "windows")]
        let builder = builder.additional_browser_args(FAST_WEBVIEW_ARGS);
        match builder.build() {
            Ok(window) => {
                bind_close(&window, model.id.clone());
                allow_small_onlyfans(&window);
                schedule_session_sync(app, model.id.clone(), label.clone());
                if model.of_user_id.is_none() {
                    schedule_profile_link(app, model.id.clone(), label);
                }
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

fn bind_close(window: &WebviewWindow, raw_model_id: String) {
    let target = window.clone();
    let app_handle = window.app_handle().clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = target.destroy();
            let _ = app_handle.emit("models-changed", ());
            let app_resume = app_handle.clone();
            let effective_id = resolve_model_alias(&raw_model_id);
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

pub fn shutdown_secondary_windows(app: &AppHandle) {
    APP_QUITTING.store(true, Ordering::SeqCst);
    HUB_VISIBLE.store(false, Ordering::SeqCst);
    let webview_labels: Vec<String> = app
        .webviews()
        .keys()
        .filter(|label| label.as_str() != "main")
        .cloned()
        .collect();
    for label in webview_labels {
        if let Some(view) = app.get_webview(&label) {
            let _ = view.close();
        }
    }
    let window_labels: Vec<String> = app
        .webview_windows()
        .keys()
        .filter(|label| label.as_str() != "main")
        .cloned()
        .collect();
    for label in window_labels {
        close_label(app, &label);
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
    insights_open: bool,
) -> Option<MessagePanes> {
    if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
        return None;
    }
    match placement {
        MessageBarPlacement::Top => {
            let tab_bar_height = TAB_BAR_HEIGHT.min(height);
            let content_height = (height - tab_bar_height).max(0.0);
            let insights_width = if insights_open {
                INSIGHTS_WIDTH.min(width * 0.42)
            } else {
                0.0
            };
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
            let sidebar_width = SIDEBAR_WIDTH.min(width);
            let remaining_width = (width - sidebar_width).max(0.0);
            let insights_width = if insights_open {
                INSIGHTS_WIDTH.min(remaining_width * 0.40)
            } else {
                0.0
            };
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
    let Some(panes) = message_panes(
        logical_width,
        logical_height,
        state.bar_placement,
        state.insights_open,
    ) else {
        return;
    };
    if let Some(tabs) = app.get_webview(MESSAGES_TABS_LABEL) {
        let _ = tabs.set_position(tauri::LogicalPosition::new(panes.tabs_x, panes.tabs_y));
        let _ = tabs.set_size(tauri::LogicalSize::new(panes.tabs_width, panes.tabs_height));
        let _ = tabs.show();
    }
    let mut all_ids = state.open_model_ids.clone();
    for id in known_model_ids()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
    {
        if !all_ids.contains(id) {
            all_ids.push(id.clone());
        }
    }
    let hub_visible = HUB_VISIBLE.load(Ordering::SeqCst);
    let mut active_of: Option<tauri::Webview> = None;
    for id in &all_ids {
        let is_active = hub_visible
            && state.active_model_id.as_deref() == Some(id.as_str())
            && state.open_model_ids.contains(id);
        if let Some(of_view) = app.get_webview(&messages_of_label(id)) {
            if is_active {
                active_of = Some(of_view);
            } else if hub_visible {
                place_webview(&of_view, -1400.0, 0.0, 1100.0, 760.0);
            } else {
                park_of_webview(&of_view);
            }
        }
        if let Some(insights) = app.get_webview(&messages_insights_label(id)) {
            if is_active && panes.insights_width >= 8.0 {
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

pub fn onlyfans_webview(app: &AppHandle, model_id: &str) -> Result<tauri::Webview, String> {
    for label in [window_label(model_id), format!("messages-{model_id}-of")] {
        if let Some(webview) = app.get_webview(&label) {
            return Ok(webview);
        }
    }
    Err("Abre la ventana de la modelo e inicia sesión en OnlyFans.".to_owned())
}

pub async fn fetch_onlyfans_json(app: &AppHandle, model_id: &str, url: &str) -> Result<String, String> {
    let path = url
        .split_once("onlyfans.com")
        .map(|(_, path)| path)
        .unwrap_or(url);
    let path = if path.starts_with('/') { path } else { url };
    crate::of_api::request_raw(app, model_id, "GET", path, None).await
}

fn insights_script(model_id: &str, partition_id: &str) -> String {
    let id = serde_json::to_string(model_id).unwrap_or_else(|_| "\"\"".to_string());
    let partition = serde_json::to_string(partition_id).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "window.__CF_VIEW__ = \"insights\"; window.__CF_MODEL_ID__ = {id}; window.__CF_PARTITION_ID__ = {partition};{}",
        theme_bootstrap()
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

fn navigation_allowed(model_id: &str, url: &Url) -> bool {
    chats_navigation(url) || (earnings_gate(model_id).load(Ordering::SeqCst) && earnings_page(url))
}

fn earnings_gate(model_id: &str) -> Arc<AtomicBool> {
    use std::sync::OnceLock;
    static GATES: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    let gates = GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut gates = gates.lock().unwrap_or_else(|poison| poison.into_inner());
    gates
        .entry(model_id.to_owned())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

fn messages_of_label(model_id: &str) -> String {
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
    model_id: String,
    webview: tauri::Webview,
    restore_url: String,
}

impl Drop for MessagesSyncGuard {
    fn drop(&mut self) {
        earnings_gate(&self.model_id).store(false, Ordering::SeqCst);
        let _ = self.webview.eval(restore_chat_script(&self.restore_url));
    }
}

fn begin_messages_sync(webview: &tauri::Webview, model_id: &str) -> Result<MessagesSyncGuard, String> {
    let restore_url = restore_target(webview.url().ok());
    let script = open_earnings_script()?;
    earnings_gate(model_id).store(true, Ordering::SeqCst);
    let guard = MessagesSyncGuard {
        model_id: model_id.to_owned(),
        webview: webview.clone(),
        restore_url,
    };
    webview.eval(script).map_err(|err| err.to_string())?;
    Ok(guard)
}

fn session_sync_slots() -> &'static Mutex<HashSet<String>> {
    static SLOTS: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashSet::new()))
}

struct SessionSyncSlot(String);

impl Drop for SessionSyncSlot {
    fn drop(&mut self) {
        if let Ok(mut slots) = session_sync_slots().lock() {
            slots.remove(&self.0);
        }
    }
}

fn schedule_session_sync(app: &AppHandle, model_id: String, label: String) {
    if APP_QUITTING.load(Ordering::SeqCst) || label.is_empty() {
        return;
    }
    {
        let mut slots = session_sync_slots()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !slots.insert(label.clone()) {
            return;
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _slot = SessionSyncSlot(label.clone());
        for delay in [2u64, 6, 15] {
            if APP_QUITTING.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_secs(delay));
            if APP_QUITTING.load(Ordering::SeqCst) {
                return;
            }
            let cookies = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                read_onlyfans_cookies(&app, &label)
            }))
            .ok()
            .flatten();
            let Some(cookies) = cookies else {
                continue;
            };
            let app_http = app.clone();
            let model_http = model_id.clone();
            let label_http = label.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            tauri::async_runtime::spawn(async move {
                let done = publish_or_restore_session(&app_http, &model_http, &label_http, cookies).await;
                let _ = tx.send(done);
            });
            if rx.recv_timeout(Duration::from_secs(20)).unwrap_or(false) {
                return;
            }
        }
    });
}

fn read_onlyfans_cookies(app: &AppHandle, label: &str) -> Option<Vec<serde_json::Value>> {
    let webview = app.get_webview(label)?;
    let url = "https://onlyfans.com/".parse().ok()?;
    let mut cookies = webview
        .cookies()
        .or_else(|_| webview.cookies_for_url(url))
        .ok()?;
    cookies.retain(|cookie| {
        onlyfans_cookie(cookie)
            && cookie.name().len() <= 128
            && cookie.value().len() <= 4096
    });
    cookies.sort_by_key(|cookie| match cookie.name() {
        "auth_id" => 0,
        "sess" => 1,
        "fp" => 2,
        "st" => 3,
        "csrf" => 4,
        "auth_hash" => 5,
        "ref_src" => 6,
        _ => 10,
    });
    let values = cookies.iter().map(cookie_json).take(80).collect();
    Some(values)
}

fn onlyfans_cookie(cookie: &tauri::webview::Cookie<'_>) -> bool {
    match cookie.domain() {
        Some(domain) => domain.contains("onlyfans.com"),
        None => true,
    }
}

fn has_authenticated_cookies(cookies: &[serde_json::Value]) -> bool {
    let has_auth_id = cookies.iter().any(|cookie| {
        cookie.get("name").and_then(|v| v.as_str()) == Some("auth_id")
            && cookie
                .get("value")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .is_some_and(|v| !v.is_empty() && v != "0")
    });
    let has_sess = cookies.iter().any(|cookie| {
        cookie.get("name").and_then(|v| v.as_str()) == Some("sess")
            && cookie
                .get("value")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .is_some_and(|v| !v.is_empty())
    });
    has_auth_id && has_sess
}

fn cookie_fingerprint(
    cookies: &[serde_json::Value],
    local_storage: &HashMap<String, String>,
    user_agent: &str,
) -> String {
    let mut pairs: Vec<String> = cookies
        .iter()
        .filter_map(|cookie| {
            let name = cookie.get("name")?.as_str()?;
            let value = cookie.get("value")?.as_str()?;
            Some(format!("c:{name}={value}"))
        })
        .collect();
    for (k, v) in local_storage {
        pairs.push(format!("ls:{k}={v}"));
    }
    if !user_agent.trim().is_empty() {
        pairs.push(format!("ua:{}", user_agent.trim()));
    }
    pairs.sort();
    let mut hash: u64 = 0xcbf29ce484222325;
    for pair in pairs {
        for byte in pair.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
    }
    format!("{hash:x}")
}

fn cookie_json(cookie: &tauri::webview::Cookie<'_>) -> serde_json::Value {
    serde_json::json!({
        "name": cookie.name(),
        "value": cookie.value(),
        "domain": cookie.domain(),
        "path": cookie.path().unwrap_or("/"),
        "secure": cookie.secure().unwrap_or(true),
        "httpOnly": cookie.http_only().unwrap_or(false),
        "sameSite": cookie.same_site().map(|site| format!("{site:?}")).unwrap_or_default(),
        "expires": cookie.expires_datetime().map(|when| when.unix_timestamp()),
    })
}

fn cookie_from_stored(value: &serde_json::Value) -> Option<tauri::webview::Cookie<'static>> {
    let name = value.get("name")?.as_str()?.trim();
    let raw = value.get("value")?.as_str()?;
    if name.is_empty()
        || name.len() > 128
        || raw.is_empty()
        || raw.len() > 4096
        || name.contains(';')
        || name.contains('\n')
        || raw.contains(';')
        || raw.contains('\n')
    {
        return None;
    }
    let mut max_age: Option<i64> = None;
    if let Some(expires) = value.get("expires").and_then(|item| item.as_i64()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        if expires > 0 {
            if expires < now {
                return None;
            }
            max_age = Some(expires - now);
        }
    }
    let path = value.get("path").and_then(|item| item.as_str()).unwrap_or("/");
    if !path.starts_with('/') || path.contains(';') {
        return None;
    }
    let domain = value
        .get("domain")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|domain| {
            domain.contains("onlyfans.com") && !domain.contains(';') && !domain.contains(' ')
        })
        .unwrap_or(".onlyfans.com");
    let mut header = format!("{name}={raw}; Path={path}; Domain={domain}");
    if let Some(secs) = max_age {
        header.push_str(&format!("; Max-Age={secs}"));
    }
    if value.get("secure").and_then(|item| item.as_bool()).unwrap_or(true) {
        header.push_str("; Secure");
    }
    if value.get("httpOnly").and_then(|item| item.as_bool()).unwrap_or(false) {
        header.push_str("; HttpOnly");
    }
    match value.get("sameSite").and_then(|item| item.as_str()) {
        Some("None") => header.push_str("; SameSite=None"),
        Some("Strict") => header.push_str("; SameSite=Strict"),
        Some("Lax") => header.push_str("; SameSite=Lax"),
        _ => {}
    }
    tauri::webview::Cookie::parse(header)
        .ok()
        .map(|cookie| cookie.into_owned())
}

async fn read_onlyfans_client_state(
    app: &AppHandle,
    label: &str,
) -> (HashMap<String, String>, String) {
    let Some(webview) = app.get_webview(label) else {
        return (HashMap::new(), String::new());
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let script = r#"(() => {
        try {
            const out = {};
            const keys = [];
            for (let i = 0; i < localStorage.length; i += 1) {
                const k = localStorage.key(i);
                if (!k || k === 'cf-theme' || k.length > 128) continue;
                keys.push(k);
            }
            keys.sort((a, b) => {
                const rank = (k) => k === 'bcTokenSha' ? 0 : /bc|auth|fp|token|user|device|init/i.test(k) ? 1 : 2;
                return rank(a) - rank(b);
            });
            let count = 0;
            for (const k of keys) {
                if (count >= 40) break;
                const v = localStorage.getItem(k);
                if (typeof v === 'string' && v.length <= 4096) {
                    out[k] = v;
                    count += 1;
                }
            }
            return { ls: out, ua: String(navigator.userAgent || '').slice(0, 512) };
        } catch (e) {
            return { ls: {}, ua: '' };
        }
    })()"#;
    if webview
        .eval_with_callback(script, move |raw| {
            let _ = tx.try_send(raw);
        })
        .is_err()
    {
        return (HashMap::new(), String::new());
    }
    let Ok(Some(raw)) = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await else {
        return (HashMap::new(), String::new());
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (HashMap::new(), String::new());
    };
    let ls = parsed
        .get("ls")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let ua = parsed
        .get("ua")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_owned();
    (ls, ua)
}

async fn publish_or_restore_session(
    app: &AppHandle,
    model_id: &str,
    label: &str,
    cookies: Vec<serde_json::Value>,
) -> bool {
    if APP_QUITTING.load(Ordering::SeqCst) {
        return true;
    }
    let Some(store) = app.try_state::<SupabaseStore>() else {
        return false;
    };
    let local_auth = has_authenticated_cookies(&cookies);
    if local_auth {
        let (local_storage, user_agent) = read_onlyfans_client_state(app, label).await;
        let fingerprint = cookie_fingerprint(&cookies, &local_storage, &user_agent);
        let remote = store.fetch_model_session(model_id).await.ok();
        if remote.as_ref().is_some_and(|session| {
            session.fingerprint == fingerprint && has_authenticated_cookies(&session.cookies)
        }) {
            return true;
        }
        return store
            .save_model_cookies(model_id, &cookies, &fingerprint, &local_storage, &user_agent)
            .await
            .is_ok();
    }

    let Ok(remote) = store.fetch_model_session(model_id).await else {
        return false;
    };
    if !has_authenticated_cookies(&remote.cookies) {
        return false;
    }
    let Some(webview) = app.get_webview(label) else {
        return true;
    };
    let mut applied = 0usize;
    for value in &remote.cookies {
        if let Some(cookie) = cookie_from_stored(value) {
            if webview.set_cookie(cookie).is_ok() {
                applied += 1;
            }
        }
    }
    if applied == 0 {
        return false;
    }
    if !remote.local_storage.is_empty() {
        if let Ok(ls_json) = serde_json::to_string(&remote.local_storage) {
            let ls_script = format!(
                "(() => {{ try {{ const items = {ls_json}; for (const [k, v] of Object.entries(items)) {{ localStorage.setItem(k, String(v)); }} }} catch (e) {{}} }})()"
            );
            let _ = webview.eval(&ls_script);
        }
    }
    tokio::time::sleep(Duration::from_millis(450)).await;
    let target = if label.ends_with("-of") {
        "https://onlyfans.com/my/chats"
    } else {
        "https://onlyfans.com/"
    };
    let _ = webview.eval(&format!("location.replace({})", js_string(target)));
    true
}

fn eval_force_link(app: &AppHandle, label: &str) {
    const SCRIPT: &str = "window.__CF_FORCE_LINK && window.__CF_FORCE_LINK(true)";
    if let Some(webview) = app.get_webview(label) {
        let _ = webview.eval(SCRIPT);
    } else if let Some(window) = app.get_webview_window(label) {
        let _ = window.eval(SCRIPT);
    }
}

fn schedule_profile_link(app: &AppHandle, model_id: String, label: String) {
    if APP_QUITTING.load(Ordering::SeqCst) || label.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        for delay in [1200_u64, 3000, 7000, 14000, 26000] {
            if APP_QUITTING.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(delay));
            let app_eval = app.clone();
            let label_eval = label.clone();
            let app_thread = app_eval.clone();
            let _ = app_eval.run_on_main_thread(move || {
                eval_force_link(&app_thread, &label_eval);
            });
            let app_check = app.clone();
            let model_check = model_id.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            tauri::async_runtime::spawn(async move {
                let linked = if let Some(store) = app_check.try_state::<SupabaseStore>() {
                    store
                        .get_model(&model_check)
                        .await
                        .map(|model| model.of_user_id.is_some())
                        .unwrap_or(false)
                } else {
                    false
                };
                let _ = tx.send(linked);
            });
            if rx.recv_timeout(Duration::from_secs(8)).unwrap_or(false) {
                return;
            }
        }
    });
}

fn capture_script(model_id: &str, stay_in_chats: bool) -> String {
    let model_id_json = serde_json::to_string(model_id).unwrap_or_else(|_| "\"\"".to_owned());
    let dark = app_is_dark();
    format!(
        r##"
(() => {{
  const modelId = {model_id_json};
  const stayInChats = {stay_in_chats};
  let ofDark = {dark};
  window.__CF_FORCE_LINK = (force) => {{
    try {{
      if (!force && window.__CF_CREATOR_ID) return true;
      if (isLoginOrAuthSurface()) return false;
      if (window.__CF_LINK_BUSY) return false;
      const client = findSignedClient();
      if (!client) return false;
      window.__CF_LINK_BUSY = true;
      client.get("https://onlyfans.com/api2/v2/users/me").then((response) => {{
        window.__CF_LINK_BUSY = false;
        const status = (response && response.status) || 200;
        if (status >= 400) {{
          window.__CF_PROFILE_HYDRATED = false;
          return;
        }}
        const data = response && response.data;
        const text = typeof data === "string" ? data : JSON.stringify(data || {{}});
        window.__CF_PROFILE_HYDRATED = true;
        deliver("https://onlyfans.com/api2/v2/users/me", status, text);
      }}).catch(() => {{
        window.__CF_LINK_BUSY = false;
        window.__CF_PROFILE_HYDRATED = false;
      }});
      return true;
    }} catch (error) {{
      window.__CF_LINK_BUSY = false;
      return false;
    }}
  }};
  window.__CF_REPORT_VISIBLE = () => window.__CF_FORCE_LINK(true);
  setTimeout(() => window.__CF_FORCE_LINK(), 900);
  setInterval(() => {{
    if (!window.__CF_CREATOR_ID && !isLoginOrAuthSurface()) window.__CF_FORCE_LINK();
  }}, 2500);
  window.__CF_APPLY_THEME = (next) => {{
    ofDark = !!next;
    try {{
      const root = document.documentElement;
      if (root) {{
        root.style.colorScheme = ofDark ? "dark" : "light";
        root.classList.toggle("m-mode-dark", ofDark);
      }}
      if (document.body) document.body.classList.toggle("m-mode-dark", ofDark);
      localStorage.setItem("dark_mode", ofDark ? "true" : "false");
      localStorage.setItem("theme", ofDark ? "dark" : "light");
    }} catch (error) {{}}
  }};
  window.__CF_APPLY_THEME(ofDark);
  try {{
    document.addEventListener("DOMContentLoaded", () => window.__CF_APPLY_THEME(ofDark), {{ once: true }});
  }} catch (error) {{}}
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
      if (path === "/api2/v2/users/notifications" || path.indexOf("/api2/v2/users/notifications/") === 0) return true;
      if (path.indexOf("/api2/v2/chats/") !== -1 && path.indexOf("/messages") !== -1) return true;
      if (/^\/api2\/v2\/(chats|users)\/u?\d+\/(mute|unmute)\b/i.test(path)) return true;
      if (/^\/api2\/v2\/chats\/\d+$/.test(path)) return true;
      if (path.indexOf("/api2/v2/subscriptions") === 0) return true;
      if (path.indexOf("/api2/v2/vault/") === 0) return true;
      if (window.__CF_CREATOR_ID && path === "/api2/v2/users/" + window.__CF_CREATOR_ID + "/posts") return true;
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
  function hydrateProfile() {{
    if (window.__CF_CREATOR_ID) return;
    const attempt = (tries) => {{
      if (window.__CF_CREATOR_ID) return;
      if (window.__CF_FORCE_LINK && window.__CF_FORCE_LINK()) return;
      if (tries < 24) setTimeout(() => attempt(tries + 1), tries < 10 ? 750 : 2200);
    }};
    setTimeout(() => attempt(0), 500);
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
    if (existing) existing.remove();
  }}
  function deliver(url, status, body) {{
    const text = String(body || "");
    rememberCreator(url, text);
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
      if (p === "/api2/v2/chats" && String(url).indexOf("_cf_muted_list=1") !== -1) {{
        window.__CF_LAST_MUTED_CHATS_AT = Date.now();
      }} else if (p === "/api2/v2/chats" && String(url).indexOf("_cf_backfill") === -1) {{
        window.__CF_LAST_INBOX_AT = Date.now();
      }} else if (p === "/api2/v2/payouts/transactions") {{
        window.__CF_LAST_TX_AT = Date.now();
      }} else if (p.indexOf("/api2/v2/subscriptions") === 0) {{
        window.__CF_LAST_SUBS_AT = Date.now();
      }} else if (p === "/api2/v2/users/notifications" && String(url).indexOf("type=subscribed") !== -1) {{
        window.__CF_LAST_SUB_NOTIFS_AT = Date.now();
      }} else if (p === "/api2/v2/users/notifications" && (String(url).indexOf("type=purchases") !== -1 || String(url).indexOf("type=tips") !== -1)) {{
        window.__CF_LAST_SALE_NOTIFS_AT = Date.now();
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
  hydrateProfile();
  window.addEventListener("popstate", () => {{
    fanFromUrl();
    installVaultButton();
  }});
  fanFromUrl();
  let bgPollBusy = false;
  let lastWsRefreshAt = 0;
  async function runBgPoll(forceInbox, forceSubNotifs) {{
    if (bgPollBusy || isLoginOrAuthSurface()) return;
    if (/statements|statistics/i.test(location.pathname)) return;
    const client = findSignedClient();
    if (!client) return;
    const now = Date.now();
    const lastInbox = window.__CF_LAST_INBOX_AT || 0;
    const lastMutedChats = window.__CF_LAST_MUTED_CHATS_AT || 0;
    const lastTx = window.__CF_LAST_TX_AT || 0;
    const lastSubs = window.__CF_LAST_SUBS_AT || 0;
    const lastSubNotifs = window.__CF_LAST_SUB_NOTIFS_AT || 0;
    const lastSaleNotifs = window.__CF_LAST_SALE_NOTIFS_AT || 0;
    const needInbox = forceInbox || (now - lastInbox >= 1500);
    const needMutedChats = (now - lastMutedChats >= 90000);
    const needTx = (now - lastTx >= 14000);
    const needSubs = (now - lastSubs >= 30000);
    const needSubNotifs = forceSubNotifs || (now - lastSubNotifs >= 15000);
    const needSaleNotifs = (now - lastSaleNotifs >= 12000);
    if (!needInbox && !needMutedChats && !needTx && !needSubs && !needSubNotifs && !needSaleNotifs) return;
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
      if (needMutedChats) {{
        window.__CF_LAST_MUTED_CHATS_AT = Date.now();
        try {{
          const mutedUrl = "https://onlyfans.com/api2/v2/chats?limit=50&offset=0&filter=muted&_cf_muted_list=1";
          const res = await client.get(mutedUrl);
          if (res && res.data) {{
            const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
            deliver(mutedUrl, res.status || 200, text);
          }}
        }} catch (e) {{}}
      }}
      if (needInbox) {{
        window.__CF_LAST_INBOX_AT = Date.now();
        try {{
          const chatUrl = "https://onlyfans.com/api2/v2/chats?limit=10&offset=0&order=recent";
          const res = await client.get(chatUrl);
          if (res && res.data) {{
            const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
            deliver(chatUrl, res.status || 200, text);
          }}
        }} catch (e) {{}}
      }}
      if (needSubNotifs) {{
        window.__CF_LAST_SUB_NOTIFS_AT = Date.now();
        try {{
          const maxPages = window.__CF_SUB_NOTIFS_BACKFILLED ? 1 : 5;
          const cutoffMs = Date.now() - 36 * 3600 * 1000;
          let fromId = "";
          let fetchedAny = false;
          for (let page = 0; page < maxPages; page += 1) {{
            const notifUrl = fromId
              ? "https://onlyfans.com/api2/v2/users/notifications?limit=50&type=subscribed&fromId=" + encodeURIComponent(fromId)
              : "https://onlyfans.com/api2/v2/users/notifications?limit=50&type=subscribed";
            const res = await client.get(notifUrl);
            if (!res || !res.data) break;
            fetchedAny = true;
            const data = typeof res.data === "string" ? JSON.parse(res.data) : res.data;
            const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
            deliver(notifUrl, res.status || 200, text);
            const list = Array.isArray(data) ? data : ((data && (data.list || data.items || data.notifications)) || []);
            const hasMore = !!(data && data.hasMore);
            if (!hasMore || !Array.isArray(list) || list.length === 0) break;
            const lastItem = list[list.length - 1];
            const lastStamp = lastItem && (lastItem.createdAt || lastItem.created_at || lastItem.date);
            const lastMs = lastStamp ? Date.parse(String(lastStamp)) : NaN;
            if (Number.isFinite(lastMs) && lastMs < cutoffMs) break;
            const nextId = lastItem && lastItem.id != null ? String(lastItem.id) : "";
            if (!nextId || nextId === fromId) break;
            fromId = nextId;
            if (page + 1 < maxPages) {{
              await new Promise((resolve) => setTimeout(resolve, 300));
            }}
          }}
          if (fetchedAny) window.__CF_SUB_NOTIFS_BACKFILLED = true;
        }} catch (e) {{}}
      }}
      if (needSaleNotifs) {{
        window.__CF_LAST_SALE_NOTIFS_AT = Date.now();
        try {{
          for (const saleType of ["purchases", "tips"]) {{
            const saleUrl = "https://onlyfans.com/api2/v2/users/notifications?limit=50&type=" + saleType;
            const res = await client.get(saleUrl);
            if (res && res.data) {{
              const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
              deliver(saleUrl, res.status || 200, text);
            }}
          }}
        }} catch (e) {{}}
      }}
      if (needTx) {{
        window.__CF_LAST_TX_AT = Date.now();
        try {{
          const txUrl = "https://onlyfans.com/api2/v2/payouts/transactions?limit=50";
          const res = await client.get(txUrl);
          if (res && res.data) {{
            const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
            deliver(txUrl, res.status || 200, text);
          }}
        }} catch (e) {{}}
      }}
      if (needSubs) {{
        window.__CF_LAST_SUBS_AT = Date.now();
        try {{
          const subsUrl = "https://onlyfans.com/api2/v2/subscriptions/subscribers?limit=50&offset=0&type=active&sort=desc&field=last_activity";
          const res = await client.get(subsUrl);
          if (res && res.data) {{
            const text = typeof res.data === "string" ? res.data : JSON.stringify(res.data);
            deliver(subsUrl, res.status || 200, text);
          }}
        }} catch (e) {{}}
      }}
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
      rawText.indexOf("toast") !== -1 ||
      rawText.indexOf("subscrib") !== -1
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
      const isSubSignal =
        rawText.indexOf("subscrib") !== -1 ||
        rawText.indexOf("notification") !== -1 ||
        rawText.indexOf("toast") !== -1;
      const now = Date.now();
      if (now - lastWsRefreshAt >= 1200) {{
        lastWsRefreshAt = now;
        void runBgPoll(true, isSubSignal);
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
  function isMuteToggle(url) {{
    try {{
      const parsed = new URL(url, location.href);
      if (parsed.hostname !== "onlyfans.com" && !parsed.hostname.endsWith(".onlyfans.com")) return false;
      const path = parsed.pathname.replace(/\/+$/, "");
      return /^\/api2\/v2\/(chats|users)\/u?\d+\/(mute|unmute)$/i.test(path);
    }} catch (error) {{
      return false;
    }}
  }}
  function muteToggleUrl(method, url) {{
    const absoluteUrl = absolute(url);
    const verb = encodeURIComponent(String(method || "POST").toUpperCase());
    return absoluteUrl + (absoluteUrl.indexOf("?") === -1 ? "?" : "&") + "_cf_method=" + verb;
  }}
  const originalFetch = window.fetch;
  if (typeof originalFetch === "function") {{
    window.fetch = function () {{
      const input = arguments[0];
      const init = arguments[1];
      const url = typeof input === "string" ? input : (input && input.url) || "";
      const method = (init && init.method) || (input && input.method) || "GET";
      const pending = originalFetch.apply(this, arguments);
      const publish = isPublish(method, url);
      const muteToggle = isMuteToggle(url);
      if (!watched(url) && !publish && !muteToggle) {{
        return pending;
      }}
      return pending.then((response) => {{
        const target = publish ? publishUrl(url) : muteToggle ? muteToggleUrl(method, url) : url;
        response.clone().text().then((body) => deliver(target, response.status, body)).catch(() => {{}});
        return response;
      }});
    }};
  }}
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  function isPublish(method, url) {{
    try {{
      const parsed = new URL(url, location.href);
      if (parsed.hostname !== "onlyfans.com" && !parsed.hostname.endsWith(".onlyfans.com")) return false;
      const path = parsed.pathname.replace(/\/+$/, "");
      return String(method || "GET").toUpperCase() === "POST" && path === "/api2/v2/posts";
    }} catch (error) {{
      return false;
    }}
  }}
  function publishUrl(url) {{
    const absoluteUrl = absolute(url);
    return absoluteUrl + (absoluteUrl.indexOf("?") === -1 ? "?" : "&") + "_cf_publish=1";
  }}
  XMLHttpRequest.prototype.open = function (method, url) {{
    this.__cfUrl = url;
    this.__cfMethod = method;
    return originalOpen.apply(this, arguments);
  }};
  XMLHttpRequest.prototype.send = function () {{
    this.addEventListener("load", function () {{
      const url = this.__cfUrl || "";
      const publish = isPublish(this.__cfMethod, url);
      const muteToggle = isMuteToggle(url);
      if (!watched(url) && !publish && !muteToggle) {{
        return;
      }}
      let body = "";
      try {{
        if (!this.responseType || this.responseType === "text") {{
          body = typeof this.responseText === "string" ? this.responseText : "";
        }} else if (this.response != null) {{
          body = typeof this.response === "string" ? this.response : JSON.stringify(this.response);
        }}
      }} catch (error) {{
        body = "";
      }}
      const target = publish ? publishUrl(url) : muteToggle ? muteToggleUrl(this.__cfMethod, url) : url;
      deliver(target, this.status, body);
    }});
    return originalSend.apply(this, arguments);
  }};
  function fanFromLocation() {{
    const match = location.pathname.match(/\/my\/chats\/chat\/u?(\d+)/i);
    return usableFan(match && match[1]);
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
  function installChatLayoutStyles() {{
    if (document.getElementById("cf-chat-layout")) return;
    const style = document.createElement("style");
    style.id = "cf-chat-layout";
    style.textContent = ".b-chats__header .b-tabs__nav,.b-chats__header .b-tabs__nav__list,.b-chats .b-separate-filter{{display:flex!important;flex-wrap:nowrap!important;overflow-x:auto!important;overflow-y:hidden!important;max-width:100%!important;scrollbar-width:thin}}.b-chats__header .b-tabs__nav__item,.b-chats__header .b-tabs__nav__text{{flex:0 0 auto!important;max-width:none!important;white-space:nowrap!important;overflow:visible!important;text-overflow:clip!important}}";
    const target = document.head || document.documentElement;
    if (target) target.appendChild(style);
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
    installChatLayoutStyles();
    const widened = onEarnings ? collectEarnings() : false;
    const authReady = !!window.__CF_CREATOR_ID;
    if (authReady && !onChat) cleanComposerButtons();
    if (!onEarnings) {{
      void runBgPoll(false);
    }}
    return {{ fanId: onChat ? fanId : null, widened: widened, active: (onChat || onEarnings), creator: !!window.__CF_CREATOR_ID, ready: !!window.__CF_CREATOR_ID && !!window.__CF_HTTP, api: api, profile: null }};
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
    if (node.closest && node.closest("#cf-emoji-dock")) return false;
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

  const CHAT_EMOJIS = ["😀","😂","🥰","😍","😘","😉","😊","🔥","❤️","💋","👍","🙏","😭","🤔","😏","🎉","✨","💯","🌹","😈","😎","🤗"];
  function findSendButton() {{
    const buttons = document.querySelectorAll("button, [role='button']");
    for (const btn of buttons) {{
      if (btn.closest && btn.closest("#cf-emoji-dock")) continue;
      const text = (btn.innerText || btn.textContent || "").replace(/\s+/g, " ").trim().toLowerCase();
      const label = ((btn.getAttribute && (btn.getAttribute("aria-label") || btn.getAttribute("title"))) || "").trim().toLowerCase();
      if (text === "enviar" || text === "send" || label === "enviar" || label === "send") return btn;
    }}
    return null;
  }}
  function composerBar(send) {{
    let node = send.parentElement;
    for (let depth = 0; node && depth < 6; depth += 1) {{
      if (node.querySelectorAll("button, [role='button']").length >= 3) return node;
      node = node.parentElement;
    }}
    return send.parentElement;
  }}
  function composerInput(send) {{
    let node = send;
    for (let depth = 0; node && depth < 8; depth += 1) {{
      const input = node.querySelector && node.querySelector("[contenteditable='true'], textarea");
      if (input) return input;
      node = node.parentElement;
    }}
    return document.querySelector(".b-chat [contenteditable='true'], .b-chat textarea, [contenteditable='true']");
  }}
  function insertChatEmoji(emoji) {{
    const send = findSendButton();
    const target = send ? composerInput(send) : document.querySelector("[contenteditable='true'], textarea");
    if (!target) return;
    target.focus();
    try {{
      document.execCommand("insertText", false, emoji);
    }} catch (error) {{}}
    target.dispatchEvent(new InputEvent("input", {{ bubbles: true, data: emoji, inputType: "insertText" }}));
  }}
  function mountChatEmoji() {{
    const send = findSendButton();
    const existing = document.getElementById("cf-emoji-dock");
    if (!send) {{
      if (existing) existing.remove();
      return;
    }}
    const bar = composerBar(send);
    if (!bar) return;
    if (existing && existing.parentElement === bar) return;
    if (existing) existing.remove();
    const dock = document.createElement("span");
    dock.id = "cf-emoji-dock";
    dock.style.cssText = "position:relative;display:inline-flex;align-items:center;flex:0 0 auto;margin:0 4px 0 0;z-index:20;";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.textContent = "😀";
    toggle.title = "Emojis";
    toggle.setAttribute("aria-label", "Insertar emoji");
    toggle.style.cssText = "width:32px;height:32px;border:1px solid #c5d4f5;border-radius:8px;background:#e8f0ff;font-size:18px;line-height:30px;cursor:pointer;padding:0;";
    const panel = document.createElement("div");
    panel.style.cssText = "display:none;position:absolute;bottom:38px;left:0;width:228px;padding:8px;border-radius:12px;background:#111827;box-shadow:0 10px 24px rgba(0,0,0,.32);";
    for (const emoji of CHAT_EMOJIS) {{
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = emoji;
      button.style.cssText = "width:32px;height:32px;border:0;border-radius:8px;background:transparent;font-size:18px;cursor:pointer;";
      button.addEventListener("mousedown", (event) => event.preventDefault());
      button.addEventListener("click", (event) => {{
        event.preventDefault();
        event.stopPropagation();
        insertChatEmoji(emoji);
      }});
      panel.appendChild(button);
    }}
    toggle.addEventListener("mousedown", (event) => event.preventDefault());
    toggle.addEventListener("click", (event) => {{
      event.preventDefault();
      event.stopPropagation();
      panel.style.display = panel.style.display === "none" ? "grid" : "none";
    }});
    panel.style.gridTemplateColumns = "repeat(6, 32px)";
    panel.style.gap = "4px";
    dock.appendChild(panel);
    dock.appendChild(toggle);
    bar.insertBefore(dock, bar.firstChild);
  }}

  let cleanQueued = false;
  const runComposerPass = () => {{
    if (isLoginOrAuthSurface()) return;
    if (window.__CF_APPLY_THEME) window.__CF_APPLY_THEME(ofDark);
    cleanComposerButtons();
    mountChatEmoji();

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
    if (isLoginOrAuthSurface()) return;
    mountChatEmoji();
    if (!isChatPage()) runComposerPass();
  }}, 800);
  setInterval(() => {{
    if (onVaultPage()) installVaultButton();
  }}, 1200);
  installVaultButton();
  if (!window.__CF_API_BRIDGE_INSTALLED) {{
    window.__CF_API_BRIDGE_INSTALLED = true;
    window.__CF_API_CALL = async (reqId, method, path, payload) => {{
      const internals = window.__TAURI_INTERNALS__;
      const reply = (status, body, error) => {{
        if (internals && typeof internals.invoke === "function") {{
          internals.invoke("of_api_bridge_reply", {{
            reqId: String(reqId),
            status: Number(status || 0),
            body: String(body || ""),
            error: String(error || "")
          }}).catch(() => {{}});
        }}
      }};
      try {{
        const client = findSignedClient();
        if (!client) {{
          reply(0, "", "CLIENT_NOT_READY");
          return;
        }}
        const url = String(path || "").indexOf("http") === 0
          ? String(path)
          : "https://onlyfans.com" + (String(path || "").indexOf("/") === 0 ? "" : "/") + String(path || "");
        const upper = String(method || "GET").toUpperCase();
        const response = upper === "POST" ? await client.post(url, payload || {{}}) : await client.get(url);
        const data = response && response.data;
        const text = typeof data === "string" ? data : JSON.stringify(data || {{}});
        reply((response && response.status) || 200, text, "");
      }} catch (err) {{
        const status = err && err.response ? err.response.status : 0;
        const data = err && err.response ? err.response.data : null;
        const body = data ? (typeof data === "string" ? data : JSON.stringify(data)) : "";
        reply(status, body, status ? "" : String((err && err.message) || err));
      }}
    }};
  }}
}})();
"##
    )
}

fn spawn_reader(app: AppHandle, webview_label: String, model_id: String) {
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
            let Some(store) = app.try_state::<SupabaseStore>() else {
                continue;
            };
            let bus = app.try_state::<SyncBus>();
            apply_tick(
                &app,
                store.inner(),
                bus.as_deref(),
                &model_id,
                &webview_label,
                &raw,
            )
            .await;
        }
    });
}

async fn apply_tick(
    app: &AppHandle,
    store: &SupabaseStore,
    bus: Option<&SyncBus>,
    model_id: &str,
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
                let _ = record_capture(app, store, bus, label, model_id, url, status, body).await;
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
    if crate::sync::ingest_observation(store, model_id, &fan_id, Some(&profile))
        .await
        .is_ok()
    {
        let _ = app.emit(
            "fan-insights-changed",
            serde_json::json!({ "modelId": model_id, "ofFanId": fan_id }),
        );
        let _ = app.emit("models-changed", ());
    }
}

fn observation_changed(model_id: &str, fan_id: &str, signature: &str) -> bool {
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
        assert!(message_panes(0.0, 0.0, MessageBarPlacement::Top, true).is_none());
        assert!(message_panes(1280.0, 0.0, MessageBarPlacement::Sidebar, true).is_none());

        let top = message_panes(1280.0, 800.0, MessageBarPlacement::Top, true).unwrap();
        assert_eq!(top.tabs_x, 0.0);
        assert_eq!(top.tabs_y, 0.0);
        assert_eq!(top.tabs_width, 1280.0);
        assert_eq!(top.tabs_height, 44.0);
        assert_eq!(top.chat_x, 0.0);
        assert_eq!(top.chat_y, 44.0);
        assert_eq!(top.chat_height, 756.0);
        assert_eq!(top.chat_width, 980.0);
        assert_eq!(top.insights_x, 980.0);
        assert_eq!(top.insights_width, 300.0);
        let collapsed = message_panes(1280.0, 800.0, MessageBarPlacement::Top, false).unwrap();
        assert_eq!(collapsed.insights_width, 0.0);
        assert_eq!(collapsed.chat_width, 1280.0);

        let side = message_panes(1280.0, 800.0, MessageBarPlacement::Sidebar, true).unwrap();
        assert_eq!(side.tabs_x, 0.0);
        assert_eq!(side.tabs_y, 0.0);
        assert_eq!(side.tabs_width, 60.0);
        assert_eq!(side.tabs_height, 800.0);
        assert_eq!(side.chat_x, 60.0);
        assert_eq!(side.chat_y, 0.0);
        assert_eq!(side.chat_height, 800.0);
        assert!(side.chat_width > 600.0);
        assert_eq!(side.insights_x, side.chat_x + side.chat_width);
        assert_eq!(side.insights_height, 800.0);
    }

    #[test]
    fn capture_script_includes_clean_composer_and_auto_expiry() {
        let script = super::capture_script("model-uuid-42", false);
        assert!(script.contains("cleanComposerButtons"));
        assert!(script.contains("mountChatEmoji"));
        assert!(script.contains("__CF_APPLY_THEME"));
        assert!(script.contains("__CF_FORCE_LINK"));
        assert!(script.contains("__CF_REPORT_VISIBLE"));
        assert!(!script.contains("mountChatComposer"));
        assert!(!script.contains("pullPurchases"));
        assert!(!script.contains("pullFanStats"));
        assert!(!script.contains("observeMessages"));
        assert!(!script.contains("cf-msg-tools"));
        assert!(script.contains("/api2/v2/chats/"));
        assert!(script.contains("/api2/v2/users/notifications"));
        assert!(script.contains("type=subscribed"));
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
        assert!(script.contains("__CF_API_CALL"));
        assert!(script.contains("of_api_bridge_reply"));
        assert!(script.contains("findSignedClient"));
        assert!(!script.contains("__CF_FETCH_ATTEMPTED"));
        assert!(!script.contains("earnings/chart?startDate"));
    }

    #[test]
    fn statements_stay_inside_chats_until_sync_opens_them() {
        let blocked: tauri::Url = "https://onlyfans.com/my/statements/earnings".parse().unwrap();
        let chats: tauri::Url = "https://onlyfans.com/my/chats/chat/42".parse().unwrap();
        let model_id = "model-uuid-9000001";
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
        let script = super::capture_script("model-uuid-42", true);
        assert!(script.contains("function isChatPage()"));
        assert!(script.contains("if (isChatPage()) return false;"));
        assert!(script.contains("if (isChatPage()) return;"));
        assert!(!script.contains("createChatTools"));
        assert!(!script.contains("cf-msg-tools"));
        assert!(!script.contains("cf-msg-strip"));
    }

    #[test]
    fn capture_script_guards_null_document_element_at_document_start() {
        let script = super::capture_script("model-uuid-42", false);
        assert!(script.contains("const root = document.documentElement;\n      if (root) {"));
        assert!(script.contains("DOMContentLoaded"));
    }

    #[test]
    fn authenticated_cookies_require_both_auth_id_and_sess() {
        let guest_only = vec![serde_json::json!({ "name": "sess", "value": "guest-token" })];
        assert!(!super::has_authenticated_cookies(&guest_only));

        let zero_auth_id = vec![
            serde_json::json!({ "name": "sess", "value": "guest-token" }),
            serde_json::json!({ "name": "auth_id", "value": "0" }),
        ];
        assert!(!super::has_authenticated_cookies(&zero_auth_id));

        let real_session = vec![
            serde_json::json!({ "name": "sess", "value": "real-sess-token" }),
            serde_json::json!({ "name": "auth_id", "value": "412345678" }),
        ];
        assert!(super::has_authenticated_cookies(&real_session));
    }

    #[test]
    fn cookie_from_stored_defaults_domain_and_preserves_max_age() {
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let stored = serde_json::json!({
            "name": "auth_id",
            "value": "412345678",
            "path": "/",
            "secure": true,
            "httpOnly": false,
            "sameSite": "Lax",
            "expires": future,
        });
        let cookie = super::cookie_from_stored(&stored).expect("valid cookie");
        assert_eq!(cookie.name(), "auth_id");
        assert_eq!(cookie.domain(), Some("onlyfans.com"));
        assert!(cookie.max_age().is_some());
    }
}
