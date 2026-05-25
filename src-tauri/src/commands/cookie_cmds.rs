use risuko_cookies::{cookies_for_url, list_browsers, BrowserInfo, HostCookies};
use risuko_engine::engine;
use risuko_engine::engine::cookie_store::{cookies_to_header, CookieEntry, StoredCookie};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedCookieView {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedCookies {
    pub host: String,
    pub user_agent: String,
    pub cookie_header: String,
    pub count: usize,
    /// True when the imported cookies include a `cf_clearance` token. The
    /// renderer uses this to warn before retrying, since a CF-blocked
    /// site will reject the next request immediately without it
    pub has_cf_clearance: bool,
    /// Names of imported cookies (values omitted). Helpful for diagnosing
    /// "import succeeded but request still blocked" cases
    pub cookie_names: Vec<String>,
    /// Full cookie list with values, surfaced to the dialog so the user
    /// can confirm what's being sent. Local-IPC only
    pub cookies: Vec<ImportedCookieView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieEntryView {
    pub host: String,
    pub browser_id: String,
    pub user_agent: String,
    pub cookie_count: usize,
    pub imported_at: u64,
    pub last_validated_at: u64,
}

impl From<&CookieEntry> for CookieEntryView {
    fn from(e: &CookieEntry) -> Self {
        Self {
            host: e.host.clone(),
            browser_id: e.browser_id.clone(),
            user_agent: e.user_agent.clone(),
            cookie_count: e.cookies.len(),
            imported_at: e.imported_at,
            last_validated_at: e.last_validated_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryWithCookiesPayload {
    pub cookie: Option<String>,
    pub user_agent: Option<String>,
}

#[tauri::command]
pub async fn list_browsers_cmd() -> Vec<BrowserInfo> {
    list_browsers().await
}

#[tauri::command]
pub async fn import_browser_cookies(
    browser: String,
    url: String,
    persist: Option<bool>,
) -> Result<ImportedCookies, String> {
    log::info!("import_browser_cookies: browser={browser} url={url}");
    let HostCookies {
        host,
        user_agent,
        cookies,
    } = cookies_for_url(&browser, &url).await?;

    log::info!(
        "import_browser_cookies: rookie returned {} cookie(s) for host={host} (browser={browser})",
        cookies.len()
    );
    for c in cookies.iter() {
        log::debug!(
            "  imported cookie name={} domain={} path={} secure={} http_only={} value_len={}",
            c.name,
            c.domain,
            c.path,
            c.secure,
            c.http_only,
            c.value.len()
        );
    }

    if cookies.is_empty() {
        return Err(format!("no cookies found in {browser} for {host}"));
    }

    let stored: Vec<StoredCookie> = cookies
        .iter()
        .map(|c| StoredCookie {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            secure: c.secure,
            http_only: c.http_only,
            expires: c.expires,
        })
        .collect();
    let cookie_header = cookies_to_header(&stored);
    let has_cf_clearance = stored
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case("cf_clearance"));
    let cookie_names: Vec<String> = stored.iter().map(|c| c.name.clone()).collect();

    log::info!(
        "import_browser_cookies: cookie names for host={host}: {cookie_names:?} (cf_clearance present: {has_cf_clearance})"
    );

    if persist.unwrap_or(true) {
        if let Some(manager) = engine::get_manager().await {
            let entry = CookieEntry {
                host: host.clone(),
                browser_id: browser.clone(),
                user_agent: user_agent.clone(),
                cookies: stored.clone(),
                imported_at: 0,
                last_validated_at: 0,
            };
            manager.cookie_store().upsert(entry)?;
        }
    }

    let cookies_view: Vec<ImportedCookieView> = stored
        .iter()
        .map(|c| ImportedCookieView {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            secure: c.secure,
            http_only: c.http_only,
            expires: c.expires,
        })
        .collect();

    Ok(ImportedCookies {
        host,
        user_agent,
        cookie_header,
        count: cookies.len(),
        has_cf_clearance,
        cookie_names,
        cookies: cookies_view,
    })
}

#[tauri::command]
pub async fn list_cookie_entries() -> Result<Vec<CookieEntryView>, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    Ok(manager
        .cookie_store()
        .list()
        .iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
pub async fn delete_cookie_entry(host: String) -> Result<bool, String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.cookie_store().remove(&host)
}

#[tauri::command]
pub async fn clear_cookie_entries() -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager.cookie_store().clear()
}

#[tauri::command]
pub async fn retry_with_cookies(
    gid: String,
    payload: RetryWithCookiesPayload,
) -> Result<(), String> {
    let manager = engine::get_manager().await.ok_or("Engine not running")?;
    manager
        .retry_with_cookies(&gid, payload.cookie, payload.user_agent)
        .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedUserAgent {
    pub user_agent: String,
}

/// Open `http://127.0.0.1:<random>/` in the user's default browser, read
/// the User-Agent header from the first GET, and return it. cf_clearance
/// validates against the UA in effect when the challenge was solved, so
/// reusing the cookie requires the matching UA
///
/// The internal listener replies with a small HTML page that auto-closes,
/// then shuts down. Times out after 60s if nothing connects
#[tauri::command]
pub async fn capture_user_agent() -> Result<CapturedUserAgent, String> {
    log::info!("capture_user_agent: starting one-shot listener");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("bind localhost listener failed: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("read local addr failed: {e}"))?
        .port();
    let url = format!("http://127.0.0.1:{port}/risuko-ua-capture");

    // Open in the user's default browser. Best effort; the user can also
    // paste the URL into a different browser if they prefer
    if let Err(e) = open::that(&url) {
        log::warn!("capture_user_agent: open::that({url}) failed: {e}");
        // Keep the listener alive in case the user pastes the URL manually
    }

    // Loop accept(): browsers may prefetch favicons or do CORS preflight
    // before the real GET arrives. Take the first request to
    // /risuko-ua-capture that parses cleanly
    let captured = timeout(Duration::from_secs(60), async {
        loop {
            let (mut stream, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("capture_user_agent: accept failed: {e}");
                    continue;
                }
            };
            log::debug!("capture_user_agent: connection from {peer}");

            let mut buf = vec![0u8; 4096];
            let mut total = 0usize;
            // Read until we see end-of-headers (double CRLF). Cap at 4 KiB
            let parsed: Option<(String, String)> = loop {
                match timeout(Duration::from_secs(5), stream.read(&mut buf[total..])).await {
                    Ok(Ok(0)) => break None,
                    Ok(Ok(n)) => {
                        total += n;
                        if let Some(end) = find_header_end(&buf[..total]) {
                            break parse_request(&buf[..end]);
                        }
                        if total == buf.len() {
                            break None;
                        }
                    }
                    _ => break None,
                }
            };

            // Always send a tiny HTML body so the browser tab doesn't hang
            let body = "<!doctype html><meta charset=utf-8><title>Risuko</title>\
                 <style>body{font:14px/1.5 system-ui;padding:2rem;color:#333}\
                 h1{font-size:18px}</style>\
                 <h1>User-Agent captured \u{2713}</h1>\
                 <p>You can close this tab and return to Risuko.</p>\
                 <script>setTimeout(()=>window.close(),1500)</script>";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;

            if let Some((path, ua)) = parsed {
                if path.starts_with("/risuko-ua-capture") && !ua.is_empty() {
                    return ua;
                }
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for browser to open the capture URL".to_string())?;

    log::info!("capture_user_agent: captured ua={captured}");
    Ok(CapturedUserAgent {
        user_agent: captured,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn parse_request(buf: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(buf).ok()?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next()?;
    // GET /path HTTP/1.1
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    let path = parts.next()?.to_string();

    let mut user_agent = String::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("user-agent") {
                user_agent = value.trim().to_string();
            }
        }
    }
    Some((path, user_agent))
}
