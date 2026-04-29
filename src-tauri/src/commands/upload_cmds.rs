//! Tauri commands for cloud upload sinks. The handler shape mirrors
//! `rss_cmds` so the frontend wrapper layer stays consistent

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use risuko_engine::engine::upload::{UploadRule, UploadSinkManager, UploadSinkRecord};

use crate::state::AppState;

fn get_mgr(state: &State<'_, AppState>) -> Result<Arc<UploadSinkManager>, String> {
    state
        .upload_sinks
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "Upload manager not initialized".to_string())
}

// -- sinks

#[tauri::command]
pub async fn list_upload_sinks(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = get_mgr(&state)?;
    let sinks = mgr.list_sinks().await;
    serde_json::to_value(sinks).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_upload_sink(
    state: State<'_, AppState>,
    record: UploadSinkRecord,
) -> Result<Value, String> {
    let mgr = get_mgr(&state)?;
    let created = mgr.add_sink(record).await?;
    serde_json::to_value(created).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_upload_sink(
    state: State<'_, AppState>,
    record: UploadSinkRecord,
) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.update_sink(record).await
}

#[tauri::command]
pub async fn remove_upload_sink(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.remove_sink(&id).await
}

#[tauri::command]
pub async fn test_upload_sink(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.test_sink(&id).await
}

#[tauri::command]
pub async fn get_default_upload_sink(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let mgr = get_mgr(&state)?;
    Ok(mgr.default_sink_id().await)
}

#[tauri::command]
pub async fn set_default_upload_sink(
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.set_default_sink(id).await
}

#[tauri::command]
pub async fn set_upload_max_concurrency(
    state: State<'_, AppState>,
    n: usize,
) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.set_max_concurrency(n).await
}

// -- rules

#[tauri::command]
pub async fn list_upload_rules(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = get_mgr(&state)?;
    let rules = mgr.list_rules().await;
    serde_json::to_value(rules).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_upload_rule(
    state: State<'_, AppState>,
    rule: UploadRule,
) -> Result<Value, String> {
    let mgr = get_mgr(&state)?;
    let created = mgr.add_rule(rule).await?;
    serde_json::to_value(created).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_upload_rule(
    state: State<'_, AppState>,
    rule: UploadRule,
) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.update_rule(rule).await
}

#[tauri::command]
pub async fn remove_upload_rule(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.remove_rule(&id).await
}

// -- jobs

#[tauri::command]
pub async fn list_upload_jobs(state: State<'_, AppState>) -> Result<Value, String> {
    let mgr = get_mgr(&state)?;
    let jobs = mgr.list_jobs().await;
    serde_json::to_value(jobs).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cancel_upload_job(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.cancel_job(&id).await
}

#[tauri::command]
pub async fn clear_upload_history(state: State<'_, AppState>) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    mgr.clear_history().await;
    Ok(())
}
