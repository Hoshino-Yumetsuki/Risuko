mod ftp_download;
mod sftp_download;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::speed_limiter::SpeedLimiter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpProtocol {
    Ftp,
    Ftps,
    Sftp,
}

#[derive(Debug, Clone)]
pub struct FtpUri {
    pub protocol: FtpProtocol,
    pub user: Option<String>,
    pub password: Option<String>,
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Check if a URI uses ftp://, ftps://, or sftp:// scheme
pub fn is_ftp_uri(uri: &str) -> bool {
    detect_ftp_protocol(uri).is_some()
}

pub fn detect_ftp_protocol(uri: &str) -> Option<FtpProtocol> {
    let lower = uri.trim().to_lowercase();
    if lower.starts_with("sftp://") {
        Some(FtpProtocol::Sftp)
    } else if lower.starts_with("ftps://") {
        Some(FtpProtocol::Ftps)
    } else if lower.starts_with("ftp://") {
        Some(FtpProtocol::Ftp)
    } else {
        None
    }
}

/// Parse an FTP/FTPS/SFTP URI into components.
///
/// Supports formats:
/// - `ftp://host/path`
/// - `ftp://user:pass@host:21/path`
/// - `sftp://host/path`
/// - `ftps://host/path`
pub fn parse_ftp_uri(uri: &str) -> Result<FtpUri, String> {
    let protocol = detect_ftp_protocol(uri).ok_or("Not an FTP/FTPS/SFTP URI")?;

    let default_port = match protocol {
        FtpProtocol::Ftp => 21,
        FtpProtocol::Ftps => 990,
        FtpProtocol::Sftp => 22,
    };

    // Strip scheme
    let scheme_end = uri.find("://").ok_or("Invalid URI: missing scheme")? + 3;
    let rest = &uri[scheme_end..];

    // Split authority from path
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    // Parse user:pass@host:port
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(idx) => (Some(&authority[..idx]), &authority[idx + 1..]),
        None => (None, authority),
    };

    let (user, password) = match userinfo {
        Some(info) => match info.find(':') {
            Some(idx) => (
                Some(urlencoding::decode(&info[..idx]).map_err(|e| e.to_string())?.into_owned()),
                Some(urlencoding::decode(&info[idx + 1..]).map_err(|e| e.to_string())?.into_owned()),
            ),
            None => (
                Some(urlencoding::decode(info).map_err(|e| e.to_string())?.into_owned()),
                None,
            ),
        },
        None => (None, None),
    };

    // Parse host:port (handle IPv6 [::1]:port)
    let (host, port) = if hostport.starts_with('[') {
        // IPv6
        let bracket_end = hostport.find(']').ok_or("Invalid IPv6 address")?;
        let host = &hostport[1..bracket_end];
        let port = if bracket_end + 1 < hostport.len() && hostport.as_bytes()[bracket_end + 1] == b':' {
            hostport[bracket_end + 2..]
                .parse::<u16>()
                .map_err(|e| format!("Invalid port: {e}"))?
        } else {
            default_port
        };
        (host.to_string(), port)
    } else {
        match hostport.rfind(':') {
            Some(idx) => {
                let port = hostport[idx + 1..]
                    .parse::<u16>()
                    .map_err(|e| format!("Invalid port: {e}"))?;
                (hostport[..idx].to_string(), port)
            }
            None => (hostport.to_string(), default_port),
        }
    };

    if host.is_empty() {
        return Err("Empty host in URI".to_string());
    }

    let decoded_path = urlencoding::decode(path)
        .map_err(|e| e.to_string())?
        .into_owned();

    Ok(FtpUri {
        protocol,
        user,
        password,
        host,
        port,
        path: decoded_path,
    })
}

/// Infer filename from an FTP URI path
pub fn infer_filename_from_ftp_uri(uri: &str) -> String {
    if let Ok(parsed) = parse_ftp_uri(uri) {
        let path = parsed.path.trim_end_matches('/');
        if let Some(idx) = path.rfind('/') {
            let name = &path[idx + 1..];
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "download".to_string()
}

/// Main dispatcher: calls FTP/FTPS or SFTP worker based on protocol
#[allow(clippy::too_many_arguments)]
pub async fn run_ftp_download(
    uri: &str,
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
) -> Result<PathBuf, String> {
    let parsed = parse_ftp_uri(uri)?;

    match parsed.protocol {
        FtpProtocol::Ftp | FtpProtocol::Ftps => {
            ftp_download::run_ftp_ftps_download(
                &parsed,
                dir,
                out,
                options,
                total,
                completed,
                speed,
                cancelled,
                connections,
                cancel_token,
                global_limiter,
                task_limiter,
            )
            .await
        }
        FtpProtocol::Sftp => {
            sftp_download::run_sftp_download(
                &parsed,
                dir,
                out,
                options,
                total,
                completed,
                speed,
                cancelled,
                connections,
                cancel_token,
                global_limiter,
                task_limiter,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_ftp_protocol() {
        assert_eq!(detect_ftp_protocol("ftp://host/file"), Some(FtpProtocol::Ftp));
        assert_eq!(detect_ftp_protocol("ftps://host/file"), Some(FtpProtocol::Ftps));
        assert_eq!(detect_ftp_protocol("sftp://host/file"), Some(FtpProtocol::Sftp));
        assert_eq!(detect_ftp_protocol("http://host/file"), None);
        assert_eq!(detect_ftp_protocol("FTP://HOST/file"), Some(FtpProtocol::Ftp));
    }

    #[test]
    fn test_parse_ftp_uri_simple() {
        let uri = parse_ftp_uri("ftp://example.com/pub/file.zip").unwrap();
        assert_eq!(uri.protocol, FtpProtocol::Ftp);
        assert_eq!(uri.host, "example.com");
        assert_eq!(uri.port, 21);
        assert_eq!(uri.path, "/pub/file.zip");
        assert!(uri.user.is_none());
        assert!(uri.password.is_none());
    }

    #[test]
    fn test_parse_ftp_uri_with_credentials() {
        let uri = parse_ftp_uri("ftp://user:p%40ss@host:2121/dir/file.zip").unwrap();
        assert_eq!(uri.user.as_deref(), Some("user"));
        assert_eq!(uri.password.as_deref(), Some("p@ss"));
        assert_eq!(uri.host, "host");
        assert_eq!(uri.port, 2121);
        assert_eq!(uri.path, "/dir/file.zip");
    }

    #[test]
    fn test_parse_sftp_uri() {
        let uri = parse_ftp_uri("sftp://myuser@server.com/home/file.tar.gz").unwrap();
        assert_eq!(uri.protocol, FtpProtocol::Sftp);
        assert_eq!(uri.user.as_deref(), Some("myuser"));
        assert!(uri.password.is_none());
        assert_eq!(uri.host, "server.com");
        assert_eq!(uri.port, 22);
        assert_eq!(uri.path, "/home/file.tar.gz");
    }

    #[test]
    fn test_parse_ftps_uri() {
        let uri = parse_ftp_uri("ftps://host:990/file.bin").unwrap();
        assert_eq!(uri.protocol, FtpProtocol::Ftps);
        assert_eq!(uri.port, 990);
    }

    #[test]
    fn test_infer_filename() {
        assert_eq!(infer_filename_from_ftp_uri("ftp://host/dir/file.zip"), "file.zip");
        assert_eq!(infer_filename_from_ftp_uri("sftp://host/"), "download");
        assert_eq!(infer_filename_from_ftp_uri("ftp://host"), "download");
    }

    #[test]
    fn test_is_ftp_uri() {
        assert!(is_ftp_uri("ftp://host/file"));
        assert!(is_ftp_uri("ftps://host/file"));
        assert!(is_ftp_uri("sftp://host/file"));
        assert!(!is_ftp_uri("http://host/file"));
        assert!(!is_ftp_uri("magnet:?xt=urn:btih:abc"));
    }
}
