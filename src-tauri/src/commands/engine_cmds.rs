use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;
use std::{collections::HashMap, collections::HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::AppHandle;
use tokio::task::spawn_blocking;
use tokio::time::sleep;

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

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut index = 0usize;

    while index < input.len() {
        let b0 = input[index];
        let b1 = if index + 1 < input.len() {
            input[index + 1]
        } else {
            0
        };
        let b2 = if index + 2 < input.len() {
            input[index + 2]
        } else {
            0
        };

        output.push(TABLE[(b0 >> 2) as usize] as char);
        output.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);

        if index + 1 < input.len() {
            output.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            output.push('=');
        }

        if index + 2 < input.len() {
            output.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }

        index += 3;
    }

    output
}

fn resolve_rpc_endpoint(
    state: &tauri::State<'_, crate::state::AppState>,
) -> Result<(String, u16, String), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    let user = config.get_user_config();
    let system = config.get_system_config();

    let host = user
        .get("rpc-host")
        .and_then(|value| value.as_str())
        .unwrap_or("127.0.0.1")
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();

    let port = system
        .get("rpc-listen-port")
        .and_then(|value| value.as_u64())
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(16800);

    let secret = system
        .get("rpc-secret")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();

    Ok((host, port, secret))
}

fn split_http_response(raw: &[u8]) -> Result<&[u8], String> {
    let marker = b"\r\n\r\n";
    let Some(pos) = raw
        .windows(marker.len())
        .position(|window| window == marker)
    else {
        return Err("Invalid aria2 RPC response".to_string());
    };
    let header_end = pos + marker.len();
    let headers = &raw[..header_end];
    let status_ok = headers.starts_with(b"HTTP/1.1 200") || headers.starts_with(b"HTTP/1.0 200");
    if !status_ok {
        return Err("aria2 RPC returned non-200 status".to_string());
    }
    Ok(&raw[header_end..])
}

fn call_aria2_rpc(host: &str, port: u16, body: &Value) -> Result<Value, String> {
    let normalized_host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if normalized_host.is_empty() {
        return Err("Invalid RPC host".to_string());
    }

    let mut stream = TcpStream::connect((normalized_host, port))
        .map_err(|e| format!("RPC connect failed: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;

    let payload = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let request = format!(
        "POST /jsonrpc HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("RPC write failed: {e}"))?;
    stream
        .write_all(&payload)
        .map_err(|e| format!("RPC write failed: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("RPC flush failed: {e}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| format!("RPC read failed: {e}"))?;

    let body_bytes = split_http_response(&response)?;
    serde_json::from_slice::<Value>(body_bytes).map_err(|e| format!("Invalid RPC JSON: {e}"))
}

fn should_retry_add_torrent_rpc(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("connection reset by peer")
        || normalized.contains("broken pipe")
        || normalized.contains("connection aborted")
}

fn parse_add_torrent_response(response: Value) -> Result<String, String> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("task.new-task-fail");
        return Err(message.to_string());
    }

    response
        .get("result")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .ok_or_else(|| "task.new-task-fail".to_string())
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
pub fn get_engine_status(state: tauri::State<'_, crate::state::AppState>) -> Result<bool, String> {
    let running = state.engine_running.lock().map_err(|e| e.to_string())?;
    Ok(*running)
}

#[tauri::command]
pub async fn add_torrent_by_path(
    handle: AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
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

    let (host, port, secret) = resolve_rpc_endpoint(&state)?;
    let mut params = Vec::new();
    if !secret.is_empty() {
        params.push(Value::String(format!("token:{secret}")));
    }

    params.push(Value::String(encode_base64(&bytes)));
    params.push(Value::Array(Vec::new()));

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
    let options = Value::Object(options);
    params.push(options);

    let payload = json!({
        "jsonrpc": "2.0",
        "id": "motrix-tauri",
        "method": "aria2.addTorrent",
        "params": params,
    });

    let mut retried_after_restart = false;
    loop {
        let host_for_call = host.clone();
        let payload_for_call = payload.clone();
        let rpc_result =
            spawn_blocking(move || call_aria2_rpc(&host_for_call, port, &payload_for_call))
                .await
                .map_err(|e| format!("RPC task failed: {e}"))?;
        match rpc_result {
            Ok(response) => return parse_add_torrent_response(response),
            Err(err) => {
                if retried_after_restart || !should_retry_add_torrent_rpc(&err) {
                    return Err(err);
                }
                retried_after_restart = true;
                crate::engine::restart_engine(&handle)
                    .await
                    .map_err(|e| e.to_string())?;
                sleep(Duration::from_millis(500)).await;
            }
        }
    }
}
