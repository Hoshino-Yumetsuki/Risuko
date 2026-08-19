//! BitTorrent trackers: types and orchestration used by `torrent::` to discover peers via HTTP (BEP-3 / BEP-23 compact) and UDP (BEP-15) trackers

pub mod http;
pub mod udp;

use std::net::SocketAddr;
use std::time::Duration;

use super::core::Id20;

/// Common "announce" parameters sent to every tracker
#[derive(Debug, Clone)]
pub struct AnnounceRequest {
    pub info_hash: Id20,
    pub peer_id: Id20,
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub event: AnnounceEvent,
    pub num_want: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceEvent {
    None,
    Started,
    Stopped,
    Completed,
}

impl AnnounceEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            AnnounceEvent::None => "",
            AnnounceEvent::Started => "started",
            AnnounceEvent::Stopped => "stopped",
            AnnounceEvent::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnnounceResponse {
    pub interval: Duration,
    pub peers: Vec<SocketAddr>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("bencode: {0}")]
    Bencode(#[from] super::bencode::Error),
    #[error("tracker rejected request: {0}")]
    Rejected(String),
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("timeout")]
    Timeout,
    #[error("url: {0}")]
    Url(String),
}

/// Dispatch a single announce to a tracker URL; returns the parsed response
pub async fn announce(
    url: &str,
    req: &AnnounceRequest,
    timeout: Duration,
) -> Result<AnnounceResponse, TrackerError> {
    announce_with_proxy(url, req, timeout, None).await
}

pub async fn announce_with_proxy(
    url: &str,
    req: &AnnounceRequest,
    timeout: Duration,
    proxy: Option<&risuko_http::ProxyConnector>,
) -> Result<AnnounceResponse, TrackerError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        tokio::time::timeout(timeout, http::announce_with_proxy(url, req, proxy))
            .await
            .map_err(|_| TrackerError::Timeout)?
    } else if url.starts_with("udp://") {
        tokio::time::timeout(timeout, udp::announce_with_proxy(url, req, proxy))
            .await
            .map_err(|_| TrackerError::Timeout)?
    } else {
        Err(TrackerError::UnsupportedScheme(url.to_string()))
    }
}
