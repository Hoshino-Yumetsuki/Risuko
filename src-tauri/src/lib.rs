mod commands;
mod config;
mod engine;
mod managers;
mod state;

use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--opened-at-login=1"]),
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let app_state = state::AppState::new(app.handle())?;
            app.manage(app_state);

            // Windows/Linux use a custom title bar, so disable native decorations.
            // macOS keeps decorations and uses `titleBarStyle: Overlay`.
            #[cfg(not(target_os = "macos"))]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_decorations(false);
            }

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = engine::start_engine(&handle).await {
                    log::error!("Failed to start aria2 engine: {}", e);
                }
            });

            managers::menu::setup_menu(app)?;
            managers::tray::setup_tray(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_cmds::get_app_config,
            commands::config_cmds::save_preference,
            commands::app_cmds::relaunch_app,
            commands::app_cmds::quit_app,
            commands::app_cmds::show_window,
            commands::app_cmds::hide_window,
            commands::app_cmds::factory_reset,
            commands::app_cmds::check_for_updates,
            commands::app_cmds::reset_session,
            commands::app_cmds::auto_hide_window,
            commands::file_cmds::reveal_in_folder,
            commands::file_cmds::open_path,
            commands::file_cmds::trash_item,
            commands::engine_cmds::restart_engine,
            commands::engine_cmds::get_engine_status,
            commands::event_cmds::on_download_status_change,
            commands::event_cmds::on_speed_change,
            commands::event_cmds::on_progress_change,
            commands::event_cmds::on_task_download_complete,
            commands::event_cmds::update_tray,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Motrix");
}
