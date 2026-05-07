//! Gnutella2 (G2) — Phase 4. Single-file module since the wire format is
//! tightly scoped here. URI scheme: `g2://host:port/sha1/<base32>?xl=...&dn=...`

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::engine::gnutella::peer::fetch_by_urn;
use crate::engine::options::EngineOptions;

/// Errors emitted by the G2 download pipeline
#[derive(Debug, thiserror::Error)]
pub enum G2Error {
    #[error("invalid URI: {0}")]
    InvalidUri(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("network: {0}")]
    Network(String),
    #[error("no source has the requested file")]
    NoSource,
}

/// Parsed `g2://` content URI carrying a SHA-1 URN, optional display name and
/// file size hint
#[derive(Debug, Clone, Default)]
pub struct G2Link {
    pub host: String,
    pub port: u16,
    pub urn: Option<String>,
    pub file_name: String,
    pub file_size: u64,
}

/// True when the input begins with `g2://` (case-insensitive)
pub fn is_g2_uri(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    lower.starts_with("g2://")
}

/// Parse a `g2://host[:port]/sha1/<base32>[?xl=&dn=&urn=]` URI.
/// Returns `None` for malformed input or non-G2 schemes
pub fn parse_g2_uri(uri: &str) -> Option<G2Link> {
    let s = uri.trim();
    let rest = s
        .strip_prefix("g2://")
        .or_else(|| s.strip_prefix("G2://"))?;
    let (host_port, path_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = if let Some(idx) = host_port.find(':') {
        (
            host_port[..idx].to_string(),
            host_port[idx + 1..].parse().ok()?,
        )
    } else {
        (host_port.to_string(), 6346)
    };
    let (path, query) = match path_query.find('?') {
        Some(idx) => (&path_query[..idx], &path_query[idx + 1..]),
        None => (path_query, ""),
    };
    let mut urn: Option<String> = None;
    if let Some(rest) = path.strip_prefix("/sha1/") {
        urn = Some(format!("urn:sha1:{}", rest.trim_end_matches('/')));
    }
    let mut file_name = String::new();
    let mut file_size: u64 = 0;
    for part in query.split('&') {
        if let Some(rest) = part.strip_prefix("dn=") {
            file_name = url_decode(rest);
        } else if let Some(rest) = part.strip_prefix("xl=") {
            file_size = rest.parse().unwrap_or(0);
        } else if let Some(rest) = part.strip_prefix("urn=") {
            urn = Some(rest.to_string());
        }
    }
    Some(G2Link {
        host,
        port,
        urn,
        file_name,
        file_size,
    })
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if b == b'+' { b' ' } else { b });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Run a single G2 download to completion. Reuses the Gnutella HTTP
/// `uri-res/N2R` fetch path since both networks serve content over the same
/// peer-to-peer HTTP/1.1 endpoint. Returns the absolute output path on
/// success or a string describing the failure (`"cancelled"` when aborted)
pub async fn run_g2_download(
    uri: &str,
    dir: &str,
    _opts: &EngineOptions,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    let _ = (speed, connections);
    if !is_g2_uri(uri) {
        return Err(format!("not a G2 URI: {uri}"));
    }
    let link = parse_g2_uri(uri).ok_or_else(|| "invalid g2 URI".to_string())?;
    let urn = link
        .urn
        .as_deref()
        .ok_or_else(|| "G2 URI missing sha1/urn".to_string())?;
    if link.file_size == 0 {
        return Err("G2 URI missing xl/size".into());
    }
    total.store(link.file_size, Ordering::Relaxed);
    if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
        return Err("cancelled".into());
    }
    let safe = sanitize(if link.file_name.is_empty() {
        urn.trim_start_matches("urn:sha1:")
    } else {
        &link.file_name
    });
    let out_path = PathBuf::from(dir).join(safe);
    fetch_by_urn(
        &link.host,
        link.port,
        "/uri-res/N2R",
        urn,
        link.file_size,
        &out_path,
        completed,
        cancel_token,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(out_path)
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '|' | '?' | '*' | '\0')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() || trimmed == ".." {
        "g2-download".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects() {
        assert!(is_g2_uri("g2://h:6346/sha1/ABC?xl=10&dn=x"));
        assert!(!is_g2_uri("gnutella://"));
    }
    #[test]
    fn parses() {
        let l = parse_g2_uri("g2://peer.example.com:6346/sha1/ABCDEF?xl=42&dn=foo.bin").unwrap();
        assert_eq!(l.host, "peer.example.com");
        assert_eq!(l.port, 6346);
        assert_eq!(l.urn.as_deref(), Some("urn:sha1:ABCDEF"));
        assert_eq!(l.file_size, 42);
        assert_eq!(l.file_name, "foo.bin");
    }
}
