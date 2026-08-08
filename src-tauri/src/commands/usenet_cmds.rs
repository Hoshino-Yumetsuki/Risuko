//! Usenet provider commands

use risuko_engine::engine::usenet::{UsenetCredentialResolver, UsenetProviderProfile};
use risuko_engine::engine::usenet_transport::NntpConnection;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

use crate::state::AppState;

fn credential_key(profile_id: &str) -> String {
    format!("usenet:{profile_id}")
}

pub struct VaultCredentialResolver {
    vault: Arc<crate::managers::vault::VaultManager>,
    fallback: risuko_engine::engine::FileUsenetCredentialResolver,
}

impl VaultCredentialResolver {
    #[allow(dead_code)]
    pub fn new(vault: Arc<crate::managers::vault::VaultManager>) -> Self {
        let config_dir = dirs::config_dir()
            .map(|path| path.join("dev.risuko.app"))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::with_config_dir(vault, config_dir)
    }

    pub fn with_config_dir(
        vault: Arc<crate::managers::vault::VaultManager>,
        config_dir: PathBuf,
    ) -> Self {
        Self {
            vault,
            fallback: risuko_engine::engine::FileUsenetCredentialResolver::new(config_dir),
        }
    }
}

#[async_trait::async_trait]
impl risuko_engine::engine::usenet::UsenetCredentialResolver for VaultCredentialResolver {
    async fn resolve(
        &self,
        profile_id: &str,
    ) -> Result<Option<risuko_engine::engine::usenet::UsenetCredentials>, String> {
        if self.vault.enabled() {
            match self.vault.get(&credential_key(profile_id)) {
                Ok(Some(value)) => {
                    let username = value
                        .get("username")
                        .and_then(|value| value.as_str())
                        .map(str::to_string);
                    let password = value
                        .get("password")
                        .and_then(|value| value.as_str())
                        .map(str::to_string);
                    return Ok(Some(risuko_engine::engine::usenet::UsenetCredentials {
                        username,
                        password,
                    }));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        "Failed to load Usenet credentials for {profile_id}: {error}; trying fallback"
                    );
                }
            }
        }
        self.fallback.resolve(profile_id).await
    }
}

fn load_fallback(path: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data).map_err(|e| e.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn save_fallback(path: &Path, profile_id: &str, credentials: &Value) -> Result<(), String> {
    let mut entries = load_fallback(path)?;
    entries.insert(profile_id.to_string(), credentials.clone());
    let data = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, data).map_err(|e| e.to_string())
}

fn remove_fallback(path: &Path, profile_id: &str) -> Result<(), String> {
    let mut entries = load_fallback(path)?;
    entries.remove(profile_id);
    if entries.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    } else {
        let data = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
        std::fs::write(path, data).map_err(|e| e.to_string())
    }
}

fn resolver_for_state(state: &AppState) -> Result<VaultCredentialResolver, String> {
    let config_dir = state
        .config
        .lock()
        .map_err(|_| "Configuration lock poisoned".to_string())?
        .config_dir()
        .to_path_buf();
    Ok(VaultCredentialResolver::with_config_dir(
        state.vault.clone(),
        config_dir,
    ))
}

#[tauri::command]
pub fn usenet_save_credentials(
    state: State<'_, AppState>,
    profile_id: String,
    username: Option<String>,
    password: Option<String>,
) -> Result<(), String> {
    if profile_id.trim().is_empty() {
        return Err("Usenet profile id is required".into());
    }
    let credentials = json!({
        "username": username.filter(|value| !value.is_empty()),
        "password": password.filter(|value| !value.is_empty()),
    });
    let config_dir = state
        .config
        .lock()
        .map_err(|_| "Configuration lock poisoned".to_string())?
        .config_dir()
        .to_path_buf();
    let fallback_path = risuko_engine::engine::usenet_credential_fallback_path(&config_dir);
    if state.vault.enabled() {
        match state.vault.put(&credential_key(&profile_id), &credentials) {
            Ok(()) => {
                if let Err(error) = remove_fallback(&fallback_path, &profile_id) {
                    tracing::warn!(
                        "Failed to clear stale Usenet credential fallback for {profile_id}: {error}"
                    );
                }
                return Ok(());
            }
            Err(error) => tracing::warn!(
                "Failed to store Usenet credentials in vault for {profile_id}: {error}; using fallback"
            ),
        }
    }
    save_fallback(&fallback_path, &profile_id, &credentials)
}

#[tauri::command]
pub fn usenet_remove_credentials(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<(), String> {
    let config_dir = state
        .config
        .lock()
        .map_err(|_| "Configuration lock poisoned".to_string())?
        .config_dir()
        .to_path_buf();
    let fallback_path = risuko_engine::engine::usenet_credential_fallback_path(&config_dir);
    let vault_error = if state.vault.enabled() {
        state.vault.remove(&credential_key(&profile_id)).err()
    } else {
        None
    };
    let fallback_result = remove_fallback(&fallback_path, &profile_id);
    if let Some(error) = vault_error {
        return Err(error);
    }
    fallback_result
}

#[tauri::command]
pub fn usenet_has_credentials(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<bool, String> {
    let config_dir = state
        .config
        .lock()
        .map_err(|_| "Configuration lock poisoned".to_string())?
        .config_dir()
        .to_path_buf();
    if state.vault.enabled() {
        match state.vault.get(&credential_key(&profile_id)) {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                "Failed to inspect Usenet credentials for {profile_id}: {error}; trying fallback"
            ),
        }
    }
    let fallback_path = risuko_engine::engine::usenet_credential_fallback_path(&config_dir);
    Ok(load_fallback(&fallback_path)?.contains_key(&profile_id))
}

#[tauri::command]
pub async fn usenet_test_profile(
    state: State<'_, AppState>,
    profile: UsenetProviderProfile,
) -> Result<String, String> {
    let credentials = resolver_for_state(&state)?.resolve(&profile.id).await?;
    let _capacity_lease = match risuko_engine::engine::get_manager().await {
        Some(manager) => Some(
            manager
                .try_acquire_usenet_profile_connection(&profile)?
                .ok_or_else(|| {
                    "The provider's configured connection capacity is currently in use".to_string()
                })?,
        ),
        None => None,
    };
    let mut connection = NntpConnection::connect(&profile, credentials)
        .await
        .map_err(|error| error.to_string())?;
    let _ = connection
        .capabilities()
        .await
        .map_err(|error| error.to_string())?;
    Ok("ok".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fallback_credentials_round_trip_and_remove() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("usenet-credentials.json");
        let credentials = json!({"username": "alice", "password": "secret"});

        save_fallback(&path, "primary", &credentials).unwrap();
        let entries = load_fallback(&path).unwrap();
        assert_eq!(entries.get("primary"), Some(&credentials));

        remove_fallback(&path, "primary").unwrap();
        assert!(!path.exists());
    }
}
