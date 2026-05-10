//! URI parsing for Gnutella content-direct URLs

/// Errors emitted by the Gnutella download pipeline
#[derive(Debug, thiserror::Error)]
pub enum GnutellaError {
    #[error("invalid URI: {0}")]
    InvalidUri(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("network: {0}")]
    Network(String),
    #[error("no source has the requested file")]
    NoSource,
}

/// Parsed `gnutella://` / `gnet://` content URI carrying SHA-1 / bitprint
/// URN, optional display name, file size hint and the `uri-res/N2R` path
#[derive(Debug, Clone, Default)]
pub struct GnutellaLink {
    pub host: String,
    pub port: u16,
    pub urn: Option<String>,
    pub file_name: String,
    pub file_size: u64,
    pub n2r_path: String,
}

/// True when the input begins with `gnutella://` or `gnet://`
/// (case-insensitive)
pub fn is_gnutella_uri(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    lower.starts_with("gnutella://") || lower.starts_with("gnet://")
}

/// Parse a content-direct Gnutella URI of the form
/// `gnutella://host[:port]/uri-res/N2R?urn:sha1:<base32>[&dn=&xl=]`.
/// Returns `None` for malformed input or non-Gnutella schemes
pub fn parse_gnutella_uri(uri: &str) -> Option<GnutellaLink> {
    let s = uri.trim();
    let lower = s.to_ascii_lowercase();
    let prefix_len = if lower.starts_with("gnutella://") {
        "gnutella://".len()
    } else if lower.starts_with("gnet://") {
        "gnet://".len()
    } else {
        return None;
    };
    let rest = &s[prefix_len..];
    // Split host:port and the rest
    let (host_port, path_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = if let Some(idx) = host_port.find(':') {
        let p: u16 = host_port[idx + 1..].parse().ok()?;
        (host_port[..idx].to_string(), p)
    } else {
        (host_port.to_string(), 6346)
    };

    let (path, query) = match path_query.find('?') {
        Some(idx) => (&path_query[..idx], &path_query[idx + 1..]),
        None => (path_query, ""),
    };

    let mut file_name = String::new();
    let mut file_size: u64 = 0;
    let mut urn: Option<String> = None;
    if !query.is_empty() {
        for part in query.split('&') {
            if let Some(rest) = part.strip_prefix("urn=") {
                urn = Some(rest.to_string());
            } else if let Some(rest) = part.strip_prefix("dn=") {
                file_name = url_decode(rest);
            } else if let Some(rest) = part.strip_prefix("xl=") {
                file_size = rest.parse().unwrap_or(0);
            } else if part.starts_with("urn:sha1:") || part.starts_with("urn:bitprint:") {
                urn = Some(part.to_string());
            }
        }
    }
    if file_name.is_empty() {
        if let Some(name) = path.rsplit('/').next() {
            file_name = url_decode(name);
        }
    }
    Some(GnutellaLink {
        host,
        port,
        urn,
        file_name,
        file_size,
        n2r_path: path.to_string(),
    })
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if b == b'+' { b' ' } else { b });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects() {
        assert!(is_gnutella_uri(
            "gnutella://host:6346/uri-res/N2R?urn:sha1:ABC"
        ));
        assert!(is_gnutella_uri("gnet://x"));
        assert!(!is_gnutella_uri("ed2k://"));
    }
    #[test]
    fn parses_n2r() {
        let l = parse_gnutella_uri(
            "gnutella://peer.example.com:6346/uri-res/N2R?urn:sha1:PLSTQHKO5F2F5OJG6DNCKEXNV6YLQ47A",
        )
        .unwrap();
        assert_eq!(l.host, "peer.example.com");
        assert_eq!(l.port, 6346);
        assert_eq!(l.n2r_path, "/uri-res/N2R");
        assert!(l.urn.unwrap().starts_with("urn:sha1:"));
    }
}
