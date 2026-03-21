use serde_json::Value;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn get_app_config(state: State<'_, AppState>) -> Result<Value, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    Ok(config.get_merged_config())
}

#[tauri::command]
pub fn save_preference(state: State<'_, AppState>, config: Value) -> Result<(), String> {
    let mut mgr = state.config.lock().map_err(|e| e.to_string())?;

    if let Some(system) = config.get("system").and_then(|v| v.as_object()) {
        mgr.set_system_config_map(system)?;
    }

    if let Some(user) = config.get("user").and_then(|v| v.as_object()) {
        mgr.set_user_config_map(user)?;
    }

    Ok(())
}
