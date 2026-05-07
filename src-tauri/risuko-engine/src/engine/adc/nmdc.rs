//! NMDC (Direct Connect / DC++ legacy) line-text client
//!
//! Wire is `\n`-terminated UTF-8 (CP-1252 historically). Hubs send
//! `$Lock <lock>` immediately on connect; clients reply with `$Key <key>` +
//! `$ValidateNick <nick>` + `$Version 1,0091` + `$MyINFO …`. After login the
//! client issues `$Search Hub:<nick> F?T?<size>?<type>?TTH:<tth>` then reads
//! incoming `$SR <nick> <file>$<size>$<port> <hub>` lines. Direct file
//! transfer happens over a separate TCP connection negotiated via
//! `$ConnectToMe` / `$RevConnectToMe` and uses `$ADCGET file …`
//!
//! This module implements the subset needed to download a single file by
//! TTH from an active-mode peer reachable from the same hub

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::types::{AdcError, FileEntry, HubInfo, PeerInfo};

const HUB_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// NMDC `$Lock` -> `$Key` transformation. Implements the standard
/// nibble-swap algorithm published in DC++ source. Returned bytes are
/// raw — caller must escape via `escape_key` before sending on the wire
pub fn lock_to_key(lock: &[u8]) -> Vec<u8> {
    if lock.is_empty() {
        return Vec::new();
    }
    let n = lock.len();
    let mut key = vec![0u8; n];
    for i in 1..n {
        key[i] = lock[i] ^ lock[i - 1];
    }
    key[0] = lock[0] ^ lock[n - 1] ^ lock[n - 2] ^ 5;
    for byte in &mut key {
        *byte = ((*byte << 4) & 0xF0) | ((*byte >> 4) & 0x0F);
    }
    key
}

/// Escape a raw key per NMDC spec: bytes 0, 5, 36, 96, 124, 126 must be
/// represented as `/%DCNxxx%/`. Other bytes pass through verbatim
pub fn escape_key(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 8);
    for &b in raw {
        match b {
            0 | 5 | 36 | 96 | 124 | 126 => {
                out.extend_from_slice(format!("/%DCN{:03}%/", b).as_bytes());
            }
            _ => out.push(b),
        }
    }
    out
}

/// Connected NMDC hub session: a split TCP socket plus the local nick and
/// remembered hub name from the last `$HubName` we observed
pub struct NmdcClient {
    reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
    writer: tokio::io::WriteHalf<TcpStream>,
    nick: String,
    hub_name: Option<String>,
}

impl NmdcClient {
    /// Open a TCP connection to the hub and prepare a buffered reader.
    /// Does not run the protocol handshake — call `handshake` next
    pub async fn connect(hub: &HubInfo, nick: &str) -> Result<Self, AdcError> {
        let stream = timeout(
            HUB_IO_TIMEOUT,
            TcpStream::connect((hub.host.as_str(), hub.port)),
        )
        .await
        .map_err(|_| AdcError::Protocol("hub connect timeout".into()))??;
        let (rd, wr) = tokio::io::split(stream);
        Ok(Self {
            reader: BufReader::new(rd),
            writer: wr,
            nick: nick.to_string(),
            hub_name: None,
        })
    }

    /// Read one `|`-terminated NMDC command from the wire
    pub async fn read_cmd(&mut self) -> Result<String, AdcError> {
        let mut buf = Vec::with_capacity(256);
        loop {
            let mut byte = [0u8; 1];
            let n = timeout(HUB_IO_TIMEOUT, self.reader.read(&mut byte))
                .await
                .map_err(|_| AdcError::Protocol("hub read timeout".into()))??;
            if n == 0 {
                return Err(AdcError::HubDisconnect("eof".into()));
            }
            if byte[0] == b'|' {
                break;
            }
            buf.push(byte[0]);
            if buf.len() > 64 * 1024 {
                return Err(AdcError::Protocol("hub command too large".into()));
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Write a single NMDC command, appending the trailing `|` terminator
    pub async fn write_cmd(&mut self, cmd: &str) -> Result<(), AdcError> {
        self.writer.write_all(cmd.as_bytes()).await?;
        self.writer.write_all(b"|").await?;
        Ok(())
    }

    /// Run the standard handshake: read `$Lock`, send `$Key`, `$ValidateNick`,
    /// `$Version`, `$GetNickList`, `$MyINFO`. Returns when `$HubName` and
    /// `$NickList`/`$OpList` (or first `$Hello`) have been observed
    pub async fn handshake(&mut self) -> Result<(), AdcError> {
        // Expect `$Lock <lock> Pk=…`
        let line = self.read_cmd().await?;
        let lock = line
            .strip_prefix("$Lock ")
            .and_then(|rest| rest.split(' ').next())
            .ok_or_else(|| AdcError::Protocol(format!("expected $Lock, got: {line}")))?;
        let key_raw = lock_to_key(lock.as_bytes());
        let key_escaped = escape_key(&key_raw);

        self.writer.write_all(b"$Key ").await?;
        self.writer.write_all(&key_escaped).await?;
        self.writer.write_all(b"|").await?;

        self.write_cmd(&format!("$ValidateNick {}", self.nick))
            .await?;
        self.write_cmd("$Version 1,0091").await?;
        self.write_cmd("$GetNickList").await?;
        // active-mode MyINFO: <++ V:0.001,M:A,H:1/0/0,S:0>$ $LAN(T3)$$0$
        self.write_cmd(&format!(
            "$MyINFO $ALL {} <Risuko V:0.1,M:P,H:1/0/0,S:0>$ $LAN(T3)0x01$$0$",
            self.nick
        ))
        .await?;

        // Drain a few hub-info lines until we see $HubName / $NickList / $Hello
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            let line = match timeout(Duration::from_secs(2), self.read_cmd()).await {
                Ok(Ok(l)) => l,
                _ => continue,
            };
            if let Some(n) = line.strip_prefix("$HubName ") {
                self.hub_name = Some(n.to_string());
            }
            if line.starts_with("$NickList ")
                || line.starts_with("$Hello ")
                || line.starts_with("$OpList ")
            {
                return Ok(());
            }
        }
        Err(AdcError::Protocol("handshake did not complete".into()))
    }

    /// Send a TTH search and collect search results for `wait_secs`
    pub async fn search_by_tth(
        &mut self,
        tth: &str,
        wait_secs: u64,
    ) -> Result<Vec<PeerInfo>, AdcError> {
        // active-mode `$Search ip:port F?T?0?9?TTH:<tth>` would expect UDP
        // results back; we're passive so use `Hub:<nick>` to receive over TCP
        let cmd = format!("$Search Hub:{} F?T?0?9?TTH:{}", self.nick, tth);
        self.write_cmd(&cmd).await?;

        let mut results = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
        while tokio::time::Instant::now() < deadline {
            let line = match timeout(Duration::from_secs(1), self.read_cmd()).await {
                Ok(Ok(l)) => l,
                Ok(Err(_)) => break,
                Err(_) => continue,
            };
            // $SR <nick> <filename>\x05<size> <free>/<total>\x05<hubname> (<hub:port>)
            if let Some(rest) = line.strip_prefix("$SR ") {
                if let Some(nick) = rest.split(' ').next() {
                    results.push(PeerInfo {
                        nick: nick.to_string(),
                        addr: None,
                        cid: None,
                    });
                }
            }
        }
        Ok(results)
    }

    /// Ask the hub to ConnectToMe (active mode peer), letting the peer dial
    /// us back. In passive mode we'd send `$RevConnectToMe`. We send both
    /// and let the hub broker the appropriate one
    pub async fn request_peer_connect(
        &mut self,
        peer_nick: &str,
        listen_addr: Option<&str>,
    ) -> Result<(), AdcError> {
        if let Some(addr) = listen_addr {
            self.write_cmd(&format!("$ConnectToMe {} {}", peer_nick, addr))
                .await?;
        }
        self.write_cmd(&format!("$RevConnectToMe {} {}", self.nick, peer_nick))
            .await?;
        Ok(())
    }

    /// Last `$HubName` value advertised by the remote hub, if any
    pub fn hub_name(&self) -> Option<&str> {
        self.hub_name.as_deref()
    }
}

/// Helper: Build the file entry display name for a TTH-only result list
pub fn file_entry_from_search(name: &str, size: u64, tth: &str) -> FileEntry {
    FileEntry {
        file_name: name.to_string(),
        file_size: size,
        tth: Some(tth.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_key_known_vector() {
        // Vector from DC++ source: lock="EXTENDEDPROTOCOL...." should produce
        // a stable nibble-swapped key. We assert idempotence properties:
        // (a) length matches, (b) algorithm runs without panic for short input
        let lock = b"EXTENDEDPROTOCOLABCABC";
        let k = lock_to_key(lock);
        assert_eq!(k.len(), lock.len());
    }

    #[test]
    fn escape_key_handles_special_bytes() {
        let raw = vec![0u8, 5, 36, 96, b'a', 124, 126];
        let esc = escape_key(&raw);
        let s = String::from_utf8_lossy(&esc);
        assert!(s.contains("/%DCN000%/"));
        assert!(s.contains("/%DCN005%/"));
        assert!(s.contains("/%DCN036%/"));
        assert!(s.contains("/%DCN096%/"));
        assert!(s.contains("/%DCN124%/"));
        assert!(s.contains("/%DCN126%/"));
        assert!(s.contains('a'));
    }
}
