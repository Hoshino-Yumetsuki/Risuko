// Safari cookie extraction

#[cfg(target_os = "macos")]
use crate::browser::chromium::Cookie;
#[cfg(target_os = "macos")]
use crate::utils::{paths, time};
#[cfg(target_os = "macos")]
use eyre::{bail, Result};

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

    let data = std::fs::read(&cookie_db)?;
    let raw_cookies = parse_binary_cookies(&data)?;

    tracing::debug!(target: "risuko_cookies", "safari: total cookies parsed = {}", raw_cookies.len());

    let mut cookies = Vec::new();
    let mut skipped_domain = 0usize;

    for raw in raw_cookies {
        if let Some(request_host) = host {
            if !cookie_covers_host(request_host, &raw.domain) {
                tracing::trace!(target: "risuko_cookies", "safari: skip cookie '{}' domain={} (does not cover {})", raw.name, raw.domain, request_host);
                skipped_domain += 1;
                continue;
            }
        }
        tracing::trace!(target: "risuko_cookies", "safari: keep cookie '{}' domain={} value_len={}", raw.name, raw.domain, raw.value.len());
        cookies.push(Cookie {
            name: raw.name,
            value: raw.value,
            domain: raw.domain,
            path: raw.path,
            secure: raw.secure,
            http_only: raw.http_only,
            expires: raw.expires,
        });
    }

    tracing::debug!(target: "risuko_cookies",
        "safari: host_filter={:?} -> kept={} skipped_domain={}",
        host, cookies.len(), skipped_domain
    );

    Ok(cookies)
}

#[cfg(target_os = "macos")]
struct RawSafariCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires: Option<u64>,
}

#[cfg(target_os = "macos")]
fn parse_binary_cookies(data: &[u8]) -> Result<Vec<RawSafariCookie>> {
    if data.len() < 8 {
        bail!("binary cookies file too short");
    }

    if &data[..4] != b"cook" {
        bail!("invalid binary cookies magic");
    }

    let num_pages = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
    if num_pages == 0 {
        return Ok(Vec::new());
    }

    let header_size = 8 + num_pages * 4;
    if data.len() < header_size {
        bail!("binary cookies header truncated");
    }

    let mut cookies = Vec::new();
    for i in 0..num_pages {
        let off = 8 + i * 4;
        let page_offset = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        if page_offset >= data.len() {
            bail!("page offset out of bounds");
        }
        parse_page(&data[page_offset..], &mut cookies)?;
    }

    Ok(cookies)
}

#[cfg(target_os = "macos")]
fn parse_page(page: &[u8], cookies: &mut Vec<RawSafariCookie>) -> Result<()> {
    if page.len() < 8 {
        bail!("page too short");
    }

    let num_cookies = u32::from_be_bytes(page[4..8].try_into().unwrap()) as usize;
    if num_cookies == 0 {
        return Ok(());
    }

    let header_size = 8 + num_cookies * 4;
    if page.len() < header_size {
        bail!("page header truncated");
    }

    for i in 0..num_cookies {
        let off = 8 + i * 4;
        let cookie_offset = u32::from_be_bytes(page[off..off + 4].try_into().unwrap()) as usize;
        if cookie_offset >= page.len() {
            bail!("cookie offset out of bounds");
        }
        parse_cookie(&page[cookie_offset..], cookies)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn parse_cookie(cookie: &[u8], cookies: &mut Vec<RawSafariCookie>) -> Result<()> {
    if cookie.len() < 44 {
        bail!("cookie record too short");
    }

    let flags = u32::from_be_bytes(cookie[4..8].try_into().unwrap());
    let url_offset = u32::from_be_bytes(cookie[12..16].try_into().unwrap()) as usize;
    let name_offset = u32::from_be_bytes(cookie[16..20].try_into().unwrap()) as usize;
    let path_offset = u32::from_be_bytes(cookie[20..24].try_into().unwrap()) as usize;
    let value_offset = u32::from_be_bytes(cookie[24..28].try_into().unwrap()) as usize;

    let expiry_bytes: [u8; 8] = cookie[28..36].try_into().unwrap();
    let expiry = f64::from_be_bytes(expiry_bytes);

    let domain = read_cstr(cookie, url_offset)?;
    let name = read_cstr(cookie, name_offset)?;
    let path = read_cstr(cookie, path_offset)?;
    let value = read_cstr(cookie, value_offset)?;

    cookies.push(RawSafariCookie {
        name,
        value,
        domain,
        path,
        secure: (flags & 0x01) != 0,
        http_only: (flags & 0x04) != 0,
        expires: time::safari_to_unix(expiry),
    });

    Ok(())
}

#[cfg(target_os = "macos")]
fn read_cstr(data: &[u8], offset: usize) -> Result<String> {
    if offset >= data.len() {
        bail!("string offset out of bounds");
    }
    let end = data[offset..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(data.len() - offset);
    Ok(String::from_utf8_lossy(&data[offset..offset + end]).to_string())
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
