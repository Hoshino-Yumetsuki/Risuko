//! Gnutella 0.6 handshake (HTTP-style three-step):
//!
//! C -> S:  GNUTELLA CONNECT/0.6\r\n<headers>\r\n\r\n
//! S -> C:  GNUTELLA/0.6 200 OK\r\n<headers>\r\n\r\n
//! C -> S:  GNUTELLA/0.6 200 OK\r\n<headers>\r\n\r\n

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::types::GnutellaError;

/// Run the Gnutella 0.6 three-step handshake on an open TCP socket. Sends
/// the leaf-mode `CONNECT/0.6` request, reads the remote `200 OK` reply
/// and acknowledges. Returns `Network` error on EOF or non-200 responses.
/// The entire exchange is bounded by a 10-second timeout so a stalled peer
/// does not block the caller indefinitely
pub async fn perform_handshake(sock: &mut TcpStream) -> Result<(), GnutellaError> {
    timeout(Duration::from_secs(10), do_handshake(sock))
        .await
        .map_err(|_| GnutellaError::Network("handshake timeout".into()))?
}

async fn do_handshake(sock: &mut TcpStream) -> Result<(), GnutellaError> {
    let req = "GNUTELLA CONNECT/0.6\r\n\
User-Agent: Risuko/0.1\r\n\
X-Ultrapeer: False\r\n\
\r\n";
    sock.write_all(req.as_bytes()).await?;

    let mut reader = BufReader::new(&mut *sock);
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Err(GnutellaError::Network("eof during handshake".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(GnutellaError::Network("handshake too large".into()));
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let first = head.lines().next().unwrap_or("");
    if !first.starts_with("GNUTELLA/0.6 200") {
        return Err(GnutellaError::Network(format!("rejected: {first}")));
    }
    let confirm = "GNUTELLA/0.6 200 OK\r\n\r\n";
    reader.get_mut().write_all(confirm.as_bytes()).await?;
    Ok(())
}
