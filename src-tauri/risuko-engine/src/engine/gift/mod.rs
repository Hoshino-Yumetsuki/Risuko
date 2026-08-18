//! giFT (FastTrack/OpenFT/Gnutella IPC bridge) — Phase 5. Connects to a local giftd over TCP/1213 and forwards a download request; URI scheme `gift://<inner-uri>` forwards `<inner-uri>` verbatim to giftd. The legacy IPC frame format is line-oriented: each command is one line ending in `\n`, fields space-separated

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use crate::engine::options::EngineOptions;

/// Parsed `gift://` URI; the part after the scheme is forwarded verbatim to the local giFT daemon as the inner network/file identifier
pub struct GiftLink {
    pub inner: String,
}

/// True when the input begins with `gift://` (case-insensitive)
pub fn is_gift_uri(uri: &str) -> bool {
    uri.trim().to_ascii_lowercase().starts_with("gift://")
}

/// Parse a `gift://<inner>` URI and capture the inner payload as-is
pub fn parse_gift_uri(uri: &str) -> Option<GiftLink> {
    let s = uri.trim();
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("gift://") {
        Some(GiftLink {
            inner: s[7..].to_string(),
        })
    } else {
        None
    }
}

/// Drive a download through a locally-running `giftd` daemon over its IPC socket (default `127.0.0.1:1213`), honouring the `gift-enabled`, `gift-host` and `gift-port` engine options; returns the output path on success or a failure string (`"cancelled"` on abort)
pub async fn run_gift_download(
    uri: &str,
    dir: &str,
    opts: &EngineOptions,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    if !is_gift_uri(uri) {
        return Err(format!("not a giFT URI: {uri}"));
    }
    let link = parse_gift_uri(uri).ok_or_else(|| "invalid gift URI".to_string())?;
    if link
        .inner
        .chars()
        .any(|c| c == '"' || c == '\n' || c == '\r' || c == '\0')
    {
        return Err("giFT inner URI contains forbidden control characters".into());
    }
    if !opts.get_bool("gift-enabled").unwrap_or(false) {
        return Err("giFT bridge is disabled in preferences".into());
    }
    let host = opts.get_str("gift-host").unwrap_or("127.0.0.1").to_string();
    let port: u16 = match opts.get_u64("gift-port") {
        Some(n) if (1..=u16::MAX as u64).contains(&n) => n as u16,
        Some(n) => {
            return Err(format!("invalid gift-port {n}: expected 1..={}", u16::MAX));
        }
        None => 1213,
    };

    let proxy = opts.p2p_proxy_connector()?;
    let stream = timeout(Duration::from_secs(5), proxy.connect_tcp(&host, port))
        .await
        .map_err(|_| "giftd connect timeout".to_string())?
        .map_err(|e| format!("giftd connect: {e}"))?;
    let (rd, mut wr) = tokio::io::split(stream);
    let mut reader = BufReader::new(rd);

    // ATTACH: register as a UI client
    wr.write_all(b"ATTACH risuko\n")
        .await
        .map_err(|e| e.to_string())?;

    let out_dir = PathBuf::from(dir);
    tokio::fs::create_dir_all(&out_dir)
        .await
        .map_err(|e| e.to_string())?;

    // TRANSFER ADD url="..." path="..."
    let safe_name =
        crate::engine::util::safe_filename(&extract_gift_name(&link.inner), "gift-download");
    let out_path = out_dir.join(&safe_name);
    let out_path_str = out_path.display().to_string();
    if out_path_str
        .chars()
        .any(|c| matches!(c, '"' | '\n' | '\r' | '\0'))
    {
        return Err("output path contains forbidden control characters".into());
    }
    let cmd = format!(
        "TRANSFER ADD url=\"{}\" path=\"{}\"\n",
        link.inner,
        out_path.display()
    );
    wr.write_all(cmd.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    // Watch transfer status events; giftd emits lines like `TRANSFER STATUS id=<n> total=<bytes> done=<bytes> state=<active|complete|...>`
    let mut transfer_id: Option<String> = None;
    let mut last_progress: Option<(tokio::time::Instant, u64)> = None;
    let mut line = String::new();
    loop {
        if cancel_token.is_cancelled() {
            if let Some(id) = transfer_id.as_ref() {
                let _ = wr
                    .write_all(format!("TRANSFER REMOVE id={id}\n").as_bytes())
                    .await;
            }
            return Err("cancelled".into());
        }
        line.clear();
        let read = timeout(Duration::from_secs(5), reader.read_line(&mut line)).await;
        match read {
            Ok(Ok(0)) => return Err("giftd disconnected".into()),
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => {
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        }
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("TRANSFER STATUS ") {
            let mut id = None;
            let mut t = 0u64;
            let mut d = 0u64;
            let mut state = String::new();
            for kv in rest.split_whitespace() {
                if let Some(v) = kv.strip_prefix("id=") {
                    id = Some(v.to_string());
                } else if let Some(v) = kv.strip_prefix("total=") {
                    t = v.parse().unwrap_or(0);
                } else if let Some(v) = kv.strip_prefix("done=") {
                    d = v.parse().unwrap_or(0);
                } else if let Some(v) = kv.strip_prefix("state=") {
                    state = v.to_string();
                }
            }
            if let Some(i) = id {
                transfer_id = Some(i);
            }
            if t > 0 {
                total.store(t, Ordering::Relaxed);
            }
            completed.store(d, Ordering::Relaxed);
            // Compute speed from delta bytes / elapsed time
            let now = tokio::time::Instant::now();
            if let Some((prev_time, prev_done)) = last_progress {
                let elapsed_ms = now.duration_since(prev_time).as_millis() as u64;
                let delta = d.saturating_sub(prev_done);
                if let Some(rate) = delta.saturating_mul(1000).checked_div(elapsed_ms) {
                    speed.store(rate, Ordering::Relaxed);
                }
            }
            last_progress = Some((now, d));
            if matches!(state.as_str(), "active" | "complete") {
                connections.store(1, Ordering::Relaxed);
            }
            match state.as_str() {
                "complete" => return Ok(out_path),
                "cancelled" | "error" | "failed" => return Err(format!("giftd transfer {state}")),
                _ => {}
            }
        }
    }
}

/// Derive an output filename from a giFT inner URI by taking the last path component before any query string; returns `"gift-download"` when no usable name is present
pub fn extract_gift_name(inner: &str) -> String {
    if let Some(rest) = inner.split('?').next() {
        if let Some(name) = rest.rsplit('/').next() {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "gift-download".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects() {
        assert!(is_gift_uri("gift://FastTrack/sha1/ABC"));
        assert!(!is_gift_uri("gnutella://"));
    }
    #[test]
    fn parses() {
        let l = parse_gift_uri("gift://OpenFT/file?dn=foo&xl=42").unwrap();
        assert_eq!(l.inner, "OpenFT/file?dn=foo&xl=42");
    }
}
