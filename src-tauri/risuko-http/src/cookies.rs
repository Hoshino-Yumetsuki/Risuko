use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Mutex};

use http::HeaderValue;
use url::Url;

/// Pluggable cookie store. Implementations decide how cookies are persisted
/// and matched.
pub trait CookieStore: Send + Sync {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url);
    fn cookies(&self, url: &Url) -> Option<HeaderValue>;
}

/// In-memory cookie jar backed by the `cookie_store` crate. Honors the
/// `Set-Cookie` semantics that matter for downloaders: domain matching,
/// path matching, secure-only, expiration, HostOnly. Persistence to disk is
/// out of scope here — `engine::cookies` handles file I/O before installing
/// cookies into a fresh `Jar`
pub struct Jar {
    inner: Mutex<cookie_store::CookieStore>,
}

impl Default for Jar {
    fn default() -> Self {
        Self {
            inner: Mutex::new(cookie_store::CookieStore::default()),
        }
    }
}

impl Jar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a single raw `Set-Cookie` value as if returned by `url`
    pub fn add_cookie_str(&self, cookie_str: &str, url: &Url) {
        // `cookie::Cookie::parse` accepts `impl Into<Cow<'_, str>>`, so a
        // borrowed slice avoids the per-call `String` allocation
        if let Ok(parsed) = cookie::Cookie::parse(cookie_str) {
            if let Ok(mut store) = self.inner.lock() {
                let _ = store.insert_raw(&parsed, url);
            }
        }
    }

    /// Load Netscape `cookies.txt` format (also accepts the `# HttpOnly_`
    /// prefix used by curl/wget). Lines that don't match the 7-field shape
    /// are silently skipped — the format is famously loose
    pub fn load_netscape<R: Read>(&self, reader: R) -> std::io::Result<usize> {
        let mut count = 0usize;
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // `#HttpOnly_` prefix is a real cookie line; anything else
            // starting with `#` is a comment
            let payload = if let Some(rest) = trimmed.strip_prefix("#HttpOnly_") {
                rest
            } else if let Some(rest) = trimmed.strip_prefix("# HttpOnly_") {
                rest
            } else if trimmed.starts_with('#') {
                continue;
            } else {
                trimmed
            };
            // domain \t flag \t path \t secure \t expires \t name \t value
            let cols: Vec<&str> = payload.split('\t').collect();
            if cols.len() < 7 {
                // A bare comment or trailing blank produces a single token;
                // anything else with content but a wrong column count is
                // worth flagging so users can debug their cookies.txt
                if cols.len() > 1 {
                    tracing::warn!(
                        "cookies.txt: skipping malformed line ({} tab-separated fields, need 7)",
                        cols.len()
                    );
                }
                continue;
            }
            let domain_field = cols[0];
            let include_subdomains = cols[1].eq_ignore_ascii_case("TRUE");
            let path = cols[2];
            let secure = cols[3].eq_ignore_ascii_case("TRUE");
            let expires_raw = cols[4].trim();
            let name = cols[5];
            let value = cols[6];
            if name.is_empty() {
                continue;
            }
            // `0` means "session cookie, no expiry" in the Netscape format
            // Anything else is a unix epoch — drop already-expired entries
            let expires_epoch: Option<i64> = if expires_raw.is_empty() || expires_raw == "0" {
                None
            } else {
                match expires_raw.parse::<i64>() {
                    Ok(v) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        if v > 0 && v <= now {
                            // Already expired; skip this line.
                            continue;
                        }
                        Some(v)
                    }
                    Err(_) => None,
                }
            };
            // Host-only cookies (include_subdomains == FALSE) must NOT carry
            // a Domain attribute — without it, the cookie store treats them
            // as host-only and refuses to send them to subdomains. Domain
            // cookies keep the (dot-less) Domain attribute so the underlying
            // store will match subdomains
            let bare_domain = domain_field.trim_start_matches('.');
            let scheme = if secure { "https" } else { "http" };
            let url_str = format!("{scheme}://{bare_domain}{path}");
            let Ok(url) = Url::parse(&url_str) else {
                continue;
            };
            let mut cookie_str = format!("{name}={value}; Path={path}");
            if include_subdomains {
                cookie_str.push_str(&format!("; Domain={bare_domain}"));
            }
            if secure {
                cookie_str.push_str("; Secure");
            }
            if let Some(epoch) = expires_epoch {
                // RFC 1123 / IMF-fixdate is the cookie spec format. Build it
                // manually to avoid pulling chrono in for one timestamp
                if let Some(s) = format_http_date(epoch) {
                    cookie_str.push_str(&format!("; Expires={s}"));
                }
            }
            self.add_cookie_str(&cookie_str, &url);
            count += 1;
        }
        Ok(count)
    }
}

impl CookieStore for Jar {
    fn set_cookies(&self, cookies: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let Ok(mut store) = self.inner.lock() else {
            return;
        };
        for hv in cookies {
            let Ok(s) = hv.to_str() else { continue };
            let Ok(parsed) = cookie::Cookie::parse(s) else {
                continue;
            };
            let _ = store.insert_raw(&parsed, url);
        }
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let store = self.inner.lock().ok()?;
        let s = store
            .matches(url)
            .into_iter()
            .map(|c| format!("{}={}", c.name(), c.value()))
            .collect::<Vec<_>>()
            .join("; ");
        if s.is_empty() {
            return None;
        }
        HeaderValue::from_str(&s).ok()
    }
}

pub(crate) type SharedJar = Arc<dyn CookieStore>;

/// Render a unix epoch as an RFC 1123 / IMF-fixdate string suitable for the
/// `Expires=` cookie attribute. Returns `None` for values outside the
/// representable range (we just drop the attribute in that case so the
/// cookie becomes a session cookie instead of being rejected)
fn format_http_date(epoch: i64) -> Option<String> {
    if epoch < 0 {
        return None;
    }
    // Days from civil date — Howard Hinnant's algorithm. Avoids pulling in
    // chrono just to format one header
    let secs = epoch as u64;
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;

    // 1970-01-01 is day 0; convert to civil date
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = y + if m <= 2 { 1 } else { 0 };

    // Day-of-week via Zeller-like calc using days since 1970-01-01 (a Thu)
    let dow_idx = ((days % 7 + 7 + 4) % 7) as usize;
    const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTH_NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month_name = MONTH_NAMES.get((m as usize).checked_sub(1)?)?;
    Some(format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAY_NAMES[dow_idx], d, month_name, y, hour, minute, second
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple_cookie() {
        let jar = Jar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.add_cookie_str("session=abc; Path=/; Domain=example.com", &url);
        let header = CookieStore::cookies(&jar, &url).expect("cookie should match");
        assert_eq!(header.to_str().unwrap(), "session=abc");
    }

    #[test]
    fn netscape_format_loads() {
        let txt = "# Netscape HTTP Cookie File\n\
                   .example.com\tTRUE\t/\tFALSE\t0\tfoo\tbar\n\
                   #HttpOnly_.example.com\tTRUE\t/\tFALSE\t0\thid\tval\n";
        let jar = Jar::new();
        let n = jar.load_netscape(txt.as_bytes()).unwrap();
        assert_eq!(n, 2);
        let url = Url::parse("http://example.com/").unwrap();
        let header = CookieStore::cookies(&jar, &url).expect("cookie should match");
        let s = header.to_str().unwrap();
        assert!(s.contains("foo=bar"));
        assert!(s.contains("hid=val"));
    }

    #[test]
    fn secure_cookie_not_sent_over_http() {
        let jar = Jar::new();
        let https = Url::parse("https://example.com/").unwrap();
        jar.add_cookie_str("k=v; Path=/; Domain=example.com; Secure", &https);
        let http = Url::parse("http://example.com/").unwrap();
        assert!(CookieStore::cookies(&jar, &http).is_none());
        assert!(CookieStore::cookies(&jar, &https).is_some());
    }
}
