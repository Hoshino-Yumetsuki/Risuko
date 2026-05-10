//! GWebCache HTTP bootstrap. A GWebCache responds to `?hostfile=1` with a
//! line-separated list of `host:port` ultrapeers. We treat the configured
//! `gnutella-cache` value as a comma-separated list of cache URLs

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

const MAX_LINE_BYTES: usize = 8 * 1024;

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
    match timeout(Duration::from_secs(5), wr.write_all(req.as_bytes())).await {
        Ok(Ok(())) => {}
        _ => return peers,
    }
    let mut reader = BufReader::new(rd);
    let mut in_body = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = match timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut line)).await {
            Ok(Ok(n)) => n,
            Ok(Err(_)) => break,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        if line.len() > MAX_LINE_BYTES {
            break;
        }
        let trimmed = String::from_utf8_lossy(&line).trim_end().to_string();
        if !in_body {
            if trimmed.is_empty() {
                in_body = true;
            }
        } else if trimmed.contains(':') && !trimmed.starts_with("HTTP/") {
            peers.push(trimmed);
        }
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
        None => match scheme_stripped.find('?') {
            Some(idx) => (&scheme_stripped[..idx], &scheme_stripped[idx - 0..]),
            None => (scheme_stripped, "/"),
        },
    };
    let path = if path.starts_with('?') {
        format!("/{}", path)
    } else {
        path.to_string()
    };
    let (host, port) = if let Some(idx) = host_port.find(':') {
        (
            host_port[..idx].to_string(),
            host_port[idx + 1..].parse().ok()?,
        )
    } else {
        (host_port.to_string(), default_port)
    };
    Some((host, port, path))
}
