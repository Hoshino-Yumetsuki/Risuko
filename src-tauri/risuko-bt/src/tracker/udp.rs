//! UDP tracker (BEP-15)
//!
//! Two-step protocol: client sends a `Connect` request with a magic number
//! and receives a 64-bit `connection_id`; then sends an `Announce` request
//! quoting that id and receives peers + interval + seeders/leechers
//!
//! Retransmits use the BEP-15 recommendation: n = 0..8, timeout = 15 * 2^n
//! seconds. For v1 we shorten to 3 tries to fit into our async budget

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use byteorder::{BigEndian, ByteOrder};
use rand::RngExt;
use tokio::net::{lookup_host, UdpSocket};
use tokio::time::timeout;

use super::{AnnounceRequest, AnnounceResponse, TrackerError};

const PROTOCOL_ID: u64 = 0x41727101980;
const ACTION_CONNECT: u32 = 0;
const ACTION_ANNOUNCE: u32 = 1;
const ACTION_ERROR: u32 = 3;

pub async fn announce(url: &str, req: &AnnounceRequest) -> Result<AnnounceResponse, TrackerError> {
    let (host, port) = parse_udp_url(url)?;
    let target = lookup_host((host.as_str(), port))
        .await?
        .next()
        .ok_or_else(|| TrackerError::Url(format!("no DNS result for {host}")))?;

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
    BigEndian::write_u64(&mut req[0..8], PROTOCOL_ID);
    BigEndian::write_u32(&mut req[8..12], ACTION_CONNECT);
    BigEndian::write_u32(&mut req[12..16], txn);

    for attempt in 0..3u32 {
        sock.send(&req).await?;
        // Shorter than BEP-15 (15 * 2^n) to keep resolve-magnet snappy
        let wait = Duration::from_secs(5u64 << attempt);
        match timeout(wait, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= 8 => {
                let action = BigEndian::read_u32(&buf[0..4]);
                let rtxn = BigEndian::read_u32(&buf[4..8]);
                if action == ACTION_ERROR {
                    return Err(TrackerError::Rejected(read_error(&buf[8..n])));
                }
                if n < 16 || action != ACTION_CONNECT || rtxn != txn {
                    continue;
                }
                return Ok(BigEndian::read_u64(&buf[8..16]));
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
    BigEndian::write_u64(&mut body[0..8], conn_id);
    BigEndian::write_u32(&mut body[8..12], ACTION_ANNOUNCE);
    BigEndian::write_u32(&mut body[12..16], txn);
    body[16..36].copy_from_slice(req.info_hash.as_bytes());
    body[36..56].copy_from_slice(req.peer_id.as_bytes());
    BigEndian::write_u64(&mut body[56..64], req.downloaded);
    BigEndian::write_u64(&mut body[64..72], req.left);
    BigEndian::write_u64(&mut body[72..80], req.uploaded);
    BigEndian::write_u32(&mut body[80..84], event_code(req));
    BigEndian::write_u32(&mut body[84..88], 0); // IP (default)
    BigEndian::write_u32(&mut body[88..92], rand::rng().random::<u32>());
    BigEndian::write_u32(&mut body[92..96], req.num_want);
    BigEndian::write_u16(&mut body[96..98], req.port);

    let is_ipv6 = sock.peer_addr().map(|a| a.is_ipv6()).unwrap_or(false);
    let mut buf = vec![0u8; 2048];
    for attempt in 0..3u32 {
        sock.send(&body).await?;
        let wait = Duration::from_secs(5u64 << attempt);
        match timeout(wait, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= 8 => {
                let action = BigEndian::read_u32(&buf[0..4]);
                let rtxn = BigEndian::read_u32(&buf[4..8]);
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

fn parse_announce_response(buf: &[u8], is_ipv6: bool) -> AnnounceResponse {
    let interval = BigEndian::read_u32(&buf[8..12]).max(1) as u64;
    let leechers = BigEndian::read_u32(&buf[12..16]);
    let seeders = BigEndian::read_u32(&buf[16..20]);
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
    // Trim trailing path like `/announce` and any query string. UDP trackers
    // ignore both
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
    fn rejects_wrong_scheme() {
        assert!(parse_udp_url("http://x:1").is_err());
    }
}
