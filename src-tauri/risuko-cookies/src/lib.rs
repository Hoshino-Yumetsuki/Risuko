//! Browser cookie extraction façade
//!
//! Wraps the synchronous `rookie` crate behind async helpers and exposes a
//! flat list of supported browsers per host OS. Host-agnostic: both the
//! engine and the Tauri command layer pull from here

use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<u64>,
}

impl From<rookie::enums::Cookie> for Cookie {
    fn from(c: rookie::enums::Cookie) -> Self {
        Self {
            name: c.name,
            value: c.value,
            domain: c.domain,
            path: c.path,
            secure: c.secure,
            http_only: c.http_only,
            expires: c.expires,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub user_agent: String,
}

/// Format cookies into a single `Cookie:` header value
pub fn cookies_to_header(cookies: &[Cookie]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// List browsers known to rookie on this OS. `available` reflects whether
/// a quick probe (no domain filter) returned without an error; sandbox
/// failures or missing profiles surface as `false`
pub async fn list_browsers() -> Vec<BrowserInfo> {
    spawn_blocking(list_browsers_sync).await.unwrap_or_default()
}

fn list_browsers_sync() -> Vec<BrowserInfo> {
    let mut out: Vec<BrowserInfo> = Vec::new();

    macro_rules! probe {
        ($id:literal, $name:literal, $ua:expr, $f:expr) => {{
            let available = $f(None).is_ok();
            out.push(BrowserInfo {
                id: $id.into(),
                name: $name.into(),
                available,
                user_agent: $ua.into(),
            });
        }};
    }

    probe!(
        "chrome",
        "Google Chrome",
        default_ua("chrome"),
        rookie::chrome
    );
    probe!(
        "chromium",
        "Chromium",
        default_ua("chrome"),
        rookie::chromium
    );
    probe!("brave", "Brave", default_ua("chrome"), rookie::brave);
    probe!("edge", "Microsoft Edge", default_ua("edge"), rookie::edge);
    probe!("vivaldi", "Vivaldi", default_ua("chrome"), rookie::vivaldi);
    probe!("opera", "Opera", default_ua("chrome"), rookie::opera);
    probe!("arc", "Arc", default_ua("chrome"), rookie::arc);
    probe!("firefox", "Firefox", default_ua("firefox"), rookie::firefox);
    probe!(
        "librewolf",
        "LibreWolf",
        default_ua("firefox"),
        rookie::librewolf
    );
    probe!("zen", "Zen", default_ua("firefox"), rookie::zen);

    #[cfg(target_os = "windows")]
    {
        probe!(
            "opera_gx",
            "Opera GX",
            default_ua("chrome"),
            rookie::opera_gx
        );
    }
    #[cfg(target_os = "macos")]
    {
        probe!(
            "opera_gx",
            "Opera GX",
            default_ua("chrome"),
            rookie::opera_gx
        );
        probe!("safari", "Safari", default_ua("safari"), rookie::safari);
    }
    #[cfg(target_os = "linux")]
    {
        probe!(
            "cachy",
            "Cachy Browser",
            default_ua("firefox"),
            rookie::cachy
        );
    }

    out
}

/// Pull cookies for `host` (and parent domains) from the named browser.
/// Returns `Err(message)` on rookie failure so callers can surface it
pub async fn cookies_for_host(browser: &str, host: &str) -> Result<Vec<Cookie>, String> {
    let browser = browser.to_string();
    let host = host.to_string();
    spawn_blocking(move || cookies_for_host_sync(&browser, &host))
        .await
        .map_err(|e| format!("join error: {e}"))?
}

fn cookies_for_host_sync(browser: &str, host: &str) -> Result<Vec<Cookie>, String> {
    // rookie does a substring match on the cookie's domain field. Passing
    // both the bare host and its eTLD+1 catches `example.com` and
    // `.example.com` alike. The substring filter can also pull in
    // unrelated cookies (e.g. for `co.uk` when the host is on a multi-part
    // ccTLD); the post-filter below enforces a real RFC 6265 host-match
    // before returning to callers
    let mut domains: Vec<String> = vec![host.to_string()];
    if let Some(parent) = registrable_domain(host) {
        if parent != host {
            domains.push(parent);
        }
    }

    tracing::info!("risuko-cookies: rookie fetch browser={browser} domains={domains:?}");
    let raw = call_rookie(browser, Some(domains.clone())).inspect_err(|e| {
        tracing::warn!(
            "risuko-cookies: rookie fetch failed browser={browser} domains={domains:?} err={e}"
        );
    })?;
    tracing::info!(
        "risuko-cookies: rookie returned {} cookie(s) for browser={browser} host={host}",
        raw.len()
    );
    for c in raw.iter() {
        tracing::debug!(
            "  cookie name={name} domain={domain} path={path} secure={secure} http_only={http_only} value_len={value_len}",
            name = c.name,
            domain = c.domain,
            path = c.path,
            secure = c.secure,
            http_only = c.http_only,
            value_len = c.value.len(),
        );
    }
    let total = raw.len();
    let filtered: Vec<Cookie> = raw
        .into_iter()
        .filter(|c| cookie_domain_matches_host(&c.domain, host))
        .map(Cookie::from)
        .collect();
    if filtered.len() != total {
        tracing::info!(
            "risuko-cookies: filtered {} cookie(s) down to {} after host-match for {}",
            total,
            filtered.len(),
            host
        );
    }
    Ok(filtered)
}

/// RFC 6265 host-match: a cookie applies to `host` when the cookie's
/// domain attribute equals `host`, or when `host` is a subdomain of the
/// cookie domain. Leading dots on the cookie domain (legacy form) are
/// stripped before comparison. Hosts and domains are matched
/// case-insensitively
fn cookie_domain_matches_host(cookie_domain: &str, host: &str) -> bool {
    let host_l = host.trim().to_ascii_lowercase();
    let domain_l = cookie_domain.trim().trim_start_matches('.').to_ascii_lowercase();
    if domain_l.is_empty() || host_l.is_empty() {
        return false;
    }
    if host_l == domain_l {
        return true;
    }
    host_l.ends_with(&format!(".{domain_l}"))
}

/// Resolve a URL or bare host to (host, cookies, ua) for the named browser
pub async fn cookies_for_url(browser: &str, target: &str) -> Result<HostCookies, String> {
    let host = extract_host(target).ok_or_else(|| format!("invalid url: {target}"))?;
    let cookies = cookies_for_host(browser, &host).await?;
    Ok(HostCookies {
        host,
        user_agent: default_ua_for(browser),
        cookies,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCookies {
    pub host: String,
    pub user_agent: String,
    pub cookies: Vec<Cookie>,
}

fn call_rookie(
    browser: &str,
    domains: Option<Vec<String>>,
) -> Result<Vec<rookie::enums::Cookie>, String> {
    let r = match browser {
        "chrome" => rookie::chrome(domains),
        "chromium" => rookie::chromium(domains),
        "brave" => rookie::brave(domains),
        "edge" => rookie::edge(domains),
        "vivaldi" => rookie::vivaldi(domains),
        "opera" => rookie::opera(domains),
        "arc" => rookie::arc(domains),
        "firefox" => rookie::firefox(domains),
        "librewolf" => rookie::librewolf(domains),
        "zen" => rookie::zen(domains),
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        "opera_gx" => rookie::opera_gx(domains),
        #[cfg(target_os = "macos")]
        "safari" => rookie::safari(domains),
        #[cfg(target_os = "linux")]
        "cachy" => rookie::cachy(domains),
        other => return Err(format!("unknown browser: {other}")),
    };
    r.map_err(|e| e.to_string())
}

fn default_ua_for(browser: &str) -> String {
    let kind = match browser {
        "firefox" | "librewolf" | "zen" | "cachy" => "firefox",
        "edge" => "edge",
        "safari" => "safari",
        _ => "chrome",
    };
    default_ua(kind).to_string()
}

fn default_ua(kind: &str) -> &'static str {
    // Fallback UAs used before the user captures a real one through the
    // dialog's "Detect from browser" flow. Whatever they capture is what
    // should reach Cloudflare; these strings only keep imports from
    // shipping empty values
    match kind {
        "firefox" => "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:141.0) Gecko/20100101 Firefox/141.0",
        "edge" => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36 Edg/139.0.0.0",
        "safari" => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
        _ => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36",
    }
}

fn extract_host(target: &str) -> Option<String> {
    if let Ok(url) = Url::parse(target) {
        return url.host_str().map(|s| s.to_string());
    }
    let trimmed = target
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = trimmed.split('/').next()?;
    let host = host.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Best-effort registrable domain. Returns the eTLD+1 for typical
/// hostnames, and falls back to eTLD+2 when the trailing two labels look
/// like a known multi-label public suffix (`co.uk`, `com.au`, `co.jp`,
/// etc). We avoid pulling in the full Public Suffix List — this only
/// needs to be good enough to widen rookie's substring filter without
/// over-broadening into a public suffix that would match unrelated
/// sites. The `cookie_domain_matches_host` post-filter in
/// `cookies_for_host_sync` enforces correctness for whatever rookie
/// returns
fn registrable_domain(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    let two = &labels[labels.len() - 2..];
    if labels.len() >= 3 && is_multi_label_public_suffix(two[0], two[1]) {
        return Some(labels[labels.len() - 3..].join("."));
    }
    Some(two.join("."))
}

/// True for the most common second-level public suffixes that are
/// shaped like `<role>.<cc>` (`co.uk`, `com.au`, `ac.jp`, ...). Not
/// exhaustive; this is a conservative widening guard, not a security
/// boundary
fn is_multi_label_public_suffix(role: &str, cc: &str) -> bool {
    if cc.len() != 2 {
        return false;
    }
    matches!(
        role.to_ascii_lowercase().as_str(),
        "co" | "com" | "net" | "org" | "edu" | "gov" | "mil" | "ac" | "or" | "ne" | "go"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_from_url() {
        assert_eq!(
            extract_host("https://example.com/path"),
            Some("example.com".into())
        );
        assert_eq!(
            extract_host("http://sub.example.com:8080"),
            Some("sub.example.com".into())
        );
        assert_eq!(extract_host("example.com/foo"), Some("example.com".into()));
    }

    #[test]
    fn registrable_strips_one_label() {
        assert_eq!(
            registrable_domain("a.b.example.com"),
            Some("example.com".into())
        );
        assert_eq!(
            registrable_domain("example.com"),
            Some("example.com".into())
        );
        assert_eq!(registrable_domain("localhost"), None);
    }

    #[test]
    fn registrable_widens_to_three_labels_for_known_multi_part_tlds() {
        assert_eq!(
            registrable_domain("foo.example.co.uk"),
            Some("example.co.uk".into())
        );
        assert_eq!(
            registrable_domain("a.b.bbc.co.uk"),
            Some("bbc.co.uk".into())
        );
        assert_eq!(
            registrable_domain("shop.example.com.au"),
            Some("example.com.au".into())
        );
        // 2-label inputs cannot widen further; return as-is
        assert_eq!(registrable_domain("co.uk"), Some("co.uk".into()));
    }

    #[test]
    fn cookie_domain_host_match_rules() {
        // Exact match
        assert!(cookie_domain_matches_host("example.com", "example.com"));
        // Leading dot is treated like host-only (legacy compatibility)
        assert!(cookie_domain_matches_host(".example.com", "dl.example.com"));
        // Subdomain match
        assert!(cookie_domain_matches_host("example.com", "dl.example.com"));
        // Suffix-but-not-dot-bounded must NOT match
        assert!(!cookie_domain_matches_host("example.com", "notexample.com"));
        // Unrelated domain
        assert!(!cookie_domain_matches_host("attacker.com", "example.com"));
        // Public-suffix-style cookie should not match unrelated subdomains
        assert!(!cookie_domain_matches_host(".co.uk", "example.com"));
        // But still legitimately matches when host is on that domain
        assert!(cookie_domain_matches_host(".co.uk", "foo.co.uk"));
    }

    #[test]
    fn cookies_to_header_format() {
        let cs = vec![
            Cookie {
                name: "a".into(),
                value: "1".into(),
                domain: "example.com".into(),
                path: "/".into(),
                secure: true,
                http_only: false,
                expires: None,
            },
            Cookie {
                name: "b".into(),
                value: "two".into(),
                domain: "example.com".into(),
                path: "/".into(),
                secure: true,
                http_only: false,
                expires: None,
            },
        ];
        assert_eq!(cookies_to_header(&cs), "a=1; b=two");
    }
}
