use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::notifications::{AppNotification, NotificationBus};
use crate::sessions::{self, MessageBarPlacement, ModelAccount, SyncBus, SyncReport};
use crate::supabase::SupabaseStore;
use crate::vault::{self, VaultDownloadFile, VaultDownloadReport, VaultPage};

#[tauri::command]
pub fn set_supabase_session(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    notifications: State<'_, NotificationBus>,
    access_token: Option<String>,
    user_id: Option<String>,
    role: Option<String>,
) {
    let had_session = store.has_session();
    let has_next = access_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty());
    let user_changed = store.set_session(access_token, user_id, role);
    if user_changed {
        notifications.reset_caches();
    }
    if has_next && !had_session {
        sessions::start_background_monitors(app);
    }
}

#[tauri::command]
pub async fn get_models(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
) -> Result<Vec<ModelAccount>, String> {
    sessions::list_accounts(&app, &store).await
}

#[tauri::command]
pub async fn refresh_models(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
) -> Result<Vec<ModelAccount>, String> {
    let previous = store.cached_model_ids();
    store.invalidate_model_list();
    let accounts = sessions::list_accounts(&app, &store).await?;
    let next: Vec<String> = accounts.iter().map(|account| account.model.id.clone()).collect();
    for removed in previous.iter().filter(|id| !next.contains(id)) {
        sessions::revoke_model_access(&app, removed).await;
    }
    if previous != next {
        let _ = app.emit("models-changed", ());
    }
    Ok(accounts)
}

#[tauri::command]
pub fn set_app_theme(app: AppHandle, dark: bool) {
    sessions::apply_app_theme(&app, dark);
}

#[tauri::command]
pub async fn open_model_session(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    model_id: Option<String>,
) -> Result<ModelAccount, String> {
    let account = sessions::open_model_session(&app, &store, model_id).await?;
    let _ = app.emit("models-changed", ());
    Ok(account)
}

#[tauri::command]
pub async fn open_messages(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    model_id: String,
) -> Result<(), String> {
    sessions::open_messages(&app, &store, &model_id).await
}

#[tauri::command]
pub async fn get_message_tabs(app: AppHandle) -> Result<sessions::MessageTabsState, String> {
    Ok(sessions::get_message_tabs(&app).await)
}

#[tauri::command]
pub async fn set_message_bar_placement(
    app: AppHandle,
    placement: MessageBarPlacement,
) -> Result<sessions::MessageTabsState, String> {
    sessions::set_message_bar_placement(&app, placement).await
}

#[tauri::command]
pub async fn set_insights_open(
    app: AppHandle,
    open: bool,
) -> Result<sessions::MessageTabsState, String> {
    sessions::set_insights_open(&app, open).await
}

#[tauri::command]
pub async fn get_recent_notifications(
    store: State<'_, SupabaseStore>,
    notifications: State<'_, NotificationBus>,
) -> Result<Vec<AppNotification>, String> {
    notifications.recent(&store).await
}

#[tauri::command]
pub async fn clear_notifications(
    store: State<'_, SupabaseStore>,
    notifications: State<'_, NotificationBus>,
) -> Result<(), String> {
    notifications.clear_history(&store).await
}

#[tauri::command]
pub async fn dismiss_notification(
    store: State<'_, SupabaseStore>,
    notifications: State<'_, NotificationBus>,
    id: String,
) -> Result<(), String> {
    notifications.dismiss(&store, &id).await
}

#[tauri::command]
pub async fn open_incoming_chat(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    model_id: String,
    of_fan_id: String,
) -> Result<(), String> {
    sessions::open_incoming_chat(&app, &store, &model_id, &of_fan_id).await
}

#[tauri::command]
pub fn set_notification_delivery(
    notifications: State<'_, NotificationBus>,
    messages_enabled: bool,
    sales_enabled: bool,
    os_push_enabled: bool,
) -> Result<(), String> {
    notifications.set_delivery(messages_enabled, sales_enabled, os_push_enabled);
    Ok(())
}

#[tauri::command]
pub async fn select_message_tab(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    model_id: String,
) -> Result<(), String> {
    sessions::select_message_tab(&app, &store, &model_id).await
}

#[tauri::command]
pub async fn close_message_tab(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    model_id: String,
) -> Result<(), String> {
    sessions::close_message_tab(&app, Some(&store), &model_id).await
}

#[tauri::command]
pub async fn delete_model(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    id: String,
) -> Result<(), String> {
    sessions::delete_model_session(&app, &store, &id).await
}

#[tauri::command]
pub async fn sync_model_data(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    bus: State<'_, SyncBus>,
    id: String,
) -> Result<SyncReport, String> {
    sessions::sync_model_data(&app, &store, &bus, &id).await
}

#[tauri::command]
pub fn fan_selected(app: AppHandle, model_id: String, of_fan_id: String) -> Result<(), String> {
    sessions::note_viewing_fan(&model_id, &of_fan_id);
    app.emit(
        "fan-selected",
        serde_json::json!({ "modelId": model_id, "ofFanId": of_fan_id }),
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_vault_media(
    app: AppHandle,
    model_id: String,
    offset: u32,
    kind: String,
) -> Result<VaultPage, String> {
    vault::list_vault_media(&app, &model_id, offset, &kind).await
}

#[tauri::command]
pub async fn download_vault_media(
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    model_id: String,
    files: Vec<VaultDownloadFile>,
) -> Result<VaultDownloadReport, String> {
    vault::download_vault_media(&app, &store, &model_id, files).await
}

#[tauri::command]
pub async fn ingest_of_response(
    window: WebviewWindow,
    app: AppHandle,
    store: State<'_, SupabaseStore>,
    bus: State<'_, SyncBus>,
    model_id: String,
    url: String,
    status: u16,
    body: String,
) -> Result<(), String> {
    sessions::record_capture(
        &app,
        &store,
        &bus,
        window.label(),
        &model_id,
        &url,
        status,
        &body,
    )
    .await
}
