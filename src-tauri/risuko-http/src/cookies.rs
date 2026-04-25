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
        if let Ok(parsed) = cookie::Cookie::parse(cookie_str.to_string()) {
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
            let domain = cols[0].trim_start_matches('.');
            let path = cols[2];
            let secure = cols[3].eq_ignore_ascii_case("TRUE");
            let name = cols[5];
            let value = cols[6];
            if name.is_empty() {
                continue;
            }
            let scheme = if secure { "https" } else { "http" };
            let url_str = format!("{scheme}://{domain}{path}");
            let Ok(url) = Url::parse(&url_str) else {
                continue;
            };
            let mut cookie_str = format!("{name}={value}; Path={path}; Domain={}", cols[0]);
            if secure {
                cookie_str.push_str("; Secure");
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
            let Ok(parsed) = cookie::Cookie::parse(s.to_string()) else {
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
