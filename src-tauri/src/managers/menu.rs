use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{
    menu::{AboutMetadataBuilder, Menu, MenuBuilder, MenuItemBuilder, Submenu, SubmenuBuilder},
    App, AppHandle, Emitter, Manager,
};

use super::{emit_command, show_and_emit};

static CACHED_LABELS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn get_menu_text(labels: &HashMap<String, String>, id: &str, fallback: &str) -> String {
    labels
        .get(id)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

pub fn setup_menu(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle();
    let menu = build_menu(handle, &HashMap::new())?;
    app.set_menu(menu)?;
    setup_menu_event_handler(app);
    Ok(())
}

pub fn update_menu_labels(
    handle: &AppHandle,
    labels: &HashMap<String, String>,
) -> Result<(), String> {
    // Cache labels so toggle_app_menu can restore them
    if let Ok(mut cached) = CACHED_LABELS.lock() {
        *cached = Some(labels.clone());
    }
    let menu = build_menu(handle, labels).map_err(|e| e.to_string())?;
    handle.set_menu(menu).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn toggle_app_menu(handle: &AppHandle, hidden: bool) -> Result<(), String> {
    if hidden {
        handle.remove_menu().map_err(|e| e.to_string())?;
    } else {
        let labels = CACHED_LABELS
            .lock()
            .ok()
            .and_then(|c| c.clone())
            .unwrap_or_default();
        let menu = build_menu(handle, &labels).map_err(|e| e.to_string())?;
        handle.set_menu(menu).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn build_menu(
    handle: &AppHandle,
    labels: &HashMap<String, String>,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    if cfg!(target_os = "macos") {
        build_macos_menu(handle, labels)
    } else {
        build_default_menu(handle, labels)
    }
}

fn build_macos_menu(
    handle: &AppHandle,
    labels: &HashMap<String, String>,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let app_menu = SubmenuBuilder::new(handle, get_menu_text(labels, "menu-app", "Risuko"))
        .about(Some(
            AboutMetadataBuilder::new()
                .name(Some("Risuko"))
                .version(Some(env!("CARGO_PKG_VERSION")))
                .build(),
        ))
        .separator()
        .item(
            &MenuItemBuilder::with_id(
                "preferences",
                get_menu_text(labels, "preferences", "Preferences..."),
            )
            .accelerator("CmdOrCtrl+,")
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "check-for-updates",
                get_menu_text(labels, "check-for-updates", "Check for Updates..."),
            )
            .build(handle)?,
        )
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(
            &MenuItemBuilder::with_id("quit", get_menu_text(labels, "quit", "Quit Risuko"))
                .accelerator("CmdOrCtrl+Q")
                .build(handle)?,
        )
        .build()?;

    let task_menu = build_task_submenu(handle, true, labels)?;
    let edit_menu = build_edit_submenu(handle, labels)?;

    let window_menu = SubmenuBuilder::new(handle, get_menu_text(labels, "menu-window", "Window"))
        .item(
            &MenuItemBuilder::with_id("reload", get_menu_text(labels, "reload", "Reload"))
                .accelerator("CmdOrCtrl+R")
                .build(handle)?,
        )
        .close_window()
        .minimize()
        .maximize()
        .fullscreen()
        .separator()
        .item(
            &MenuItemBuilder::with_id(
                "front",
                get_menu_text(labels, "front", "Bring All to Front"),
            )
            .build(handle)?,
        )
        .build()?;

    let help_menu = build_help_submenu(handle, labels)?;

    let menu = MenuBuilder::new(handle)
        .items(&[&app_menu, &task_menu, &edit_menu, &window_menu, &help_menu])
        .build()?;

    Ok(menu)
}

fn build_default_menu(
    handle: &AppHandle,
    labels: &HashMap<String, String>,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let file_menu = SubmenuBuilder::new(handle, get_menu_text(labels, "menu-file", "File"))
        .item(
            &MenuItemBuilder::with_id("about", get_menu_text(labels, "about", "About Risuko"))
                .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(
                "preferences",
                get_menu_text(labels, "preferences", "Preferences..."),
            )
            .accelerator("CmdOrCtrl+,")
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "check-for-updates",
                get_menu_text(labels, "check-for-updates", "Check for Updates..."),
            )
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "show-window",
                get_menu_text(labels, "show-window", "Show Risuko"),
            )
            .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("quit", get_menu_text(labels, "quit", "Quit Risuko"))
                .accelerator("CmdOrCtrl+Q")
                .build(handle)?,
        )
        .build()?;

    let task_menu = build_task_submenu(handle, false, labels)?;
    let edit_menu = build_edit_submenu(handle, labels)?;

    let window_menu = SubmenuBuilder::new(handle, get_menu_text(labels, "menu-window", "Window"))
        .item(
            &MenuItemBuilder::with_id("reload", get_menu_text(labels, "reload", "Reload"))
                .accelerator("CmdOrCtrl+R")
                .build(handle)?,
        )
        .close_window()
        .minimize()
        .fullscreen()
        .build()?;

    let help_menu = build_help_submenu(handle, labels)?;

    let menu = MenuBuilder::new(handle)
        .items(&[&file_menu, &task_menu, &edit_menu, &window_menu, &help_menu])
        .build()?;

    Ok(menu)
}

fn build_task_submenu(
    handle: &tauri::AppHandle,
    include_clear_recent: bool,
    labels: &HashMap<String, String>,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let mut builder = SubmenuBuilder::new(handle, get_menu_text(labels, "menu-task", "Task"))
        .item(
            &MenuItemBuilder::with_id("new-task", get_menu_text(labels, "new-task", "New Task"))
                .accelerator("CmdOrCtrl+N")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "new-bt-task",
                get_menu_text(labels, "new-bt-task", "New BT Task"),
            )
            .accelerator("CmdOrCtrl+Shift+N")
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "open-file",
                get_menu_text(labels, "open-file", "Open Torrent File..."),
            )
            .accelerator("CmdOrCtrl+O")
            .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id("task-list", get_menu_text(labels, "task-list", "Task List"))
                .accelerator("CmdOrCtrl+L")
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "pause-task",
                get_menu_text(labels, "pause-task", "Pause Task"),
            )
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "resume-task",
                get_menu_text(labels, "resume-task", "Resume Task"),
            )
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "delete-task",
                get_menu_text(labels, "delete-task", "Delete Task"),
            )
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "move-task-up",
                get_menu_text(labels, "move-task-up", "Move Task Up"),
            )
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "move-task-down",
                get_menu_text(labels, "move-task-down", "Move Task Down"),
            )
            .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(
                "pause-all-task",
                get_menu_text(labels, "pause-all-task", "Pause All Tasks"),
            )
            .accelerator("CmdOrCtrl+Shift+P")
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "resume-all-task",
                get_menu_text(labels, "resume-all-task", "Resume All Tasks"),
            )
            .accelerator("CmdOrCtrl+Shift+R")
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "select-all-task",
                get_menu_text(labels, "select-all-task", "Select All Tasks"),
            )
            .accelerator("CmdOrCtrl+Shift+A")
            .build(handle)?,
        );

    if include_clear_recent {
        builder = builder.separator().item(
            &MenuItemBuilder::with_id(
                "clear-recent-tasks",
                get_menu_text(labels, "clear-recent-tasks", "Clear Recent Tasks"),
            )
            .build(handle)?,
        );
    }

    Ok(builder.build()?)
}

fn build_edit_submenu(
    handle: &tauri::AppHandle,
    labels: &HashMap<String, String>,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    Ok(
        SubmenuBuilder::new(handle, get_menu_text(labels, "menu-edit", "Edit"))
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?,
    )
}

fn build_help_submenu(
    handle: &tauri::AppHandle,
    labels: &HashMap<String, String>,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let mut builder = SubmenuBuilder::new(handle, get_menu_text(labels, "menu-help", "Help"))
        .item(
            &MenuItemBuilder::with_id(
                "official-website",
                get_menu_text(labels, "official-website", "Official Website"),
            )
            .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id("manual", get_menu_text(labels, "manual", "Manual"))
                .build(handle)?,
        )
        .item(
            &MenuItemBuilder::with_id(
                "release-notes",
                get_menu_text(labels, "release-notes", "Release Notes"),
            )
            .build(handle)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::with_id(
                "report-problem",
                get_menu_text(labels, "report-problem", "Report Problem"),
            )
            .build(handle)?,
        );

    if cfg!(debug_assertions) {
        builder = builder.separator().item(
            &MenuItemBuilder::with_id(
                "toggle-dev-tools",
                get_menu_text(labels, "toggle-dev-tools", "Toggle Developer Tools"),
            )
            .accelerator("F12")
            .build(handle)?,
        );
    }

    Ok(builder.build()?)
}

fn setup_menu_event_handler(app: &App) {
    app.on_menu_event(move |app, event| {
        let id = event.id().as_ref();
        match id {
            "new-task" => show_and_emit(app, "application:new-task"),
            "new-bt-task" => show_and_emit(app, "application:new-bt-task"),
            "open-file" => show_and_emit(app, "application:open-file"),
            "task-list" => emit_command(app, "application:task-list"),
            "pause-task" => emit_command(app, "application:pause-task"),
            "resume-task" => emit_command(app, "application:resume-task"),
            "delete-task" => emit_command(app, "application:delete-task"),
            "move-task-up" => emit_command(app, "application:move-task-up"),
            "move-task-down" => emit_command(app, "application:move-task-down"),
            "pause-all-task" => emit_command(app, "application:pause-all-task"),
            "resume-all-task" => emit_command(app, "application:resume-all-task"),
            "select-all-task" => emit_command(app, "application:select-all-task"),
            "clear-recent-tasks" => emit_command(app, "application:clear-recent-tasks"),
            "preferences" => emit_command(app, "application:preferences"),
            "about" => emit_command(app, "application:about"),
            "check-for-updates" => emit_command(app, "application:check-for-updates"),
            "show-window" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "official-website" => {
                let _ = open::that("https://risuko.vercel.app");
            }
            "manual" => {
                let _ = open::that("https://github.com/YueMiyuki/Risuko/wiki");
            }
            "release-notes" => {
                let _ = open::that("https://github.com/YueMiyuki/Risuko/releases");
            }
            "report-problem" => {
                let _ = open::that("https://github.com/YueMiyuki/Risuko/issues");
            }
            "reload" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.eval("window.location.reload()");
                }
            }
            "front" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "toggle-dev-tools" =>
            {
                #[cfg(debug_assertions)]
                if let Some(window) = app.get_webview_window("main") {
                    {
                        if window.is_devtools_open() {
                            window.close_devtools();
                        } else {
                            window.open_devtools();
                        }
                    }
                }
            }
            "quit" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                let _ = app.emit("confirm-quit", ());
            }
            _ => {}
        }
    });
}
