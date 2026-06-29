//! Gnutella2 (G2) — Phase 4. Single-file module since the wire format is
//! tightly scoped here. URI scheme: `g2://host:port/sha1/<base32>?xl=...&dn=...`

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use percent_encoding::percent_decode_str;
use tokio_util::sync::CancellationToken;

use crate::engine::gnutella::peer::fetch_by_urn;
use crate::engine::gnutella::types::GnutellaError;
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
            if urn.is_none() {
                if rest.starts_with("urn:sha1:") {
                    urn = Some(rest.to_string());
                } else if rest.len() == 32
                    && rest
                        .bytes()
                        .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'2'..=b'7'))
                {
                    urn = Some(format!("urn:sha1:{}", rest.to_ascii_uppercase()));
                }
            }
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
    let normalized = s.replace('+', " ");
    percent_decode_str(&normalized)
        .decode_utf8_lossy()
        .to_string()
}

/// Run a single G2 download to completion. Reuses the Gnutella HTTP
/// `uri-res/N2R` fetch path since both networks serve content over the same
/// peer-to-peer HTTP/1.1 endpoint. Returns the absolute output path on
/// success or a typed `G2Error` describing the failure
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
) -> Result<PathBuf, G2Error> {
    if !is_g2_uri(uri) {
        return Err(G2Error::InvalidUri(format!("not a G2 URI: {uri}")));
    }
    let link = parse_g2_uri(uri).ok_or_else(|| G2Error::InvalidUri("invalid g2 URI".into()))?;
    let urn = link
        .urn
        .as_deref()
        .ok_or_else(|| G2Error::InvalidUri("G2 URI missing sha1/urn".into()))?;
    if link.file_size == 0 {
        return Err(G2Error::InvalidUri("G2 URI missing xl/size".into()));
    }
    total.store(link.file_size, Ordering::Relaxed);
    if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
        return Err(G2Error::Network("cancelled".into()));
    }
    connections.store(1, Ordering::Relaxed);
    let sampler_cancel = CancellationToken::new();
    let sampler_cancel_task = sampler_cancel.clone();
    let cancel_token_sampler = cancel_token.clone();
    let sampler_speed = speed.clone();
    let sampler_completed = completed.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        let mut prev = sampler_completed.load(Ordering::Relaxed);
        loop {
            tokio::select! {
                _ = sampler_cancel_task.cancelled() => break,
                _ = cancel_token_sampler.cancelled() => break,
                _ = interval.tick() => {
                    let current = sampler_completed.load(Ordering::Relaxed);
                    sampler_speed.store(current.saturating_sub(prev), Ordering::Relaxed);
                    prev = current;
                }
            }
        }
    });
    let safe = crate::engine::util::safe_filename(
        if link.file_name.is_empty() {
            urn.trim_start_matches("urn:sha1:")
        } else {
            &link.file_name
        },
        "g2-download",
    );
    let out_path = PathBuf::from(dir).join(safe);
    let download_result = fetch_by_urn(
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
    .map_err(|e| match e {
        GnutellaError::InvalidUri(msg) => G2Error::InvalidUri(msg),
        GnutellaError::Io(err) => G2Error::Io(err),
        GnutellaError::Network(msg) => G2Error::Network(msg),
        GnutellaError::NoSource => G2Error::NoSource,
    });
    sampler_cancel.cancel();
    connections.store(0, Ordering::Relaxed);
    speed.store(0, Ordering::Relaxed);
    download_result?;
    Ok(out_path)
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
