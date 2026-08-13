//! Process-wide SSH/SFTP TOFU known-hosts store: records `host:port` -> SHA256 fingerprint on first connect, rejects any subsequent mismatch, backed by a JSON file under the user config dir and shared by the SFTP download path and upload sink

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
        // Atomic replace: write to a sibling temp file, fsync, then rename over the target so a partial write can never truncate the existing pinned-host state
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

/// SSH client handler enforcing TOFU host-key verification: consults the shared known-hosts store — accept on match, refuse on mismatch, pin-and-persist on first sight; if persisting fails the connection is refused so a future mismatch can't go silently undetected
pub struct TofuHandler {
    pub host_key: String,
}

impl TofuHandler {
    pub fn new(host_key: String) -> Self {
        Self { host_key }
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
                tracing::warn!(
                    "SFTP host key fingerprint failed for {}: {} — refusing connection",
                    self.host_key,
                    e
                );
                return Ok(false);
            }
        };
        let mut store = KNOWN_HOSTS.lock();
        match store.map.get(&self.host_key) {
            Some(existing) if existing == &fp => Ok(true),
            Some(existing) => {
                tracing::warn!(
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
                    tracing::error!(
                        "SFTP TOFU: refusing to trust {} because known_hosts persist failed: {}",
                        self.host_key,
                        e
                    );
                    return Ok(false);
                }
                tracing::info!("SFTP TOFU: pinning {} -> {}", self.host_key, fp);
                store.map.insert(self.host_key.clone(), fp);
                Ok(true)
            }
        }
    }
}
