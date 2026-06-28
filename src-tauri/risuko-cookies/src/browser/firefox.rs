// Firefox-family cookie extraction

use crate::browser::chromium::Cookie;
use crate::utils::paths;
use eyre::{bail, Result};
use rusqlite::Connection;

pub struct BrowserConfig {
    pub cookie_paths: Vec<&'static str>,
}

impl BrowserConfig {
    pub fn firefox() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%APPDATA%/Mozilla/Firefox/Profiles/*/cookies.sqlite"]
            } else if cfg!(target_os = "macos") {
                vec!["~/Library/Application Support/Firefox/Profiles/*/cookies.sqlite"]
            } else {
                vec!["~/.mozilla/firefox/*/cookies.sqlite"]
            },
        }
    }

    pub fn librewolf() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%APPDATA%/LibreWolf/Profiles/*/cookies.sqlite"]
            } else if cfg!(target_os = "macos") {
                vec!["~/Library/Application Support/LibreWolf/Profiles/*/cookies.sqlite"]
            } else {
                vec!["~/.librewolf/*/cookies.sqlite"]
            },
        }
    }

    pub fn zen() -> Self {
        Self {
            cookie_paths: if cfg!(windows) {
                vec!["%APPDATA%/Zen/Profiles/*/cookies.sqlite"]
            } else if cfg!(target_os = "macos") {
                vec!["~/Library/Application Support/Zen/Profiles/*/cookies.sqlite"]
            } else {
                vec!["~/.zen/*/cookies.sqlite"]
            },
        }
    }
}

pub fn extract_cookies(config: &BrowserConfig, host: Option<&str>) -> Result<Vec<Cookie>> {
    // Browsers can have multiple profiles
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
        bail!("firefox cookie database not found");
    }

    let mut cookies = Vec::new();
    let mut skipped_domain = 0usize;

    for cookie_db in &all_dbs {
        tracing::debug!(target: "risuko_cookies", "firefox: using db path {}", cookie_db.display());

        // Copy to a temp file because Firefox keeps the DB locked while running
        let temp = tempfile::NamedTempFile::new()?;
        std::fs::copy(cookie_db, temp.path())?;

        let conn = Connection::open(temp.path())?;

        let total_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM moz_cookies", [], |row| row.get(0))?;
        tracing::debug!(target: "risuko_cookies", "firefox: total rows in moz_cookies = {}", total_count);

        let mut stmt = conn.prepare(
            "SELECT name, value, host, path, isSecure, isHttpOnly, expiry FROM moz_cookies",
        )?;
        let cookie_iter = stmt.query_map([], parse_row)?;

        for cookie in cookie_iter.flatten() {
            if let Some(request_host) = host {
                if !cookie_covers_host(request_host, &cookie.domain) {
                    tracing::trace!(target: "risuko_cookies", "firefox: skip cookie '{}' host={} (does not cover {})", cookie.name, cookie.domain, request_host);
                    skipped_domain += 1;
                    continue;
                }
            }
            tracing::trace!(target: "risuko_cookies", "firefox: keep cookie '{}' host={} value_len={}", cookie.name, cookie.domain, cookie.value.len());
            cookies.push(cookie);
        }
    }

    tracing::debug!(target: "risuko_cookies",
        "firefox: scanned {} db(s), host_filter={:?} -> kept={} skipped_domain={}",
        all_dbs.len(), host, cookies.len(), skipped_domain
    );

    Ok(cookies)
}

// Modern Firefox stores domain cookies without a leading dot
fn cookie_covers_host(request_host: &str, cookie_host: &str) -> bool {
    let r = request_host.to_lowercase();
    let c = cookie_host.to_lowercase();

    if let Some(domain) = c.strip_prefix('.') {
        // Older Firefox: domain cookie with explicit leading dot
        r == domain || r.ends_with(&format!(".{domain}"))
    } else {
        // Modern Firefox: bare domain is a domain cookie (covers subdomains)
        r == c || r.ends_with(&format!(".{c}"))
    }
}

fn parse_row(row: &rusqlite::Row) -> rusqlite::Result<Cookie> {
    let expiry: i64 = row.get(6)?;
    let expires = if expiry > 0 {
        Some(expiry as u64)
    } else {
        None
    };

    Ok(Cookie {
        name: row.get(0)?,
        value: row.get(1)?,
        domain: row.get(2)?,
        path: row.get(3)?,
        secure: row.get::<_, i32>(4)? != 0,
        http_only: row.get::<_, i32>(5)? != 0,
        expires,
    })
}

#[cfg(test)]
mod tests {
    use super::cookie_covers_host;

    #[test]
    fn domain_cookie_without_dot_covers_subdomain() {
        // Modern Firefox: domain cookie stored as "spigotmc.org" (no dot)
        assert!(cookie_covers_host("www.spigotmc.org", "spigotmc.org"));
        assert!(cookie_covers_host("dl.spigotmc.org", "spigotmc.org"));
        assert!(cookie_covers_host("spigotmc.org", "spigotmc.org"));
    }

    #[test]
    fn old_domain_cookie_with_dot_covers_subdomain() {
        // Older Firefox: domain cookie stored as ".spigotmc.org"
        assert!(cookie_covers_host("www.spigotmc.org", ".spigotmc.org"));
        assert!(cookie_covers_host("dl.spigotmc.org", ".spigotmc.org"));
    }

    #[test]
    fn host_only_cookie_exact_match() {
        assert!(cookie_covers_host("www.spigotmc.org", "www.spigotmc.org"));
    }

    #[test]
    fn no_false_match_on_suffix() {
        assert!(!cookie_covers_host("notspigotmc.org", "spigotmc.org"));
        assert!(!cookie_covers_host("www.spigotmc.org", "example.com"));
    }
}
