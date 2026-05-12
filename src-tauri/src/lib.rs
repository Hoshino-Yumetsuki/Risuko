mod bridge;
pub mod cli;
mod commands;
mod managers;
mod state;

// Re-export risuko_engine modules for use by commands and other app code
pub use risuko_engine::config;
pub use risuko_engine::engine;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::{Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use risuko_engine::engine::rss::RssManager;

/// Set up tracing subscriber with both stdout and file output.
/// Returns a guard that must be held for the lifetime of the application.
fn init_logging(
    log_dir: &std::path::Path,
    log_level: &str,
) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    if let Err(e) = std::fs::create_dir_all(log_dir) {
        eprintln!("Failed to create log directory {:?}: {}", log_dir, e);
    }

    let file_appender = tracing_appender::rolling::daily(log_dir, "risuko.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("warn"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();

    guard
}

/// Apply environment workarounds for known WebKitGTK/Linux rendering issues
/// before any GTK/WebKit code initializes. Only sets variables that are not
/// already defined, so users can override.
#[cfg(target_os = "linux")]
fn apply_linux_webkit_workarounds() {
    // WebKitGTK's DMA-BUF renderer (default since 2.42) frequently fails to
    // initialize EGL/GBM on NVIDIA, virtualized GPUs, Wayland sessions without
    // a working GBM backend, or older Mesa stacks. The failure manifests as:
    //   "Could not create GBM EGL display: EGL_NOT_INITIALIZED. Aborting..."
    // followed by SIGABRT before the window is shown.
    // Disabling DMA-BUF forces the legacy GLES path which is far more
    // compatible. See: https://github.com/tauri-apps/tauri/issues/9304
    for (key, value) in [
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
    ] {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    }
}

pub fn run() {
    #[cfg(target_os = "linux")]
    apply_linux_webkit_workarounds();

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
            let handle = app.handle();

            // Create Tauri-backed trait implementations
            let config_dir_provider = bridge::TauriConfigDir::new(handle);
            let event_sink: Arc<dyn risuko_engine::EventSink> =
                Arc::new(bridge::TauriEventSink::new(handle));
            let storage: Arc<dyn risuko_engine::StorageBackend> =
                Arc::new(bridge::TauriStorage::new(handle));

            // Resolve log directory and read log level from user config before AppState
            let log_dir = handle.path().app_log_dir().unwrap_or_else(|_| {
                handle
                    .path()
                    .app_config_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
                    .join("logs")
            });
            let config = risuko_engine::config::ConfigManager::new(&config_dir_provider)
                .map_err(|e| e.to_string())?;
            let log_level = config
                .get_user_config()
                .get("log-level")
                .and_then(|v| v.as_str())
                .unwrap_or("warn")
                .to_string();

            let log_guard = init_logging(&log_dir, &log_level);
            log::info!("Log directory: {}", log_dir.display());

            let app_state = state::AppState::new(config, storage.clone(), log_dir, log_guard)?;
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

            let config_guard = app.state::<state::AppState>();
            let should_start = {
                let config = config_guard.config.lock().unwrap();
                risuko_engine::engine::should_start_embedded_engine(&config)
            };

            // Push the live Tauri event sink into the upload manager so
            // upload events reach the frontend (we constructed it with a
            // NoopEventSink before AppHandle was available). This must run
            // regardless of whether the embedded engine auto-starts so that
            // sink configuration UI receives upload events when the user
            // starts the engine later.
            //
            // Fail fast on a poisoned lock or missing manager: the previous
            // `lock().ok().and_then(...)` swallowed both, leaving the app
            // running with no upload event plumbing and no obvious symptom
            let event_sink_clone = event_sink.clone();
            let upload_mgr = {
                let state = app.state::<state::AppState>();
                let guard = state
                    .upload_sinks
                    .lock()
                    .map_err(|e| format!("upload_sinks lock poisoned: {e}"))?;
                guard
                    .clone()
                    .ok_or_else(|| "upload_sinks not initialized".to_string())?
            };
            upload_mgr.set_event_sink(event_sink_clone.clone());
            let upload_mgr = Some(upload_mgr);

            // Inject SFTP/FTP/WebDAV/S3 secrets stored in the OS keychain
            // back into the upload manager. The on-disk records omit them
            // (`skip_serializing` on the protocol Configs) so without this
            // pass the user has to re-enter every password after a restart
            if let Some(mgr) = upload_mgr.clone() {
                let state = app.state::<state::AppState>();
                let vault = state.vault.clone();
                tauri::async_runtime::block_on(commands::upload_cmds::rehydrate_upload_sinks(
                    &mgr, &vault,
                ));
            }

            if should_start {
                let config_ref = config_guard.config.lock().unwrap();
                let config_dir = config_ref.config_dir().to_path_buf();
                drop(config_ref);

                let storage_clone = storage.clone();
                tauri::async_runtime::spawn(async move {
                    let config = match risuko_engine::config::ConfigManager::with_dir(config_dir) {
                        Ok(c) => c,
                        Err(e) => {
                            log::error!("Failed to create ConfigManager: {}", e);
                            return;
                        }
                    };
                    if let Err(e) = risuko_engine::engine::start_engine(
                        &config,
                        event_sink_clone,
                        storage_clone,
                        upload_mgr,
                    )
                    .await
                    {
                        log::error!("Failed to start engine: {}", e);
                    }
                });
            }

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
                    // Must spawn into Tauri's async runtime so the tokio
                    // reactor is available for the inner tokio::spawn call.
                    tauri::async_runtime::spawn(async move {
                        std::mem::drop(RssManager::start_polling(rss));
                    });
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
                let _ = commands::app_cmds::hide_main_window(window.app_handle());
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
            commands::health_cmds::run_health_checks,
            commands::engine_cmds::add_uri,
            commands::engine_cmds::add_youtube,
            commands::engine_cmds::get_youtube_video_info,
            commands::engine_cmds::add_torrent_by_path,
            commands::engine_cmds::add_torrents_by_paths,
            commands::engine_cmds::resolve_magnet,
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
            commands::engine_cmds::list_routing_rules,
            commands::engine_cmds::add_routing_rule,
            commands::engine_cmds::update_routing_rule,
            commands::engine_cmds::remove_routing_rule,
            commands::engine_cmds::resolve_routing,
            commands::event_cmds::on_download_status_change,
            commands::event_cmds::set_sleep_inhibit_flag,
            commands::event_cmds::on_speed_change,
            commands::event_cmds::on_progress_change,
            commands::event_cmds::on_task_download_complete,
            commands::completion_script_cmds::run_completion_script,
            commands::completion_script_cmds::test_completion_script,
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
            commands::rss_cmds::update_rss_rule,
            commands::rss_cmds::reorder_rss_rules,
            commands::rss_cmds::dry_run_rss_rule,
            commands::rss_cmds::parse_rss_item_title,
            commands::rss_cmds::remove_rss_rule,
            commands::rss_cmds::get_rss_rules,
            commands::rss_cmds::download_rss_item,
            commands::rss_cmds::delete_rss_items,
            commands::rss_cmds::mark_rss_downloaded,
            commands::rss_cmds::clear_rss_download,
            commands::rss_cmds::read_rss_download,
            commands::rss_cmds::download_rss_item_tracked,
            commands::rss_cmds::mark_rss_item_read,
            commands::rss_cmds::mark_rss_items_read,
            commands::upload_cmds::list_upload_sinks,
            commands::upload_cmds::add_upload_sink,
            commands::upload_cmds::update_upload_sink,
            commands::upload_cmds::remove_upload_sink,
            commands::upload_cmds::test_upload_sink,
            commands::upload_cmds::get_default_upload_sink,
            commands::upload_cmds::set_default_upload_sink,
            commands::upload_cmds::set_upload_max_concurrency,
            commands::upload_cmds::list_upload_rules,
            commands::upload_cmds::add_upload_rule,
            commands::upload_cmds::update_upload_rule,
            commands::upload_cmds::remove_upload_rule,
            commands::upload_cmds::list_upload_jobs,
            commands::upload_cmds::cancel_upload_job,
            commands::upload_cmds::clear_upload_history,
            commands::vault_cmds::vault_status,
            commands::vault_cmds::vault_put_credential,
            commands::vault_cmds::vault_get_credential,
            commands::vault_cmds::vault_remove_credential,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Risuko");

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
