use tauri::{AppHandle, Manager};

#[tauri::command]
pub fn on_download_status_change(
    _downloading: bool,
    _state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    // TODO: add platform-specific power-save blocking.
    // macOS: IOPMAssertionCreateWithName
    // Windows: SetThreadExecutionState
    // Linux: org.freedesktop.ScreenSaver.Inhibit via D-Bus
    Ok(())
}

#[tauri::command]
pub fn on_speed_change(
    handle: AppHandle,
    upload_speed: u64,
    download_speed: u64,
) -> Result<(), String> {
    if let Some(tray) = handle.tray_by_id("main") {
        let tooltip = if upload_speed > 0 || download_speed > 0 {
            format!(
                "Motrix\nDL: {}/s  UL: {}/s",
                format_speed(download_speed),
                format_speed(upload_speed)
            )
        } else {
            "Motrix".to_string()
        };
        let _ = tray.set_tooltip(Some(&tooltip));
    }
    Ok(())
}

#[tauri::command]
pub fn on_progress_change(handle: AppHandle, progress: f64) -> Result<(), String> {
    if let Some(window) = handle.get_webview_window("main") {
        let (status, prog) = if (0.0..=1.0).contains(&progress) {
            (
                tauri::window::ProgressBarStatus::Normal,
                Some((progress * 100.0) as u64),
            )
        } else {
            (tauri::window::ProgressBarStatus::None, None)
        };
        let _ = window.set_progress_bar(tauri::window::ProgressBarState {
            status: Some(status),
            progress: prog,
        });
    }
    Ok(())
}

#[tauri::command]
pub fn on_task_download_complete(
    _handle: AppHandle,
    _path: String,
) -> Result<(), String> {
    // TODO: add platform-specific recent-documents handling.
    Ok(())
}

#[tauri::command]
pub fn update_tray(
    handle: AppHandle,
    image_data: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    if let Some(tray) = handle.tray_by_id("main") {
        let image = tauri::image::Image::new_owned(image_data, width, height);
        let _ = tray.set_icon(Some(image));
    }
    Ok(())
}

fn format_speed(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
