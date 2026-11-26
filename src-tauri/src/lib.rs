mod commands;
mod notifications;
mod sessions;
pub mod supabase;
mod sync;
mod vault;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            app.manage(supabase::SupabaseStore::new());
            app.manage(sessions::SyncBus::new());
            app.manage(notifications::NotificationBus::new());
            sessions::warm_session_environment(app.handle());
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
            commands::set_supabase_session,
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
