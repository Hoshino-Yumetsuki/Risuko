//! ADC / DC URI parsing and shared types

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

/// Wire-format dialect for an ADC/DC hub: `Adc` speaks the modern binary command set, `Nmdc` the legacy line-text DC++ protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubDialect {
    Adc,
    Nmdc,
}

/// One result row from a hub search: file name, size, and optional TTH
#[derive(Debug, Clone, Default)]
pub struct FileEntry {
    pub file_name: String,
    pub file_size: u64,
    /// 39-char base32-encoded Tiger Tree Hash
    pub tth: Option<String>,
}

/// True for any DC-family scheme this engine recognises
pub fn is_adc_uri(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    lower.starts_with("adc://")
        || lower.starts_with("adcs://")
        || lower.starts_with("dchub://")
        || lower.starts_with("nmdc://")
}

/// Parse a hub-only URI of the form `<scheme>://host[:port][/]`; returns `Err` with a human-readable reason when malformed
pub fn parse_adc_hub_uri(uri: &str) -> Result<HubInfo, AdcError> {
    let trimmed = uri.trim();
    // Detect the scheme case-insensitively but keep the rest of the URI in original case (only the scheme is case-insensitive; a future path/query must not be silently lowercased)
    let scheme_end = trimmed.find("://").map(|i| i + 3).unwrap_or(0);
    let scheme_lower = trimmed[..scheme_end].to_ascii_lowercase();
    let (dialect, tls, rest) = if let Some(len) = scheme_prefix_len(&scheme_lower, "adcs://") {
        (HubDialect::Adc, true, &trimmed[len..])
    } else if let Some(len) = scheme_prefix_len(&scheme_lower, "adc://") {
        (HubDialect::Adc, false, &trimmed[len..])
    } else if let Some(len) = scheme_prefix_len(&scheme_lower, "dchub://") {
        (HubDialect::Nmdc, false, &trimmed[len..])
    } else if let Some(len) = scheme_prefix_len(&scheme_lower, "nmdc://") {
        (HubDialect::Nmdc, false, &trimmed[len..])
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

    let (host, port) = split_host_port(host_port, default_port)?;

    Ok(HubInfo {
        host,
        port,
        tls,
        dialect,
    })
}

/// Returns the byte length of `full_scheme` if `scheme` (already lowercased, including trailing `://`) starts with it
fn scheme_prefix_len(scheme: &str, full_scheme: &str) -> Option<usize> {
    scheme.starts_with(full_scheme).then_some(full_scheme.len())
}

/// Split a `host[:port]` authority into host and port, handling bracketed IPv6 literals like `[::1]:411` so the port is only taken after the closing `]`
fn split_host_port(host_port: &str, default_port: u16) -> Result<(String, u16), AdcError> {
    if let Some(rest) = host_port.strip_prefix('[') {
        // Bracketed IPv6 literal: `[addr]` or `[addr]:port`
        let close = rest
            .find(']')
            .ok_or_else(|| AdcError::InvalidUri("unterminated IPv6 host".into()))?;
        let host = rest[..close].to_string();
        if host.is_empty() {
            return Err(AdcError::InvalidUri("missing host".into()));
        }
        let after = &rest[close + 1..];
        let port = if let Some(p) = after.strip_prefix(':') {
            p.parse()
                .map_err(|e| AdcError::InvalidUri(format!("bad port: {e}")))?
        } else if after.is_empty() {
            default_port
        } else {
            return Err(AdcError::InvalidUri(format!(
                "unexpected characters after IPv6 host: {after}"
            )));
        };
        return Ok((host, port));
    }

    match host_port.rfind(':') {
        Some(idx) => {
            let host = &host_port[..idx];
            if host.is_empty() {
                return Err(AdcError::InvalidUri("missing host".into()));
            }
            let port: u16 = host_port[idx + 1..]
                .parse()
                .map_err(|e| AdcError::InvalidUri(format!("bad port: {e}")))?;
            Ok((host.to_string(), port))
        }
        None => Ok((host_port.to_string(), default_port)),
    }
}

/// Parse a direct-file URI carrying TTH+size+name in the query string; returns `None` when required parameters are missing
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
    percent_encoding::percent_decode_str(&s.replace('+', " "))
        .decode_utf8_lossy()
        .to_string()
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
    fn parses_ipv6_hub_uri() {
        let h = parse_adc_hub_uri("adc://[::1]:411").unwrap();
        assert_eq!(h.host, "::1");
        assert_eq!(h.port, 411);

        let h = parse_adc_hub_uri("adc://[2001:db8::1]").unwrap();
        assert_eq!(h.host, "2001:db8::1");
        assert_eq!(h.port, 411);
    }

    #[test]
    fn rejects_missing_hub_host() {
        for uri in ["adc://", "adc://:411", "adc://[]:411"] {
            assert!(matches!(
                parse_adc_hub_uri(uri),
                Err(AdcError::InvalidUri(message)) if message == "missing host"
            ));
        }
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
