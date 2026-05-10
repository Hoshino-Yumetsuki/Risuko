//! ADC binary-tagged frame protocol (DC++ ADC dialect)
//!
//! Wire frames are space-separated UTF-8 with the type prefix in the first
//! token: `CSUP ADBASE ADTIGR …`, `BINF AAAA NIRisuko VE0.1`, `BGET file
//! TTH/<base32> 0 -1`, `BRES file TTH/<base32> SI<size> …`. Each frame is
//! terminated by `\n`
//!
//! This build supports plain `adc://` hubs only; `connect()` rejects
//! `adcs://` (TLS)

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::nmdc::HUB_IO_TIMEOUT;
use super::types::{AdcError, HubInfo};
use tiger::{Digest, Tiger};

const MAX_HANDSHAKE_FRAMES: usize = 3;

/// Connected ADC hub session: split read/write halves of the TCP stream
/// plus the Session ID assigned by the hub during `ISID`
pub struct AdcClient {
    reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
    writer: tokio::io::WriteHalf<TcpStream>,
    sid: Option<String>,
}

impl AdcClient {
    /// Open a plain TCP connection to the hub. `adcs://` (TLS) is rejected
    /// in this build until the rustls dependency is wired in
    pub async fn connect(hub: &HubInfo) -> Result<Self, AdcError> {
        // TLS via `adcs://` is not yet wired; non-TLS hubs work directly
        // The plan calls out using rustls from the workspace; left as a
        // follow-up since `tokio-rustls` is not yet a direct dep of
        // `risuko-engine`. Treat `adcs://` as a hard error for now
        if hub.tls {
            return Err(AdcError::Protocol(
                "ADCS (TLS) not yet supported in this build".into(),
            ));
        }
        let stream = tokio::time::timeout(
            HUB_IO_TIMEOUT,
            TcpStream::connect((hub.host.as_str(), hub.port)),
        )
        .await
        .map_err(|_| AdcError::Protocol("hub connect timeout".into()))??;
        let (rd, wr) = tokio::io::split(stream);
        Ok(Self {
            reader: BufReader::new(rd),
            writer: wr,
            sid: None,
        })
    }

    /// Send a single ADC frame, appending the trailing newline terminator
    pub async fn write_frame(&mut self, frame: &str) -> Result<(), AdcError> {
        self.writer.write_all(frame.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        Ok(())
    }

    /// Read one newline-terminated ADC frame, stripping any trailing CR/LF
    pub async fn read_frame(&mut self) -> Result<String, AdcError> {
        let mut buf = String::new();
        let n = tokio::time::timeout(HUB_IO_TIMEOUT, self.reader.read_line(&mut buf))
            .await
            .map_err(|_| AdcError::Protocol("read frame timeout".into()))??;
        if n == 0 {
            return Err(AdcError::HubDisconnect("eof".into()));
        }
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        Ok(buf)
    }

    /// Run the ADC handshake: HSUP -> SID -> INF
    pub async fn handshake(&mut self, nick: &str) -> Result<(), AdcError> {
        if nick
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\0'))
        {
            return Err(AdcError::Protocol("invalid nick".into()));
        }
        self.write_frame("HSUP ADBASE ADTIGR").await?;
        for _ in 0..MAX_HANDSHAKE_FRAMES {
            let frame = self.read_frame().await?;
            if let Some(rest) = frame.strip_prefix("ISID ") {
                let sid = rest.split_whitespace().next().unwrap_or("AAAA").to_string();
                self.sid = Some(sid.clone());
                // Send our INF: PD = pid hash, ID = cid (base32 of 24 bytes)
                let mut pid_raw = [0u8; 24];
                let mut rng = rand::rng();
                use rand::RngExt;
                rng.fill(&mut pid_raw);
                let pid = base32_encode(&pid_raw);
                let cid = base32_encode(Tiger::digest(pid_raw).as_ref());
                let inf = format!(
                    "BINF {sid} ID{cid} PD{pid} NI{nick} VERisuko/0.1 SS0 SF0 EM- HN0 HR0 HO0 SUADBASE,ADTIGR"
                );
                self.write_frame(&inf).await?;
                return Ok(());
            }
            if frame.starts_with("ISTA ") {
                return Err(AdcError::Protocol(format!("hub status: {frame}")));
            }
        }
        Err(AdcError::Protocol(format!(
            "handshake exceeded {MAX_HANDSHAKE_FRAMES} frames"
        )))
    }

    /// Session ID assigned to us by the hub via `ISID`, if handshake done
    pub fn sid(&self) -> Option<&str> {
        self.sid.as_deref()
    }
}

/// RFC 4648 upper-case base32 encoding used for ADC PID/CID values
fn base32_encode(bytes: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    for &b in bytes {
        bits = (bits << 8) | b as u32;
        nbits += 8;
        while nbits >= 5 {
            nbits -= 5;
            let idx = ((bits >> nbits) & 0x1f) as usize;
            out.push(alphabet[idx] as char);
        }
    }
    if nbits > 0 {
        let idx = ((bits << (5 - nbits)) & 0x1f) as usize;
        out.push(alphabet[idx] as char);
    }
    out
}
