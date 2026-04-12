use serde_json::{Map, Value};
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

use crate::{config::parse_keep_seeding_option, state::AppState};

#[tauri::command]
pub fn get_app_config(handle: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let mut merged = config.get_merged_config();

    if let Ok(enabled) = handle.autolaunch().is_enabled() {
        if let Some(map) = merged.as_object_mut() {
            map.insert("open-at-login".into(), Value::Bool(enabled));
        }
    }

    Ok(merged)
}

#[tauri::command]
pub fn save_preference(
    handle: AppHandle,
    state: State<'_, AppState>,
    config: Value,
) -> Result<(), String> {
    let open_at_login = config
        .get("user")
        .and_then(|v| v.get("open-at-login"))
        .and_then(|v| v.as_bool())
        .or_else(|| config.get("open-at-login").and_then(|v| v.as_bool()));

    let previous_open_at_login = if open_at_login.is_some() {
        Some(
            handle
                .autolaunch()
                .is_enabled()
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let mut user = config
        .get("user")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    if let Some(enabled) = open_at_login {
        user.insert("open-at-login".into(), Value::Bool(enabled));
    }

    if let Some(enabled) = open_at_login {
        if previous_open_at_login != Some(enabled) {
            apply_open_at_login(&handle, enabled)?;
        }
    }

    let save_result = {
        let mut mgr = state.config.lock().map_err(|e| e.to_string())?;

        if let Some(system) = config.get("system").and_then(|v| v.as_object()) {
            let mut system = system.clone();
            system.remove("enable-upnp");
            mgr.set_system_config_map(&system)?;
        }
        mgr.remove_system_config_key("enable-upnp")?;

        if !user.is_empty() {
            mgr.set_user_config_map(&user)?;
        }

        Ok(())
    };

    if let Err(err) = save_result {
        if let (Some(previous), Some(current)) = (previous_open_at_login, open_at_login) {
            if previous != current {
                if let Err(rollback_err) = apply_open_at_login(&handle, previous) {
                    return Err(format!(
                        "{}; also failed to restore open-at-login to {}: {}",
                        err, previous, rollback_err
                    ));
                }
            }
        }
        return Err(err);
    }

    Ok(())
}

fn normalize_proxy_bypass(value: &str) -> String {
    value
        .split(|c| c == ',' || c == '\r' || c == '\n')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn contains_download_scope(value: Option<&Value>) -> bool {
    value
        .and_then(|scope| scope.as_array())
        .map(|scope| {
            scope.iter().any(|item| {
                item.as_str()
                    .map(|text| text.trim() == "download")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[tauri::command]
pub fn prepare_preference_patch(params: Value) -> Result<Value, String> {
    let mut map = match params {
        Value::Object(map) => map,
        _ => Map::new(),
    };

    if matches!(
        parse_keep_seeding_option(map.get("keep-seeding")),
        Some(false)
    ) {
        map.insert("seed-time".to_string(), Value::from(0));
        map.insert("seed-ratio".to_string(), Value::from(0));
    }

    // Sync use-remote-file-time user pref → remote-time system option
    if let Some(val) = map.get("use-remote-file-time").cloned() {
        let enabled = val
            .as_bool()
            .unwrap_or_else(|| val.as_str().map(|s| s == "true").unwrap_or(false));
        map.insert("remote-time".to_string(), Value::from(enabled));
    }

    let Some(proxy) = map.get("proxy").and_then(|value| value.as_object()) else {
        return Ok(Value::Object(map));
    };

    let proxy_enabled = proxy
        .get("enable")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let proxy_server = proxy
        .get("server")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let use_download_proxy =
        proxy_enabled && !proxy_server.is_empty() && contains_download_scope(proxy.get("scope"));

    let no_proxy = if use_download_proxy {
        proxy
            .get("bypass")
            .and_then(|value| value.as_str())
            .map(normalize_proxy_bypass)
            .unwrap_or_default()
    } else {
        String::new()
    };

    map.insert(
        "all-proxy".to_string(),
        Value::String(if use_download_proxy {
            proxy_server
        } else {
            String::new()
        }),
    );
    map.insert("no-proxy".to_string(), Value::String(no_proxy));

    Ok(Value::Object(map))
}

fn apply_open_at_login(handle: &AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        handle.autolaunch().enable().map_err(|e| e.to_string())?;
    } else {
        handle.autolaunch().disable().map_err(|e| e.to_string())?;
    }

    Ok(())
}
