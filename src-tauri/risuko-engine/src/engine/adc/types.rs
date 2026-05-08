//! ADC / DC URI parsing and shared types

use std::net::SocketAddr;

/// Errors emitted by the ADC / NMDC pipeline
#[derive(Debug, thiserror::Error)]
pub enum AdcError {
    #[error("invalid URI: {0}")]
    InvalidUri(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("hub disconnect: {0}")]
    HubDisconnect(String),
    #[error("peer error: {0}")]
    Peer(String),
    #[error("no source has the requested file")]
    NoSource,
}

/// Parsed connection target for a hub
#[derive(Debug, Clone)]
pub struct HubInfo {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// Wire dialect: "adc" or "nmdc"
    pub dialect: HubDialect,
}

/// Wire-format dialect for an ADC/DC hub. `Adc` speaks the modern binary
/// command set; `Nmdc` speaks the legacy line-text DC++ protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubDialect {
    Adc,
    Nmdc,
}

/// Identity and reachability of a peer participating in the same hub
#[derive(Debug, Clone, Default)]
pub struct PeerInfo {
    pub nick: String,
    pub addr: Option<SocketAddr>,
    /// CID for ADC, hashed for NMDC active mode handshake
    pub cid: Option<String>,
}

/// One result row from a hub search: file name, size, and optional TTH
#[derive(Debug, Clone, Default)]
pub struct FileEntry {
    pub file_name: String,
    pub file_size: u64,
    /// 39-char base32-encoded Tiger Tree Hash
    pub tth: Option<String>,
}

/// Parsed direct-file URI (e.g. `dchub://host/?TTH=…&xl=…&dn=…`)
#[derive(Debug, Clone)]
pub struct DchubFileLink {
    pub hub: HubInfo,
    pub tth: String,
    pub file_size: u64,
    pub file_name: String,
}

/// True for any DC-family scheme this engine recognises
pub fn is_adc_uri(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    lower.starts_with("adc://")
        || lower.starts_with("adcs://")
        || lower.starts_with("dchub://")
        || lower.starts_with("nmdc://")
}

/// Parse a hub-only URI of the form `<scheme>://host[:port][/]`
/// Returns `Err` with a human-readable reason when the URI is malformed
pub fn parse_adc_hub_uri(uri: &str) -> Result<HubInfo, AdcError> {
    let lower = uri.trim().to_ascii_lowercase();
    let (dialect, tls, rest) = if let Some(rest) = lower.strip_prefix("adcs://") {
        (HubDialect::Adc, true, rest)
    } else if let Some(rest) = lower.strip_prefix("adc://") {
        (HubDialect::Adc, false, rest)
    } else if let Some(rest) = lower.strip_prefix("dchub://") {
        (HubDialect::Nmdc, false, rest)
    } else if let Some(rest) = lower.strip_prefix("nmdc://") {
        (HubDialect::Nmdc, false, rest)
    } else {
        return Err(AdcError::InvalidUri(format!("unknown scheme: {uri}")));
    };

    // Default ports per protocol
    let default_port = match dialect {
        HubDialect::Adc => {
            if tls {
                412
            } else {
                411
            }
        }
        HubDialect::Nmdc => 411,
    };

    // Strip path/query
    let host_port = rest.split(['/', '?']).next().unwrap_or("");
    if host_port.is_empty() {
        return Err(AdcError::InvalidUri("missing host".into()));
    }

    let (host, port) = if let Some(idx) = host_port.rfind(':') {
        let port: u16 = host_port[idx + 1..]
            .parse()
            .map_err(|e| AdcError::InvalidUri(format!("bad port: {e}")))?;
        (host_port[..idx].to_string(), port)
    } else {
        (host_port.to_string(), default_port)
    };

    Ok(HubInfo {
        host,
        port,
        tls,
        dialect,
    })
}

/// Parse a direct-file URI carrying TTH+size+name in the query string
/// Returns `None` when the URI lacks the required parameters
pub fn parse_dchub_file_uri(uri: &str) -> Option<FileEntry> {
    let trimmed = uri.trim();
    let qpos = trimmed.find('?')?;
    let query = &trimmed[qpos + 1..];
    let mut tth: Option<String> = None;
    let mut size: u64 = 0;
    let mut name: String = String::new();
    for kv in query.split('&') {
        let mut it = kv.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        match k.to_ascii_lowercase().as_str() {
            "tth" | "kt" => {
                tth = Some(v.to_string());
            }
            "xl" | "size" => {
                size = v.parse().unwrap_or(0);
            }
            "dn" | "name" => {
                name = urlencoding_decode(v);
            }
            _ => {}
        }
    }
    if name.is_empty() && tth.is_none() {
        return None;
    }
    Some(FileEntry {
        file_name: name,
        file_size: size,
        tth,
    })
}

/// Minimal URL-encoded string decoder (handles %XX hex escapes and `+` -> space)
fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        if b == b'+' {
            out.push(b' ');
        } else {
            out.push(b);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_adc_schemes() {
        assert!(is_adc_uri("adc://hub:411"));
        assert!(is_adc_uri("ADCS://hub:412"));
        assert!(is_adc_uri("dchub://hub.example.com"));
        assert!(is_adc_uri("nmdc://hub.example.com"));
        assert!(!is_adc_uri("magnet:?xt=…"));
    }

    #[test]
    fn parses_hub_uri_with_default_ports() {
        let h = parse_adc_hub_uri("adc://hub.example.com").unwrap();
        assert_eq!(h.host, "hub.example.com");
        assert_eq!(h.port, 411);
        assert!(!h.tls);
        assert_eq!(h.dialect, HubDialect::Adc);

        let h = parse_adc_hub_uri("adcs://hub:9999").unwrap();
        assert!(h.tls);
        assert_eq!(h.port, 9999);
    }

    #[test]
    fn parses_dchub_file_uri() {
        let link = parse_dchub_file_uri(
            "dchub://hub.example.com:411/?TTH=ABCDEF1234567890ABCDEF1234567890ABCDEF12&xl=1024&dn=my+file.bin",
        )
        .unwrap();
        assert_eq!(link.file_name, "my file.bin");
        assert_eq!(link.file_size, 1024);
        assert_eq!(
            link.tth.as_deref(),
            Some("ABCDEF1234567890ABCDEF1234567890ABCDEF12")
        );
    }
}
