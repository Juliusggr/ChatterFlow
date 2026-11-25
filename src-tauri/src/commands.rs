use serde_json::Value;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::analytics::{self, AnalyticsRange, AnalyticsReport, ExportResult, TrendBucket};
use crate::dashboard::{self, ClockedInEmployee, EarningsBasis, EarningsOverview, EmployeeSale, ShiftRow};
use crate::fans;
use crate::messages::{self, ChatMessage, InboxChat, OutboundDraft};
use crate::db::{Database, Employee, EmployeeRole};
use crate::notifications::{AppNotification, NotificationBus};
use crate::sessions::{self, MessageBarPlacement, ModelAccount, SyncBus, SyncReport};
use crate::vault::{self, VaultDownloadFile, VaultDownloadReport, VaultPage};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncAllReport {
    pub synced: u32,
    pub skipped: u32,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn sync_all_models(
    app: AppHandle,
    database: State<'_, Database>,
    bus: State<'_, SyncBus>,
) -> Result<SyncAllReport, String> {
    let models = database.list_models().map_err(|err| err.to_string())?;
    let mut report = SyncAllReport {
        synced: 0,
        skipped: 0,
        errors: Vec::new(),
    };
    for model in models {
        if model.of_user_id.is_none() {
            report.skipped += 1;
            continue;
        }
        match sessions::sync_model_data(&app, &database, &bus, model.id).await {
            Ok(_) => report.synced += 1,
            Err(err) => report.errors.push(format!(
                "{}: {err}",
                if model.username.is_empty() { "Unknown".to_owned() } else { model.username }
            )),
        }
    }
    Ok(report)
}

#[tauri::command]
pub fn get_analytics(
    database: State<'_, Database>,
    range: AnalyticsRange,
    model_ids: Vec<i64>,
    compare_ids: Vec<i64>,
    net: bool,
    bucket: TrendBucket,
) -> Result<AnalyticsReport, String> {
    analytics::report(&database, range, &model_ids, &compare_ids, net, bucket)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn export_analytics(
    database: State<'_, Database>,
    range: AnalyticsRange,
    model_ids: Vec<i64>,
    format: String,
) -> Result<ExportResult, String> {
    analytics::export_report(&database, range, &model_ids, &format).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_earnings_overview(
    database: State<'_, Database>,
    range: dashboard::DashboardRange,
    basis: EarningsBasis,
) -> Result<EarningsOverview, String> {
    dashboard::earnings_overview(&database, range, basis).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_my_shifts(
    database: State<'_, Database>,
    range: dashboard::DashboardRange,
    basis: EarningsBasis,
) -> Result<Vec<ShiftRow>, String> {
    dashboard::my_shifts(&database, range, basis).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_clocked_in_employees(
    database: State<'_, Database>,
) -> Result<Vec<ClockedInEmployee>, String> {
    dashboard::clocked_in_employees(&database).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_employee_sales(
    database: State<'_, Database>,
    range: dashboard::DashboardRange,
    basis: EarningsBasis,
) -> Result<Vec<EmployeeSale>, String> {
    dashboard::employee_sales(&database, range, basis).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_employees(database: State<'_, Database>) -> Result<Vec<Employee>, String> {
    database.list_employees().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn add_employee(
    database: State<'_, Database>,
    name: String,
    role: EmployeeRole,
    schedule_json: Value,
) -> Result<Employee, String> {
    database
        .add_employee(&name, role, &schedule_json)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn update_employee_role_schedule(
    database: State<'_, Database>,
    id: i64,
    role: EmployeeRole,
    schedule_json: Value,
) -> Result<Employee, String> {
    database
        .update_employee_role_schedule(id, role, &schedule_json)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn delete_employee(database: State<'_, Database>, id: i64) -> Result<(), String> {
    database.delete_employee(id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn toggle_clock_in(database: State<'_, Database>, id: i64) -> Result<Employee, String> {
    database.toggle_clock_in(id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_models(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<ModelAccount>, String> {
    sessions::list_accounts(&app, &database)
}

#[tauri::command]
pub async fn open_model_session(
    app: AppHandle,
    database: State<'_, Database>,
    model_id: Option<i64>,
) -> Result<ModelAccount, String> {
    let account = sessions::open_model_session(&app, &database, model_id)?;
    let _ = app.emit("models-changed", ());
    Ok(account)
}

#[tauri::command]
pub async fn open_messages(
    app: AppHandle,
    database: State<'_, Database>,
    model_id: i64,
) -> Result<(), String> {
    sessions::open_messages(&app, &database, model_id)
}

#[tauri::command]
pub fn get_message_tabs(app: AppHandle) -> Result<sessions::MessageTabsState, String> {
    Ok(sessions::get_message_tabs(&app))
}

#[tauri::command]
pub fn set_message_bar_placement(
    app: AppHandle,
    placement: MessageBarPlacement,
) -> Result<sessions::MessageTabsState, String> {
    sessions::set_message_bar_placement(&app, placement)
}

#[tauri::command]
pub fn set_insights_open(app: AppHandle, open: bool) -> Result<sessions::MessageTabsState, String> {
    sessions::set_insights_open(&app, open)
}

#[tauri::command]
pub fn get_recent_notifications(
    database: State<'_, Database>,
    notifications: State<'_, NotificationBus>,
) -> Result<Vec<AppNotification>, String> {
    notifications.recent(&database)
}

#[tauri::command]
pub fn clear_notifications(
    database: State<'_, Database>,
    notifications: State<'_, NotificationBus>,
) -> Result<(), String> {
    notifications.clear_history(&database)
}

#[tauri::command]
pub fn dismiss_notification(
    database: State<'_, Database>,
    notifications: State<'_, NotificationBus>,
    id: String,
) -> Result<(), String> {
    notifications.dismiss(&database, &id)
}

#[tauri::command]
pub fn open_incoming_chat(
    app: AppHandle,
    database: State<'_, Database>,
    model_id: i64,
    of_fan_id: String,
) -> Result<(), String> {
    sessions::open_incoming_chat(&app, &database, model_id, &of_fan_id)
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
    database: State<'_, Database>,
    model_id: i64,
) -> Result<(), String> {
    sessions::select_message_tab(&app, &database, model_id)
}

#[tauri::command]
pub async fn close_message_tab(
    app: AppHandle,
    database: State<'_, Database>,
    model_id: i64,
) -> Result<(), String> {
    sessions::close_message_tab(&app, Some(&database), model_id)
}

#[tauri::command]
pub fn delete_model(app: AppHandle, database: State<'_, Database>, id: i64) -> Result<(), String> {
    sessions::delete_model_session(&app, &database, id)
}

#[tauri::command]
pub async fn sync_model_data(
    app: AppHandle,
    database: State<'_, Database>,
    bus: State<'_, SyncBus>,
    id: i64,
) -> Result<SyncReport, String> {
    sessions::sync_model_data(&app, &database, &bus, id).await
}

#[tauri::command]
pub fn list_inbox(database: State<'_, Database>, model_id: i64) -> Result<Vec<InboxChat>, String> {
    messages::list_inbox(&database, model_id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn list_thread(
    database: State<'_, Database>,
    model_id: i64,
    of_fan_id: String,
) -> Result<Vec<ChatMessage>, String> {
    messages::list_thread(&database, model_id, &of_fan_id).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn send_message(
    database: State<'_, Database>,
    model_id: i64,
    of_fan_id: String,
    draft: OutboundDraft,
) -> Result<ChatMessage, String> {
    messages::send_message(&database, model_id, &of_fan_id, draft).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn save_fan_notes(
    database: State<'_, Database>,
    model_id: i64,
    of_fan_id: String,
    notes_text: String,
) -> Result<(), String> {
    fans::save_fan_notes(&database, model_id, &of_fan_id, &notes_text).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn fan_selected(app: AppHandle, model_id: i64, of_fan_id: String) -> Result<(), String> {
    sessions::note_viewing_fan(model_id, &of_fan_id);
    app.emit(
        "fan-selected",
        serde_json::json!({ "modelId": model_id, "ofFanId": of_fan_id }),
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn list_vault_media(app: AppHandle, model_id: i64, offset: u32, kind: String) -> Result<VaultPage, String> {
    vault::list_vault_media(&app, model_id, offset, &kind).await
}

#[tauri::command]
pub async fn download_vault_media(
    app: AppHandle,
    database: State<'_, Database>,
    model_id: i64,
    files: Vec<VaultDownloadFile>,
) -> Result<VaultDownloadReport, String> {
    vault::download_vault_media(&app, &database, model_id, files).await
}

#[tauri::command]
pub fn ingest_of_response(
    window: WebviewWindow,
    app: AppHandle,
    database: State<'_, Database>,
    bus: State<'_, SyncBus>,
    model_id: i64,
    url: String,
    status: u16,
    body: String,
) -> Result<(), String> {
    sessions::record_capture(
        &app,
        &database,
        &bus,
        window.label(),
        model_id,
        &url,
        status,
        &body,
    )
}
