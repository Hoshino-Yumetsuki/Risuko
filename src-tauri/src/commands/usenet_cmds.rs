//! Usenet provider commands

use risuko_engine::engine::usenet::{UsenetCredentials, UsenetProviderProfile};
use risuko_engine::engine::usenet_transport::NntpConnection;
use serde_json::json;
use std::sync::Arc;
use tauri::State;

use crate::state::AppState;

fn credential_key(profile_id: &str) -> String {
    format!("usenet:{profile_id}")
}

pub struct VaultCredentialResolver {
    vault: Arc<crate::managers::vault::VaultManager>,
}

impl VaultCredentialResolver {
    pub fn new(vault: Arc<crate::managers::vault::VaultManager>) -> Self {
        Self { vault }
    }
}

#[async_trait::async_trait]
impl risuko_engine::engine::usenet::UsenetCredentialResolver for VaultCredentialResolver {
    async fn resolve(
        &self,
        profile_id: &str,
    ) -> Result<Option<risuko_engine::engine::usenet::UsenetCredentials>, String> {
        Ok(self
            .vault
            .get(&credential_key(profile_id))?
            .and_then(|value| {
                Some(risuko_engine::engine::usenet::UsenetCredentials {
                    username: value.get("username")?.as_str().map(str::to_string),
                    password: value.get("password")?.as_str().map(str::to_string),
                })
            }))
    }
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
    state.vault.put(
        &credential_key(&profile_id),
        &json!({
            "username": username.filter(|value| !value.is_empty()),
            "password": password.filter(|value| !value.is_empty()),
        }),
    )
}

#[tauri::command]
pub fn usenet_remove_credentials(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<(), String> {
    state.vault.remove(&credential_key(&profile_id))
}

#[tauri::command]
pub fn usenet_has_credentials(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<bool, String> {
    Ok(state.vault.get(&credential_key(&profile_id))?.is_some())
}

#[tauri::command]
pub async fn usenet_test_profile(
    state: State<'_, AppState>,
    profile: UsenetProviderProfile,
) -> Result<String, String> {
    let credentials = state
        .vault
        .get(&credential_key(&profile.id))?
        .and_then(|value| {
            Some(UsenetCredentials {
                username: value.get("username")?.as_str().map(str::to_string),
                password: value.get("password")?.as_str().map(str::to_string),
            })
        });
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
