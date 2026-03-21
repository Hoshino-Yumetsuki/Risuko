use tauri::{AppHandle, Manager};

#[tauri::command]
pub async fn relaunch_app(handle: AppHandle) -> Result<(), String> {
    crate::engine::stop_engine(&handle)
        .await
        .map_err(|e| e.to_string())?;
    handle.restart();
}

#[tauri::command]
pub async fn quit_app(handle: AppHandle) -> Result<(), String> {
    crate::engine::stop_engine(&handle)
        .await
        .map_err(|e| e.to_string())?;
    handle.exit(0);
    Ok(())
}

#[tauri::command]
pub fn show_window(handle: AppHandle) -> Result<(), String> {
    if let Some(window) = handle.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn hide_window(handle: AppHandle) -> Result<(), String> {
    if let Some(window) = handle.get_webview_window("main") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn factory_reset(
    handle: AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().map_err(|e| e.to_string())?;
    config.reset()?;
    drop(config);
    handle.restart();
}

#[tauri::command]
pub fn check_for_updates() -> Result<(), String> {
    Err("Update checking is not implemented for this build".to_string())
}

#[tauri::command]
pub async fn reset_session(handle: AppHandle) -> Result<(), String> {
    crate::engine::stop_engine(&handle)
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(config_dir) = handle.path().app_config_dir() {
        let session_path = config_dir.join(crate::engine::SESSION_FILENAME);
        let _ = std::fs::remove_file(&session_path);
    }
    crate::engine::start_engine(&handle)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn auto_hide_window(enabled: bool) -> Result<(), String> {
    log::info!("auto_hide_window: {}", enabled);
    Ok(())
}
