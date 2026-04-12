pub mod cli;
mod commands;
pub mod config;
pub mod engine;
mod managers;
mod state;

use std::sync::atomic::Ordering;

use tauri::{Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

pub fn run() {
    tracing_subscriber::fmt().init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_nosleep::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--opened-at-login=1"]),
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();

                if let Some(path) = args.get(1) {
                    if std::path::Path::new(path).exists() {
                        log::info!("Single instance received file: {}", path);
                        let _ = window.emit("open-file", path);
                    }
                }
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let app_state = state::AppState::new(app.handle())?;
            app.manage(app_state);
            sync_open_at_login_setting(app);

            // Windows/Linux use a custom title bar, so disable native decorations
            // macOS keeps decorations and uses `titleBarStyle: Overlay`
            #[cfg(not(target_os = "macos"))]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_decorations(false);
            }

            let opened_at_login = std::env::args().any(|arg| arg == "--opened-at-login=1");
            if opened_at_login {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = engine::start_engine(&handle).await {
                    log::error!("Failed to start engine: {}", e);
                }
            });

            managers::menu::setup_menu(app)?;

            // On non-macOS, respect the hide-app-menu user preference
            #[cfg(not(target_os = "macos"))]
            {
                let hide_menu = app
                    .state::<state::AppState>()
                    .config
                    .lock()
                    .ok()
                    .and_then(|cfg| {
                        cfg.get_user_config()
                            .get("hide-app-menu")
                            .and_then(|v| v.as_bool())
                    })
                    .unwrap_or(true);
                if hide_menu {
                    let _ = app.handle().remove_menu();
                }
            }

            managers::tray::setup_tray(app)?;

            // Start RSS background polling
            if let Ok(guard) = app.state::<state::AppState>().rss.lock() {
                if let Some(rss) = guard.clone() {
                    engine::rss::RssManager::start_polling(rss);
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let quitting = window
                    .app_handle()
                    .state::<state::AppState>()
                    .is_quitting
                    .load(Ordering::SeqCst);
                if quitting {
                    return;
                }
                api.prevent_close();
                let _ = commands::app_cmds::hide_main_window(&window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_cmds::get_app_config,
            commands::config_cmds::save_preference,
            commands::config_cmds::prepare_preference_patch,
            commands::app_cmds::relaunch_app,
            commands::app_cmds::quit_app,
            commands::app_cmds::show_window,
            commands::app_cmds::hide_window,
            commands::app_cmds::factory_reset,
            commands::app_cmds::check_for_updates,
            commands::app_cmds::reset_session,
            commands::app_cmds::auto_hide_window,
            commands::app_cmds::toggle_app_menu,
            commands::app_cmds::is_opened_at_login,
            commands::file_cmds::reveal_in_folder,
            commands::file_cmds::open_path,
            commands::file_cmds::trash_item,
            commands::file_cmds::rename_path,
            commands::file_cmds::read_binary_file,
            commands::file_cmds::resolve_torrent_path,
            commands::file_cmds::trash_generated_torrent_sidecars,
            commands::file_cmds::cleanup_generated_torrent_sidecars_for_task,
            commands::engine_cmds::restart_engine,
            commands::engine_cmds::get_engine_status,
            commands::engine_cmds::add_uri,
            commands::engine_cmds::add_torrent_by_path,
            commands::engine_cmds::probe_m3u8,
            commands::engine_cmds::calculate_active_task_progress,
            commands::engine_cmds::evaluate_low_speed_tasks,
            commands::engine_cmds::plan_auto_retry,
            commands::engine_cmds::sync_selected_task_order,
            commands::engine_cmds::tell_status,
            commands::engine_cmds::tell_active,
            commands::engine_cmds::tell_waiting,
            commands::engine_cmds::tell_stopped,
            commands::engine_cmds::pause_task,
            commands::engine_cmds::unpause_task,
            commands::engine_cmds::remove_task,
            commands::engine_cmds::change_option,
            commands::engine_cmds::change_global_option_engine,
            commands::engine_cmds::get_option_engine,
            commands::engine_cmds::get_global_option_engine,
            commands::engine_cmds::get_global_stat,
            commands::engine_cmds::change_position,
            commands::engine_cmds::save_session,
            commands::engine_cmds::get_version,
            commands::engine_cmds::pause_all_tasks,
            commands::engine_cmds::unpause_all_tasks,
            commands::engine_cmds::purge_download_result,
            commands::engine_cmds::remove_download_result,
            commands::engine_cmds::get_peers,
            commands::engine_cmds::multicall_engine,
            commands::engine_cmds::infer_out_from_uri,
            commands::engine_cmds::resolve_file_category,
            commands::event_cmds::on_download_status_change,
            commands::event_cmds::on_speed_change,
            commands::event_cmds::on_progress_change,
            commands::event_cmds::on_task_download_complete,
            commands::event_cmds::update_tray,
            commands::event_cmds::update_tray_menu_labels,
            commands::event_cmds::update_app_menu_labels,
            commands::rss_cmds::add_rss_feed,
            commands::rss_cmds::remove_rss_feed,
            commands::rss_cmds::refresh_rss_feed,
            commands::rss_cmds::refresh_all_rss_feeds,
            commands::rss_cmds::get_rss_feeds,
            commands::rss_cmds::get_rss_items,
            commands::rss_cmds::update_rss_feed_settings,
            commands::rss_cmds::add_rss_rule,
            commands::rss_cmds::remove_rss_rule,
            commands::rss_cmds::get_rss_rules,
            commands::rss_cmds::download_rss_item,
            commands::rss_cmds::delete_rss_items,
            commands::rss_cmds::mark_rss_downloaded,
            commands::rss_cmds::clear_rss_download,
            commands::rss_cmds::read_rss_download,
            commands::rss_cmds::download_rss_item_tracked,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Motrix");

    app.run(|_, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            commands::event_cmds::cleanup_download_inhibit();
        }
    });
}

fn sync_open_at_login_setting(app: &tauri::App) {
    let desired = app
        .state::<state::AppState>()
        .config
        .lock()
        .ok()
        .and_then(|cfg| {
            cfg.get_user_config()
                .get("open-at-login")
                .and_then(|v| v.as_bool())
        });

    let Some(desired) = desired else {
        return;
    };

    let autolaunch = app.autolaunch();
    let needs_update = match autolaunch.is_enabled() {
        Ok(current) => current != desired,
        Err(_) => true,
    };

    if !needs_update {
        return;
    }

    let result = if desired {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };

    if let Err(err) = result {
        log::warn!("Failed to sync open-at-login setting: {}", err);
    }
}
