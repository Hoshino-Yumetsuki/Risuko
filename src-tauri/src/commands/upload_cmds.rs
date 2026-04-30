//! Tauri commands for cloud upload sinks. The handler shape mirrors
//! `rss_cmds` so the frontend wrapper layer stays consistent

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::State;

use risuko_engine::engine::upload::{SinkConfig, UploadRule, UploadSinkManager, UploadSinkRecord};

use crate::managers::vault::VaultManager;
use crate::state::AppState;

fn get_mgr(state: &State<'_, AppState>) -> Result<Arc<UploadSinkManager>, String> {
    state
        .upload_sinks
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "Upload manager not initialized".to_string())
}

/// Extract the sensitive fields from a sink config into a JSON object
/// suitable for keychain storage. Returns `None` when no secret is set
/// (so callers can `remove()` instead of writing an empty object)
fn extract_sink_secrets(config: &SinkConfig) -> Option<Value> {
    let mut obj = serde_json::Map::new();
    match config {
        SinkConfig::Webdav(c) => {
            if !c.password.is_empty() {
                obj.insert("password".into(), json!(c.password));
            }
        }
        SinkConfig::S3(c) => {
            if !c.secret_access_key.is_empty() {
                obj.insert("secretAccessKey".into(), json!(c.secret_access_key));
            }
        }
        SinkConfig::Sftp(c) => {
            if !c.password.is_empty() {
                obj.insert("password".into(), json!(c.password));
            }
            if !c.private_key.is_empty() {
                obj.insert("privateKey".into(), json!(c.private_key));
            }
        }
        SinkConfig::Ftp(c) => {
            if !c.password.is_empty() {
                obj.insert("password".into(), json!(c.password));
            }
        }
    }
    if obj.is_empty() {
        None
    } else {
        Some(Value::Object(obj))
    }
}

/// Apply secrets pulled from the keychain back onto a sink config. Empty
/// or missing fields are silently ignored so a partial vault entry never
/// clobbers a value the user just typed
fn apply_sink_secrets(config: &mut SinkConfig, secrets: &Value) {
    let obj = match secrets.as_object() {
        Some(o) => o,
        None => return,
    };
    let s = |k: &str| {
        obj.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match config {
        SinkConfig::Webdav(c) => {
            let v = s("password");
            if !v.is_empty() {
                c.password = v;
            }
        }
        SinkConfig::S3(c) => {
            let v = s("secretAccessKey");
            if !v.is_empty() {
                c.secret_access_key = v;
            }
        }
        SinkConfig::Sftp(c) => {
            let pw = s("password");
            if !pw.is_empty() {
                c.password = pw;
            }
            let pk = s("privateKey");
            if !pk.is_empty() {
                c.private_key = pk;
            }
        }
        SinkConfig::Ftp(c) => {
            let v = s("password");
            if !v.is_empty() {
                c.password = v;
            }
        }
    }
}

/// Whether the sink config currently carries any plaintext secret. Used
/// when deciding whether to clear a stale vault entry
fn sink_has_secrets(config: &SinkConfig) -> bool {
    match config {
        SinkConfig::Webdav(c) => !c.password.is_empty(),
        SinkConfig::S3(c) => !c.secret_access_key.is_empty(),
        SinkConfig::Sftp(c) => !c.password.is_empty() || !c.private_key.is_empty(),
        SinkConfig::Ftp(c) => !c.password.is_empty(),
    }
}

/// Pull stored secrets from the vault into the config when the incoming
/// record left them blank. Mirrors the engine's `merge_secrets` behavior
/// (treat empty as "unchanged") so editing a sink without retyping the
/// password keeps it working
fn fill_from_vault(vault: &VaultManager, id: &str, config: &mut SinkConfig) {
    if !vault.enabled() || sink_has_secrets(config) {
        return;
    }
    if let Ok(Some(secrets)) = vault.get_sink(id) {
        apply_sink_secrets(config, &secrets);
    }
}

/// Persist a record's secrets to the vault, or remove the entry when no
/// secret is set. Failures are logged but never block the user-visible
/// operation — the engine still has the runtime value in memory
fn persist_sink_secrets(vault: &VaultManager, id: &str, config: &SinkConfig) {
    if !vault.enabled() {
        return;
    }
    match extract_sink_secrets(config) {
        Some(v) => {
            if let Err(e) = vault.put_sink(id, &v) {
                log::warn!("Failed to store sink secrets in vault for {id}: {e}");
            }
        }
        None => {
            if let Err(e) = vault.remove_sink(id) {
                log::warn!("Failed to clear vault entry for sink {id}: {e}");
            }
        }
    }
}

/// Rehydrate every loaded sink's secrets from the vault. Called once at
/// startup after the upload manager loads its on-disk records (which omit
/// secrets by design — see `skip_serializing` on the protocol Configs)
pub async fn rehydrate_upload_sinks(mgr: &UploadSinkManager, vault: &VaultManager) {
    if !vault.enabled() {
        return;
    }
    let sinks = mgr.list_sinks().await;
    for mut record in sinks {
        let secrets = match vault.get_sink(&record.id) {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("Failed to load vault entry for sink {}: {e}", record.id);
                continue;
            }
        };
        apply_sink_secrets(&mut record.config, &secrets);
        if let Err(e) = mgr.update_sink(record).await {
            log::warn!("Failed to inject vault secrets into upload manager: {e}");
        }
    }
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
    let vault = state.vault.clone();
    let created = mgr.add_sink(record).await?;
    persist_sink_secrets(&vault, &created.id, &created.config);
    serde_json::to_value(created).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_upload_sink(
    state: State<'_, AppState>,
    mut record: UploadSinkRecord,
) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    let vault = state.vault.clone();
    // Empty incoming secrets mean "unchanged" — fill from vault before the
    // engine's own merge_secrets fallback runs against a disk copy that
    // never held the secret in the first place
    fill_from_vault(&vault, &record.id, &mut record.config);
    let id = record.id.clone();
    let config_for_vault = record.config.clone();
    mgr.update_sink(record).await?;
    persist_sink_secrets(&vault, &id, &config_for_vault);
    Ok(())
}

#[tauri::command]
pub async fn remove_upload_sink(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    let vault = state.vault.clone();
    mgr.remove_sink(&id).await?;
    if let Err(e) = vault.remove_sink(&id) {
        log::warn!("Failed to remove vault entry for sink {id}: {e}");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use risuko_engine::engine::upload::{FtpConfig, S3Config, SftpConfig, WebdavConfig};

    fn sftp(password: &str, private_key: &str) -> SinkConfig {
        SinkConfig::Sftp(SftpConfig {
            host: "h".into(),
            port: 22,
            username: "u".into(),
            password: password.into(),
            private_key: private_key.into(),
            base_path: String::new(),
        })
    }

    #[test]
    fn extract_returns_none_when_empty() {
        assert!(extract_sink_secrets(&sftp("", "")).is_none());
        assert!(extract_sink_secrets(&SinkConfig::Ftp(FtpConfig {
            host: "h".into(),
            port: 21,
            username: String::new(),
            password: String::new(),
            base_path: String::new(),
            secure: false,
            insecure: false,
        }))
        .is_none());
    }

    #[test]
    fn extract_picks_up_sftp_secrets() {
        let v = extract_sink_secrets(&sftp("p", "k")).unwrap();
        assert_eq!(v["password"], "p");
        assert_eq!(v["privateKey"], "k");
    }

    #[test]
    fn extract_picks_up_s3_secret() {
        let cfg = SinkConfig::S3(S3Config {
            endpoint: "e".into(),
            region: String::new(),
            bucket: "b".into(),
            access_key_id: "a".into(),
            secret_access_key: "shh".into(),
            prefix: String::new(),
            force_path_style: false,
        });
        let v = extract_sink_secrets(&cfg).unwrap();
        assert_eq!(v["secretAccessKey"], "shh");
    }

    #[test]
    fn apply_round_trips_sftp() {
        let mut cfg = sftp("", "");
        let payload = serde_json::json!({"password": "p", "privateKey": "k"});
        apply_sink_secrets(&mut cfg, &payload);
        match cfg {
            SinkConfig::Sftp(c) => {
                assert_eq!(c.password, "p");
                assert_eq!(c.private_key, "k");
            }
            _ => panic!("expected sftp"),
        }
    }

    #[test]
    fn apply_does_not_clobber_with_empty() {
        let mut cfg = sftp("existing", "");
        // Empty fields in payload must NOT overwrite a value already typed.
        let payload = serde_json::json!({"password": "", "privateKey": ""});
        apply_sink_secrets(&mut cfg, &payload);
        match cfg {
            SinkConfig::Sftp(c) => assert_eq!(c.password, "existing"),
            _ => panic!("expected sftp"),
        }
    }

    #[test]
    fn apply_ignores_unrelated_fields() {
        let mut cfg = SinkConfig::Webdav(WebdavConfig {
            endpoint: "e".into(),
            base_path: String::new(),
            username: String::new(),
            password: String::new(),
            insecure: false,
        });
        let payload = serde_json::json!({"privateKey": "x"}); // wrong field
        apply_sink_secrets(&mut cfg, &payload);
        match cfg {
            SinkConfig::Webdav(c) => assert_eq!(c.password, ""),
            _ => panic!("expected webdav"),
        }
    }

    #[test]
    fn sink_has_secrets_detects_each_variant() {
        assert!(!sink_has_secrets(&sftp("", "")));
        assert!(sink_has_secrets(&sftp("p", "")));
        assert!(sink_has_secrets(&sftp("", "k")));
    }
}
