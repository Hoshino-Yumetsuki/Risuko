//! DoH resolver (RFC 8484): POSTs binary A+AAAA queries in parallel; inner client uses a pinned BootstrapResolver so endpoint lookup can't loop back into DoH; caches answers by host (clamped TTL + short negative cache); returns every SocketAddr on port 0 per the Resolve contract

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::client::{Client, ClientBuilder};
use crate::error::{Error, Result};
use crate::resolver::{Addrs, GaiResolver, Resolve, Resolving};

/// Floor on cached TTL, so a 0s-TTL host isn't re-queried constantly
const MIN_TTL: Duration = Duration::from_secs(30);
/// Cap on cached TTL, so a stale answer can't pin a dead IP for hours
const MAX_TTL: Duration = Duration::from_secs(3600);
/// How long we remember a failure before retrying the endpoint
const NEGATIVE_TTL: Duration = Duration::from_secs(5);
/// Timeout for one HTTP round-trip to the endpoint
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for a [`DohResolver`]
#[derive(Clone, Debug)]
pub struct DohConfig {
    /// DoH endpoint, e.g. `https://cloudflare-dns.com/dns-query`
    pub url: String,
    /// IPs to reach the endpoint host without the system resolver; empty means resolve it with system DNS
    pub bootstrap: Vec<IpAddr>,
    /// Fall back to the system resolver on DoH failure, so a bad endpoint doesn't take every download down
    pub fallback: bool,
}

struct CacheEntry {
    /// Resolved IPs; empty means this is a cached failure
    addrs: Vec<IpAddr>,
    expires: Instant,
}

/// DoH resolver. Cheap to clone, everything sits behind an `Arc`
#[derive(Clone)]
pub struct DohResolver {
    inner: Arc<Inner>,
}

struct Inner {
    url: String,
    fallback: bool,
    client: Client,
    gai: GaiResolver,
    cache: RwLock<HashMap<String, CacheEntry>>,
    /// Feeds the DNS message ID; cosmetic over one TLS connection, but varied per query anyway
    next_id: AtomicU16,
}

impl DohResolver {
    /// Build a resolver from `cfg`; only fails if the endpoint URL or inner bootstrap client don't check out
    pub fn new(cfg: DohConfig) -> Result<Self> {
        let url = url::Url::parse(&cfg.url).map_err(|e| Error::Url(e.to_string()))?;
        if url.scheme() != "https" {
            return Err(Error::Url(format!(
                "DoH endpoint must be https://, got {}",
                url.scheme()
            )));
        }
        let endpoint_host = url
            .host_str()
            .ok_or_else(|| Error::Url("DoH endpoint missing host".into()))?
            .to_string();

        // Pin an explicit bootstrap resolver on the inner client so resolving the endpoint's own hostname can't loop back into DoH
        let bootstrap = Arc::new(BootstrapResolver {
            host: endpoint_host,
            ips: cfg.bootstrap.clone(),
            gai: GaiResolver,
        });
        let client = ClientBuilder::new()
            .resolver_arc(bootstrap)
            .connect_timeout(QUERY_TIMEOUT)
            .timeout(QUERY_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(2)
            .build()?;

        Ok(Self {
            inner: Arc::new(Inner {
                url: cfg.url,
                fallback: cfg.fallback,
                client,
                gai: GaiResolver,
                cache: RwLock::new(HashMap::new()),
                next_id: AtomicU16::new(1),
            }),
        })
    }

    /// Look up `host` in the cache. The returned IPs may be empty, which means we've got a cached failure. `None` means nothing fresh is cached. Expired entries are removed when discovered
    fn cache_get(&self, host: &str) -> Option<Vec<IpAddr>> {
        let cache = self.inner.cache.read().ok()?;
        let entry = cache.get(host)?;
        if entry.expires > Instant::now() {
            return Some(entry.addrs.clone());
        }
        // Expired - drop the read lock and acquire write lock to remove it
        drop(cache);
        if let Ok(mut cache) = self.inner.cache.write() {
            cache.remove(host);
        }
        None
    }

    fn cache_put(&self, host: &str, addrs: Vec<IpAddr>, ttl: Duration) {
        if let Ok(mut cache) = self.inner.cache.write() {
            // Opportunistic prune: remove expired entries to prevent unbounded growth
            let now = Instant::now();
            cache.retain(|_, entry| entry.expires > now);

            cache.insert(
                host.to_string(),
                CacheEntry {
                    addrs,
                    expires: Instant::now() + ttl,
                },
            );
        }
    }

    /// Fire off one DoH query for `host`/`qtype` and hand back the IPs plus the smallest TTL we saw
    async fn query_one(&self, host: &str, qtype: u16) -> Result<(Vec<IpAddr>, Duration)> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = encode_query(id, host, qtype)?;
        let resp = self
            .inner
            .client
            .post(self.inner.url.as_str())
            .header("content-type", "application/dns-message")
            .header("accept", "application/dns-message")
            .body(msg)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Status(status));
        }
        let body = resp.bytes().await?;
        parse_response(&body, qtype)
    }

    /// Resolve `host` over DoH, firing A and AAAA at the same time and merging
    async fn resolve_doh(&self, host: &str) -> Result<(Vec<IpAddr>, Duration)> {
        let (v4, v6) = tokio::join!(
            self.query_one(host, TYPE_A),
            self.query_one(host, TYPE_AAAA)
        );

        let mut addrs = Vec::new();
        let mut ttl = MAX_TTL;
        let mut last_err: Option<Error> = None;

        for res in [v4, v6] {
            match res {
                Ok((ips, t)) => {
                    if !ips.is_empty() {
                        ttl = ttl.min(t);
                    }
                    addrs.extend(ips);
                }
                Err(e) => last_err = Some(e),
            }
        }

        if addrs.is_empty() {
            // Both families came back empty or errored. If at least one query errored, surface that; an empty-but-successful answer (NXDOMAIN/SERVFAIL with no records) is a definitive DNS miss and should not fall back to system DNS
            return Err(last_err.unwrap_or(Error::NoRecords));
        }

        Ok((addrs, ttl.clamp(MIN_TTL, MAX_TTL)))
    }
}

impl Resolve for DohResolver {
    fn resolve(&self, host: &str) -> Resolving {
        let this = self.clone();
        let host = host.to_string();
        Box::pin(async move {
            // Fast path: IP-literal hosts don't need DNS resolution
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Ok(ips_to_addrs(vec![ip]));
            }

            // Fast path: hand back a fresh cached answer, hit or miss
            if let Some(cached) = this.cache_get(&host) {
                if cached.is_empty() {
                    // Cached failure. Fall back to system DNS if that's on
                    if this.inner.fallback {
                        return this.inner.gai.resolve(&host).await;
                    }
                    return Err(Error::NoRecords);
                }
                return Ok(ips_to_addrs(cached));
            }

            match this.resolve_doh(&host).await {
                Ok((addrs, ttl)) => {
                    this.cache_put(&host, addrs.clone(), ttl);
                    Ok(ips_to_addrs(addrs))
                }
                Err(e) => {
                    // Remember the failure for a few seconds so a flaky endpoint doesn't get re-queried on every connection a download opens
                    this.cache_put(&host, Vec::new(), NEGATIVE_TTL);
                    if this.inner.fallback {
                        tracing::warn!("DoH resolve failed for {host}: {e}; using system DNS");
                        this.inner.gai.resolve(&host).await
                    } else {
                        Err(e)
                    }
                }
            }
        })
    }
}

/// Box the IPs into the trait's iterator type, each on port 0 as the contract requires
fn ips_to_addrs(ips: Vec<IpAddr>) -> Addrs {
    let v: Vec<SocketAddr> = ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect();
    Box::new(v.into_iter()) as Addrs
}

/// Resolver wired only into the endpoint's own inner client. Returns the configured bootstrap IPs for the endpoint host (so the lookup doesn't leak to system DNS) and passes any other host through to the system resolver
struct BootstrapResolver {
    host: String,
    ips: Vec<IpAddr>,
    gai: GaiResolver,
}

impl Resolve for BootstrapResolver {
    fn resolve(&self, host: &str) -> Resolving {
        if host.eq_ignore_ascii_case(&self.host) && !self.ips.is_empty() {
            let addrs = ips_to_addrs(self.ips.clone());
            return Box::pin(async move { Ok(addrs) });
        }
        self.gai.resolve(host)
    }
}

// === DNS wire format (RFC 1035 / RFC 8484) ===

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;

/// Encode a single-question DNS query for `host`/`qtype` in wire format
fn encode_query(id: u16, host: &str, qtype: u16) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(host.len() + 32);
    // Header: ID, flags (RD=1), QDCOUNT=1, the rest zero
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // recursion desired
    buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

    // QNAME: length-prefixed labels, then a zero byte for the root
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() {
            return Err(Error::Url(format!("invalid hostname for DoH: {host}")));
        }
        if label.len() > 63 {
            return Err(Error::Url(format!("DNS label too long in {host}")));
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0); // root label

    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(buf)
}

/// Parse a DNS response: pull out the `want_type` (A or AAAA) addresses and the smallest TTL among the records that matched
fn parse_response(buf: &[u8], want_type: u16) -> Result<(Vec<IpAddr>, Duration)> {
    if buf.len() < 12 {
        return Err(Error::Decode("DNS response too short".into()));
    }
    // Header
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    let rcode = flags & 0x000F;
    if rcode != 0 {
        return Err(Error::Decode(format!("DNS rcode {rcode}")));
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    let ancount = u16::from_be_bytes([buf[6], buf[7]]);

    let mut pos = 12usize;

    // Skip past the question section
    for _ in 0..qdcount {
        pos = skip_name(buf, pos)?;
        // QTYPE (2) + QCLASS (2)
        pos = pos
            .checked_add(4)
            .filter(|&p| p <= buf.len())
            .ok_or_else(|| Error::Decode("truncated question".into()))?;
    }

    let mut addrs = Vec::new();
    let mut min_ttl = MAX_TTL;

    for _ in 0..ancount {
        pos = skip_name(buf, pos)?;
        // TYPE(2) CLASS(2) TTL(4) RDLENGTH(2)
        if pos + 10 > buf.len() {
            return Err(Error::Decode("truncated answer header".into()));
        }
        let rtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let ttl = u32::from_be_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]);
        let rdlength = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlength > buf.len() {
            return Err(Error::Decode("truncated rdata".into()));
        }
        let rdata = &buf[pos..pos + rdlength];

        if rtype == want_type {
            match (want_type, rdlength) {
                (TYPE_A, 4) => {
                    let octets: [u8; 4] = rdata.try_into().unwrap();
                    addrs.push(IpAddr::V4(Ipv4Addr::from(octets)));
                    min_ttl = min_ttl.min(Duration::from_secs(ttl as u64));
                }
                (TYPE_AAAA, 16) => {
                    let octets: [u8; 16] = rdata.try_into().unwrap();
                    addrs.push(IpAddr::V6(Ipv6Addr::from(octets)));
                    min_ttl = min_ttl.min(Duration::from_secs(ttl as u64));
                }
                // Length doesn't match the type, so skip it to be safe
                _ => {}
            }
        }
        // We skip CNAMEs and anything else and only keep the address family we asked for. The recursive resolver behind the endpoint has already chased the CNAMEs and dropped the address records in here for us
        pos += rdlength;
    }

    Ok((addrs, min_ttl))
}

/// Walk past a DNS name starting at `pos`, handling compression pointers. Returns the offset right after the name in the current record; a compression pointer ends the name in 2 bytes
fn skip_name(buf: &[u8], mut pos: usize) -> Result<usize> {
    loop {
        let len = *buf
            .get(pos)
            .ok_or_else(|| Error::Decode("name past end".into()))?;
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    return Ok(pos + 1);
                }
                pos = pos
                    .checked_add(1 + len as usize)
                    .filter(|&p| p <= buf.len())
                    .ok_or_else(|| Error::Decode("label past end".into()))?;
            }
            0xC0 => {
                // Compression pointer: 2 bytes, and the name ends here
                if pos + 2 > buf.len() {
                    return Err(Error::Decode("truncated pointer".into()));
                }
                return Ok(pos + 2);
            }
            _ => return Err(Error::Decode("invalid label length".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_query_basic() {
        let q = encode_query(0x1234, "example.com", TYPE_A).unwrap();
        // Header(12) + 7"example" + 3"com" + len bytes(2) + root(1) + qtype(2) + qclass(2)
        assert_eq!(&q[0..2], &[0x12, 0x34]);
        assert_eq!(&q[2..4], &[0x01, 0x00]); // RD
        assert_eq!(&q[4..6], &[0x00, 0x01]); // QDCOUNT=1
                                             // QNAME
        assert_eq!(q[12], 7);
        assert_eq!(&q[13..20], b"example");
        assert_eq!(q[20], 3);
        assert_eq!(&q[21..24], b"com");
        assert_eq!(q[24], 0); // root
        assert_eq!(&q[25..27], &[0x00, 0x01]); // A
        assert_eq!(&q[27..29], &[0x00, 0x01]); // IN
    }

    #[test]
    fn encode_query_rejects_empty_label() {
        assert!(encode_query(1, "a..b", TYPE_A).is_err());
    }

    #[test]
    fn encode_query_trailing_dot_ok() {
        // A trailing dot (FQDN form) should encode the same as without one
        let a = encode_query(1, "example.com", TYPE_A).unwrap();
        let b = encode_query(1, "example.com.", TYPE_A).unwrap();
        assert_eq!(a, b);
    }

    /// Build a minimal A response: one question plus `n` A answers
    fn build_a_response(host: &str, ips: &[(Ipv4Addr, u32)]) -> Vec<u8> {
        let mut buf = Vec::new();
        // Header: id, flags (QR=1, RD=1, RA=1), qd=1, an=n
        buf.extend_from_slice(&0x1234u16.to_be_bytes());
        buf.extend_from_slice(&0x8180u16.to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&(ips.len() as u16).to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        // Question
        for label in host.split('.') {
            buf.push(label.len() as u8);
            buf.extend_from_slice(label.as_bytes());
        }
        buf.push(0);
        buf.extend_from_slice(&TYPE_A.to_be_bytes());
        buf.extend_from_slice(&CLASS_IN.to_be_bytes());
        // Answers, each pointing back at the question name via compression
        for (ip, ttl) in ips {
            buf.extend_from_slice(&0xC00Cu16.to_be_bytes()); // pointer to offset 12
            buf.extend_from_slice(&TYPE_A.to_be_bytes());
            buf.extend_from_slice(&CLASS_IN.to_be_bytes());
            buf.extend_from_slice(&ttl.to_be_bytes());
            buf.extend_from_slice(&4u16.to_be_bytes());
            buf.extend_from_slice(&ip.octets());
        }
        buf
    }

    #[test]
    fn parse_response_single_a() {
        let resp = build_a_response("example.com", &[(Ipv4Addr::new(93, 184, 216, 34), 300)]);
        let (addrs, ttl) = parse_response(&resp, TYPE_A).unwrap();
        assert_eq!(addrs, vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
        assert_eq!(ttl, Duration::from_secs(300));
    }

    #[test]
    fn parse_response_multiple_a_min_ttl() {
        let resp = build_a_response(
            "example.com",
            &[
                (Ipv4Addr::new(1, 1, 1, 1), 600),
                (Ipv4Addr::new(2, 2, 2, 2), 120),
            ],
        );
        let (addrs, ttl) = parse_response(&resp, TYPE_A).unwrap();
        assert_eq!(addrs.len(), 2);
        assert_eq!(ttl, Duration::from_secs(120));
    }

    #[test]
    fn parse_response_filters_other_family() {
        let resp = build_a_response("example.com", &[(Ipv4Addr::new(1, 1, 1, 1), 300)]);
        // Asking for AAAA against an A-only response should come back empty
        let (addrs, _) = parse_response(&resp, TYPE_AAAA).unwrap();
        assert!(addrs.is_empty());
    }

    #[test]
    fn parse_response_rejects_servfail() {
        let mut resp = build_a_response("example.com", &[]);
        // Set rcode = 2 (SERVFAIL) in the low nibble of the flags
        resp[3] |= 0x02;
        assert!(parse_response(&resp, TYPE_A).is_err());
    }

    #[test]
    fn parse_response_too_short() {
        assert!(parse_response(&[0u8; 4], TYPE_A).is_err());
    }

    #[test]
    fn ips_to_addrs_uses_port_zero() {
        let addrs: Vec<SocketAddr> = ips_to_addrs(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]).collect();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].port(), 0);
    }

    #[test]
    fn new_rejects_non_https() {
        let cfg = DohConfig {
            url: "http://insecure.example/dns-query".into(),
            bootstrap: vec![],
            fallback: true,
        };
        assert!(DohResolver::new(cfg).is_err());
    }
}
