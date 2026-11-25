mod analytics;
mod commands;
mod dashboard;
pub mod db;
mod fans;
mod messages;
mod notifications;
mod sessions;
mod sync;
mod vault;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let database = tauri::async_runtime::block_on(db::Database::initialize(app_data_dir))?;
            app.manage(database);
            app.manage(sessions::SyncBus::new());
            app.manage(notifications::NotificationBus::new());
            sessions::warm_session_environment(app.handle());
            sessions::start_background_monitors(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::Destroyed = event {
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::sync_all_models,
            commands::get_analytics,
            commands::export_analytics,
            commands::get_earnings_overview,
            commands::get_my_shifts,
            commands::get_clocked_in_employees,
            commands::get_employee_sales,
            commands::get_employees,
            commands::add_employee,
            commands::update_employee_role_schedule,
            commands::delete_employee,
            commands::toggle_clock_in,
            commands::get_models,
            commands::open_model_session,
            commands::open_messages,
            commands::get_message_tabs,
            commands::set_message_bar_placement,
            commands::set_insights_open,
            commands::select_message_tab,
            commands::close_message_tab,
            commands::get_recent_notifications,
            commands::clear_notifications,
            commands::dismiss_notification,
            commands::open_incoming_chat,
            commands::set_notification_delivery,
            commands::save_fan_notes,
            commands::list_inbox,
            commands::list_thread,
            commands::send_message,
            commands::fan_selected,
            commands::delete_model,
            commands::sync_model_data,
            commands::ingest_of_response,
            commands::list_vault_media,
            commands::download_vault_media,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CreatorFloww");
}
