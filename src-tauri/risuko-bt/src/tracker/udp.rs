//! UDP tracker (BEP-15): client sends `Connect` (magic) to get a 64-bit `connection_id`, then `Announce` quoting it to get peers + interval + seeders/leechers; retransmits shortened to 3 tries to fit our async budget

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use rand::RngExt;
use tokio::net::{lookup_host, UdpSocket};
use tokio::task::JoinSet;
use tokio::time::timeout;

use super::{AnnounceRequest, AnnounceResponse, TrackerError};

/// Big-endian read/write helpers over byte slices, replacing `byteorder`; each maps 1:1 to `std` be-bytes and slices are sized exactly by the caller so `try_into` never fails
mod be {
    pub fn read_u32(b: &[u8]) -> u32 {
        u32::from_be_bytes(b[..4].try_into().unwrap())
    }
    pub fn read_u64(b: &[u8]) -> u64 {
        u64::from_be_bytes(b[..8].try_into().unwrap())
    }
    pub fn write_u16(b: &mut [u8], v: u16) {
        b[..2].copy_from_slice(&v.to_be_bytes());
    }
    pub fn write_u32(b: &mut [u8], v: u32) {
        b[..4].copy_from_slice(&v.to_be_bytes());
    }
    pub fn write_u64(b: &mut [u8], v: u64) {
        b[..8].copy_from_slice(&v.to_be_bytes());
    }
}

const PROTOCOL_ID: u64 = 0x41727101980;
const ACTION_CONNECT: u32 = 0;
const ACTION_ANNOUNCE: u32 = 1;
const ACTION_ERROR: u32 = 3;

pub async fn announce(url: &str, req: &AnnounceRequest) -> Result<AnnounceResponse, TrackerError> {
    let (host, port) = parse_udp_url(url)?;
    let targets = dedupe_endpoints(lookup_host((host.as_str(), port)).await?);
    if targets.is_empty() {
        return Err(TrackerError::Url(format!("no DNS result for {host}")));
    }

    let mut attempts = JoinSet::new();
    for target in targets {
        let req = req.clone();
        attempts.spawn(async move { announce_endpoint(target, &req).await });
    }

    let mut last_error = None;
    while let Some(result) = attempts.join_next().await {
        match result {
            Ok(Ok(response)) => {
                attempts.abort_all();
                return Ok(response);
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(error) => {
                last_error = Some(TrackerError::Io(std::io::Error::other(format!(
                    "UDP tracker endpoint task failed: {error}"
                ))));
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| TrackerError::Url(format!("no usable DNS endpoint for {host}"))))
}

fn dedupe_endpoints(endpoints: impl IntoIterator<Item = SocketAddr>) -> Vec<SocketAddr> {
    let mut seen = HashSet::new();
    endpoints
        .into_iter()
        .filter(|endpoint| seen.insert(*endpoint))
        .collect()
}

async fn announce_endpoint(
    target: SocketAddr,
    req: &AnnounceRequest,
) -> Result<AnnounceResponse, TrackerError> {
    let sock = UdpSocket::bind(if target.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })
    .await?;
    sock.connect(target).await?;

    let conn_id = connect(&sock).await?;
    announce_inner(&sock, conn_id, req).await
}

async fn connect(sock: &UdpSocket) -> Result<u64, TrackerError> {
    let mut buf = [0u8; 16];
    let txn = rand::rng().random::<u32>();
    let mut req = [0u8; 16];
    be::write_u64(&mut req[0..8], PROTOCOL_ID);
    be::write_u32(&mut req[8..12], ACTION_CONNECT);
    be::write_u32(&mut req[12..16], txn);

    for attempt in 0..3u32 {
        sock.send(&req).await?;
        // Shorter than BEP-15 (15 * 2^n) to keep resolve-magnet snappy
        let wait = Duration::from_secs(5u64 << attempt);
        match timeout(wait, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= 8 => {
                let action = be::read_u32(&buf[0..4]);
                let rtxn = be::read_u32(&buf[4..8]);
                if action == ACTION_ERROR {
                    return Err(TrackerError::Rejected(read_error(&buf[8..n])));
                }
                if n < 16 || action != ACTION_CONNECT || rtxn != txn {
                    continue;
                }
                return Ok(be::read_u64(&buf[8..16]));
            }
            _ => continue,
        }
    }
    Err(TrackerError::Timeout)
}

async fn announce_inner(
    sock: &UdpSocket,
    conn_id: u64,
    req: &AnnounceRequest,
) -> Result<AnnounceResponse, TrackerError> {
    let txn = rand::rng().random::<u32>();
    let mut body = [0u8; 98];
    be::write_u64(&mut body[0..8], conn_id);
    be::write_u32(&mut body[8..12], ACTION_ANNOUNCE);
    be::write_u32(&mut body[12..16], txn);
    body[16..36].copy_from_slice(req.info_hash.as_bytes());
    body[36..56].copy_from_slice(req.peer_id.as_bytes());
    be::write_u64(&mut body[56..64], req.downloaded);
    be::write_u64(&mut body[64..72], req.left);
    be::write_u64(&mut body[72..80], req.uploaded);
    be::write_u32(&mut body[80..84], event_code(req));
    be::write_u32(&mut body[84..88], 0); // IP (default)
    be::write_u32(&mut body[88..92], rand::rng().random::<u32>());
    be::write_u32(&mut body[92..96], req.num_want);
    be::write_u16(&mut body[96..98], req.port);

    let is_ipv6 = sock.peer_addr().map(|a| a.is_ipv6()).unwrap_or(false);
    let mut buf = vec![0u8; announce_response_buffer_len(is_ipv6, req.num_want)];
    for attempt in 0..3u32 {
        sock.send(&body).await?;
        let wait = Duration::from_secs(5u64 << attempt);
        match timeout(wait, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= 8 => {
                let action = be::read_u32(&buf[0..4]);
                let rtxn = be::read_u32(&buf[4..8]);
                if action == ACTION_ERROR {
                    return Err(TrackerError::Rejected(read_error(&buf[8..n])));
                }
                if n < 20 || action != ACTION_ANNOUNCE || rtxn != txn {
                    continue;
                }
                return Ok(parse_announce_response(&buf[..n], is_ipv6));
            }
            _ => continue,
        }
    }
    Err(TrackerError::Timeout)
}

fn announce_response_buffer_len(is_ipv6: bool, num_want: u32) -> usize {
    const HEADER: usize = 20;
    const MIN_PACKET: usize = 2048;
    const MAX_UDP_PACKET: usize = 65_536;
    let stride: usize = if is_ipv6 { 18 } else { 6 };
    HEADER
        .saturating_add(stride.saturating_mul(num_want as usize))
        .clamp(MIN_PACKET, MAX_UDP_PACKET)
}

fn parse_announce_response(buf: &[u8], is_ipv6: bool) -> AnnounceResponse {
    let interval = be::read_u32(&buf[8..12]).max(1) as u64;
    let leechers = be::read_u32(&buf[12..16]);
    let seeders = be::read_u32(&buf[16..20]);
    let mut peers = Vec::new();
    if is_ipv6 {
        // BEP-15 IPv6 extension: 18-byte compact peer entries (16 addr + 2 port)
        for chunk in buf[20..].chunks_exact(18) {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&chunk[0..16]);
            let ip = std::net::Ipv6Addr::from(octets);
            let port = u16::from_be_bytes([chunk[16], chunk[17]]);
            peers.push(SocketAddr::new(IpAddr::V6(ip), port));
        }
    } else {
        for chunk in buf[20..].chunks_exact(6) {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            peers.push(SocketAddr::new(IpAddr::V4(ip), port));
        }
    }
    AnnounceResponse {
        interval: Duration::from_secs(interval),
        peers,
        seeders: Some(seeders),
        leechers: Some(leechers),
    }
}

fn event_code(req: &AnnounceRequest) -> u32 {
    match req.event {
        super::AnnounceEvent::None => 0,
        super::AnnounceEvent::Completed => 1,
        super::AnnounceEvent::Started => 2,
        super::AnnounceEvent::Stopped => 3,
    }
}

fn read_error(tail: &[u8]) -> String {
    String::from_utf8_lossy(tail).to_string()
}

fn parse_udp_url(url: &str) -> Result<(String, u16), TrackerError> {
    let rest = url
        .strip_prefix("udp://")
        .ok_or_else(|| TrackerError::UnsupportedScheme(url.to_string()))?;
    // Trim trailing path like `/announce` and any query string; UDP trackers ignore both
    let rest = rest.split('/').next().unwrap_or(rest);
    let rest = rest.split('?').next().unwrap_or(rest);
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (
            h.trim_start_matches('[').trim_end_matches(']').to_string(),
            p.parse::<u16>()
                .map_err(|_| TrackerError::Url(format!("bad port in {url}")))?,
        ),
        None => return Err(TrackerError::Url(format!("missing port in {url}"))),
    };
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_udp_url() {
        assert_eq!(
            parse_udp_url("udp://tracker.example.com:1337/announce").unwrap(),
            ("tracker.example.com".to_string(), 1337)
        );
        assert_eq!(
            parse_udp_url("udp://[::1]:2710").unwrap(),
            ("::1".to_string(), 2710)
        );
    }

    #[test]
    fn dns_endpoints_are_deduplicated_without_reordering() {
        let v4: SocketAddr = "192.0.2.10:80".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::10]:80".parse().unwrap();
        let other: SocketAddr = "192.0.2.11:80".parse().unwrap();
        assert_eq!(
            dedupe_endpoints([v4, v6, v4, other, v6]),
            vec![v4, v6, other]
        );
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(parse_udp_url("http://x:1").is_err());
    }

    #[test]
    fn announce_response_buffer_fits_requested_ipv6_peers() {
        assert_eq!(announce_response_buffer_len(false, 200), 2048);
        assert_eq!(announce_response_buffer_len(true, 200), 3620);
    }
}
