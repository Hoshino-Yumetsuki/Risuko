//! Domain-keyed cookie / User-Agent store
//!
//! Persists imported browser credentials to a single JSON file in the
//! engine's config directory. Two consumers:
//!
//! 1. The HTTP downloader auto-applies a saved entry when a new task's
//!    URL matches by host
//! 2. The IPC layer surfaces these entries in the renderer's "Saved
//!    domain credentials" preferences pane, with delete controls
//!
//! Thin by design: no encryption, no scheduled refresh. Entries leave
//! the store when the user deletes them in preferences, when the
//! downloader re-detects a Cloudflare challenge for a saved host (logic
//! lives in `manager.rs`), or when the store grows past `MAX_ENTRIES`
//! and the oldest by `last_validated_at` is evicted

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_FILE: &str = "browser_cookies.json";
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CookieEntry {
    pub host: String,
    pub browser_id: String,
    pub user_agent: String,
    pub cookies: Vec<StoredCookie>,
    pub imported_at: u64,
    pub last_validated_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    entries: HashMap<String, CookieEntry>,
}

pub struct CookieStore {
    path: PathBuf,
    state: RwLock<StoreFile>,
}

impl CookieStore {
    pub fn new(config_dir: &Path) -> Self {
        let path = config_dir.join(STORE_FILE);
        let state = match load_from_disk(&path) {
            Ok(s) => s,
            Err(err) => {
                // A corrupted or unreadable file would be silently
                // overwritten by the next write if we treated it as
                // empty. Sidestep the file by renaming it with a
                // timestamped `.corrupt-N` suffix so the user can recover
                // it manually, and start with an empty in-memory store
                tracing::warn!(
                    "cookie store load failed for {}: {err}. Backing up to .corrupt and starting empty",
                    path.display()
                );
                if path.exists() {
                    let backup = corrupt_backup_path(&path);
                    if let Err(rename_err) = std::fs::rename(&path, &backup) {
                        tracing::error!(
                            "cookie store backup failed (rename {} -> {}): {rename_err}; refusing to overwrite",
                            path.display(),
                            backup.display()
                        );
                        // Last-resort safety: if we can't move the file
                        // aside, don't risk clobbering it. Return a
                        // store whose path points elsewhere so writes
                        // can't destroy the original
                        return Self {
                            path: backup,
                            state: RwLock::new(StoreFile::default()),
                        };
                    }
                    tracing::info!("cookie store backup created at {}", backup.display());
                }
                StoreFile::default()
            }
        };
        Self {
            path,
            state: RwLock::new(state),
        }
    }

    /// Find an entry whose host matches `target_url`. Tries an exact
    /// match first, then a suffix match so `example.com` covers
    /// `dl.example.com`
    pub fn find_for_url(&self, target_url: &str) -> Option<CookieEntry> {
        let host = host_of(target_url)?;
        let s = self.state.read();
        if let Some(entry) = s.entries.get(&host) {
            return Some(entry.clone());
        }
        // Suffix match for subdomains
        let mut best: Option<&CookieEntry> = None;
        for entry in s.entries.values() {
            if host_matches(&host, &entry.host) {
                best = match best {
                    Some(prev) if prev.host.len() >= entry.host.len() => Some(prev),
                    _ => Some(entry),
                };
            }
        }
        best.cloned()
    }

    /// Insert or replace by host. Bumps `last_validated_at` and persists
    pub fn upsert(&self, mut entry: CookieEntry) -> Result<(), String> {
        let now = now_secs();
        if entry.imported_at == 0 {
            entry.imported_at = now;
        }
        entry.last_validated_at = now;

        // Lowercase host keys so exact lookup, remove, and touch share one form
        let key = entry.host.to_ascii_lowercase();
        let mut s = self.state.write();
        s.entries.insert(key, entry);

        if s.entries.len() > MAX_ENTRIES {
            let mut by_age: Vec<(String, u64)> = s
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v.last_validated_at))
                .collect();
            by_age.sort_by_key(|(_, t)| *t);
            let to_drop = s.entries.len() - MAX_ENTRIES;
            for (host, _) in by_age.into_iter().take(to_drop) {
                s.entries.remove(&host);
            }
        }
        write_to_disk(&self.path, &s)
    }

    /// Bump `last_validated_at` for an entry that's still working. Called
    /// from the HTTP downloader after a successful task start
    pub fn touch(&self, host: &str) {
        let host = host.to_ascii_lowercase();
        let mut s = self.state.write();
        if let Some(entry) = s.entries.get_mut(&host) {
            entry.last_validated_at = now_secs();
            // Best-effort persist; a write failure isn't fatal
            let _ = write_to_disk(&self.path, &s);
        }
    }

    pub fn remove(&self, host: &str) -> Result<bool, String> {
        let host = host.to_ascii_lowercase();
        let mut s = self.state.write();
        let removed = s.entries.remove(&host).is_some();
        if removed {
            write_to_disk(&self.path, &s)?;
        }
        Ok(removed)
    }

    pub fn list(&self) -> Vec<CookieEntry> {
        let s = self.state.read();
        let mut entries: Vec<_> = s.entries.values().cloned().collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.last_validated_at));
        entries
    }

    pub fn clear(&self) -> Result<(), String> {
        let mut s = self.state.write();
        s.entries.clear();
        write_to_disk(&self.path, &s)
    }
}

fn host_of(target_url: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(target_url) {
        return url.host_str().map(|s| s.to_lowercase());
    }
    let trimmed = target_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = trimmed.split('/').next()?.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

fn host_matches(request_host: &str, entry_host: &str) -> bool {
    let r = request_host.to_lowercase();
    let e = entry_host.to_lowercase();
    if r == e {
        return true;
    }
    // entry_host as suffix, with dot boundary
    r.ends_with(&format!(".{e}"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_from_disk(path: &Path) -> Result<StoreFile, String> {
    if !path.exists() {
        return Ok(StoreFile::default());
    }
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("read cookie store failed: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("parse cookie store failed: {e}"))
}

fn write_to_disk(path: &Path, state: &StoreFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create cookie store dir failed: {e}"))?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("serialize cookie store failed: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("write cookie store failed: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        // On Windows rename fails if the destination exists; remove it and retry.
        // Only attempt removal for AlreadyExists — other errors (permission, I/O)
        // are returned as-is so the existing cookie store is never deleted on
        // transient failures.
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("rename cookie store failed: {e}"));
        }
        if let Err(re) = std::fs::remove_file(path) {
            // Clean up the temp file so we don't leave it behind, then surface
            // the original rename error together with the remove failure
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "rename cookie store failed: {e}; remove existing failed: {re}"
            ));
        }
        std::fs::rename(&tmp, path).map_err(|e2| format!("rename cookie store failed: {e2}"))?;
    }
    Ok(())
}

/// Pick a side-by-side backup path that doesn't already exist. The
/// browser_cookies.json file is the canonical one we don't want to
/// overwrite when load fails, so we move it aside before starting fresh
fn corrupt_backup_path(path: &Path) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut candidate = path.with_extension(format!("json.corrupt-{stamp}"));
    // Extremely unlikely to collide, but be defensive in case the user
    // already has a stale backup with the same timestamp
    let mut suffix = 1u32;
    while candidate.exists() {
        candidate = path.with_extension(format!("json.corrupt-{stamp}-{suffix}"));
        suffix += 1;
        if suffix > 100 {
            break;
        }
    }
    candidate
}

/// Render the entry's cookies into a single `Cookie:` header value
pub fn cookies_to_header(cookies: &[StoredCookie]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn dummy_entry(host: &str) -> CookieEntry {
        CookieEntry {
            host: host.to_string(),
            browser_id: "chrome".into(),
            user_agent: "ua".into(),
            cookies: vec![StoredCookie {
                name: "cf_clearance".into(),
                value: "abc".into(),
                domain: format!(".{host}"),
                path: "/".into(),
                secure: true,
                http_only: false,
                expires: None,
            }],
            imported_at: 0,
            last_validated_at: 0,
        }
    }

    #[test]
    fn upsert_and_find_exact() {
        let dir = TempDir::new().unwrap();
        let store = CookieStore::new(dir.path());
        store.upsert(dummy_entry("example.com")).unwrap();
        let hit = store.find_for_url("https://example.com/foo").unwrap();
        assert_eq!(hit.host, "example.com");
        assert_eq!(hit.cookies[0].name, "cf_clearance");
    }

    #[test]
    fn find_for_url_subdomain() {
        let dir = TempDir::new().unwrap();
        let store = CookieStore::new(dir.path());
        store.upsert(dummy_entry("example.com")).unwrap();
        let hit = store.find_for_url("https://dl.example.com/file.zip");
        assert!(hit.is_some());
    }

    #[test]
    fn find_for_url_unrelated_host_misses() {
        let dir = TempDir::new().unwrap();
        let store = CookieStore::new(dir.path());
        store.upsert(dummy_entry("example.com")).unwrap();
        assert!(store.find_for_url("https://attacker.com/foo").is_none());
        // suffix-but-not-dot-bounded
        assert!(store.find_for_url("https://notexample.com/foo").is_none());
    }

    #[test]
    fn persist_and_reload() {
        let dir = TempDir::new().unwrap();
        {
            let store = CookieStore::new(dir.path());
            store.upsert(dummy_entry("example.com")).unwrap();
        }
        let store = CookieStore::new(dir.path());
        assert!(store.find_for_url("https://example.com").is_some());
    }

    #[test]
    fn remove_works() {
        let dir = TempDir::new().unwrap();
        let store = CookieStore::new(dir.path());
        store.upsert(dummy_entry("example.com")).unwrap();
        assert!(store.remove("example.com").unwrap());
        assert!(!store.remove("example.com").unwrap());
        assert!(store.find_for_url("https://example.com").is_none());
    }

    #[test]
    fn lru_eviction() {
        let dir = TempDir::new().unwrap();
        let store = CookieStore::new(dir.path());
        // Fill to MAX with synthetic timestamps so eviction is deterministic
        for i in 0..(MAX_ENTRIES + 5) {
            let mut e = dummy_entry(&format!("h{i}.example.com"));
            e.last_validated_at = i as u64;
            e.imported_at = i as u64;
            // Bypass touch() in upsert by constructing low timestamp first
            store.upsert(e).unwrap();
            // Manually backdate to make ordering meaningful
            store.state.write().entries.values_mut().for_each(|e| {
                if e.host == format!("h{i}.example.com") {
                    e.last_validated_at = i as u64;
                }
            });
        }
        let listed = store.list();
        assert!(listed.len() <= MAX_ENTRIES);
        // The oldest (h0..h4) should be evicted. h0 specifically should not exist.
        assert!(store.find_for_url("https://h0.example.com").is_none());
    }

    #[test]
    fn cookies_to_header_format() {
        let cs = vec![
            StoredCookie {
                name: "a".into(),
                value: "1".into(),
                ..Default::default()
            },
            StoredCookie {
                name: "b".into(),
                value: "two".into(),
                ..Default::default()
            },
        ];
        assert_eq!(cookies_to_header(&cs), "a=1; b=two");
    }

    #[test]
    fn corrupted_store_is_backed_up_not_overwritten() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STORE_FILE);
        // Plant garbage so serde_json::from_str returns an error
        std::fs::write(&path, b"{not valid json").unwrap();

        // Loading must NOT silently default. The corrupt file should be
        // moved aside, and a sibling .corrupt-* file should now exist
        let store = CookieStore::new(dir.path());
        assert!(store.list().is_empty());
        assert!(
            !path.exists(),
            "original corrupt file should be moved aside"
        );

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("browser_cookies.json.corrupt-")
            })
            .collect();
        assert_eq!(backups.len(), 1, "expected exactly one .corrupt-* backup");

        // A subsequent upsert should write a fresh, valid file at the
        // canonical path without touching the backup
        store.upsert(dummy_entry("example.com")).unwrap();
        assert!(path.exists());
        assert!(backups[0].path().exists());
    }
}
