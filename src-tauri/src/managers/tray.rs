use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, Emitter, Manager,
};

use super::{emit_command, show_and_emit};

pub fn setup_tray(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle();

    let new_task = MenuItemBuilder::with_id("tray-new-task", "New Task").build(handle)?;
    let new_bt_task = MenuItemBuilder::with_id("tray-new-bt-task", "New BT Task").build(handle)?;
    let open_file = MenuItemBuilder::with_id("tray-open-file", "Open Torrent File...").build(handle)?;
    let sep1 = PredefinedMenuItem::separator(handle)?;
    let show = MenuItemBuilder::with_id("tray-show", "Show Motrix").build(handle)?;
    let manual = MenuItemBuilder::with_id("tray-manual", "Manual").build(handle)?;
    let check_updates = MenuItemBuilder::with_id("tray-check-updates", "Check for Updates...").build(handle)?;
    let sep2 = PredefinedMenuItem::separator(handle)?;
    let task_list = MenuItemBuilder::with_id("tray-task-list", "Task List").build(handle)?;
    let preferences = MenuItemBuilder::with_id("tray-preferences", "Preferences...").build(handle)?;
    let sep3 = PredefinedMenuItem::separator(handle)?;
    let quit = MenuItemBuilder::with_id("tray-quit", "Quit").build(handle)?;

    let menu = MenuBuilder::new(handle)
        .items(&[
            &new_task, &new_bt_task, &open_file, &sep1, &show, &manual, &check_updates, &sep2,
            &task_list, &preferences, &sep3, &quit,
        ])
        .build()?;

    let icon_bytes = include_bytes!("../../icons/icon.png");
    let icon = Image::from_bytes(icon_bytes).expect("Failed to load tray icon");

    let _tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .menu(&menu)
        .tooltip("Motrix")
        .icon_as_template(true)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            match id {
                "tray-new-task" => show_and_emit(app, "application:new-task"),
                "tray-new-bt-task" => show_and_emit(app, "application:new-bt-task"),
                "tray-open-file" => show_and_emit(app, "application:open-file"),
                "tray-show" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "tray-manual" => {
                    let _ = open::that("https://github.com/agalwood/Motrix/wiki");
                }
                "tray-check-updates" => emit_command(app, "application:check-for-updates"),
                "tray-task-list" => show_and_emit(app, "application:task-list"),
                "tray-preferences" => show_and_emit(app, "application:preferences"),
                "tray-quit" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                    let _ = app.emit("confirm-quit", ());
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}
