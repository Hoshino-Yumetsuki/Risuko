//! Process-wide SSH/SFTP TOFU (trust-on-first-use) known-hosts store
//!
//! Records `host:port` -> SHA256 fingerprint pairs on first connect and
//! rejects any subsequent mismatch. Backed by a JSON file under the user
//! config dir so pinning survives restarts. Shared between the SFTP
//! download path and the SFTP upload sink so trust state stays consistent

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use parking_lot::Mutex;
use russh::client;
use sha2::{Digest, Sha256};

struct KnownHosts {
    path: PathBuf,
    map: HashMap<String, String>,
}

impl KnownHosts {
    fn load() -> Self {
        let path = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("risuko")
            .join("sftp_known_hosts.json");
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .unwrap_or_default();
        Self { path, map }
    }

    fn write_to_disk(path: &std::path::Path, map: &HashMap<String, String>) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let s =
            serde_json::to_string_pretty(map).map_err(|e| format!("serialize known_hosts: {e}"))?;
        // Atomic replace: write to a sibling temp file, fsync, then rename
        // over the target so a partial write can never truncate the existing
        // pinned-host state
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)
                .map_err(|e| format!("create {}: {e}", tmp.display()))?;
            f.write_all(s.as_bytes())
                .map_err(|e| format!("write {}: {e}", tmp.display()))?;
            f.sync_all()
                .map_err(|e| format!("fsync {}: {e}", tmp.display()))?;
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            // best-effort cleanup of the temp file
            let _ = std::fs::remove_file(&tmp);
            format!("rename {} -> {}: {e}", tmp.display(), path.display())
        })
    }
}

static KNOWN_HOSTS: LazyLock<Mutex<KnownHosts>> = LazyLock::new(|| Mutex::new(KnownHosts::load()));

pub fn fingerprint(key: &russh::keys::PublicKey) -> Result<String, russh::Error> {
    let mut h = Sha256::new();
    h.update(key.to_bytes()?);
    Ok(format!("SHA256:{}", STANDARD_NO_PAD.encode(h.finalize())))
}

/// SSH client handler enforcing TOFU host-key verification.
///
/// - When `pinned` is set: accept only that exact fingerprint, never persist.
/// - Otherwise consult the shared known-hosts store: accept on match,
///   refuse on mismatch, pin-and-persist on first sight. If persisting
///   fails the connection is refused so a future mismatch can't go
///   silently undetected.
pub struct TofuHandler {
    pub host_key: String,
    pub pinned: Option<String>,
}

impl TofuHandler {
    pub fn new(host_key: String) -> Self {
        Self {
            host_key,
            pinned: None,
        }
    }
}

impl client::Handler for TofuHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fp = match fingerprint(key) {
            Ok(fp) => fp,
            Err(e) => {
                log::warn!(
                    "SFTP host key fingerprint failed for {}: {} — refusing connection",
                    self.host_key,
                    e
                );
                return Ok(false);
            }
        };
        if let Some(ref pin) = self.pinned {
            if pin == &fp {
                return Ok(true);
            }
            log::warn!(
                "SFTP host key mismatch for {}: expected {} got {}",
                self.host_key,
                pin,
                fp
            );
            return Ok(false);
        }
        let mut store = KNOWN_HOSTS.lock();
        match store.map.get(&self.host_key) {
            Some(existing) if existing == &fp => Ok(true),
            Some(existing) => {
                log::warn!(
                    "SFTP host key mismatch for {}: stored {} got {}",
                    self.host_key,
                    existing,
                    fp
                );
                Ok(false)
            }
            None => {
                let path = store.path.clone();
                let mut next = store.map.clone();
                next.insert(self.host_key.clone(), fp.clone());
                if let Err(e) = KnownHosts::write_to_disk(&path, &next) {
                    log::error!(
                        "SFTP TOFU: refusing to trust {} because known_hosts persist failed: {}",
                        self.host_key,
                        e
                    );
                    return Ok(false);
                }
                log::info!("SFTP TOFU: pinning {} -> {}", self.host_key, fp);
                store.map.insert(self.host_key.clone(), fp);
                Ok(true)
            }
        }
    }
}
