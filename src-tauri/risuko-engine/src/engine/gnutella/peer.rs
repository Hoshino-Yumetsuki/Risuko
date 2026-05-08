//! Peer-to-peer file fetch over HTTP/1.1 with Gnutella URN headers

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::types::GnutellaError;

/// Download a single file from a Gnutella servent over HTTP/1.1 by URN.
/// Issues `GET <n2r_path>?<urn> HTTP/1.1` with `X-Gnutella-Content-URN` and
/// streams the body to `out_path`. The `completed` counter is updated as
/// bytes arrive; cancellation aborts the read loop
pub async fn fetch_by_urn(
    host: &str,
    port: u16,
    n2r_path: &str,
    urn: &str,
    file_size: u64,
    out_path: &Path,
    completed: Arc<AtomicU64>,
    cancel: CancellationToken,
) -> Result<(), GnutellaError> {
    let stream = timeout(Duration::from_secs(15), TcpStream::connect((host, port)))
        .await
        .map_err(|_| GnutellaError::Network("peer connect timeout".into()))??;
    let (rd, mut wr) = tokio::io::split(stream);
    let req = format!(
        "GET {}?{} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: Risuko/0.1\r\nX-Gnutella-Content-URN: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        n2r_path, urn, host, port, urn
    );
    wr.write_all(req.as_bytes()).await?;

    let mut reader = BufReader::new(rd);
    // read headers line-by-line until the blank CRLF separator
    let mut header_buf = Vec::new();
    loop {
        let n = reader.read_until(b'\n', &mut header_buf).await?;
        if n == 0 {
            return Err(GnutellaError::Network("eof in headers".into()));
        }
        if header_buf.len() > 32 * 1024 {
            return Err(GnutellaError::Network("header too large".into()));
        }
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let status_line = header_str.lines().next().unwrap_or("");
    let code: Option<u16> = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|t| t.parse().ok());
    if !matches!(code, Some(200) | Some(206)) {
        return Err(GnutellaError::Network(format!("HTTP: {status_line}")));
    }

    if let Some(parent) = out_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut f = tokio::fs::File::create(out_path).await?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut remaining = file_size;
    while remaining > 0 {
        let take = buf.len().min(remaining as usize);
        let n = tokio::select! {
            read_res = timeout(Duration::from_secs(30), reader.read(&mut buf[..take])) => {
                match read_res {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return Err(GnutellaError::Io(e)),
                    Err(_) => return Err(GnutellaError::Network("read timeout".into())),
                }
            }
            _ = cancel.cancelled() => {
                return Err(GnutellaError::Network("cancelled".into()));
            }
        };
        if n == 0 {
            return Err(GnutellaError::Network(
                "unexpected EOF while reading file".into(),
            ));
        }
        f.write_all(&buf[..n]).await?;
        remaining -= n as u64;
        completed.fetch_add(n as u64, Ordering::Relaxed);
    }
    f.flush().await?;
    Ok(())
}
