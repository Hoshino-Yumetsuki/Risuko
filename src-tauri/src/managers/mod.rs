pub mod menu;
pub mod tray;

use tauri::Emitter;

pub fn emit_command(app: &tauri::AppHandle, command: &str) {
    let _ = app.emit("command", serde_json::json!({ "command": command }));
}

pub fn show_and_emit(app: &tauri::AppHandle, command: &str) {
    let _ = crate::commands::app_cmds::show_main_window(app);
    emit_command(app, command);
}
