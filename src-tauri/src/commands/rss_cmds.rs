use std::sync::Arc;

use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, State};

use crate::engine::rss::types::RssRule;
use crate::engine::rss::RssManager;
use crate::state::AppState;

/// Strip the explicit `out` filename hint when fanning extra inline media URLs
/// out as separate http tasks — otherwise every queued image would land at the
/// same path and clobber each other. The `dir` and other routing options stay
fn strip_explicit_filename(opts: &Map<String, Value>) -> Map<String, Value> {
    let mut cloned = opts.clone();
    cloned.remove("out");
    cloned
}

/// Write an Article-kind RSS item's body as a self-contained HTML file under
/// `dir`, mark it downloaded, then queue every inline media URL (the primary
/// enclosure plus scraped `media_urls`) as separate http tasks alongside it.
/// Returns the saved file path and the list of fanned-out media gids.
///
/// Used by both the one-shot and tracked download paths so they stay in lockstep
async fn write_article_and_fan_media(
    mgr: &Arc<RssManager>,
    manager: &Arc<crate::engine::manager::TaskManager>,
    item: &crate::engine::rss::types::RssItem,
    feed_id: &str,
    item_id: &str,
    dir: &str,
    opts: &Map<String, Value>,
) -> Result<(String, Vec<String>), String> {
    let filename = crate::engine::rss::article_filename(item);
    let html = crate::engine::rss::build_article_html(item);
    let dir_path = std::path::Path::new(dir);
    tokio::fs::create_dir_all(dir_path)
        .await
        .map_err(|e| format!("Failed to create dir {}: {}", dir, e))?;
    let file_path = dir_path.join(&filename);
    let path_str = file_path.to_string_lossy().to_string();
    tokio::fs::write(&file_path, html.as_bytes())
        .await
        .map_err(|e| format!("Failed to write article html: {}", e))?;

    mgr.mark_item_downloaded(feed_id, item_id, Some(path_str.clone()))
        .await?;

    let media_opts = strip_explicit_filename(opts);
    let mut extra_gids: Vec<String> = Vec::new();
    let mut queued: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for url in item
        .enclosure_url
        .iter()
        .cloned()
        .chain(item.media_urls.iter().cloned())
    {
        if !queued.insert(url.clone()) {
            continue;
        }
        match manager
            .add_http_task(vec![url.clone()], media_opts.clone())
            .await
        {
            Ok(g) => extra_gids.push(g),
            Err(e) => tracing::warn!("Failed to queue inline media {}: {}", url, e),
        }
    }
    Ok((path_str, extra_gids))
}

#[tauri::command]
pub async fn add_rss_feed(state: State<'_, AppState>, url: String) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let feed = mgr.add_feed(&url).await?;
    serde_json::to_value(feed).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_rss_feed(state: State<'_, AppState>, feed_id: String) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.remove_feed(&feed_id).await
}

#[tauri::command]
pub async fn refresh_rss_feed(
    state: State<'_, AppState>,
    feed_id: String,
) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let items = mgr.update_feed(&feed_id).await?;
    serde_json::to_value(items).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn refresh_all_rss_feeds(state: State<'_, AppState>) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.update_all_feeds().await;
    Ok(())
}

#[tauri::command]
pub async fn get_rss_feeds(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let feeds = mgr.get_feeds().await;
    serde_json::to_value(feeds).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_rss_items(state: State<'_, AppState>, feed_id: String) -> Result<Value, String> {
    let mgr = state.rss.clone();
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
    let mgr = state.rss.clone();
    mgr.update_feed_settings(&feed_id, interval, is_active)
        .await
}

#[tauri::command]
pub async fn add_rss_rule(state: State<'_, AppState>, rule: RssRule) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let created = mgr.add_rule(rule).await?;
    serde_json::to_value(created).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_rss_rule(state: State<'_, AppState>, rule_id: String) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.remove_rule(&rule_id).await
}

#[tauri::command]
pub async fn get_rss_rules(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let rules = mgr.get_rules().await;
    serde_json::to_value(rules).map_err(|e| e.to_string())
}

/// Resolve the destination directory from per-task options, falling back to
/// the engine's global default
async fn resolve_dir(
    opts: &Map<String, Value>,
    manager: &std::sync::Arc<crate::engine::manager::TaskManager>,
) -> String {
    if let Some(d) = opts.get("dir").and_then(|v| v.as_str()) {
        return d.to_string();
    }
    let global = manager.get_global_option().await;
    global
        .get("dir")
        .and_then(|v| v.as_str())
        .unwrap_or(".")
        .to_string()
}

#[tauri::command]
pub async fn delete_rss_items(
    state: State<'_, AppState>,
    items_by_feed: Vec<(String, Vec<String>)>,
) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.delete_items(items_by_feed).await
}

#[tauri::command]
pub async fn mark_rss_downloaded(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
    download_path: Option<String>,
) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.mark_item_downloaded(&feed_id, &item_id, download_path)
        .await
}

#[tauri::command]
pub async fn clear_rss_download(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.clear_item_download(&feed_id, &item_id).await
}

#[tauri::command]
pub async fn read_rss_download(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
) -> Result<String, String> {
    let mgr = state.rss.clone();
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
    let mgr = state.rss.clone();
    let item = mgr.get_item(&feed_id, &item_id).await?;

    let opts = match options {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let manager = crate::engine::get_manager()
        .await
        .ok_or("Engine not running")?;

    let dir = resolve_dir(&opts, &manager).await;
    let kind = crate::engine::rss::classify_item_kind(&item);

    if kind == crate::engine::rss::ItemKind::Article {
        // Article: write the HTML synchronously, queue inline media, fire the
        // completion event immediately. There is no engine gid for the html
        // file itself, but the inline media tasks behave like normal http
        // downloads
        let (path_str, extra_gids) =
            write_article_and_fan_media(&mgr, &manager, &item, &feed_id, &item_id, &dir, &opts)
                .await?;

        let _ = handle.emit(
            "rss:download-complete",
            serde_json::json!({
                "feedId": feed_id,
                "itemId": item_id,
                "gid": Value::Null,
                "downloadPath": path_str,
                "kind": "article",
            }),
        );

        return Ok(serde_json::json!({
            "gid": Value::Null,
            "extraGids": extra_gids,
            "downloadPath": path_str,
            "kind": "article",
        }));
    }

    let urls = mgr.get_item_download_urls(&feed_id, &item_id).await?;

    let download_path = opts.get("out").and_then(|v| v.as_str()).map(|out| {
        std::path::Path::new(&dir)
            .join(out)
            .to_string_lossy()
            .to_string()
    });

    let mut iter = urls.into_iter();
    let primary_url = iter
        .next()
        .ok_or_else(|| "No downloadable URL found for this item".to_string())?;
    let gid = manager
        .add_http_task(vec![primary_url], opts.clone())
        .await?;

    let mut extra_gids: Vec<String> = Vec::new();
    let extra_opts = strip_explicit_filename(&opts);
    for url in iter {
        match manager
            .add_http_task(vec![url.clone()], extra_opts.clone())
            .await
        {
            Ok(g) => extra_gids.push(g),
            Err(e) => tracing::warn!("Failed to queue inline media {}: {}", url, e),
        }
    }

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
                .tell_status(
                    &monitor_gid,
                    &["status".to_string(), "dir".to_string(), "files".to_string()],
                )
                .await;

            let status_val = match status_result {
                Ok(val) => val,
                Err(_) => break,
            };
            let status = status_val
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            match status {
                "complete" => {
                    // Prefer the pre-computed path from opts; when absent, derive
                    // it from the completed task's file list reported by the engine
                    let resolved_path = monitor_download_path.clone().or_else(|| {
                        status_val
                            .get("files")
                            .and_then(|f| f.as_array())
                            .and_then(|a| a.first())
                            .and_then(|f| f.get("path"))
                            .and_then(|p| p.as_str())
                            .filter(|p| !p.is_empty())
                            .map(|p| p.to_string())
                    });
                    let _ = monitor_mgr
                        .mark_item_downloaded(
                            &monitor_feed_id,
                            &monitor_item_id,
                            resolved_path.clone(),
                        )
                        .await;
                    let _ = handle.emit(
                        "rss:download-complete",
                        serde_json::json!({
                            "feedId": monitor_feed_id,
                            "itemId": monitor_item_id,
                            "gid": monitor_gid,
                            "downloadPath": resolved_path,
                            "kind": "media",
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
        "extraGids": extra_gids,
        "downloadPath": download_path,
        "kind": "media",
    }))
}

#[tauri::command]
pub async fn update_rss_rule(state: State<'_, AppState>, rule: RssRule) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let updated = mgr.update_rule(rule).await?;
    serde_json::to_value(updated).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reorder_rss_rules(
    state: State<'_, AppState>,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.reorder_rules(ordered_ids).await
}

#[tauri::command]
pub async fn dry_run_rss_rule(
    state: State<'_, AppState>,
    rule: RssRule,
    sample_size: Option<usize>,
) -> Result<Value, String> {
    let mgr = state.rss.clone();
    let results = mgr.dry_run_rule(rule, sample_size.unwrap_or(50)).await;
    serde_json::to_value(results).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn parse_rss_item_title(title: String) -> Result<Value, String> {
    let parsed = crate::engine::rss::parser::parse_title(&title);
    serde_json::to_value(parsed).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mark_rss_item_read(
    state: State<'_, AppState>,
    feed_id: String,
    item_id: String,
) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.mark_item_read(&feed_id, &item_id).await
}

#[tauri::command]
pub async fn mark_rss_items_read(
    state: State<'_, AppState>,
    entries: Vec<(String, String)>,
) -> Result<(), String> {
    let mgr = state.rss.clone();
    mgr.mark_items_read(entries).await
}
