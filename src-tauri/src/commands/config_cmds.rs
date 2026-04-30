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

    // Inject app-log-path so the frontend can display it
    if let Some(map) = merged.as_object_mut() {
        map.insert(
            "app-log-path".into(),
            Value::String(state.log_dir.to_string_lossy().to_string()),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- normalize_proxy_bypass --

    #[test]
    fn normalize_proxy_bypass_empty() {
        assert_eq!(normalize_proxy_bypass(""), "");
    }

    #[test]
    fn normalize_proxy_bypass_comma_and_newline() {
        assert_eq!(
            normalize_proxy_bypass("localhost, 127.0.0.1\n::1 , 192.168.1.1"),
            "localhost,127.0.0.1,::1,192.168.1.1"
        );
    }

    #[test]
    fn normalize_proxy_bypass_trims_and_deduplicates_no_op() {
        // Normalization does not deduplicate; it only trims and filters empties
        assert_eq!(normalize_proxy_bypass("a, a, b"), "a,a,b");
    }

    // -- contains_download_scope --

    #[test]
    fn contains_download_scope_none() {
        assert!(!contains_download_scope(None));
    }

    #[test]
    fn contains_download_scope_missing() {
        assert!(!contains_download_scope(Some(&json!("download"))));
    }

    #[test]
    fn contains_download_scope_empty_array() {
        assert!(!contains_download_scope(Some(&json!([]))));
    }

    #[test]
    fn contains_download_scope_present() {
        assert!(contains_download_scope(Some(&json!(["download"]))));
    }

    #[test]
    fn contains_download_scope_present_with_spaces() {
        assert!(contains_download_scope(Some(&json!(["  download  "]))));
    }

    #[test]
    fn contains_download_scope_mixed_array() {
        assert!(contains_download_scope(Some(&json!([
            "update", "download"
        ]))));
    }

    #[test]
    fn contains_download_scope_no_match() {
        assert!(!contains_download_scope(Some(&json!([
            "update", "tracker"
        ]))));
    }

    // -- prepare_preference_patch --

    #[test]
    fn prepare_non_object_returns_empty_map() {
        let result = prepare_preference_patch(json!("string")).unwrap();
        let map = result.as_object().unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn prepare_keep_seeding_false_zeroes_seed_options() {
        let result = prepare_preference_patch(json!({"keep-seeding": false})).unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("seed-time"), Some(&json!(0)));
        assert_eq!(map.get("seed-ratio"), Some(&json!(0)));
    }

    #[test]
    fn prepare_keep_seeding_true_leaves_seed_options_unchanged() {
        let result = prepare_preference_patch(json!({"keep-seeding": true})).unwrap();
        let map = result.as_object().unwrap();
        assert!(!map.contains_key("seed-time"));
        assert!(!map.contains_key("seed-ratio"));
    }

    #[test]
    fn prepare_keep_seeding_string_true() {
        let result = prepare_preference_patch(json!({"keep-seeding": "true"})).unwrap();
        let map = result.as_object().unwrap();
        assert!(!map.contains_key("seed-time"));
    }

    #[test]
    fn prepare_use_remote_file_time_bool() {
        let result = prepare_preference_patch(json!({"use-remote-file-time": true})).unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("remote-time"), Some(&json!(true)));
    }

    #[test]
    fn prepare_use_remote_file_time_string() {
        let result = prepare_preference_patch(json!({"use-remote-file-time": "true"})).unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("remote-time"), Some(&json!(true)));
    }

    #[test]
    fn prepare_use_remote_file_time_false_string() {
        let result = prepare_preference_patch(json!({"use-remote-file-time": "false"})).unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("remote-time"), Some(&json!(false)));
    }

    #[test]
    fn prepare_proxy_disabled_clears_all_proxy() {
        let result = prepare_preference_patch(json!({
            "proxy": {
                "enable": false,
                "server": "http://proxy.example.com:8080",
                "scope": ["download"]
            }
        }))
        .unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("all-proxy"), Some(&json!("")));
        assert_eq!(map.get("no-proxy"), Some(&json!("")));
    }

    #[test]
    fn prepare_proxy_enabled_with_download_scope_sets_all_proxy() {
        let result = prepare_preference_patch(json!({
            "proxy": {
                "enable": true,
                "server": "http://proxy.example.com:8080",
                "scope": ["download"],
                "bypass": "localhost, 127.0.0.1"
            }
        }))
        .unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(
            map.get("all-proxy"),
            Some(&json!("http://proxy.example.com:8080"))
        );
        assert_eq!(map.get("no-proxy"), Some(&json!("localhost,127.0.0.1")));
    }

    #[test]
    fn prepare_proxy_enabled_without_server_clears_all_proxy() {
        let result = prepare_preference_patch(json!({
            "proxy": {
                "enable": true,
                "server": "",
                "scope": ["download"]
            }
        }))
        .unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("all-proxy"), Some(&json!("")));
    }

    #[test]
    fn prepare_proxy_enabled_without_download_scope_clears_all_proxy() {
        let result = prepare_preference_patch(json!({
            "proxy": {
                "enable": true,
                "server": "http://proxy.example.com:8080",
                "scope": ["update"]
            }
        }))
        .unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("all-proxy"), Some(&json!("")));
        assert_eq!(map.get("no-proxy"), Some(&json!("")));
    }

    #[test]
    fn prepare_proxy_missing_scope_treats_as_no_download() {
        let result = prepare_preference_patch(json!({
            "proxy": {
                "enable": true,
                "server": "http://proxy.example.com:8080"
            }
        }))
        .unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("all-proxy"), Some(&json!("")));
    }

    #[test]
    fn prepare_preserves_unrelated_keys() {
        let result = prepare_preference_patch(json!({"theme": "dark", "locale": "en-US"})).unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("theme"), Some(&json!("dark")));
        assert_eq!(map.get("locale"), Some(&json!("en-US")));
    }
}
