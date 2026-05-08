//! Peer-to-peer file transfer for ADC / NMDC. The transfer protocol is the
//! same in both dialects after handshake: `$ADCGET file TTH/<base32> 0 -1` /
//! `CGET file TTH/<base32> 0 -1` -> `$ADCSND file TTH/<base32> 0 <size>`
//! followed by raw bytes
//!
//! This module owns no state — call `download_from_peer` once a TCP socket
//! has been negotiated via the hub

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::types::AdcError;

/// Open a peer-to-peer connection to a DC client identified by `nick` and
/// stream `expected_size` bytes of the file with the given TTH into
/// `out_path`. Speaks NMDC `$ADCGET file …` framing on plain TCP. Returns
/// the number of bytes actually written
pub async fn download_from_peer(
    socket: TcpStream,
    tth: &str,
    file_size: u64,
    out_path: &Path,
    completed: Arc<AtomicU64>,
    cancel_token: CancellationToken,
) -> Result<(), AdcError> {
    if cancel_token.is_cancelled() {
        return Err(AdcError::Peer("cancelled".into()));
    }

    let (rd, mut wr) = tokio::io::split(socket);
    let mut reader = BufReader::new(rd);

    // Validate TTH, must be exactly 39 chars of base32 alphabet [A-Z2-7]
    if tth.len() != 39 || !tth.bytes().all(|b| matches!(b, b'A'..=b'Z' | b'2'..=b'7')) {
        return Err(AdcError::Protocol("invalid TTH".into()));
    }

    // Issue the request. Both dialects accept `$ADCGET file TTH/...` over
    // the peer connection; the server responds with `$ADCSND file TTH/...
    // <start> <bytes>` then raw payload
    let req = format!("$ADCGET file TTH/{tth} 0 {file_size}|");
    wr.write_all(req.as_bytes()).await?;

    // Read response header up to `|` — cap at 4097 bytes before the check so
    // a peer cannot trigger unbounded allocation with an unterminated response
    let mut header = Vec::with_capacity(128);
    let mut limited_reader = (&mut reader).take(4097);
    tokio::select! {
        read_res = limited_reader.read_until(b'|', &mut header) => {
            let _ = read_res?;
        }
        _ = cancel_token.cancelled() => {
            return Err(AdcError::Peer("cancelled".into()));
        }
    }
    if header.len() > 4096 {
        return Err(AdcError::Protocol("oversized $ADCSND header".into()));
    }
    if header.last() == Some(&b'|') {
        header.pop();
    }
    let header_str = String::from_utf8_lossy(&header);
    let parts: Vec<&str> = header_str.split(' ').collect();
    let expected_tth = format!("TTH/{tth}");
    let valid = parts.len() >= 5
        && parts[0] == "$ADCSND"
        && parts[1] == "file"
        && parts[2] == expected_tth
        && parts[3].parse::<u64>().ok() == Some(0)
        && parts[4].parse::<u64>().ok() == Some(file_size);
    if !valid {
        return Err(AdcError::Protocol("malformed $ADCSND header".into()));
    }
    let advertised: u64 = file_size;

    // Stream payload to file
    if let Some(parent) = out_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut f = tokio::fs::File::create(out_path).await?;
    let mut remaining = advertised;
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let take = buf.len().min(remaining as usize);
        let n = tokio::select! {
            read_res = reader.read(&mut buf[..take]) => read_res?,
            _ = cancel_token.cancelled() => {
                return Err(AdcError::Peer("cancelled".into()));
            }
        };
        if n == 0 {
            return Err(AdcError::Peer("peer closed mid-transfer".into()));
        }
        tokio::select! {
            write_res = f.write_all(&buf[..n]) => write_res?,
            _ = cancel_token.cancelled() => {
                return Err(AdcError::Peer("cancelled".into()));
            }
        }
        remaining -= n as u64;
        completed.fetch_add(n as u64, Ordering::Relaxed);
    }
    f.flush().await?;
    Ok(())
}
