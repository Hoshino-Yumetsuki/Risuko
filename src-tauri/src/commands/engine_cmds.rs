use tauri::AppHandle;

#[tauri::command]
pub async fn restart_engine(handle: AppHandle) -> Result<(), String> {
    crate::engine::restart_engine(&handle)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_engine_status(state: tauri::State<'_, crate::state::AppState>) -> Result<bool, String> {
    let running = state.engine_running.lock().map_err(|e| e.to_string())?;
    Ok(*running)
}
