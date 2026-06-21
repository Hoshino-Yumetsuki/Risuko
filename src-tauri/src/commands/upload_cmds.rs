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

/// Apply secrets pulled from the keychain back onto a sink config. Only
/// fields that are currently empty in the config are filled, so a user
/// updating one secret does not lose another secret that was left blank
/// (meaning "unchanged")
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
            if c.password.is_empty() {
                let v = s("password");
                if !v.is_empty() {
                    c.password = v;
                }
            }
        }
        SinkConfig::S3(c) => {
            if c.secret_access_key.is_empty() {
                let v = s("secretAccessKey");
                if !v.is_empty() {
                    c.secret_access_key = v;
                }
            }
        }
        SinkConfig::Sftp(c) => {
            if c.password.is_empty() {
                let pw = s("password");
                if !pw.is_empty() {
                    c.password = pw;
                }
            }
            if c.private_key.is_empty() {
                let pk = s("privateKey");
                if !pk.is_empty() {
                    c.private_key = pk;
                }
            }
        }
        SinkConfig::Ftp(c) => {
            if c.password.is_empty() {
                let v = s("password");
                if !v.is_empty() {
                    c.password = v;
                }
            }
        }
    }
}

/// Pull stored secrets from the vault (or the local fallback store when the
/// vault is unavailable) into the config when the incoming record left them
/// blank. Mirrors the engine's `merge_secrets` behavior (treat empty as
/// "unchanged") so editing a sink without retyping the password keeps it
/// working
async fn fill_from_vault(
    vault: &VaultManager,
    mgr: &UploadSinkManager,
    id: &str,
    config: &mut SinkConfig,
) {
    // Try vault first when enabled. If it returns a hit, use it. On
    // `Ok(None)` (no entry) or `Err(_)` (backend hiccup), fall through to
    // the durable local fallback so secrets that `persist_sink_secrets`
    // wrote there during a prior vault failure remain recoverable
    if vault.enabled() {
        match vault.get_sink(id) {
            Ok(Some(secrets)) => {
                apply_sink_secrets(config, &secrets);
                return;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Failed to load vault entry for sink {id}: {e}, trying fallback");
            }
        }
    }
    if let Some(secrets) = mgr.get_sink_secret_fallback(id).await {
        apply_sink_secrets(config, &secrets);
    }
}

/// Persist a record's secrets to the vault, or remove the entry when no
/// secret is set. When the vault is disabled or returns an error, secrets
/// are written to a durable fallback store inside the upload manager so
/// they survive restarts. Failures are logged but never block the
/// user-visible operation — the engine still has the runtime value in memory
async fn persist_sink_secrets(
    vault: &VaultManager,
    mgr: &UploadSinkManager,
    id: &str,
    config: &SinkConfig,
) {
    match extract_sink_secrets(config) {
        Some(v) => {
            if vault.enabled() {
                match vault.put_sink(id, &v) {
                    Ok(()) => {
                        // Vault succeeded — clear any stale fallback entry
                        if let Err(e) = mgr.remove_sink_secret_fallback(id).await {
                            tracing::warn!("Failed to clear stale fallback for sink {id}: {e}");
                        }
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to store sink secrets in vault for {id}: {e}, falling back"
                        );
                    }
                }
            }
            if let Err(e) = mgr.put_sink_secret_fallback(id, &v).await {
                tracing::warn!("Failed to store sink secrets in fallback for {id}: {e}");
            }
        }
        None => {
            if vault.enabled() {
                if let Err(e) = vault.remove_sink(id) {
                    tracing::warn!("Failed to clear vault entry for sink {id}: {e}");
                }
            }
            if let Err(e) = mgr.remove_sink_secret_fallback(id).await {
                tracing::warn!("Failed to clear fallback secrets for sink {id}: {e}");
            }
        }
    }
}

/// Rehydrate every loaded sink's secrets from the vault (or from the local
/// fallback store when the vault is unavailable). Called once at startup
/// after the upload manager loads its on-disk records (which omit secrets
/// by design — see `skip_serializing` on the protocol Configs)
pub async fn rehydrate_upload_sinks(mgr: &UploadSinkManager, vault: &VaultManager) {
    let sinks = mgr.list_sinks().await;
    for mut record in sinks {
        // Try vault first when enabled, but always fall through to the
        // local fallback when the vault has no entry or errors — secrets
        // written by `persist_sink_secrets` during a prior vault outage
        // would otherwise be unrecoverable
        let mut secrets: Option<Value> = None;
        if vault.enabled() {
            match vault.get_sink(&record.id) {
                Ok(Some(v)) => secrets = Some(v),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "Failed to load vault entry for sink {}: {e}, trying fallback",
                        record.id
                    );
                }
            }
        }
        if secrets.is_none() {
            secrets = mgr.get_sink_secret_fallback(&record.id).await;
        }
        let Some(secrets) = secrets else { continue };
        let id = record.id.clone();
        apply_sink_secrets(&mut record.config, &secrets);
        if let Err(e) = mgr.update_sink(record).await {
            tracing::warn!("Failed to inject secrets into upload manager for sink {id}: {e}");
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
    persist_sink_secrets(&vault, &mgr, &created.id, &created.config).await;
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
    fill_from_vault(&vault, &mgr, &record.id, &mut record.config).await;
    let id = record.id.clone();
    let config_for_vault = record.config.clone();
    mgr.update_sink(record).await?;
    persist_sink_secrets(&vault, &mgr, &id, &config_for_vault).await;
    Ok(())
}

#[tauri::command]
pub async fn remove_upload_sink(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mgr = get_mgr(&state)?;
    let vault = state.vault.clone();
    mgr.remove_sink(&id).await?;
    if let Err(e) = vault.remove_sink(&id) {
        tracing::warn!("Failed to remove vault entry for sink {id}: {e}");
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

    /// Regression: when the vault is enabled but its read returns no entry
    /// (or errors), `fill_from_vault` must still consult the local
    /// fallback store. Without this, secrets that `persist_sink_secrets`
    /// wrote to the fallback during a prior vault outage become
    /// unrecoverable on the very next edit
    #[tokio::test]
    async fn fill_from_vault_falls_back_when_vault_misses() {
        use risuko_engine::traits::{FileStorage, NoopEventSink};
        use std::sync::Arc;

        let dir = tempfile::TempDir::new().unwrap();
        let storage: Arc<dyn risuko_engine::traits::StorageBackend> =
            Arc::new(FileStorage::new(dir.path().to_path_buf()));
        let event_sink: Arc<dyn risuko_engine::traits::EventSink> = Arc::new(NoopEventSink);
        let mgr = UploadSinkManager::new(storage, event_sink);

        // Seed the manager's fallback store with a secret for an id the
        // OS keychain cannot possibly know about (random uuid-ish suffix)
        let id = format!("test-sink-{}-{}", std::process::id(), uuid_like());
        let stored = json!({"password": "from-fallback", "privateKey": "pk"});
        mgr.put_sink_secret_fallback(&id, &stored).await.unwrap();

        // Force the vault into "enabled" without a real keychain probe.
        // `vault.get_sink(&id)` will then return either Ok(None) (backend
        // reachable, no entry) or Err(_) (no backend) — both code paths
        // we want to confirm fall through to the fallback
        let vault = crate::managers::vault::VaultManager::for_test(true);
        assert!(vault.enabled());

        let mut cfg = sftp("", "");
        fill_from_vault(&vault, &mgr, &id, &mut cfg).await;
        match cfg {
            SinkConfig::Sftp(c) => {
                assert_eq!(c.password, "from-fallback", "fallback password not applied");
                assert_eq!(c.private_key, "pk", "fallback private key not applied");
            }
            _ => panic!("expected sftp"),
        }
    }

    /// Tiny helper: avoids pulling in the `uuid` crate just for a unique
    /// string in a single test
    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        format!(
            "{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )
    }
}
