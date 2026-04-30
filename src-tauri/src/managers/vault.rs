//! OS-keychain-backed credential vault
//!
//! One keyring entry per credential id under a fixed service name; the entry
//! value is a JSON object containing only the sensitive fields. Non-secret
//! metadata (host, label, protocol, timestamps) stays in `user.json` so the
//! frontend can list and match credentials without unlocking the keychain
//!
//! Backend: `keyring` crate — macOS Keychain, Windows Credential Manager,
//! Linux Secret Service. When the OS backend is unavailable (headless Linux
//! without D-Bus, sandboxed environments, etc.) `VaultManager::probe()`
//! reports `enabled = false` and the renderer transparently falls back to
//! storing secrets inline in `user.json` (legacy behavior)

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

const SERVICE: &str = "risuko-credentials";
/// Separate service namespace for upload sink secrets (SFTP/FTP/WebDAV
/// passwords, S3 secret access keys, SSH private keys). Kept distinct from
/// the SavedCredential service so the two stores can be cleared, audited,
/// or migrated independently
const SINK_SERVICE: &str = "risuko-sinks";
/// Account used by `probe()` to verify the keychain backend is reachable.
/// Read-only — never written to, so the OS does not log a write or prompt
/// for keychain access on every launch
const PROBE_ACCOUNT: &str = "__probe__";

pub struct VaultManager {
    enabled: AtomicBool,
}

impl VaultManager {
    pub fn new() -> Self {
        let mgr = Self {
            enabled: AtomicBool::new(false),
        };
        mgr.probe();
        mgr
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Non-destructive reachability check. We only need to confirm that the
    /// keyring backend can be opened and that lookups succeed; a `NoEntry`
    /// result is a healthy outcome (backend reachable, no probe entry
    /// stored). This avoids the spurious write-then-delete cycle the
    /// previous implementation performed on every launch, which could
    /// trigger OS keychain access prompts and audit-log noise
    fn probe(&self) {
        let ok = match keyring::Entry::new(SERVICE, PROBE_ACCOUNT) {
            Ok(entry) => match entry.get_password() {
                Ok(_) | Err(keyring::Error::NoEntry) => true,
                Err(_) => false,
            },
            Err(_) => false,
        };
        self.enabled.store(ok, Ordering::Relaxed);
        if ok {
            log::info!("Credential vault: OS keychain available");
        } else {
            log::warn!("Credential vault: OS keychain unavailable, falling back to plaintext");
        }
    }

    /// Store the secret JSON object for the given credential id.
    pub fn put(&self, id: &str, secrets: &Value) -> Result<(), String> {
        self.put_at(SERVICE, id, secrets)
    }

    /// Retrieve the secret JSON object for the given credential id.
    /// Returns `Ok(None)` when the entry does not exist
    pub fn get(&self, id: &str) -> Result<Option<Value>, String> {
        self.get_at(SERVICE, id)
    }

    /// Best-effort delete; missing entries are not an error
    pub fn remove(&self, id: &str) -> Result<(), String> {
        self.remove_at(SERVICE, id)
    }

    /// Store secrets for an upload sink (separate keychain namespace)
    pub fn put_sink(&self, id: &str, secrets: &Value) -> Result<(), String> {
        self.put_at(SINK_SERVICE, id, secrets)
    }

    /// Retrieve secrets for an upload sink
    pub fn get_sink(&self, id: &str) -> Result<Option<Value>, String> {
        self.get_at(SINK_SERVICE, id)
    }

    /// Best-effort delete of an upload sink's secrets
    pub fn remove_sink(&self, id: &str) -> Result<(), String> {
        self.remove_at(SINK_SERVICE, id)
    }

    fn put_at(&self, service: &str, account: &str, secrets: &Value) -> Result<(), String> {
        if !self.enabled() {
            return Err("vault not available".to_string());
        }
        let json = serde_json::to_string(secrets).map_err(|e| e.to_string())?;
        let entry = keyring::Entry::new(service, account).map_err(|e| e.to_string())?;
        entry.set_password(&json).map_err(|e| e.to_string())
    }

    fn get_at(&self, service: &str, account: &str) -> Result<Option<Value>, String> {
        if !self.enabled() {
            return Ok(None);
        }
        let entry = keyring::Entry::new(service, account).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(json) => {
                let v: Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
                Ok(Some(v))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn remove_at(&self, service: &str, account: &str) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        let entry = keyring::Entry::new(service, account).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a `VaultManager` with a forced `enabled` state, bypassing the
    /// real probe so unit tests can exercise the disabled code paths
    /// without depending on the host's OS keychain
    fn manager_with(enabled: bool) -> VaultManager {
        VaultManager {
            enabled: AtomicBool::new(enabled),
        }
    }

    #[test]
    fn disabled_put_is_err() {
        let m = manager_with(false);
        assert!(!m.enabled());
        let err = m.put("any-id", &json!({"ftpPasswd": "x"})).unwrap_err();
        assert!(err.contains("not available"), "unexpected error: {err}");
    }

    #[test]
    fn disabled_get_returns_none() {
        let m = manager_with(false);
        assert_eq!(m.get("any-id").unwrap(), None);
    }

    #[test]
    fn disabled_remove_is_ok() {
        let m = manager_with(false);
        // Should silently succeed so callers can blindly remove on cleanup
        m.remove("any-id").unwrap();
    }

    /// Integration test against the real OS keychain. Skipped by default
    /// because CI hosts and headless Linux environments may lack a usable
    /// backend; opt in by setting `RISUKO_VAULT_INTEGRATION_TEST=1`
    #[test]
    fn enabled_round_trip_real_keychain() {
        if std::env::var("RISUKO_VAULT_INTEGRATION_TEST")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let m = VaultManager::new();
        if !m.enabled() {
            // No usable backend on this host; treat as a skip
            return;
        }

        let id = format!("risuko-test-{}", std::process::id());
        let secrets = json!({
            "ftpUser": "alice",
            "ftpPasswd": "s3cret!",
            "authorization": "Bearer abc",
        });

        m.put(&id, &secrets).expect("put");
        let fetched = m.get(&id).expect("get").expect("entry exists");
        assert_eq!(fetched, secrets);

        m.remove(&id).expect("remove");
        assert_eq!(m.get(&id).expect("get after remove"), None);

        // Removing a missing entry must remain a no-op
        m.remove(&id).expect("idempotent remove");
    }
}
