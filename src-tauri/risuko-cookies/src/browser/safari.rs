// Safari cookie extraction

#[cfg(target_os = "macos")]
use crate::browser::chromium::Cookie;
#[cfg(target_os = "macos")]
use crate::utils::{paths, time};
#[cfg(target_os = "macos")]
use eyre::{bail, Result};
#[cfg(target_os = "macos")]
use rusqlite::Connection;

#[cfg(target_os = "macos")]
pub fn extract_cookies(host: Option<&str>) -> Result<Vec<Cookie>> {
    let cookie_db = paths::find_first_existing(&[
        "~/Library/Cookies/Cookies.binarycookies",
        "~/Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies",
    ]);
    let cookie_db = match cookie_db {
        Some(p) => p,
        None => bail!("safari cookie file not found"),
    };
    tracing::debug!(target: "risuko_cookies", "safari: using db path {}", cookie_db.display());

    // Export the binary cookie store to a temporary plist we can read
    let temp_db = tempfile::NamedTempFile::new()?;
    std::process::Command::new("defaults")
        .arg("export")
        .arg(&cookie_db)
        .arg(temp_db.path())
        .output()?;

    let conn = Connection::open(temp_db.path())?;

    // Count total rows first for debugging
    let total_count: i64 = conn.query_row("SELECT COUNT(*) FROM cookies", [], |row| row.get(0))?;
    tracing::debug!(target: "risuko_cookies", "safari: total rows in cookies = {}", total_count);

    // Read all cookies and filter by domain
    let mut stmt =
        conn.prepare("SELECT name, value, domain, path, secure, httpOnly, expires FROM cookies")?;
    let cookie_iter = stmt.query_map([], parse_row)?;

    let mut cookies = Vec::new();
    let mut skipped_domain = 0usize;

    for cookie_result in cookie_iter {
        if let Ok(cookie) = cookie_result {
            if let Some(request_host) = host {
                if !cookie_covers_host(request_host, &cookie.domain) {
                    tracing::trace!(target: "risuko_cookies", "safari: skip cookie '{}' domain={} (does not cover {})", cookie.name, cookie.domain, request_host);
                    skipped_domain += 1;
                    continue;
                }
            }
            tracing::trace!(target: "risuko_cookies", "safari: keep cookie '{}' domain={} value_len={}", cookie.name, cookie.domain, cookie.value.len());
            cookies.push(cookie);
        }
    }

    tracing::debug!(target: "risuko_cookies",
        "safari: host_filter={:?} -> kept={} skipped_domain={}",
        host, cookies.len(), skipped_domain
    );

    Ok(cookies)
}

#[cfg(target_os = "macos")]
fn cookie_covers_host(request_host: &str, cookie_domain: &str) -> bool {
    let r = request_host.to_lowercase();
    let c = cookie_domain.to_lowercase();

    if c.starts_with('.') {
        // Older Safari: domain cookie with explicit leading dot
        let domain = &c[1..];
        r == domain || r.ends_with(&format!(".{domain}"))
    } else {
        // Modern Safari: bare domain is a domain cookie (covers subdomains)
        r == c || r.ends_with(&format!(".{c}"))
    }
}

#[cfg(target_os = "macos")]
fn parse_row(row: &rusqlite::Row) -> rusqlite::Result<Cookie> {
    Ok(Cookie {
        name: row.get(0)?,
        value: row.get(1)?,
        domain: row.get(2)?,
        path: row.get(3)?,
        secure: row.get::<_, i32>(4)? != 0,
        http_only: row.get::<_, i32>(5)? != 0,
        expires: time::safari_to_unix(row.get::<_, i64>(6)? as u64),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn extract_cookies(_host: Option<&str>) -> eyre::Result<Vec<crate::browser::chromium::Cookie>> {
    eyre::bail!("safari only available on macos")
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::cookie_covers_host;

    #[test]
    #[cfg(target_os = "macos")]
    fn domain_cookie_covers_subdomain() {
        assert!(cookie_covers_host("www.spigotmc.org", "spigotmc.org"));
        assert!(cookie_covers_host("dl.spigotmc.org", "spigotmc.org"));
        assert!(cookie_covers_host("spigotmc.org", "spigotmc.org"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn old_domain_cookie_with_dot() {
        assert!(cookie_covers_host("www.spigotmc.org", ".spigotmc.org"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn no_false_match() {
        assert!(!cookie_covers_host("notspigotmc.org", "spigotmc.org"));
        assert!(!cookie_covers_host("www.spigotmc.org", "example.com"));
    }
}
