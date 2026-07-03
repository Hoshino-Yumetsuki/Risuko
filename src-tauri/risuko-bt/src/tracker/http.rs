//! HTTP / HTTPS tracker (BEP-3 + BEP-23 compact peer list)

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use percent_encoding::{percent_encode, NON_ALPHANUMERIC};

use super::super::bencode::decode_all;
use super::{AnnounceRequest, AnnounceResponse, TrackerError};

/// RFC 3986 unreserved + `-_.~` are safe in a URL without percent-encoding
const QUERY_SAFE: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn client() -> &'static risuko_http::Client {
    static CLIENT: OnceLock<risuko_http::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        risuko_http::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(4)
            .build()
            .expect("build risuko-http client")
    })
}

pub async fn announce(url: &str, req: &AnnounceRequest) -> Result<AnnounceResponse, TrackerError> {
    let query = build_query(req);
    // Append to existing query string if the URL already has one
    let sep = if url.contains('?') { '&' } else { '?' };
    let full = format!("{}{}{}", url, sep, query);

    let bytes = client()
        .get(&full)
        .send()
        .await
        .map_err(|e| TrackerError::Http(e.to_string()))?
        .error_for_status()
        .map_err(|e| TrackerError::Http(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| TrackerError::Http(e.to_string()))?;

    parse_response(&bytes)
}

fn build_query(req: &AnnounceRequest) -> String {
    let ih = percent_encode(req.info_hash.as_bytes(), QUERY_SAFE);
    let pid = percent_encode(req.peer_id.as_bytes(), QUERY_SAFE);
    let mut s = format!(
        "info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1&numwant={}",
        ih, pid, req.port, req.uploaded, req.downloaded, req.left, req.num_want
    );
    let ev = req.event.as_str();
    if !ev.is_empty() {
        s.push_str("&event=");
        s.push_str(ev);
    }
    s
}

fn parse_response(bytes: &[u8]) -> Result<AnnounceResponse, TrackerError> {
    let value = decode_all(bytes)?;
    value
        .as_dict()
        .ok_or_else(|| TrackerError::Rejected("response not a dict".into()))?;

    if let Some(reason) = value.get(b"failure reason").and_then(|v| v.as_str()) {
        return Err(TrackerError::Rejected(reason.to_string()));
    }

    let interval = value
        .get(b"interval")
        .and_then(|v| v.as_int())
        .unwrap_or(1800)
        .max(1) as u64;

    let seeders = value
        .get(b"complete")
        .and_then(|v| v.as_int())
        .and_then(|n| u32::try_from(n).ok());
    let leechers = value
        .get(b"incomplete")
        .and_then(|v| v.as_int())
        .and_then(|n| u32::try_from(n).ok());

    let mut peers = Vec::new();
    // Compact IPv4 (BEP-23): 6 bytes per peer
    if let Some(raw) = value.get(b"peers").and_then(|v| v.as_bytes()) {
        for chunk in raw.chunks_exact(6) {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            peers.push(SocketAddr::new(IpAddr::V4(ip), port));
        }
    } else if let Some(list) = value.get(b"peers").and_then(|v| v.as_list()) {
        // Dictionary model (BEP-3 original). Each entry is a dict with
        // `ip` and `port` keys
        for entry in list {
            if entry.as_dict().is_some() {
                let ip = entry.get(b"ip").and_then(|v| v.as_str());
                let port = entry.get(b"port").and_then(|v| v.as_int());
                if let (Some(ip), Some(port)) = (ip, port) {
                    if let Ok(ip) = ip.parse::<IpAddr>() {
                        if let Ok(port) = u16::try_from(port) {
                            peers.push(SocketAddr::new(ip, port));
                        }
                    }
                }
            }
        }
    }

    // Compact IPv6 (BEP-7): 18 bytes per peer
    if let Some(raw) = value.get(b"peers6").and_then(|v| v.as_bytes()) {
        for chunk in raw.chunks_exact(18) {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&chunk[..16]);
            let ip = Ipv6Addr::from(octets);
            let port = u16::from_be_bytes([chunk[16], chunk[17]]);
            peers.push(SocketAddr::new(IpAddr::V6(ip), port));
        }
    }

    Ok(AnnounceResponse {
        interval: Duration::from_secs(interval),
        peers,
        seeders,
        leechers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::{encode_to_vec, Value};
    use crate::core::Id20;

    fn req() -> AnnounceRequest {
        AnnounceRequest {
            info_hash: Id20([1u8; 20]),
            peer_id: Id20([2u8; 20]),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: 100,
            event: super::super::AnnounceEvent::Started,
            num_want: 50,
        }
    }

    #[test]
    fn query_includes_required_fields() {
        let q = build_query(&req());
        for k in [
            "info_hash=",
            "peer_id=",
            "port=6881",
            "compact=1",
            "event=started",
        ] {
            assert!(q.contains(k), "missing {k} in {q}");
        }
    }

    #[test]
    fn parses_compact_v4_response() {
        let peers_bin: Vec<u8> = vec![
            1, 2, 3, 4, 0x1a, 0xe1, // 1.2.3.4:6881
            5, 6, 7, 8, 0x1a, 0xe2, // 5.6.7.8:6882
        ];
        let body = encode_to_vec(&Value::Dict(vec![
            (b"interval".to_vec(), Value::Int(60)),
            (b"peers".to_vec(), Value::Bytes(peers_bin)),
        ]));
        let r = parse_response(&body).unwrap();
        assert_eq!(r.interval, Duration::from_secs(60));
        assert_eq!(r.peers.len(), 2);
        assert_eq!(r.peers[0].port(), 6881);
    }

    #[test]
    fn propagates_failure_reason() {
        let body = encode_to_vec(&Value::Dict(vec![(
            b"failure reason".to_vec(),
            Value::Bytes(b"nope".to_vec()),
        )]));
        let err = parse_response(&body).unwrap_err();
        assert!(matches!(err, TrackerError::Rejected(ref m) if m == "nope"));
    }
}
