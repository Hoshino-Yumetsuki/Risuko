use std::time::Duration;

use futures_util::{stream, StreamExt};
use serde_json::{Map, Value};
use tauri::{AppHandle, State};

#[cfg(not(target_os = "android"))]
use tauri_plugin_autostart::ManagerExt;

use crate::{config::parse_keep_seeding_option, state::AppState};
use risuko_http::{NoProxy, Url};

#[tauri::command]
pub fn get_app_config(handle: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let mut merged = config.get_merged_config();

    if let Ok(enabled) = is_open_at_login_enabled(&handle) {
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
        Some(is_open_at_login_enabled(&handle)?)
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
        if previous_open_at_login != Some(enabled) {
            apply_open_at_login(&handle, enabled)?;
        }
    }

    if let Some(proxy) = user.get_mut("proxy").and_then(Value::as_object_mut) {
        let bypass = proxy
            .get("bypass")
            .and_then(Value::as_str)
            .map(normalize_proxy_bypass)
            .unwrap_or_default();
        proxy.insert("bypass".into(), Value::String(bypass));
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
    NoProxy::parse(value).normalized().to_string()
}

fn value_as_bool(value: Option<&Value>) -> bool {
    value
        .and_then(|value| {
            value.as_bool().or_else(|| {
                value
                    .as_str()
                    .map(|text| text.trim().eq_ignore_ascii_case("true"))
            })
        })
        .unwrap_or(false)
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

    let Some(proxy_value) = map.get("proxy").and_then(|value| value.as_object()) else {
        return Ok(Value::Object(map));
    };

    let mut proxy = proxy_value.clone();
    let normalized_bypass = proxy
        .get("bypass")
        .and_then(|value| value.as_str())
        .map(normalize_proxy_bypass)
        .unwrap_or_default();
    proxy.insert(
        "bypass".to_string(),
        Value::String(normalized_bypass.clone()),
    );
    map.insert("proxy".to_string(), Value::Object(proxy.clone()));

    let proxy_enabled = value_as_bool(proxy.get("enable"));
    let proxy_server = proxy
        .get("server")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let use_download_proxy =
        proxy_enabled && !proxy_server.is_empty() && contains_download_scope(proxy.get("scope"));

    let no_proxy = if use_download_proxy {
        normalized_bypass
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

#[tauri::command]
pub fn resolve_configured_proxy(
    state: State<'_, AppState>,
    scope: String,
    url: String,
) -> Result<Option<String>, String> {
    let scope = scope.trim().to_ascii_lowercase();
    if !matches!(
        scope.as_str(),
        "download" | "update-app" | "update-trackers"
    ) {
        return Ok(None);
    }
    let target = Url::parse(url.trim()).map_err(|e| format!("Invalid URL: {e}"))?;

    let config = state.config.lock().map_err(|e| e.to_string())?;
    resolve_proxy_from_config(
        config.get_user_config(),
        config.get_system_config(),
        &scope,
        &target,
    )
}

#[tauri::command]
pub fn is_signed_updater_available(handle: AppHandle) -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        has_signed_updater_pubkey(handle.config().plugins.0.get("updater"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = handle;
        false
    }
}

fn has_signed_updater_pubkey(config: Option<&Value>) -> bool {
    config
        .and_then(|config| config.get("pubkey"))
        .and_then(Value::as_str)
        .is_some_and(|pubkey| !pubkey.trim().is_empty())
}

const MAX_TRACKER_SOURCE_URLS: usize = 64;
const MAX_TRACKER_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TRACKER_SOURCE_CONCURRENCY: usize = 8;

#[tauri::command]
pub async fn fetch_tracker_sources(
    state: State<'_, AppState>,
    urls: Vec<String>,
) -> Result<Vec<String>, String> {
    let (user, system) = {
        let config = state.config.lock().map_err(|e| e.to_string())?;
        (
            config.get_user_config().clone(),
            config.get_system_config().clone(),
        )
    };

    let configured_proxy = configured_proxy_from_config(&user, &system, "update-trackers")?;
    let mut requests = Vec::new();
    for raw in urls.into_iter().take(MAX_TRACKER_SOURCE_URLS) {
        let value = raw.trim();
        let Ok(target) = Url::parse(value) else {
            tracing::warn!("Skipping invalid tracker source URL");
            continue;
        };
        if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
            tracing::warn!("Skipping non-HTTP tracker source URL");
            continue;
        }

        requests.push((target, configured_proxy.clone()));
    }

    Ok(fetch_tracker_source_requests(requests).await)
}

async fn fetch_tracker_source_requests(
    requests: Vec<(Url, Option<(String, NoProxy)>)>,
) -> Vec<String> {
    let mut pending = stream::iter(requests.into_iter().enumerate().map(
        |(index, (target, proxy))| async move {
            let host = target.host_str().unwrap_or("unknown").to_string();
            (index, host, fetch_one_tracker_source(target, proxy).await)
        },
    ))
    .buffer_unordered(MAX_TRACKER_SOURCE_CONCURRENCY);

    let mut completed = Vec::new();
    while let Some((index, host, result)) = pending.next().await {
        match result {
            Ok(value) => completed.push((index, value)),
            Err(error) => tracing::warn!(host, "Tracker source fetch failed: {error}"),
        }
    }
    completed.sort_unstable_by_key(|(index, _)| *index);
    completed.into_iter().map(|(_, value)| value).collect()
}

async fn fetch_one_tracker_source(
    target: Url,
    configured_proxy: Option<(String, NoProxy)>,
) -> Result<String, String> {
    let mut builder = risuko_http::Client::builder()
        .user_agent("Risuko/0.6")
        .redirect(risuko_http::Policy::limited(10))
        .timeout(Duration::from_secs(30))
        .gzip(true)
        .brotli(true)
        .deflate(true);
    if let Some((server, bypass)) = configured_proxy {
        let configured = risuko_http::Proxy::all(server)
            .map_err(|e| format!("Invalid configured proxy: {e}"))?;
        builder = builder.proxy(configured).no_proxy(bypass);
    }
    let client = builder
        .build()
        .map_err(|e| format!("Failed to build tracker HTTP client: {e}"))?;
    let response = client
        .get(target.as_str())
        .send()
        .await
        .map_err(|e| format!("Tracker source request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Tracker source returned HTTP {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TRACKER_SOURCE_BYTES)
    {
        return Err("Tracker source response is too large".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Tracker source body read failed: {e}"))?;
        if chunk.len() > (MAX_TRACKER_SOURCE_BYTES as usize).saturating_sub(bytes.len()) {
            return Err("Tracker source response is too large".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn resolve_proxy_from_config(
    user: &Map<String, Value>,
    system: &Map<String, Value>,
    scope: &str,
    target: &Url,
) -> Result<Option<String>, String> {
    let Some((server, matcher)) = configured_proxy_from_config(user, system, scope)? else {
        return Ok(None);
    };
    if matcher.matches_url(target) {
        return Ok(None);
    }
    Ok(Some(server))
}

fn configured_proxy_from_config(
    user: &Map<String, Value>,
    system: &Map<String, Value>,
    scope: &str,
) -> Result<Option<(String, NoProxy)>, String> {
    let nested = user.get("proxy").and_then(Value::as_object);
    let enabled = value_as_bool(nested.and_then(|proxy| proxy.get("enable")));
    let server = nested
        .and_then(|proxy| proxy.get("server"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let scope_selected = nested
        .map(|proxy| match proxy.get("scope") {
            None => true,
            Some(value) => scope_contains(value, scope),
        })
        .unwrap_or(false);

    let (server, bypass) = if enabled && scope_selected {
        (
            server,
            nested
                .and_then(|proxy| proxy.get("bypass"))
                .and_then(Value::as_str),
        )
    } else if scope == "download" && nested.is_none() {
        (
            system
                .get("all-proxy")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            system.get("no-proxy").and_then(Value::as_str),
        )
    } else {
        (None, None)
    };

    let Some(server) = server else {
        return Ok(None);
    };
    let matcher = NoProxy::parse(bypass.unwrap_or_default());

    risuko_http::Proxy::all(server).map_err(|e| format!("Invalid configured proxy: {e}"))?;
    Ok(Some((server.to_string(), matcher)))
}

fn scope_contains(value: &Value, wanted: &str) -> bool {
    value.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.as_str()
                .is_some_and(|scope| scope.trim().eq_ignore_ascii_case(wanted))
        })
    })
}

fn apply_open_at_login(handle: &AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let _ = (handle, enabled);
        return Ok(());
    }
    #[cfg(not(target_os = "android"))]
    {
        if enabled {
            handle.autolaunch().enable().map_err(|e| e.to_string())?;
        } else {
            handle.autolaunch().disable().map_err(|e| e.to_string())?;
        }

        Ok(())
    }
}

fn is_open_at_login_enabled(handle: &AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = handle;
        Ok(false)
    }
    #[cfg(not(target_os = "android"))]
    {
        handle.autolaunch().is_enabled().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
    fn normalize_proxy_bypass_trims_and_deduplicates() {
        assert_eq!(
            normalize_proxy_bypass("A.example.com, a.EXAMPLE.com, b"),
            "a.example.com,b"
        );
    }

    #[test]
    fn signed_updater_requires_a_nonempty_public_key() {
        assert!(!has_signed_updater_pubkey(None));
        assert!(!has_signed_updater_pubkey(Some(&json!({}))));
        assert!(!has_signed_updater_pubkey(Some(
            &json!({ "pubkey": " \n " })
        )));
        assert!(has_signed_updater_pubkey(Some(&json!({
            "pubkey": "release-public-key"
        }))));
    }

    #[test]
    fn value_as_bool_accepts_legacy_string_values() {
        assert!(value_as_bool(Some(&json!(true))));
        assert!(value_as_bool(Some(&json!(" TRUE "))));
        assert!(!value_as_bool(Some(&json!("false"))));
        assert!(!value_as_bool(None));
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
                ,"bypass": "Example.com, example.com"
            }
        }))
        .unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.get("all-proxy"), Some(&json!("")));
        assert_eq!(map.get("no-proxy"), Some(&json!("")));
        assert_eq!(
            map.get("proxy").and_then(|value| value.get("bypass")),
            Some(&json!("example.com"))
        );
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

    #[test]
    fn resolve_proxy_uses_scope_and_bypass() {
        let user = serde_json::from_value::<Map<String, Value>>(json!({
            "proxy": {
                "enable": true,
                "server": "http://proxy.example:8080",
                "bypass": "example.com",
                "scope": ["download", "update-app"]
            }
        }))
        .unwrap();
        let system = Map::new();
        let target = Url::parse("https://cdn.example.net/file").unwrap();
        assert_eq!(
            resolve_proxy_from_config(&user, &system, "download", &target).unwrap(),
            Some("http://proxy.example:8080".to_string())
        );
        let bypassed = Url::parse("https://assets.example.com/file").unwrap();
        assert_eq!(
            resolve_proxy_from_config(&user, &system, "download", &bypassed).unwrap(),
            None
        );
        assert_eq!(
            resolve_proxy_from_config(&user, &system, "update-trackers", &target).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_proxy_does_not_resurrect_flattened_proxy_when_nested_disabled() {
        let user = serde_json::from_value::<Map<String, Value>>(json!({
            "proxy": {"enable": false, "scope": ["download"]}
        }))
        .unwrap();
        let system = serde_json::from_value::<Map<String, Value>>(json!({
            "all-proxy": "http://stale.example:8080",
            "no-proxy": ""
        }))
        .unwrap();
        let target = Url::parse("https://cdn.example.net/file").unwrap();
        assert_eq!(
            resolve_proxy_from_config(&user, &system, "download", &target).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_proxy_missing_scope_defaults_to_all_scopes() {
        let user = serde_json::from_value::<Map<String, Value>>(json!({
            "proxy": {
                "enable": true,
                "server": "http://proxy.example:8080"
            }
        }))
        .unwrap();
        let system = Map::new();
        let target = Url::parse("https://cdn.example.net/file").unwrap();
        for scope in ["download", "update-app", "update-trackers"] {
            assert_eq!(
                resolve_proxy_from_config(&user, &system, scope, &target).unwrap(),
                Some("http://proxy.example:8080".to_string()),
                "legacy proxy should apply to {scope}"
            );
        }
    }

    #[test]
    fn resolve_proxy_explicit_empty_scope_stays_disabled() {
        let user = serde_json::from_value::<Map<String, Value>>(json!({
            "proxy": {
                "enable": true,
                "server": "http://proxy.example:8080",
                "scope": []
            }
        }))
        .unwrap();
        let target = Url::parse("https://cdn.example.net/file").unwrap();
        assert_eq!(
            resolve_proxy_from_config(&user, &Map::new(), "update-app", &target).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn tracker_source_chunked_body_is_bounded() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;

            let payload = vec![b'x'; MAX_TRACKER_SOURCE_BYTES as usize + 1];
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await;
            let _ = socket
                .write_all(format!("{:X}\r\n", payload.len()).as_bytes())
                .await;
            let _ = socket.write_all(&payload).await;
            let _ = socket.write_all(b"\r\n0\r\n\r\n").await;
        });

        let target = Url::parse(&format!("http://{address}/trackers.txt")).unwrap();
        let error = fetch_one_tracker_source(target, None).await.unwrap_err();
        assert_eq!(error, "Tracker source response is too large");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tracker_sources_are_fetched_concurrently() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut sockets = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                sockets.push(socket);
            }
            for mut socket in sockets {
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nok\n\n",
                    )
                    .await;
            }
        });

        let first = Url::parse(&format!("http://{address}/first")).unwrap();
        let second = Url::parse(&format!("http://{address}/second")).unwrap();
        let results = tokio::time::timeout(
            Duration::from_secs(1),
            fetch_tracker_source_requests(vec![(first, None), (second, None)]),
        )
        .await
        .expect("tracker fetches should overlap instead of waiting serially");

        assert_eq!(results, vec!["ok\n\n", "ok\n\n"]);
        server.await.unwrap();
    }
}
