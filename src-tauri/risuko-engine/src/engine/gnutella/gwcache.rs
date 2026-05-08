//! GWebCache HTTP bootstrap. A GWebCache responds to `?hostfile=1` with a
//! line-separated list of `host:port` ultrapeers. We treat the configured
//! `gnutella-cache` value as a comma-separated list of cache URLs

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Fetch a list of `host:port` ultrapeer addresses from a GWebCache.
/// Issues `GET <path>?hostfile=1&client=RSKO&version=0.1` and parses the
/// response body line-by-line. Returns at most 32 peers; a network error
/// silently yields an empty list rather than propagating
pub async fn fetch_peers(cache_url: &str) -> Vec<String> {
    let mut peers = Vec::new();
    let parsed = match parse_http_url(cache_url) {
        Some(v) => v,
        None => return peers,
    };
    let (host, port, path) = parsed;
    let sep = if path.contains('?') { '&' } else { '?' };
    let req = format!(
        "GET {path}{sep}hostfile=1&client=RSKO&version=0.1 HTTP/1.0\r\nHost: {host}\r\nUser-Agent: Risuko/0.1\r\nConnection: close\r\n\r\n",
    );
    let stream = match timeout(
        Duration::from_secs(8),
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        _ => return peers,
    };
    let (rd, mut wr) = tokio::io::split(stream);
    if wr.write_all(req.as_bytes()).await.is_err() {
        return peers;
    }
    let mut reader = BufReader::new(rd);
    let mut in_body = false;
    let mut line = String::new();
    while let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 {
            break;
        }
        let trimmed = line.trim_end().to_string();
        if !in_body {
            if trimmed.is_empty() {
                in_body = true;
            }
        } else if trimmed.contains(':') && !trimmed.starts_with("HTTP/") {
            peers.push(trimmed);
        }
        line.clear();
        if peers.len() >= 32 {
            break;
        }
    }
    peers
}

fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let (scheme_stripped, default_port) = if let Some(r) = url.strip_prefix("http://") {
        (r, 80u16)
    } else {
        return None;
    };
    let (host_port, path) = match scheme_stripped.find('/') {
        Some(idx) => (&scheme_stripped[..idx], &scheme_stripped[idx..]),
        None => (scheme_stripped, "/"),
    };
    let (host, port) = if let Some(idx) = host_port.find(':') {
        (
            host_port[..idx].to_string(),
            host_port[idx + 1..].parse().ok()?,
        )
    } else {
        (host_port.to_string(), default_port)
    };
    Some((host, port, path.to_string()))
}
