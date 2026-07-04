pub mod clip_prompt;
pub mod flyout;
pub mod menu;
pub mod tray;
pub mod vault;

#[cfg(not(target_os = "android"))]
use tauri::Emitter;

#[cfg(not(target_os = "android"))]
pub fn emit_command(app: &tauri::AppHandle, command: &str) {
    let _ = app.emit("command", serde_json::json!({ "command": command }));
}

#[cfg(not(target_os = "android"))]
pub fn show_and_emit(app: &tauri::AppHandle, command: &str) {
    let _ = crate::commands::app_cmds::show_main_window(app);
    emit_command(app, command);
}
