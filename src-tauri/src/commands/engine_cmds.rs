use std::path::Path;
use std::time::Duration;
use std::{collections::HashMap, collections::HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::AppHandle;
use tokio::time::sleep;

use crate::engine;
use crate::engine::torrent;

const TEMP_DOWNLOAD_SUFFIX: &str = ".part";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTaskProgressInput {
    pub total_length: Value,
    pub completed_length: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LowSpeedTaskInput {
    pub gid: String,
    pub status: String,
    pub download_speed: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LowSpeedEvaluationResult {
    pub strike_map: HashMap<String, u32>,
    pub recover_at_map: HashMap<String, u64>,
    pub recover_gids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoRetryPlanResult {
    pub attempt_map: HashMap<String, u32>,
    pub next_attempt: u32,
    pub delay_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedTaskOrderInput {
    pub gid: String,
    pub status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOrderResult {
    pub moved: u32,
    pub partial_error: bool,
}

fn normalize_non_negative(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }

    let floored = value.floor();
    if floored >= u64::MAX as f64 {
        u64::MAX
    } else {
        floored as u64
    }
}

fn parse_length_like(value: &Value) -> u64 {
    match value {
        Value::Number(number) => {
            if let Some(parsed) = number.as_u64() {
                return parsed;
            }
            if let Some(parsed) = number.as_i64() {
                return u64::try_from(parsed).unwrap_or(0);
            }
            number.as_f64().map(normalize_non_negative).unwrap_or(0)
        }
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return 0;
            }

            if let Ok(parsed) = trimmed.parse::<u64>() {
                return parsed;
            }

            trimmed
                .parse::<f64>()
                .map(normalize_non_negative)
                .unwrap_or(0)
        }
        Value::Bool(flag) => {
            if *flag {
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn parse_counter_like(value: &Value) -> u32 {
    let parsed = parse_length_like(value);
    if parsed >= u32::MAX as u64 {
        u32::MAX
    } else {
        parsed as u32
    }
}

fn parse_retry_attempt_map(values: HashMap<String, Value>) -> HashMap<String, u32> {
    let mut result = HashMap::with_capacity(values.len());
    for (gid, value) in values {
        let key = gid.trim().to_string();
        if key.is_empty() {
            continue;
        }
        result.insert(key, parse_counter_like(&value));
    }
    result
}

fn compute_auto_retry_delay_ms(
    strategy: &str,
    base_delay_ms: u64,
    next_attempt: u32,
    max_delay_ms: u64,
) -> u64 {
    let min_delay_ms = 1000u64;
    let base_delay_ms = base_delay_ms.max(min_delay_ms);
    let max_delay_ms = max_delay_ms.max(min_delay_ms);

    let computed = if strategy.eq_ignore_ascii_case("exponential") {
        let exponent = next_attempt.saturating_sub(1).min(62);
        (base_delay_ms as u128).saturating_mul(1u128 << exponent)
    } else {
        base_delay_ms as u128
    };

    computed.min(max_delay_ms as u128).max(min_delay_ms as u128) as u64
}

fn infer_out_from_uri_inner(uri: &str) -> String {
    let raw = uri.trim();
    if raw.is_empty() {
        return String::new();
    }

    // M3U8 links: extract stem and use .ts extension
    let lower = raw.to_ascii_lowercase();
    let path_part = lower.split('?').next().unwrap_or(&lower);
    let path_part = path_part.split('#').next().unwrap_or(path_part);
    if path_part.ends_with(".m3u8") || path_part.ends_with(".m3u") {
        let without_query = raw.split('?').next().unwrap_or(raw);
        let without_hash = without_query.split('#').next().unwrap_or(without_query);
        let name = without_hash.rsplit('/').next().unwrap_or("");
        if name.is_empty() {
            return "download.ts".to_string();
        }
        let stem = name
            .strip_suffix(".m3u8")
            .or_else(|| name.strip_suffix(".M3U8"))
            .or_else(|| name.strip_suffix(".m3u"))
            .or_else(|| name.strip_suffix(".M3U"))
            .unwrap_or(name);
        if stem.is_empty() {
            return "download.ts".to_string();
        }
        return format!("{}.ts", stem);
    }

    // ED2K links: parse filename from ed2k://|file|<name>|<size>|<hash>|/
    if lower.starts_with("ed2k://") {
        let body = raw
            .trim_start_matches("ed2k://|file|")
            .trim_start_matches("ed2k://|FILE|")
            .trim_end_matches("|/");
        let parts: Vec<&str> = body.split('|').collect();
        if !parts.is_empty() {
            let decoded = urlencoding::decode(parts[0]).unwrap_or_default();
            let name = decoded.replace('_', " ");
            if !name.is_empty() {
                return name;
            }
        }
        return String::new();
    }

    let without_hash = raw.split('#').next().unwrap_or(raw);
    let without_query = without_hash.split('?').next().unwrap_or(without_hash);
    let candidate = without_query.rsplit('/').next().unwrap_or("").trim();
    let decoded_candidate = crate::commands::file_cmds::percent_decode_lossy(candidate);
    let decoded_candidate = decoded_candidate.trim();
    if decoded_candidate.is_empty() || !decoded_candidate.contains('.') {
        return String::new();
    }
    if decoded_candidate.contains('/') || decoded_candidate.contains('\\') {
        return String::new();
    }
    if decoded_candidate.starts_with('.') || decoded_candidate.ends_with('.') {
        return String::new();
    }

    decoded_candidate.to_string()
}

#[tauri::command]
pub fn infer_out_from_uri(uri: String) -> String {
    infer_out_from_uri_inner(&uri)
}

#[tauri::command]
pub fn resolve_file_category(filename: String) -> String {
    resolve_file_category_inner(&filename)
}

fn resolve_file_category_inner(filename: &str) -> String {
    if filename.is_empty() {
        return String::new();
    }

    let ext = match filename.rfind('.') {
        Some(idx) if idx > 0 && idx < filename.len() - 1 => {
            filename[idx..].to_ascii_lowercase()
        }
        _ => return String::new(),
    };

    static MUSIC: &[&str] = &[
        ".aac", ".ape", ".flac", ".flav", ".m4a", ".mp3", ".ogg", ".wav", ".wma",
    ];
    static VIDEO: &[&str] = &[
        ".avi", ".m3u8", ".m4v", ".mkv", ".mov", ".mp4", ".mpg", ".rmvb", ".ts", ".vob", ".wmv",
    ];
    static IMAGE: &[&str] = &[
        ".ai", ".bmp", ".eps", ".fig", ".gif", ".heic", ".icn", ".ico", ".jpeg", ".jpg", ".png",
        ".psd", ".raw", ".sketch", ".svg", ".tif", ".webp", ".xd",
    ];
    static DOCUMENT: &[&str] = &[
        ".azw3", ".csv", ".doc", ".docx", ".epub", ".key", ".mobi", ".numbers", ".pages", ".pdf",
        ".ppt", ".pptx", ".txt", ".xsl", ".xslx",
    ];
    static COMPRESSED: &[&str] = &[
        ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz", ".zst", ".iso",
    ];
    static PROGRAM: &[&str] = &[
        ".exe", ".msi", ".dmg", ".pkg", ".deb", ".rpm", ".appimage", ".apk",
    ];

    let categories: &[(&str, &[&str])] = &[
        ("music", MUSIC),
        ("video", VIDEO),
        ("image", IMAGE),
        ("document", DOCUMENT),
        ("compressed", COMPRESSED),
        ("program", PROGRAM),
    ];

    for &(category, suffixes) in categories {
        if suffixes.contains(&ext.as_str()) {
            return category.to_string();
        }
    }

    String::new()
}

fn ensure_temp_download_suffix(value: &str) -> String {
    let normalized = value.trim();
    if normalized.is_empty() {
        return String::new();
    }

    if normalized
        .to_ascii_lowercase()
        .ends_with(TEMP_DOWNLOAD_SUFFIX)
    {
        return normalized.to_string();
    }

    format!("{}{}", normalized, TEMP_DOWNLOAD_SUFFIX)
}

fn find_multicall_item_error(value: &Value) -> Option<&Value> {
    let is_error_object = |target: &Value| {
        target.is_object() && (target.get("code").is_some() || target.get("message").is_some())
    };

    if is_error_object(value) {
        return Some(value);
    }

    let Value::Array(items) = value else {
        return None;
    };

    for item in items {
        if is_error_object(item) {
            return Some(item);
        }

        if let Value::Array(entries) = item {
            for entry in entries {
                if is_error_object(entry) {
                    return Some(entry);
                }
            }
        }
    }

    None
}

#[tauri::command]
pub fn calculate_active_task_progress(tasks: Vec<ActiveTaskProgressInput>) -> Result<f64, String> {
    if tasks.is_empty() {
        return Ok(-1.0);
    }

    let mut total = 0u128;
    let mut completed = 0u128;
    for task in tasks {
        let total_length = parse_length_like(&task.total_length) as u128;
        if total_length == 0 {
            continue;
        }

        total += total_length;
        completed += parse_length_like(&task.completed_length) as u128;
    }

    if total == 0 {
        return Ok(2.0);
    }

    Ok(completed as f64 / total as f64)
}

#[tauri::command]
pub fn evaluate_low_speed_tasks(
    tasks: Vec<LowSpeedTaskInput>,
    threshold_bytes: Value,
    strike_threshold: u32,
    cooldown_ms: u64,
    now_ms: u64,
    strike_map: HashMap<String, Value>,
    recover_at_map: HashMap<String, Value>,
) -> Result<LowSpeedEvaluationResult, String> {
    let threshold = parse_length_like(&threshold_bytes);
    let strike_threshold = strike_threshold.max(1);

    let mut next_strike_map = HashMap::with_capacity(strike_map.len());
    for (gid, value) in strike_map {
        let key = gid.trim();
        if key.is_empty() {
            continue;
        }
        next_strike_map.insert(key.to_string(), parse_counter_like(&value));
    }

    let mut next_recover_at_map = HashMap::with_capacity(recover_at_map.len());
    for (gid, value) in recover_at_map {
        let key = gid.trim();
        if key.is_empty() {
            continue;
        }
        next_recover_at_map.insert(key.to_string(), parse_length_like(&value));
    }

    let mut recover_gids = Vec::new();
    let mut active_gids = HashSet::new();

    for task in tasks {
        let gid = task.gid.trim().to_string();
        if gid.is_empty() || !task.status.eq_ignore_ascii_case("active") {
            continue;
        }

        active_gids.insert(gid.clone());

        let speed = parse_length_like(&task.download_speed);
        if speed >= threshold {
            next_strike_map.remove(&gid);
            next_recover_at_map.remove(&gid);
            continue;
        }

        let strike = next_strike_map
            .get(&gid)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        next_strike_map.insert(gid.clone(), strike);

        if strike < strike_threshold {
            continue;
        }
        if next_recover_at_map.get(&gid).copied().unwrap_or(0) > now_ms {
            continue;
        }

        next_strike_map.insert(gid.clone(), 0);
        next_recover_at_map.insert(gid.clone(), now_ms.saturating_add(cooldown_ms));
        recover_gids.push(gid);
    }

    next_strike_map.retain(|gid, _| active_gids.contains(gid));
    next_recover_at_map.retain(|gid, _| active_gids.contains(gid));

    Ok(LowSpeedEvaluationResult {
        strike_map: next_strike_map,
        recover_at_map: next_recover_at_map,
        recover_gids,
    })
}

#[tauri::command]
pub fn plan_auto_retry(
    gid: String,
    strategy: String,
    interval_seconds: Value,
    max_delay_ms: Value,
    attempt_map: HashMap<String, Value>,
) -> Result<AutoRetryPlanResult, String> {
    let gid = gid.trim().to_string();
    if gid.is_empty() {
        return Err("Invalid task gid".to_string());
    }

    let mut next_attempt_map = parse_retry_attempt_map(attempt_map);
    let next_attempt = next_attempt_map
        .get(&gid)
        .copied()
        .unwrap_or(0)
        .saturating_add(1)
        .max(1);
    next_attempt_map.insert(gid, next_attempt);

    let interval_seconds = parse_length_like(&interval_seconds).max(1);
    let base_delay_ms = interval_seconds.saturating_mul(1000);
    let max_delay_ms = parse_length_like(&max_delay_ms).max(1000);
    let delay_ms =
        compute_auto_retry_delay_ms(&strategy, base_delay_ms, next_attempt, max_delay_ms);

    Ok(AutoRetryPlanResult {
        attempt_map: next_attempt_map,
        next_attempt,
        delay_ms,
    })
}

#[tauri::command]
pub async fn restart_engine(handle: AppHandle) -> Result<(), String> {
    crate::engine::restart_engine(&handle)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn probe_m3u8(url: String) -> Result<Value, String> {
    use crate::engine::m3u8::parser::{fetch_and_parse_playlist, ParsedPlaylist};

    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("URL is required".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .build()
        .map_err(|e| format!("Failed to build client: {e}"))?;

    let playlist = fetch_and_parse_playlist(&url, &client).await?;

    match playlist {
        ParsedPlaylist::Master { mut variants } => {
            variants.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
            let variant_vals: Vec<Value> = variants
                .iter()
                .map(|v| {
                    json!({
                        "bandwidth": v.bandwidth,
                        "resolution": v.resolution,
                        "codecs": v.codecs,
                        "url": v.url,
                    })
                })
                .collect();
            Ok(json!({
                "type": "master",
                "variants": variant_vals,
            }))
        }
        ParsedPlaylist::Media {
            segments,
            end_list,
            total_duration,
            ..
        } => {
            let encrypted = segments.iter().any(|s| s.encryption.is_some());
            Ok(json!({
                "type": "media",
                "segments": segments.len(),
                "duration": total_duration,
                "encrypted": encrypted,
                "endList": end_list,
            }))
        }
    }
}

#[tauri::command]
pub fn get_engine_status(state: tauri::State<'_, crate::state::AppState>) -> Result<bool, String> {
    let running = state.engine_running.lock().map_err(|e| e.to_string())?;
    Ok(*running)
}

#[tauri::command]
pub async fn add_torrent_by_path(
    _handle: AppHandle,
    _state: tauri::State<'_, crate::state::AppState>,
    path: String,
    options: Option<Value>,
) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("task.new-task-torrent-required".to_string());
    }

    let fs_path = Path::new(path);
    let is_torrent = fs_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("torrent"))
        == Some(true);
    if !is_torrent {
        return Err("task.new-task-torrent-required".to_string());
    }

    let bytes = std::fs::read(fs_path).map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("Torrent payload is empty".to_string());
    }
    let fallback_name = fs_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let (is_multi_file, torrent_root_name) =
        crate::commands::file_cmds::inspect_torrent_metadata(&bytes, fallback_name)
            .unwrap_or_else(|_| (false, fallback_name.to_string()));

    let options = options.unwrap_or(Value::Object(Map::new()));
    let mut options = match options {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if is_multi_file {
        options.remove("out");
    } else {
        let has_out = options
            .get("out")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);

        if !has_out {
            options.insert(
                "out".to_string(),
                Value::String(format!("{}{}", torrent_root_name, TEMP_DOWNLOAD_SUFFIX)),
            );
        } else if let Some(current_out) = options.get("out").and_then(|value| value.as_str()) {
            let trimmed = current_out.trim();
            if !trimmed.to_ascii_lowercase().ends_with(TEMP_DOWNLOAD_SUFFIX) {
                options.insert(
                    "out".to_string(),
                    Value::String(format!("{}{}", trimmed, TEMP_DOWNLOAD_SUFFIX)),
                );
            }
        }
    }

    let manager = engine::get_manager()
        .await
        .ok_or("Engine not running")?;

    manager.add_torrent_task(bytes, options).await
}

#[tauri::command]
pub async fn add_uri(
    _state: tauri::State<'_, crate::state::AppState>,
    uris: Vec<String>,
    outs: Option<Vec<String>>,
    options: Option<Value>,
) -> Result<Value, String> {
    let normalized_uris: Vec<String> = uris
        .into_iter()
        .map(|uri| uri.trim().to_string())
        .filter(|uri| !uri.is_empty())
        .collect();

    if normalized_uris.is_empty() {
        return Err("task.new-task-uris-required".to_string());
    }

    let out_list = outs.unwrap_or_default();
    let base_options = match options {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let manager = engine::get_manager()
        .await
        .ok_or("Engine not running")?;

    let mut results = Vec::with_capacity(normalized_uris.len());

    for (index, uri) in normalized_uris.iter().enumerate() {
        let mut task_options = base_options.clone();

        let preferred_out = out_list
            .get(index)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                task_options
                    .get("out")
                    .and_then(|value| value.as_str())
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| infer_out_from_uri_inner(uri));

        // M3u8 uses a temp directory for segments, not a .part file
        let is_m3u8 = engine::m3u8::is_m3u8_uri(uri);
        if !is_m3u8 {
            let temp_out = ensure_temp_download_suffix(&preferred_out);
            if !temp_out.is_empty() {
                task_options.insert("out".to_string(), Value::String(temp_out));
            }
        } else if !preferred_out.is_empty() {
            task_options.insert("out".to_string(), Value::String(preferred_out));
        }

        // Check if this is a magnet link
        if torrent::is_magnet_uri(uri) {
            match manager.add_magnet_task(uri, task_options).await {
                Ok(gid) => results.push(Value::Array(vec![Value::String(gid)])),
                Err(e) => results.push(json!({"code": 1, "message": e})),
            }
        } else if is_m3u8 {
            match manager.add_m3u8_task(uri, task_options).await {
                Ok(gid) => results.push(Value::Array(vec![Value::String(gid)])),
                Err(e) => results.push(json!({"code": 1, "message": e})),
            }
        } else if engine::ed2k::is_ed2k_uri(uri) {
            match manager.add_ed2k_task(uri, task_options).await {
                Ok(gid) => results.push(Value::Array(vec![Value::String(gid)])),
                Err(e) => results.push(json!({"code": 1, "message": e})),
            }
        } else if engine::ftp::is_ftp_uri(uri) {
            match manager.add_ftp_task(uri, task_options).await {
                Ok(gid) => results.push(Value::Array(vec![Value::String(gid)])),
                Err(e) => results.push(json!({"code": 1, "message": e})),
            }
        } else {
            match manager
                .add_http_task(vec![uri.clone()], task_options)
                .await
            {
                Ok(gid) => results.push(Value::Array(vec![Value::String(gid)])),
                Err(e) => results.push(json!({"code": 1, "message": e})),
            }
        }
    }

    // Check for errors
    let mut failed_count = 0usize;
    let mut first_error_message: Option<String> = None;

    for item in &results {
        if let Some(error_item) = find_multicall_item_error(item) {
            failed_count += 1;
            if first_error_message.is_none() {
                first_error_message = error_item
                    .get("message")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string());
            }
        }
    }

    if failed_count > 0 {
        let success_count = results.len().saturating_sub(failed_count);
        if success_count == 0 {
            return Err(first_error_message.unwrap_or_else(|| "task.new-task-fail".to_string()));
        }

        log::warn!(
            "[Motrix] add_uri partially failed: {} succeeded, {} failed",
            success_count,
            failed_count
        );
    }

    Ok(Value::Array(results))
}

#[tauri::command]
pub async fn sync_selected_task_order(
    _state: tauri::State<'_, crate::state::AppState>,
    direction: String,
    selected_tasks: Vec<SelectedTaskOrderInput>,
) -> Result<SyncOrderResult, String> {
    let normalized_direction = direction.trim().to_ascii_lowercase();
    if normalized_direction != "up" && normalized_direction != "down" {
        return Err("Invalid direction".to_string());
    }

    let mut selected_gids = Vec::new();
    let mut seen_gids = HashSet::new();
    let mut selected_active_gids = Vec::new();
    for task in selected_tasks {
        let gid = task.gid.trim().to_string();
        if gid.is_empty() || !seen_gids.insert(gid.clone()) {
            continue;
        }

        if task.status.eq_ignore_ascii_case("active") {
            selected_active_gids.push(gid.clone());
        }
        selected_gids.push(gid);
    }

    if selected_gids.is_empty() {
        return Ok(SyncOrderResult {
            moved: 0,
            partial_error: false,
        });
    }

    let manager = engine::get_manager()
        .await
        .ok_or("Engine not running")?;

    let selected_gid_set: HashSet<String> = selected_gids.iter().cloned().collect();
    let mut sync_error = false;
    let mut moved: u32 = 0;

    // Pause active tasks first
    if !selected_active_gids.is_empty() {
        for gid in &selected_active_gids {
            if manager.pause(gid).await.is_err() {
                sync_error = true;
            }
        }
        // little delay for status transition
        sleep(Duration::from_millis(100)).await;
    }

    // Move tasks one position at a time, matching frontend behavior
    
    // For "up": iterate in forward order so earlier items move first
    //   preserving relative order among selected tasks
    // For "down": iterate in reverse order so later items move first
    let ordered_gids: Vec<String> = if normalized_direction == "up" {
        // Get current waiting queue order, filter to selected
        manager
            .get_waiting_gids_in_order(&selected_gid_set)
            .await
    } else {
        let mut v = manager
            .get_waiting_gids_in_order(&selected_gid_set)
            .await;
        v.reverse();
        v
    };

    for gid in &ordered_gids {
        let pos = if normalized_direction == "up" { -1i64 } else { 1i64 };
        match manager.change_position(gid, pos, "POS_CUR").await {
            Ok(_) => moved += 1,
            Err(_) => sync_error = true,
        }
    }

    // Unpause previously active tasks
    if !selected_active_gids.is_empty() {
        for gid in &selected_active_gids {
            if manager.unpause(gid).await.is_err() {
                sync_error = true;
            }
        }
    }

    Ok(SyncOrderResult {
        moved,
        partial_error: sync_error,
    })
}

// Tauri commands wrapping TaskManager for direct invoke() calls

const ENGINE_VERSION: &str = "motrix-engine/0.1";

#[tauri::command]
pub async fn tell_status(gid: String, keys: Option<Vec<String>>) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.tell_status(&gid, &keys.unwrap_or_default()).await
}

#[tauri::command]
pub async fn tell_active(keys: Option<Vec<String>>) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager.tell_active(&keys.unwrap_or_default()).await)
}

#[tauri::command]
pub async fn tell_waiting(
    offset: Option<i64>,
    num: Option<usize>,
    keys: Option<Vec<String>>,
) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager
        .tell_waiting(
            offset.unwrap_or(0),
            num.unwrap_or(1000),
            &keys.unwrap_or_default(),
        )
        .await)
}

#[tauri::command]
pub async fn tell_stopped(
    offset: Option<i64>,
    num: Option<usize>,
    keys: Option<Vec<String>>,
) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager
        .tell_stopped(
            offset.unwrap_or(0),
            num.unwrap_or(1000),
            &keys.unwrap_or_default(),
        )
        .await)
}

#[tauri::command]
pub async fn pause_task(gid: String) -> Result<String, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.pause(&gid).await?;
    Ok(gid)
}

#[tauri::command]
pub async fn unpause_task(gid: String) -> Result<String, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.unpause(&gid).await?;
    Ok(gid)
}

#[tauri::command]
pub async fn remove_task(gid: String) -> Result<String, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.remove(&gid).await?;
    Ok(gid)
}

#[tauri::command]
pub async fn change_option(gid: String, options: Value) -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    let opts = match options {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    manager.change_option(&gid, opts).await
}

#[tauri::command]
pub async fn change_global_option_engine(options: Value) -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    let opts = match options {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    manager.change_global_option(opts).await;
    Ok(())
}

#[tauri::command]
pub async fn get_option_engine(gid: String) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.get_option(&gid).await
}

#[tauri::command]
pub async fn get_global_option_engine() -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager.get_global_option().await)
}

#[tauri::command]
pub async fn get_global_stat() -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager.get_global_stat().await)
}

#[tauri::command]
pub async fn change_position(gid: String, pos: i64, how: String) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.change_position(&gid, pos, &how).await
}

#[tauri::command]
pub async fn save_session() -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.save_session().await
}

#[tauri::command]
pub async fn get_version() -> Result<Value, String> {
    Ok(json!({
        "version": ENGINE_VERSION,
        "enabledFeatures": [
            "HTTP",
            "HTTPS",
            "FTP",
            "FTPS",
            "SFTP",
            "BitTorrent",
            "JSON-RPC",
        ]
    }))
}

#[tauri::command]
pub async fn pause_all_tasks() -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.pause_all().await;
    Ok(())
}

#[tauri::command]
pub async fn unpause_all_tasks() -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.unpause_all().await;
    Ok(())
}

#[tauri::command]
pub async fn purge_download_result() -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.purge_download_result().await;
    let _ = manager.save_session().await;
    Ok(())
}

#[tauri::command]
pub async fn remove_download_result(gid: String) -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.remove_download_result(&gid).await?;
    let _ = manager.save_session().await;
    Ok(())
}

#[tauri::command]
pub async fn get_peers(gid: String) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager.get_peers(&gid).await)
}

#[tauri::command]
pub async fn multicall_engine(
    method: String,
    gids: Vec<String>,
    options: Option<Value>,
) -> Result<Value, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    let opts = match options {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let mut results: Vec<Value> = Vec::with_capacity(gids.len());
    for gid in &gids {
        let result = match method.as_str() {
            "motrix.changeOption" => {
                manager
                    .change_option(gid, opts.clone())
                    .await
                    .map(|_| Value::String("OK".into()))
            }
            "motrix.remove" => manager.remove(gid).await.map(|_| Value::String(gid.clone())),
            "motrix.pause" | "motrix.forcePause" => {
                manager.pause(gid).await.map(|_| Value::String(gid.clone()))
            }
            "motrix.unpause" => {
                manager
                    .unpause(gid)
                    .await
                    .map(|_| Value::String(gid.clone()))
            }
            _ => Err(format!("Unsupported multicall method: {}", method)),
        };
        match result {
            Ok(v) => results.push(Value::Array(vec![v])),
            Err(e) => results.push(json!({ "code": 1, "message": e })),
        }
    }
    Ok(Value::Array(results))
}
