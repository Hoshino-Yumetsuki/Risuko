//! Minimal BEP-5 DHT

use std::collections::{BTreeMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::RngExt;
use tokio::net::{lookup_host, UdpSocket};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use super::bencode::{decode_all_external, encode_to_vec, DecodeLimits, Value};
use super::core::Id20;

/// Decoded `get_peers` reply: source addr, responder id (if present), peer list, and learned (id, addr) nodes
type GetPeersReply = (
    SocketAddr,
    Option<Id20>,
    Vec<SocketAddr>,
    Vec<(Id20, SocketAddr)>,
    Option<Vec<u8>>,
);

/// Body fields parsed from a `get_peers` response (no source addr)
type GetPeersResponseBody = (
    Option<Id20>,
    Vec<SocketAddr>,
    Vec<(Id20, SocketAddr)>,
    Option<Vec<u8>>,
);

const K: usize = 8;
const ALPHA: usize = 3;
const QUERY_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_ROUND_QUERIES: usize = 50;
const KRPC_DECODE_LIMITS: DecodeLimits = DecodeLimits::new(2048, 16, 1024);

pub const DEFAULT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "router.utorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "dht.libtorrent.org:25401",
    "router.bitcomet.com:6881",
];

/// A live DHT node; holds bound UDP sockets (v4 always, v6 if available) and a background reader task per socket that routes responses to pending queries by transaction id
pub struct Dht {
    sock: Arc<UdpSocket>,
    sock6: Option<Arc<UdpSocket>>,
    our_id: Id20,
    pending: Arc<Mutex<PendingMap>>,
    routing: Arc<Mutex<RoutingTable>>,
    reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reader6_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Dht {
    fn drop(&mut self) {
        if let Some(h) = self.reader_handle.lock().take() {
            h.abort();
        }
        if let Some(h) = self.reader6_handle.lock().take() {
            h.abort();
        }
    }
}

type PendingMap = std::collections::HashMap<u16, PendingEntry>;

#[derive(Clone)]
struct PendingToken(Arc<()>);

impl PendingToken {
    fn new() -> Self {
        Self(Arc::new(()))
    }

    fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

struct PendingEntry {
    tx: oneshot::Sender<KrpcResponse>,
    target: SocketAddr,
    token: PendingToken,
}

/// Removes its transaction id from `pending` on drop, keeping aborted lookup tasks from leaving orphaned pending entries
struct PendingGuard {
    pending: Arc<Mutex<PendingMap>>,
    txn: u16,
    token: PendingToken,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut pending = self.pending.lock();
        let should_remove = pending
            .get(&self.txn)
            .is_some_and(|entry| entry.token.matches(&self.token));
        if should_remove {
            pending.remove(&self.txn);
        }
    }
}

struct KrpcResponse {
    from: SocketAddr,
    body: Value,
}

impl Dht {
    /// Process-wide DHT node, lazily spawned and bootstrapped on first use
    pub async fn shared() -> Option<Arc<Dht>> {
        static SHARED: tokio::sync::OnceCell<Arc<Dht>> = tokio::sync::OnceCell::const_new();
        SHARED
            .get_or_try_init(|| async {
                let dht = Dht::spawn().await?;
                // Warm the routing table in the background so the first lookup already has live, close-ish nodes to query
                let warm = dht.clone();
                tokio::spawn(async move { warm.bootstrap().await });
                Ok::<Arc<Dht>, std::io::Error>(dht)
            })
            .await
            .ok()
            .cloned()
    }

    pub async fn spawn() -> std::io::Result<Arc<Self>> {
        let sock = UdpSocket::bind("0.0.0.0:0").await?;
        let sock = Arc::new(sock);
        // Try to bind a dual-stack IPv6 socket too; if the host lacks IPv6 the bind fails and we carry on with v4 only
        let sock6 = match UdpSocket::bind("[::]:0").await {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                tracing::debug!("dht: no ipv6 socket: {e}");
                None
            }
        };
        let our_id = random_id();
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(Default::default()));

        let this = Arc::new(Self {
            sock: sock.clone(),
            sock6: sock6.clone(),
            our_id,
            pending: pending.clone(),
            routing: Arc::new(Mutex::new(RoutingTable::new(our_id))),
            reader_handle: Mutex::new(None),
            reader6_handle: Mutex::new(None),
        });

        let reader_sock = sock.clone();
        let pending_reader = pending.clone();
        let reader_handle = tokio::spawn(async move {
            reader_loop(reader_sock, pending_reader).await;
        });
        *this.reader_handle.lock() = Some(reader_handle);

        if let Some(s6) = sock6 {
            let pending6 = pending.clone();
            let reader6_handle = tokio::spawn(async move {
                reader_loop(s6, pending6).await;
            });
            *this.reader6_handle.lock() = Some(reader6_handle);
        }

        tracing::debug!(
            "DHT started: id={}, bootstrap={} nodes, ipv6={}",
            hex::encode(our_id.as_bytes()),
            DEFAULT_BOOTSTRAP.len(),
            this.sock6.is_some(),
        );
        Ok(this)
    }

    /// Start an iterative `get_peers` lookup and stream discovered peers on the returned channel until `budget` elapses or the lookup converges; when `announce_port` is set, also re-publishes us to the DHT (BEP-5 `announce_peer`) on the closest write-token nodes so other clients searching this info-hash can dial us on that BT listen port
    pub fn get_peers_stream(
        self: &Arc<Self>,
        info_hash: Id20,
        budget: Duration,
        announce_port: Option<u16>,
    ) -> mpsc::UnboundedReceiver<SocketAddr> {
        let (tx, rx) = mpsc::unbounded_channel::<SocketAddr>();
        let this = self.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(
                budget,
                this.iterative_get_peers(info_hash, tx, announce_port),
            )
            .await;
        });
        rx
    }

    async fn iterative_get_peers(
        self: Arc<Self>,
        info_hash: Id20,
        peer_tx: mpsc::UnboundedSender<SocketAddr>,
        announce_port: Option<u16>,
    ) {
        // Seed the lookup from our warm routing table first (Kademlia reuses known-close nodes), then augment with public bootstrap hostnames; without the routing-table seed every lookup restarts cold from bootstrap servers (slow, and why the first magnet resolutions after launch were sluggish), whereas a warm table lets later lookups start close to the target and converge quickly
        let mut addrs: Vec<SocketAddr> = self.routing.lock().closest(&info_hash, K * 2);
        for host in DEFAULT_BOOTSTRAP {
            if let Ok(iter) = lookup_host(host).await {
                addrs.extend(iter);
            }
        }
        if addrs.is_empty() {
            tracing::debug!("dht: no bootstrap nodes resolved");
            return;
        }

        let mut shortlist: BTreeMap<Id20, SocketAddr> = BTreeMap::new();
        let mut queried: HashSet<SocketAddr> = HashSet::new();
        let mut peers_seen: HashSet<SocketAddr> = HashSet::new();
        let mut announce_targets: BTreeMap<Id20, (SocketAddr, Vec<u8>)> = BTreeMap::new();

        let mut seed_seen = HashSet::new();
        addrs.retain(|addr| seed_seen.insert(*addr));
        addrs.truncate(MAX_ROUND_QUERIES);
        queried.extend(addrs.iter().copied());

        let mut futs: JoinSet<Option<GetPeersReply>> = JoinSet::new();
        for a in addrs {
            let this = self.clone();
            futs.spawn(async move { this.query_get_peers(a, info_hash).await });
        }

        let mut total_peers = 0usize;
        let mut total_nodes = 0usize;
        let mut rounds_without_progress = 0usize;

        loop {
            let Some(joined) = futs.join_next().await else {
                break;
            };
            let res = joined.ok().flatten();

            let Some((from, responder_id, peers, nodes, token)) = res else {
                continue;
            };

            // Emit peers immediately
            let mut new_peers = 0usize;
            for p in peers {
                if peers_seen.insert(p) {
                    new_peers += 1;
                    if peer_tx.send(p).is_err() {
                        return;
                    }
                }
            }
            total_peers += new_peers;

            // Record the responder as a live DHT node, preferring the id it reported in its KRPC response and falling back to a pseudo-id only if omitted; the real id keeps XOR distance accurate, which matters for lookup convergence
            let node_id = responder_id.unwrap_or_else(|| pseudo_id(from));
            shortlist.insert(node_id.distance(&info_hash), from);
            if responder_id.is_some() {
                self.routing.lock().add(node_id, from);
            }
            if let Some(tok) = token {
                announce_targets.insert(node_id.distance(&info_hash), (from, tok));
                while announce_targets.len() > K * 2 {
                    announce_targets.pop_last();
                }
            }

            // Merge any learned nodes into the shortlist
            let mut progressed = false;
            for (nid, naddr) in &nodes {
                total_nodes += 1;
                let d = nid.distance(&info_hash);
                if let std::collections::btree_map::Entry::Vacant(e) = shortlist.entry(d) {
                    e.insert(*naddr);
                    progressed = true;
                }
                self.routing.lock().add(*nid, *naddr);
            }

            // Trim to K * 2 to keep memory bounded
            while shortlist.len() > K * 4 {
                shortlist.pop_last();
            }

            if progressed || new_peers > 0 {
                rounds_without_progress = 0;
            } else {
                rounds_without_progress += 1;
            }

            // Queue up to ALPHA new queries from the closest unvisited
            let mut dispatched = 0usize;
            let mut to_dispatch: Vec<SocketAddr> = Vec::new();
            for (_d, a) in shortlist.iter().take(K * 2) {
                if queried.insert(*a) {
                    to_dispatch.push(*a);
                    dispatched += 1;
                    if dispatched >= ALPHA {
                        break;
                    }
                }
            }
            for a in to_dispatch {
                let this = self.clone();
                futs.spawn(async move { this.query_get_peers(a, info_hash).await });
            }

            if dispatched == 0 && futs.is_empty() {
                break;
            }
            if rounds_without_progress > 20 && peers_seen.len() >= 40 {
                break;
            }
        }

        tracing::debug!(
            "dht get_peers: peers={} nodes_learned={} nodes_queried={}",
            total_peers,
            total_nodes,
            queried.len()
        );

        // BEP-5 announce_peer: publish ourselves on the closest token-bearing nodes so other clients doing get_peers for this info-hash discover us and can open inbound connections; fire-and-forget, we don't need the ack
        if let Some(port) = announce_port {
            for (_d, (addr, token)) in announce_targets.into_iter().take(K) {
                let pkt = build_announce_peer(
                    rand::rng().random(),
                    &self.our_id,
                    &info_hash,
                    port,
                    &token,
                );
                let _ = match addr {
                    SocketAddr::V4(_) => self.sock.send_to(&pkt, addr).await,
                    SocketAddr::V6(_) => match &self.sock6 {
                        Some(s6) => s6.send_to(&pkt, addr).await,
                        None => continue,
                    },
                };
            }
        }
    }

    async fn query_get_peers(
        self: Arc<Self>,
        target: SocketAddr,
        info_hash: Id20,
    ) -> Option<GetPeersReply> {
        let (txn, rx, _guard) = self.register_transaction(target);
        let packet = build_get_peers(txn, &self.our_id, &info_hash);
        // `_guard` removes `txn` from `pending`, including JoinSet aborts on budget timeout; route via the appropriate socket family, dropping the query if we target an IPv6 node but lack a v6 socket
        let send_res = match target {
            SocketAddr::V4(_) => self.sock.send_to(&packet, target).await,
            SocketAddr::V6(_) => {
                if let Some(s6) = &self.sock6 {
                    s6.send_to(&packet, target).await
                } else {
                    return None;
                }
            }
        };
        if send_res.is_err() {
            return None;
        }
        let resp = match tokio::time::timeout(QUERY_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            _ => {
                return None;
            }
        };
        parse_get_peers_response(&resp.body)
            .map(|(rid, peers, nodes, token)| (resp.from, rid, peers, nodes, token))
    }

    fn register_transaction(
        &self,
        target: SocketAddr,
    ) -> (u16, oneshot::Receiver<KrpcResponse>, PendingGuard) {
        let (tx, rx) = oneshot::channel();
        let mut map = self.pending.lock();
        let mut txn: u16 = rand::rng().random();
        while map.contains_key(&txn) {
            txn = txn.wrapping_add(1);
        }
        let token = PendingToken::new();
        map.insert(
            txn,
            PendingEntry {
                tx,
                target,
                token: token.clone(),
            },
        );
        let guard = PendingGuard {
            pending: self.pending.clone(),
            txn,
            token,
        };
        (txn, rx, guard)
    }

    /// Number of unique nodes currently held in the Kademlia routing table; a coarse health signal for DHT bootstrap progress
    pub fn routing_table_len(&self) -> usize {
        self.routing.lock().len()
    }

    /// BEP-5 PORT support
    pub fn add_node(&self, addr: SocketAddr) {
        self.routing.lock().add(pseudo_id(addr), addr);
    }

    /// Warm the routing table by iteratively looking up our own id (bootstrap nodes respond with contacts closest to us, which populates a fresh table), returning once the lookup converges or `budget` elapses
    pub async fn bootstrap(self: &Arc<Self>) {
        let target = self.our_id;
        // We discard discovered peers; we only care about the side-effect node population in the routing table
        let mut rx = self.get_peers_stream(target, Duration::from_secs(15), None);
        while rx.recv().await.is_some() {}
    }
}

// Kademlia routing table (BEP 5 §"Routing Table"): 160 buckets indexed by the highest differing bit between `our_id` and a node's id (XOR distance), each holding at most K=8 nodes with LRU eviction on `last_seen`; nodes are inserted passively from KRPC responses (responder plus returned contacts) with no active liveness pings, so staleness is bounded by new traffic replacing old entries

const BUCKET_SIZE: usize = K;
const NUM_BUCKETS: usize = 160;

#[derive(Debug, Clone)]
struct RoutingNode {
    id: Id20,
    addr: SocketAddr,
    last_seen: Instant,
}

pub(crate) struct RoutingTable {
    our_id: Id20,
    buckets: Vec<Vec<RoutingNode>>,
}

impl RoutingTable {
    fn new(our_id: Id20) -> Self {
        let buckets = (0..NUM_BUCKETS)
            .map(|_| Vec::with_capacity(BUCKET_SIZE))
            .collect();
        Self { our_id, buckets }
    }

    fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    fn bucket_index(&self, id: &Id20) -> Option<usize> {
        // Position of the highest set bit in XOR(our_id, id); identical ids (distance 0) belong to no bucket and are skipped
        let xord = self.our_id.distance(id);
        let bytes = xord.as_bytes();
        for (byte_pos, byte) in bytes.iter().enumerate() {
            if *byte != 0 {
                let leading = byte.leading_zeros() as usize;
                let bit_pos = byte_pos * 8 + leading;
                // bit_pos 0 == highest bit set → most-distant bucket → index 0; bit_pos 159 == lowest bit set → closest bucket → index 159
                return Some(bit_pos);
            }
        }
        None
    }

    fn add(&mut self, id: Id20, addr: SocketAddr) {
        let Some(idx) = self.bucket_index(&id) else {
            return;
        };
        let now = Instant::now();
        let bucket = &mut self.buckets[idx];
        // Refresh existing entry if seen
        if let Some(existing) = bucket.iter_mut().find(|n| n.id == id) {
            existing.addr = addr;
            existing.last_seen = now;
            return;
        }
        if bucket.len() < BUCKET_SIZE {
            bucket.push(RoutingNode {
                id,
                addr,
                last_seen: now,
            });
            return;
        }
        // Evict the stalest entry; BEP 5 wants a ping-then-evict dance, but this passive variant trades a little routing optimality for simplicity and zero extra wire traffic
        if let Some(stalest_idx) = bucket
            .iter()
            .enumerate()
            .min_by_key(|(_, n)| n.last_seen)
            .map(|(i, _)| i)
        {
            bucket[stalest_idx] = RoutingNode {
                id,
                addr,
                last_seen: now,
            };
        }
    }

    /// The `n` nodes whose ids are closest (by XOR) to `target`; used to seed an iterative lookup from the warm routing table instead of the cold public bootstrap servers
    fn closest(&self, target: &Id20, n: usize) -> Vec<SocketAddr> {
        let mut all: Vec<(Id20, SocketAddr)> = self
            .buckets
            .iter()
            .flatten()
            .map(|node| (node.id.distance(target), node.addr))
            .collect();
        all.sort_by_key(|&(dist, _)| dist);
        all.into_iter().take(n).map(|(_, addr)| addr).collect()
    }
}

fn random_id() -> Id20 {
    let mut b = [0u8; 20];
    rand::rng().fill(&mut b[..]);
    Id20::from_slice(&b).expect("20 bytes")
}

fn pseudo_id(addr: SocketAddr) -> Id20 {
    // Stable per-address id for shortlist ordering. Not a real DHT id
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    match addr {
        SocketAddr::V4(a) => {
            h.update(a.ip().octets());
            h.update(a.port().to_be_bytes());
        }
        SocketAddr::V6(a) => {
            h.update(a.ip().octets());
            h.update(a.port().to_be_bytes());
        }
    }
    Id20::from_slice(&h.finalize()[..20]).unwrap()
}

/// Build a BEP-5 `announce_peer` query; the `token` must be one we received from this node's prior `get_peers` response, otherwise it rejects us
fn build_announce_peer(
    txn: u16,
    our_id: &Id20,
    info_hash: &Id20,
    port: u16,
    token: &[u8],
) -> Vec<u8> {
    // Dict keys must be bencode-sorted: id, implied_port, info_hash, port, token
    let args = Value::Dict(vec![
        (b"id".to_vec(), Value::Bytes(our_id.as_bytes().to_vec())),
        (b"implied_port".to_vec(), Value::Int(0)),
        (
            b"info_hash".to_vec(),
            Value::Bytes(info_hash.as_bytes().to_vec()),
        ),
        (b"port".to_vec(), Value::Int(port as i64)),
        (b"token".to_vec(), Value::Bytes(token.to_vec())),
    ]);
    let tid = txn.to_be_bytes().to_vec();
    let msg = Value::Dict(vec![
        (b"a".to_vec(), args),
        (b"q".to_vec(), Value::Bytes(b"announce_peer".to_vec())),
        (b"t".to_vec(), Value::Bytes(tid)),
        (b"y".to_vec(), Value::Bytes(b"q".to_vec())),
    ]);
    encode_to_vec(&msg)
}

fn build_get_peers(txn: u16, our_id: &Id20, info_hash: &Id20) -> Vec<u8> {
    let args = Value::Dict(vec![
        (b"id".to_vec(), Value::Bytes(our_id.as_bytes().to_vec())),
        (
            b"info_hash".to_vec(),
            Value::Bytes(info_hash.as_bytes().to_vec()),
        ),
        // BEP-32: request both v4 and v6 contacts; nodes that don't understand `want` ignore it, so this is always safe to send
        (
            b"want".to_vec(),
            Value::List(vec![
                Value::Bytes(b"n4".to_vec()),
                Value::Bytes(b"n6".to_vec()),
            ]),
        ),
    ]);
    let tid = txn.to_be_bytes().to_vec();
    let msg = Value::Dict(vec![
        (b"a".to_vec(), args),
        (b"q".to_vec(), Value::Bytes(b"get_peers".to_vec())),
        (b"t".to_vec(), Value::Bytes(tid)),
        (b"y".to_vec(), Value::Bytes(b"q".to_vec())),
    ]);
    encode_to_vec(&msg)
}

fn parse_get_peers_response(body: &Value) -> Option<GetPeersResponseBody> {
    let r_val = body.get(b"r")?;
    r_val.as_dict()?;
    let responder_id = r_val
        .get(b"id")
        .and_then(|v| v.as_bytes())
        .and_then(|b| Id20::from_slice(b).ok());
    let mut peers: Vec<SocketAddr> = Vec::new();
    if let Some(values) = r_val.get(b"values").and_then(|v| v.as_list()) {
        for v in values {
            if let Some(b) = v.as_bytes() {
                match b.len() {
                    6 => {
                        let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
                        let port = u16::from_be_bytes([b[4], b[5]]);
                        peers.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
                    }
                    18 => {
                        let mut o = [0u8; 16];
                        o.copy_from_slice(&b[..16]);
                        let ip = Ipv6Addr::from(o);
                        let port = u16::from_be_bytes([b[16], b[17]]);
                        peers.push(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0)));
                    }
                    _ => {}
                }
            }
        }
    }
    let mut nodes: Vec<(Id20, SocketAddr)> = Vec::new();
    if let Some(n) = r_val.get(b"nodes").and_then(|v| v.as_bytes()) {
        for chunk in n.chunks_exact(26) {
            let id = Id20::from_slice(&chunk[..20]).ok()?;
            let ip = Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23]);
            let port = u16::from_be_bytes([chunk[24], chunk[25]]);
            nodes.push((id, SocketAddr::V4(SocketAddrV4::new(ip, port))));
        }
    }
    if let Some(n6) = r_val.get(b"nodes6").and_then(|v| v.as_bytes()) {
        // Each compact v6 node: 20 bytes id + 16 bytes ipv6 + 2 bytes port
        for chunk in n6.chunks_exact(38) {
            let id = match Id20::from_slice(&chunk[..20]) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut o = [0u8; 16];
            o.copy_from_slice(&chunk[20..36]);
            let ip = Ipv6Addr::from(o);
            let port = u16::from_be_bytes([chunk[36], chunk[37]]);
            nodes.push((id, SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0))));
        }
    }
    let token = r_val
        .get(b"token")
        .and_then(|v| v.as_bytes())
        .map(|b| b.to_vec());
    Some((responder_id, peers, nodes, token))
}

async fn reader_loop(sock: Arc<UdpSocket>, pending: Arc<Mutex<PendingMap>>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, from) = match sock.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(_) => return,
        };
        let Ok(msg) = decode_all_external(&buf[..n], KRPC_DECODE_LIMITS) else {
            continue;
        };
        let Some(ty) = msg.get(b"y").and_then(|v| v.as_bytes()) else {
            continue;
        };
        if ty != b"r" && ty != b"e" {
            continue;
        }
        let Some(tid) = msg.get(b"t").and_then(|v| v.as_bytes()) else {
            continue;
        };
        let txn = match tid.len() {
            2 => u16::from_be_bytes([tid[0], tid[1]]),
            _ => continue,
        };
        let mut guard = pending.lock();
        if let Some(entry) = guard.get(&txn) {
            if entry.target == from {
                if let Some(entry) = guard.remove(&txn) {
                    let _ = entry.tx.send(KrpcResponse { from, body: msg });
                }
            }
            // Mismatch: ignore the packet, leave entry for the real responder
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::decode_all;

    #[test]
    fn routing_table_inserts_dedupes_and_indexes_distance() {
        let me = Id20::from_slice(&[0u8; 20]).unwrap();
        let mut rt = RoutingTable::new(me);
        let n1 = Id20::from_slice(&[1u8; 20]).unwrap();
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        rt.add(n1, addr);
        rt.add(n1, addr); // dedup → still 1
        assert_eq!(rt.len(), 1);
        let n2 = Id20::from_slice(&[2u8; 20]).unwrap();
        rt.add(n2, addr);
        assert_eq!(rt.len(), 2);
        // Inserting our own id is a no-op
        rt.add(me, addr);
        assert_eq!(rt.len(), 2);
    }

    #[test]
    fn routing_table_caps_bucket_at_k() {
        let me = Id20::from_slice(&[0u8; 20]).unwrap();
        let mut rt = RoutingTable::new(me);
        // All ids share the most-significant bit set (byte[0] = 0x80) so bit_pos == 0 for the entire batch and every entry lands in bucket 0, which must cap at K
        for i in 1..=20u8 {
            let mut id = [0u8; 20];
            id[0] = 0x80;
            id[19] = i;
            rt.add(
                Id20::from_slice(&id).unwrap(),
                "127.0.0.1:1".parse().unwrap(),
            );
        }
        assert_eq!(rt.len(), BUCKET_SIZE);
    }

    #[test]
    fn get_peers_packet_is_bencoded_krpc_query() {
        let our_id = Id20::from_slice(&[0u8; 20]).unwrap();
        let info_hash = Id20::from_slice(&[1u8; 20]).unwrap();
        let packet = build_get_peers(0xBEEF, &our_id, &info_hash);
        let decoded = decode_all(&packet).unwrap();
        assert_eq!(
            decoded.get(b"q").and_then(|v| v.as_bytes()),
            Some(b"get_peers" as &[u8])
        );
        assert_eq!(
            decoded.get(b"y").and_then(|v| v.as_bytes()),
            Some(b"q" as &[u8])
        );
        assert_eq!(
            decoded.get(b"t").and_then(|v| v.as_bytes()),
            Some(&[0xBE, 0xEF][..])
        );
        let a = decoded.get(b"a").unwrap().as_dict().unwrap();
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn announce_peer_packet_is_bencoded_krpc_query() {
        let our_id = Id20::from_slice(&[0u8; 20]).unwrap();
        let info_hash = Id20::from_slice(&[1u8; 20]).unwrap();
        let packet = build_announce_peer(0xCAFE, &our_id, &info_hash, 6881, b"tok");
        let decoded = decode_all(&packet).unwrap();
        assert_eq!(
            decoded.get(b"q").and_then(|v| v.as_bytes()),
            Some(b"announce_peer" as &[u8])
        );
        assert_eq!(
            decoded.get(b"y").and_then(|v| v.as_bytes()),
            Some(b"q" as &[u8])
        );
        let a = Value::Dict(decoded.get(b"a").unwrap().as_dict().unwrap().to_vec());
        assert_eq!(a.get(b"port").and_then(|v| v.as_int()), Some(6881));
        assert_eq!(
            a.get(b"token").and_then(|v| v.as_bytes()),
            Some(b"tok" as &[u8])
        );
        assert_eq!(
            a.get(b"info_hash").and_then(|v| v.as_bytes()),
            Some(&[1u8; 20][..])
        );
    }

    #[test]
    fn parse_response_extracts_peers_and_nodes() {
        // values: [6-byte peer for 1.2.3.4:5678]; nodes: 26 bytes (id=0x22... ip=9.8.7.6 port=11111)
        let peer_bytes: Vec<u8> = vec![1, 2, 3, 4, (5678u16 >> 8) as u8, (5678u16 & 0xff) as u8];
        let mut node_bytes = vec![0x22u8; 20];
        node_bytes.extend_from_slice(&[9, 8, 7, 6]);
        node_bytes.extend_from_slice(&11111u16.to_be_bytes());

        let r = Value::Dict(vec![
            (b"id".to_vec(), Value::Bytes(vec![0u8; 20])),
            (b"nodes".to_vec(), Value::Bytes(node_bytes)),
            (b"token".to_vec(), Value::Bytes(b"abcd".to_vec())),
            (
                b"values".to_vec(),
                Value::List(vec![Value::Bytes(peer_bytes)]),
            ),
        ]);
        let body = Value::Dict(vec![
            (b"r".to_vec(), r),
            (b"t".to_vec(), Value::Bytes(b"aa".to_vec())),
            (b"y".to_vec(), Value::Bytes(b"r".to_vec())),
        ]);
        let (_id, peers, nodes, token) = parse_get_peers_response(&body).unwrap();
        assert_eq!(token.as_deref(), Some(b"abcd" as &[u8]));
        assert_eq!(peers.len(), 1);
        assert_eq!(
            peers[0],
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 5678))
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].1,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(9, 8, 7, 6), 11111))
        );
    }

    #[test]
    fn parse_response_extracts_v6_peers_and_nodes6() {
        // 18-byte compact v6 peer: ip=::1 port=6881
        let mut peer6: Vec<u8> = Vec::with_capacity(18);
        peer6.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        peer6.extend_from_slice(&6881u16.to_be_bytes());
        // 38-byte compact v6 node: id=0x33... ip=2001:db8::1 port=12345
        let mut node6 = vec![0x33u8; 20];
        let ip6: Ipv6Addr = "2001:db8::1".parse().unwrap();
        node6.extend_from_slice(&ip6.octets());
        node6.extend_from_slice(&12345u16.to_be_bytes());

        let r = Value::Dict(vec![
            (b"id".to_vec(), Value::Bytes(vec![0u8; 20])),
            (b"nodes6".to_vec(), Value::Bytes(node6)),
            (b"values".to_vec(), Value::List(vec![Value::Bytes(peer6)])),
        ]);
        let body = Value::Dict(vec![
            (b"r".to_vec(), r),
            (b"t".to_vec(), Value::Bytes(b"bb".to_vec())),
            (b"y".to_vec(), Value::Bytes(b"r".to_vec())),
        ]);
        let (_id, peers, nodes, _token) = parse_get_peers_response(&body).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(
            peers[0],
            SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 6881, 0, 0))
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].1,
            SocketAddr::V6(SocketAddrV6::new(ip6, 12345, 0, 0))
        );
    }

    #[test]
    fn get_peers_packet_includes_want_n4_n6() {
        let our_id = Id20::from_slice(&[0u8; 20]).unwrap();
        let info_hash = Id20::from_slice(&[1u8; 20]).unwrap();
        let packet = build_get_peers(0xABCD, &our_id, &info_hash);
        let decoded = decode_all(&packet).unwrap();
        let a = decoded.get(b"a").unwrap().as_dict().unwrap();
        let a_val = Value::Dict(a.to_vec());
        let want = a_val.get(b"want").unwrap().as_list().unwrap();
        let labels: Vec<&[u8]> = want.iter().filter_map(|v| v.as_bytes()).collect();
        assert!(labels.contains(&(b"n4" as &[u8])));
        assert!(labels.contains(&(b"n6" as &[u8])));
    }

    #[test]
    fn pending_guard_drop_does_not_remove_reused_transaction() {
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(Default::default()));
        let txn = 0x1234;
        let target: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (old_tx, _old_rx) = oneshot::channel();
        let (new_tx, _new_rx) = oneshot::channel();
        let old_token = PendingToken::new();
        let new_token = PendingToken::new();

        pending.lock().insert(
            txn,
            PendingEntry {
                tx: old_tx,
                target,
                token: old_token.clone(),
            },
        );
        let guard = PendingGuard {
            pending: pending.clone(),
            txn,
            token: old_token,
        };
        pending.lock().remove(&txn);
        pending.lock().insert(
            txn,
            PendingEntry {
                tx: new_tx,
                target,
                token: new_token,
            },
        );

        drop(guard);

        assert!(pending.lock().contains_key(&txn));
    }
}
