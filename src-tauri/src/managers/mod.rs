pub mod menu;
pub mod tray;

use tauri::{Emitter, Manager};

pub fn emit_command(app: &tauri::AppHandle, command: &str) {
    let _ = app.emit("command", serde_json::json!({ "command": command }));
}

pub fn show_and_emit(app: &tauri::AppHandle, command: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    emit_command(app, command);
}
