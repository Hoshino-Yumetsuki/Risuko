use std::sync::Arc;

use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, State};

use crate::engine::rss::types::RssRule;
use crate::engine::rss::RssManager;
use crate::state::AppState;

fn get_rss(state: &State<'_, AppState>) -> Result<Arc<RssManager>, String> {
    state
        .rss
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "RSS manager not initialized".to_string())
}

#[tauri::command]
pub async fn add_rss_feed(state: State<'_, AppState>, url: String) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let feed = mgr.add_feed(&url).await?;
    serde_json::to_value(feed).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_rss_feed(state: State<'_, AppState>, feed_id: String) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.remove_feed(&feed_id).await
}

#[tauri::command]
pub async fn refresh_rss_feed(
    state: State<'_, AppState>,
    feed_id: String,
) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let items = mgr.update_feed(&feed_id).await?;
    serde_json::to_value(items).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn refresh_all_rss_feeds(state: State<'_, AppState>) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.update_all_feeds().await;
    Ok(())
}

#[tauri::command]
pub async fn get_rss_feeds(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let feeds = mgr.get_feeds().await;
    serde_json::to_value(feeds).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_rss_items(state: State<'_, AppState>, feed_id: String) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let items = mgr.get_items(&feed_id).await;
    serde_json::to_value(items).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_rss_feed_settings(
    state: State<'_, AppState>,
    feed_id: String,
    interval: Option<u64>,
    is_active: Option<bool>,
) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.update_feed_settings(&feed_id, interval, is_active)
        .await
}

#[tauri::command]
pub async fn add_rss_rule(state: State<'_, AppState>, rule: RssRule) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let created = mgr.add_rule(rule).await?;
    serde_json::to_value(created).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_rss_rule(state: State<'_, AppState>, rule_id: String) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.remove_rule(&rule_id).await
}

#[tauri::command]
pub async fn get_rss_rules(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let rules = mgr.get_rules().await;
    serde_json::to_value(rules).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn download_rss_item(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
    options: Option<Value>,
) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let url = mgr.get_item_download_url(&feed_id, &item_id).await?;

    let opts = match options {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let manager = crate::engine::get_manager()
        .await
        .ok_or("Engine not running")?;

    // Compute the download path for tracking
    let download_path = if let Some(out) = opts.get("out").and_then(|v| v.as_str()) {
        let dir = if let Some(d) = opts.get("dir").and_then(|v| v.as_str()) {
            d.to_string()
        } else {
            let global = manager.get_global_option().await;
            global
                .get("dir")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string()
        };
        let p = std::path::Path::new(&dir).join(out);
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };

    let gid = manager.add_http_task(vec![url], opts).await?;

    Ok(serde_json::json!({
        "gid": gid,
        "downloadPath": download_path
    }))
}

#[tauri::command]
pub async fn delete_rss_items(
    state: State<'_, AppState>,
    items_by_feed: Vec<(String, Vec<String>)>,
) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.delete_items(items_by_feed).await
}

#[tauri::command]
pub async fn mark_rss_downloaded(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
    download_path: Option<String>,
) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.mark_item_downloaded(&feed_id, &item_id, download_path)
        .await
}

#[tauri::command]
pub async fn clear_rss_download(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
) -> Result<(), String> {
    let mgr = get_rss(&state)?;
    mgr.clear_item_download(&feed_id, &item_id).await
}

#[tauri::command]
pub async fn read_rss_download(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
) -> Result<String, String> {
    let mgr = get_rss(&state)?;
    let path = mgr.get_item_download_path(&feed_id, &item_id).await?;
    let p = std::path::Path::new(&path);

    // Also check for .part file (download renames from this on completion)
    let actual = if p.exists() {
        p.to_path_buf()
    } else {
        let part = p.with_file_name(format!(
            "{}.part",
            p.file_name().and_then(|n| n.to_str()).unwrap_or("download")
        ));
        if part.exists() {
            part
        } else {
            return Err(format!("Downloaded file not found: {}", path));
        }
    };

    tokio::fs::read_to_string(&actual)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))
}

/// Start an RSS item download and monitor its status in a background task
/// Returns immediately with the gid. Emits `rss:download-complete` or
/// `rss:download-error` events when the download finishes
#[tauri::command]
pub async fn download_rss_item_tracked(
    handle: AppHandle,
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
    options: Option<Value>,
) -> Result<Value, String> {
    let mgr = get_rss(&state)?;
    let url = mgr.get_item_download_url(&feed_id, &item_id).await?;

    let opts = match options {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let manager = crate::engine::get_manager()
        .await
        .ok_or("Engine not running")?;

    let download_path = if let Some(out) = opts.get("out").and_then(|v| v.as_str()) {
        let dir = if let Some(d) = opts.get("dir").and_then(|v| v.as_str()) {
            d.to_string()
        } else {
            let global = manager.get_global_option().await;
            global
                .get("dir")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string()
        };
        let p = std::path::Path::new(&dir).join(out);
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };

    let gid = manager.add_http_task(vec![url], opts).await?;

    // Spawn a background task to monitor download completion
    let monitor_gid = gid.clone();
    let monitor_feed_id = feed_id.clone();
    let monitor_item_id = item_id.clone();
    let monitor_download_path = download_path.clone();
    let monitor_mgr = mgr.clone();

    tauri::async_runtime::spawn(async move {
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
        const MAX_POLLS: usize = 3600;

        for _ in 0..MAX_POLLS {
            tokio::time::sleep(POLL_INTERVAL).await;

            let engine = match crate::engine::get_manager().await {
                Some(m) => m,
                None => break,
            };

            let status_result = engine
                .tell_status(&monitor_gid, &["status".to_string()])
                .await;

            let status = match &status_result {
                Ok(val) => val.get("status").and_then(|s| s.as_str()).unwrap_or(""),
                Err(_) => break,
            };

            match status {
                "complete" => {
                    let _ = monitor_mgr
                        .mark_item_downloaded(
                            &monitor_feed_id,
                            &monitor_item_id,
                            monitor_download_path.clone(),
                        )
                        .await;
                    let _ = handle.emit(
                        "rss:download-complete",
                        serde_json::json!({
                            "feedId": monitor_feed_id,
                            "itemId": monitor_item_id,
                            "gid": monitor_gid,
                            "downloadPath": monitor_download_path,
                        }),
                    );
                    return;
                }
                "error" | "removed" => {
                    let _ = handle.emit(
                        "rss:download-error",
                        serde_json::json!({
                            "feedId": monitor_feed_id,
                            "itemId": monitor_item_id,
                            "gid": monitor_gid,
                        }),
                    );
                    return;
                }
                _ => {}
            }
        }

        // Timed out
        let _ = handle.emit(
            "rss:download-error",
            serde_json::json!({
                "feedId": monitor_feed_id,
                "itemId": monitor_item_id,
                "gid": monitor_gid,
                "reason": "timeout",
            }),
        );
    });

    Ok(serde_json::json!({
        "gid": gid,
        "downloadPath": download_path,
    }))
}
