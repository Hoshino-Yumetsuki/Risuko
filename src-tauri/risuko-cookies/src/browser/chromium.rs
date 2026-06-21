// Chromium-family cookie extraction

use crate::utils::{paths, time};
use eyre::Result;
use rusqlite::Connection;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<u64>,
}

pub struct BrowserConfig {
    pub cookie_paths: Vec<&'static str>,
}

impl BrowserConfig {
    pub fn chrome() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec![
                    "%LOCALAPPDATA%/Google/Chrome/User Data/*/Cookies",
                    "%LOCALAPPDATA%/Google/Chrome/User Data/*/Network/Cookies",
                ]
            } else if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/Google/Chrome/*/Cookies",
                    "~/Library/Application Support/Google/Chrome/*/Network/Cookies",
                ]
            } else {
                vec!["~/.config/google-chrome/*/Cookies"]
            },
        }
    }

    pub fn edge() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec![
                    "%LOCALAPPDATA%/Microsoft/Edge/User Data/*/Cookies",
                    "%LOCALAPPDATA%/Microsoft/Edge/User Data/*/Network/Cookies",
                ]
            } else if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/Microsoft Edge/*/Cookies",
                    "~/Library/Application Support/Microsoft Edge/*/Network/Cookies",
                ]
            } else {
                vec!["~/.config/microsoft-edge/*/Cookies"]
            },
        }
    }

    pub fn brave() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%LOCALAPPDATA%/BraveSoftware/Brave-Browser/User Data/*/Cookies"]
            } else if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/BraveSoftware/Brave-Browser/*/Cookies",
                    "~/Library/Application Support/BraveSoftware/Brave-Browser/*/Network/Cookies",
                ]
            } else {
                vec!["~/.config/BraveSoftware/Brave-Browser/*/Cookies"]
            },
        }
    }

    pub fn chromium() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%LOCALAPPDATA%/Chromium/User Data/*/Cookies"]
            } else if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/Chromium/*/Cookies",
                    "~/Library/Application Support/Chromium/*/Network/Cookies",
                ]
            } else {
                vec!["~/.config/chromium/*/Cookies"]
            },
        }
    }

    pub fn vivaldi() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%LOCALAPPDATA%/Vivaldi/User Data/*/Cookies"]
            } else if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/Vivaldi/*/Cookies",
                    "~/Library/Application Support/Vivaldi/*/Network/Cookies",
                ]
            } else {
                vec!["~/.config/vivaldi/*/Cookies"]
            },
        }
    }

    pub fn opera() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%APPDATA%/Opera Software/Opera Stable/Cookies"]
            } else if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/com.operasoftware.Opera/Cookies",
                    "~/Library/Application Support/com.operasoftware.Opera/Network/Cookies",
                ]
            } else {
                vec!["~/.config/opera/Cookies"]
            },
        }
    }

    pub fn arc() -> Self {
        Self {
            cookie_paths: if cfg!(target_os = "macos") {
                vec![
                    "~/Library/Application Support/Arc/User Data/*/Cookies",
                    "~/Library/Application Support/Arc/User Data/*/Network/Cookies",
                ]
            } else {
                vec![]
            },
        }
    }
}

pub fn extract_cookies(config: &BrowserConfig, host: Option<&str>) -> Result<Vec<Cookie>> {
    // Chromium browsers can have multiple profiles (Default, Profile 1, etc.)
    // Read all of them and merge cookies — the active profile isn't always "Default"
    let mut all_dbs = Vec::new();
    for pattern in &config.cookie_paths {
        if let Ok(paths) = paths::find_matching(pattern) {
            for p in paths {
                if p.exists() {
                    all_dbs.push(p);
                }
            }
        }
    }
    if all_dbs.is_empty() {
        return Err(eyre::eyre!("cookie database not found"));
    }

    let mut all_cookies = Vec::new();
    for cookie_db in &all_dbs {
        tracing::debug!(target: "risuko_cookies", "chromium: using db path {}", cookie_db.display());

        // "Local State" lives next to the profile directory and holds the encrypted master key
        let local_state = paths::find_local_state(cookie_db);
        tracing::debug!(target: "risuko_cookies", "chromium: local_state path = {:?}", local_state.as_ref().map(|p| p.display().to_string()));

        let master_key = if let Some(ls) = local_state {
            #[cfg(target_os = "windows")]
            {
                let key = crate::platform::windows::extract_master_key(&ls).ok();
                tracing::debug!(target: "risuko_cookies", "chromium: windows master_key len = {:?}", key.as_ref().map(|k| k.len()));
                key
            }
            #[cfg(target_os = "macos")]
            {
                let key = crate::platform::macos::extract_master_key(&ls).ok();
                tracing::debug!(target: "risuko_cookies", "chromium: macos master_key len = {:?}", key.as_ref().map(|k| k.len()));
                key
            }
            #[cfg(target_os = "linux")]
            {
                let key = crate::platform::linux::extract_master_key(&ls).ok();
                tracing::debug!(target: "risuko_cookies", "chromium: linux master_key len = {:?}", key.as_ref().map(|k| k.len()));
                key
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
            {
                tracing::debug!(target: "risuko_cookies", "chromium: unsupported platform, no master key");
                None
            }
        } else {
            tracing::debug!(target: "risuko_cookies", "chromium: no local_state found, cookies will be raw");
            None
        };

        let mut cookies = read_cookies_from_db(cookie_db, master_key.as_deref(), host)?;
        all_cookies.append(&mut cookies);
    }

    tracing::debug!(target: "risuko_cookies",
        "chromium: scanned {} db(s), host_filter={:?} -> total kept={}",
        all_dbs.len(), host, all_cookies.len()
    );

    Ok(all_cookies)
}

fn read_cookies_from_db(
    db_path: &PathBuf,
    master_key: Option<&[u8]>,
    host: Option<&str>,
) -> Result<Vec<Cookie>> {
    // Copy to a temp file because the browser keeps the real DB locked while running
    let temp = tempfile::NamedTempFile::new()?;
    std::fs::copy(db_path, temp.path())?;

    let conn = Connection::open(temp.path())?;

    // Count total rows first for debugging
    let total_count: i64 = conn.query_row("SELECT COUNT(*) FROM cookies", [], |row| row.get(0))?;
    tracing::debug!(target: "risuko_cookies", "chromium: total rows in db = {}", total_count);

    // Read all cookies
    let mut stmt = conn.prepare(
        "SELECT name, encrypted_value, host_key, path, is_secure, is_httponly, expires_utc FROM cookies"
    )?;
    let cookie_iter = stmt.query_map([], parse_row)?;

    let mut cookies = Vec::new();
    let mut skipped_empty = 0usize;
    let mut skipped_domain = 0usize;
    let mut decrypted_ok = 0usize;
    let mut decrypted_fail = 0usize;
    let mut no_key = 0usize;

    for cookie_result in cookie_iter {
        if let Ok(raw) = cookie_result {
            if raw.raw_value.is_empty() {
                skipped_empty += 1;
                continue;
            }

            // Skip cookies that don't cover the requested host
            if let Some(request_host) = host {
                if !cookie_covers_host(request_host, &raw.domain) {
                    tracing::trace!(target: "risuko_cookies", "chromium: skip cookie '{}' host_key={} (does not cover {})", raw.name, raw.domain, request_host);
                    skipped_domain += 1;
                    continue;
                }
            }

            // Decrypt the raw BLOB bytes, then convert to string
            let value = if let Some(key) = master_key {
                match decrypt_cookie_value(&raw.raw_value, key) {
                    Ok(decrypted) => {
                        decrypted_ok += 1;
                        String::from_utf8_lossy(&decrypted).to_string()
                    }
                    Err(e) => {
                        tracing::trace!(target: "risuko_cookies", "chromium: decrypt failed for '{}' host_key={}: {}", raw.name, raw.domain, e);
                        decrypted_fail += 1;
                        String::from_utf8_lossy(&raw.raw_value).to_string()
                    }
                }
            } else {
                no_key += 1;
                String::from_utf8_lossy(&raw.raw_value).to_string()
            };

            cookies.push(Cookie {
                name: raw.name,
                value,
                domain: raw.domain,
                path: raw.path,
                secure: raw.secure,
                http_only: raw.http_only,
                expires: raw.expires,
            });
        }
    }

    tracing::debug!(target: "risuko_cookies",
        "chromium: host_filter={:?} -> kept={} (skipped_empty={} skipped_domain={} decrypted_ok={} decrypted_fail={} no_key={})",
        host, cookies.len(), skipped_empty, skipped_domain, decrypted_ok, decrypted_fail, no_key
    );

    for c in cookies.iter() {
        tracing::trace!(target: "risuko_cookies", "chromium: kept cookie name={} domain={} value_len={}", c.name, c.domain, c.value.len());
    }

    Ok(cookies)
}

fn cookie_covers_host(request_host: &str, cookie_host_key: &str) -> bool {
    let r = request_host.to_lowercase();
    let c = cookie_host_key.to_lowercase();

    if c.starts_with('.') {
        let domain = &c[1..];
        r == domain || r.ends_with(&format!(".{domain}"))
    } else {
        r == c
    }
}

struct RawCookie {
    name: String,
    raw_value: Vec<u8>,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires: Option<u64>,
}

fn parse_row(row: &rusqlite::Row) -> rusqlite::Result<RawCookie> {
    Ok(RawCookie {
        name: row.get(0)?,
        raw_value: row.get::<_, Vec<u8>>(1)?,
        domain: row.get(2)?,
        path: row.get(3)?,
        secure: row.get::<_, i32>(4)? != 0,
        http_only: row.get::<_, i32>(5)? != 0,
        expires: time::webkit_to_unix(row.get::<_, i64>(6)? as u64),
    })
}

fn decrypt_cookie_value(encrypted: &[u8], master_key: &[u8]) -> Result<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        crate::platform::windows::decrypt_value(encrypted, master_key)
    }

    #[cfg(target_os = "macos")]
    {
        crate::platform::macos::decrypt_value(encrypted, master_key)
    }

    #[cfg(target_os = "linux")]
    {
        crate::platform::linux::decrypt_value(encrypted, master_key)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = (encrypted, master_key);
        eyre::bail!("unsupported platform")
    }
}

#[cfg(test)]
mod tests {
    use super::cookie_covers_host;

    #[test]
    fn domain_cookie_with_dot_covers_subdomain() {
        assert!(cookie_covers_host("www.spigotmc.org", ".spigotmc.org"));
        assert!(cookie_covers_host("dl.spigotmc.org", ".spigotmc.org"));
        assert!(cookie_covers_host("spigotmc.org", ".spigotmc.org"));
    }

    #[test]
    fn host_only_cookie_exact_match() {
        assert!(cookie_covers_host("www.spigotmc.org", "www.spigotmc.org"));
        assert!(!cookie_covers_host("dl.spigotmc.org", "www.spigotmc.org"));
    }

    #[test]
    fn no_false_match_on_suffix() {
        assert!(!cookie_covers_host("notspigotmc.org", ".spigotmc.org"));
        assert!(!cookie_covers_host("evil.spigotmc.org", ".example.com"));
    }
}
