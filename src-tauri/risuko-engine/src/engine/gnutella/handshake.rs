//! Gnutella 0.6 handshake (HTTP-style three-step):
//!
//! C -> S:  GNUTELLA CONNECT/0.6\r\n<headers>\r\n\r\n
//! S -> C:  GNUTELLA/0.6 200 OK\r\n<headers>\r\n\r\n
//! C -> S:  GNUTELLA/0.6 200 OK\r\n<headers>\r\n\r\n

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::types::GnutellaError;

/// Run the Gnutella 0.6 three-step handshake on an open TCP socket. Sends
/// the leaf-mode `CONNECT/0.6` request, reads the remote `200 OK` reply
/// and acknowledges. Returns `Network` error on EOF or non-200 responses
pub async fn perform_handshake(sock: &mut TcpStream) -> Result<(), GnutellaError> {
    let req = "GNUTELLA CONNECT/0.6\r\n\
User-Agent: Risuko/0.1\r\n\
X-Ultrapeer: False\r\n\
Listen-IP: 0.0.0.0:6346\r\n\
\r\n";
    sock.write_all(req.as_bytes()).await?;

    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Err(GnutellaError::Network("eof during handshake".into()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
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
    sock.write_all(confirm.as_bytes()).await?;
    Ok(())
}
