//! Minimal BEP-5 DHT

use std::collections::{BTreeMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::RngExt;
use tokio::net::{lookup_host, UdpSocket};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use risuko_http::{ProxyDatagram, ProxyDatagramSource};

use super::bencode::{decode_all_external, encode_to_vec, DecodeLimits, Value};
use super::core::Id20;

/// Decoded `get_peers` reply: source addr, responder id (if present), peer list, and learned (id, addr) nodes
type GetPeersReply = (
    DhtTarget,
    Option<Id20>,
    Vec<SocketAddr>,
    Vec<(Id20, SocketAddr)>,
    Option<Vec<u8>>,
);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum DhtTarget {
    Addr(SocketAddr),
    Host(String, u16),
}

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

/// Process-wide DHT ownership and route state
#[derive(Default)]
struct SharedDhtState {
    dht: Option<Arc<Dht>>,
    proxy_requested: Option<bool>,
    last_error: Option<String>,
}

static SHARED_DHT: std::sync::OnceLock<tokio::sync::Mutex<SharedDhtState>> =
    std::sync::OnceLock::new();

fn shared_dht_cell() -> &'static tokio::sync::Mutex<SharedDhtState> {
    SHARED_DHT.get_or_init(|| tokio::sync::Mutex::new(SharedDhtState::default()))
}

fn shared_dht_install_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub const DEFAULT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "router.utorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "dht.libtorrent.org:25401",
    "router.bitcomet.com:6881",
];

fn bootstrap_targets() -> Vec<DhtTarget> {
    DEFAULT_BOOTSTRAP
        .iter()
        .filter_map(|raw| {
            let (host, port) = raw.rsplit_once(':')?;
            let port = port.parse().ok()?;
            Some(DhtTarget::Host(
                host.trim_matches(['[', ']']).to_string(),
                port,
            ))
        })
        .collect()
}

fn dht_targets_match(expected: &DhtTarget, actual: &DhtTarget) -> bool {
    match (expected, actual) {
        (DhtTarget::Addr(expected), DhtTarget::Addr(actual)) => expected == actual,
        (
            DhtTarget::Host(expected_host, expected_port),
            DhtTarget::Host(actual_host, actual_port),
        ) => expected_port == actual_port && expected_host.eq_ignore_ascii_case(actual_host),
        // Proxy readers may report the hostname-form source used in a SOCKS5
        // domain request as an IP address. Direct requests are normalized to
        // Addr before registration and therefore never use this fallback.
        (DhtTarget::Host(..), DhtTarget::Addr(..)) => true,
        _ => false,
    }
}

/// A live DHT node; holds bound UDP sockets (v4 always, v6 if available) and a background reader task per socket that routes responses to pending queries by transaction id
pub struct Dht {
    sock: Option<Arc<UdpSocket>>,
    sock6: Option<Arc<UdpSocket>>,
    proxy_datagram: Option<Arc<ProxyDatagram>>,
    our_id: Id20,
    pending: Arc<Mutex<PendingMap>>,
    routing: Arc<Mutex<RoutingTable>>,
    reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reader6_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    lookup_handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    shutdown: AtomicBool,
}

pub struct DhtRouteSwap {
    previous: Option<Arc<Dht>>,
    previous_proxy_requested: Option<bool>,
    previous_error: Option<String>,
    next: Option<Arc<Dht>>,
}

impl DhtRouteSwap {
    pub fn next(&self) -> Option<Arc<Dht>> {
        self.next.clone()
    }

    /// Finalize the route transition and stop the old runtime.
    pub async fn commit(self) {
        let owns_current = {
            let guard = shared_dht_cell().lock().await;
            match (&guard.dht, &self.next) {
                (None, None) => true,
                (Some(current), Some(next)) => Arc::ptr_eq(current, next),
                _ => false,
            }
        };
        if owns_current {
            if let Some(previous) = self.previous {
                previous.shutdown().await;
            }
        } else {
            tracing::debug!("ignoring stale DHT route commit");
            if let Some(next) = self.next {
                next.shutdown().await;
            }
        }
    }

    pub async fn rollback(self) -> Option<Arc<Dht>> {
        let restored = {
            let mut guard = shared_dht_cell().lock().await;
            let owns_current = match (&guard.dht, &self.next) {
                (None, None) => true,
                (Some(current), Some(next)) => Arc::ptr_eq(current, next),
                _ => false,
            };
            if !owns_current {
                tracing::debug!("ignoring stale DHT route rollback");
                let current = guard.dht.clone();
                drop(guard);
                if let Some(next) = self.next {
                    next.shutdown().await;
                }
                return current;
            }
            let current = guard.dht.take();
            guard.dht = self.previous.clone();
            guard.proxy_requested = self.previous_proxy_requested;
            guard.last_error = self.previous_error.clone();
            (current, guard.dht.clone())
        };

        if let Some(current) = restored.0 {
            if self
                .previous
                .as_ref()
                .is_none_or(|previous| !Arc::ptr_eq(previous, &current))
            {
                current.shutdown().await;
            }
        }
        restored.1
    }
}

impl std::fmt::Debug for Dht {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dht").finish_non_exhaustive()
    }
}

impl Drop for Dht {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.abort_lookups();
        if let Some(h) = self.reader_handle.lock().take() {
            h.abort();
        }
        if let Some(h) = self.reader6_handle.lock().take() {
            h.abort();
        }
    }
}

impl Dht {
    fn abort_lookups(&self) {
        let handles = std::mem::take(&mut *self.lookup_handles.lock());
        for handle in handles {
            handle.abort();
        }
    }

    /// Stop iterative lookups while keeping this runtime available for a
    /// possible route rollback.
    pub fn cancel_lookups(&self) {
        self.abort_lookups();
    }

    pub async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.abort_lookups();
        if let Some(handle) = self.reader_handle.lock().take() {
            handle.abort();
        }
        if let Some(handle) = self.reader6_handle.lock().take() {
            handle.abort();
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
    target: DhtTarget,
    resolved_addrs: Option<Vec<SocketAddr>>,
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
    from: DhtTarget,
    body: Value,
}

impl Dht {
    async fn send_packet(&self, packet: &[u8], target: SocketAddr) -> std::io::Result<usize> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "DHT runtime was reconfigured",
            ));
        }
        if let Some(proxy) = &self.proxy_datagram {
            return proxy
                .send_to(packet, target)
                .await
                .map_err(|error| std::io::Error::other(error.to_string()));
        }
        match target {
            SocketAddr::V4(_) => {
                self.sock
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("DHT IPv4 socket unavailable"))?
                    .send_to(packet, target)
                    .await
            }
            SocketAddr::V6(_) => match &self.sock6 {
                Some(socket) => socket.send_to(packet, target).await,
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    "IPv6 DHT socket unavailable",
                )),
            },
        }
    }

    async fn send_packet_host(
        &self,
        packet: &[u8],
        host: &str,
        port: u16,
    ) -> std::io::Result<usize> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "DHT runtime was reconfigured",
            ));
        }
        if let Some(proxy) = &self.proxy_datagram {
            return proxy
                .send_to_host(packet, host, port)
                .await
                .map_err(|error| std::io::Error::other(error.to_string()));
        }

        let targets = lookup_host((host, port)).await?;
        let mut last_error = None;
        for target in targets {
            let result = match target {
                SocketAddr::V4(_) => {
                    self.sock
                        .as_ref()
                        .ok_or_else(|| std::io::Error::other("DHT IPv4 socket unavailable"))?
                        .send_to(packet, target)
                        .await
                }
                SocketAddr::V6(_) => match &self.sock6 {
                    Some(socket) => socket.send_to(packet, target).await,
                    None => Err(std::io::Error::new(
                        std::io::ErrorKind::AddrNotAvailable,
                        "IPv6 DHT socket unavailable",
                    )),
                },
            };
            match result {
                Ok(n) => return Ok(n),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no addresses for {host}"),
            )
        }))
    }

    async fn send_target(&self, packet: &[u8], target: &DhtTarget) -> std::io::Result<usize> {
        match target {
            DhtTarget::Addr(addr) => self.send_packet(packet, *addr).await,
            DhtTarget::Host(host, port) => self.send_packet_host(packet, host, *port).await,
        }
    }

    pub async fn current_shared() -> Option<Arc<Dht>> {
        shared_dht_cell().lock().await.dht.clone()
    }

    pub async fn shared() -> Option<Arc<Dht>> {
        if let Some(dht) = Self::current_shared().await {
            return Some(dht);
        }

        if shared_dht_cell().lock().await.proxy_requested == Some(true) {
            return None;
        }
        Self::shared_with_proxy(None).await
    }

    pub async fn shared_with_proxy(proxy: Option<risuko_http::ProxyConnector>) -> Option<Arc<Dht>> {
        Self::install_shared(proxy, false).await
    }

    pub async fn replace_shared_with_proxy(
        proxy: Option<risuko_http::ProxyConnector>,
    ) -> Option<Arc<Dht>> {
        let swap = Self::prepare_shared_with_proxy(proxy).await.ok()?;
        let next = swap.next();
        swap.commit().await;
        next
    }

    pub async fn replace_shared_with_proxy_checked(
        proxy: Option<risuko_http::ProxyConnector>,
    ) -> Result<Option<Arc<Dht>>, String> {
        let swap = Self::prepare_shared_with_proxy(proxy).await?;
        let next = swap.next();
        swap.commit().await;
        Ok(next)
    }

    pub async fn prepare_shared_with_proxy(
        proxy: Option<risuko_http::ProxyConnector>,
    ) -> Result<DhtRouteSwap, String> {
        let _install_guard = shared_dht_install_lock().lock().await;
        let requested_proxy = proxy.is_some();
        let (previous, previous_proxy_requested, previous_error) = {
            let mut guard = shared_dht_cell().lock().await;
            let snapshot = (
                guard.dht.clone(),
                guard.proxy_requested,
                guard.last_error.clone(),
            );
            guard.proxy_requested = Some(requested_proxy);
            guard.last_error = None;
            snapshot
        };
        let next = match Dht::spawn_with_proxy(proxy).await {
            Ok(dht) => {
                let warm = dht.clone();
                tokio::spawn(async move { warm.bootstrap().await });
                Some(dht)
            }
            Err(error) => {
                let message = error.to_string();
                let mut guard = shared_dht_cell().lock().await;
                guard.dht = previous.clone();
                guard.proxy_requested = previous_proxy_requested.or(Some(requested_proxy));
                guard.last_error = Some(message.clone());
                return Err(message);
            }
        };
        shared_dht_cell().lock().await.dht = next.clone();

        if let Some(previous) = previous.as_ref() {
            // A route swap may take time to rebuild the surrounding runtime;
            // stop old lookups immediately so they cannot keep announcing on
            // the previous proxy while the swap is in progress.
            previous.cancel_lookups();
        }

        Ok(DhtRouteSwap {
            previous,
            previous_proxy_requested,
            previous_error,
            next,
        })
    }

    async fn install_shared(
        proxy: Option<risuko_http::ProxyConnector>,
        force: bool,
    ) -> Option<Arc<Dht>> {
        let _install_guard = shared_dht_install_lock().lock().await;
        let requested_proxy = proxy.is_some();
        let (previous, previous_proxy_requested) = {
            let mut guard = shared_dht_cell().lock().await;
            if !force && guard.proxy_requested == Some(requested_proxy) && guard.dht.is_some() {
                return guard.dht.clone();
            }
            if !force
                && guard.proxy_requested == Some(true)
                && !requested_proxy
                && guard.dht.is_none()
            {
                return None;
            }
            let previous = guard.dht.clone();
            let previous_proxy_requested = guard.proxy_requested;
            guard.last_error = None;
            (previous, previous_proxy_requested)
        };

        let next = match Dht::spawn_with_proxy(proxy).await {
            Ok(dht) => {
                let warm = dht.clone();
                tokio::spawn(async move { warm.bootstrap().await });
                Some(dht)
            }
            Err(error) => {
                let mut guard = shared_dht_cell().lock().await;
                guard.dht = previous.clone();
                guard.proxy_requested = previous_proxy_requested.or(Some(requested_proxy));
                guard.last_error = Some(error.to_string());
                tracing::warn!(
                    proxied = requested_proxy,
                    "DHT runtime initialization failed: {error}"
                );
                None
            }
        };
        if next.is_some() {
            let mut guard = shared_dht_cell().lock().await;
            guard.proxy_requested = Some(requested_proxy);
            guard.dht = next.clone();
        }

        if next.is_some() {
            if let Some(previous) = previous {
                previous.cancel_lookups();
                previous.shutdown().await;
            }
        }
        next
    }

    pub async fn spawn() -> std::io::Result<Arc<Self>> {
        Self::spawn_with_proxy(None).await
    }

    pub async fn spawn_with_proxy(
        proxy: Option<risuko_http::ProxyConnector>,
    ) -> std::io::Result<Arc<Self>> {
        let proxy_datagram = match proxy {
            Some(connector) => {
                let has_explicit_bypass = connector
                    .udp_no_proxy()
                    .is_some_and(|matcher| !matcher.is_empty());
                let result = if has_explicit_bypass {
                    connector.bind_udp_with_bypass().await
                } else {
                    connector.bind_udp().await
                };
                Some(Arc::new(result.map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::Unsupported, error.to_string())
                })?))
            }
            None => None,
        };

        let (sock, sock6) = if proxy_datagram.is_none() {
            let sock = Some(Arc::new(UdpSocket::bind("0.0.0.0:0").await?));
            let sock6 = match UdpSocket::bind("[::]:0").await {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    tracing::debug!("dht: no ipv6 socket: {e}");
                    None
                }
            };
            (sock, sock6)
        } else {
            (None, None)
        };
        let our_id = random_id();
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(Default::default()));

        let this = Arc::new(Self {
            sock: sock.clone(),
            sock6: sock6.clone(),
            proxy_datagram: proxy_datagram.clone(),
            our_id,
            pending: pending.clone(),
            routing: Arc::new(Mutex::new(RoutingTable::new(our_id))),
            reader_handle: Mutex::new(None),
            reader6_handle: Mutex::new(None),
            lookup_handles: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
        });

        let reader_sock = sock.clone();
        let pending_reader = pending.clone();
        let reader_handle = if let Some(datagram) = proxy_datagram.clone() {
            tokio::spawn(async move { proxy_reader_loop(datagram, pending_reader).await })
        } else {
            tokio::spawn(async move {
                reader_loop(reader_sock.expect("direct DHT socket"), pending_reader).await
            })
        };
        *this.reader_handle.lock() = Some(reader_handle);

        if proxy_datagram.is_none() {
            if let Some(s6) = sock6 {
                let pending6 = pending.clone();
                let reader6_handle = tokio::spawn(async move { reader_loop(s6, pending6).await });
                *this.reader6_handle.lock() = Some(reader6_handle);
            }
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
        let handle = tokio::spawn(async move {
            let _ = tokio::time::timeout(
                budget,
                this.iterative_get_peers(info_hash, tx, announce_port),
            )
            .await;
        });
        let mut handles = self.lookup_handles.lock();
        handles.retain(|handle| !handle.is_finished());
        if self.shutdown.load(Ordering::Acquire) {
            handle.abort();
        } else {
            handles.push(handle);
        }
        rx
    }

    async fn iterative_get_peers(
        self: Arc<Self>,
        info_hash: Id20,
        peer_tx: mpsc::UnboundedSender<SocketAddr>,
        announce_port: Option<u16>,
    ) {
        let mut targets: Vec<DhtTarget> = self
            .routing
            .lock()
            .closest(&info_hash, K * 2)
            .into_iter()
            .map(DhtTarget::Addr)
            .collect();
        targets.extend(bootstrap_targets());
        if targets.is_empty() {
            tracing::debug!("dht: no bootstrap nodes available");
            return;
        }

        let mut shortlist: BTreeMap<Id20, DhtTarget> = BTreeMap::new();
        let mut queried: HashSet<DhtTarget> = HashSet::new();
        let mut peers_seen: HashSet<SocketAddr> = HashSet::new();
        let mut announce_targets: BTreeMap<Id20, (DhtTarget, Vec<u8>)> = BTreeMap::new();

        let mut seed_seen = HashSet::new();
        targets.retain(|target| seed_seen.insert(target.clone()));
        targets.truncate(MAX_ROUND_QUERIES);
        queried.extend(targets.iter().cloned());

        let mut futs: JoinSet<Option<GetPeersReply>> = JoinSet::new();
        for target in targets {
            let this = self.clone();
            futs.spawn(async move { this.query_get_peers(target, info_hash).await });
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
            let node_id = responder_id.unwrap_or_else(|| pseudo_id_target(&from));
            shortlist.insert(node_id.distance(&info_hash), from.clone());
            if responder_id.is_some() {
                if let DhtTarget::Addr(from) = &from {
                    self.routing.lock().add(node_id, *from);
                }
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
                    e.insert(DhtTarget::Addr(*naddr));
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
            let mut to_dispatch: Vec<DhtTarget> = Vec::new();
            for (_d, target) in shortlist.iter().take(K * 2) {
                let target = target.clone();
                if queried.insert(target.clone()) {
                    to_dispatch.push(target);
                    dispatched += 1;
                    if dispatched >= ALPHA {
                        break;
                    }
                }
            }
            for target in to_dispatch {
                let this = self.clone();
                futs.spawn(async move { this.query_get_peers(target, info_hash).await });
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
                let _ = self.send_target(&pkt, &addr).await;
            }
        }
    }

    async fn query_get_peers(
        self: Arc<Self>,
        target: DhtTarget,
        info_hash: Id20,
    ) -> Option<GetPeersReply> {
        let (target, resolved_addrs) = match target {
            DhtTarget::Host(host, port) if self.proxy_datagram.is_none() => {
                let addresses = lookup_host((host.as_str(), port))
                    .await
                    .ok()?
                    .filter(|address| match address {
                        SocketAddr::V4(_) => self.sock.is_some(),
                        SocketAddr::V6(_) => self.sock6.is_some(),
                    })
                    .collect::<Vec<_>>();
                if addresses.is_empty() {
                    return None;
                }
                (DhtTarget::Host(host, port), Some(addresses))
            }
            target => (target, None),
        };
        let (txn, rx, _guard) = self.register_transaction(target.clone(), resolved_addrs);
        let packet = build_get_peers(txn, &self.our_id, &info_hash);
        // `_guard` removes `txn` from `pending`
        let send_res = self.send_target(&packet, &target).await;
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
        target: DhtTarget,
        resolved_addrs: Option<Vec<SocketAddr>>,
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
                resolved_addrs,
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

fn pseudo_id_target(target: &DhtTarget) -> Id20 {
    match target {
        DhtTarget::Addr(addr) => pseudo_id(*addr),
        DhtTarget::Host(host, port) => {
            use sha1::{Digest, Sha1};
            let mut h = Sha1::new();
            h.update(host.as_bytes());
            h.update(port.to_be_bytes());
            Id20::from_slice(&h.finalize()[..20]).unwrap()
        }
    }
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
            let matches = entry
                .resolved_addrs
                .as_ref()
                .is_some_and(|addresses| addresses.contains(&from))
                || (entry.resolved_addrs.is_none()
                    && dht_targets_match(&entry.target, &DhtTarget::Addr(from)));
            if matches {
                if let Some(entry) = guard.remove(&txn) {
                    let _ = entry.tx.send(KrpcResponse {
                        from: DhtTarget::Addr(from),
                        body: msg,
                    });
                }
            }
            // Mismatch: ignore the packet, leave entry for the real responder
        }
    }
}

async fn proxy_reader_loop(sock: Arc<ProxyDatagram>, pending: Arc<Mutex<PendingMap>>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, source) = match sock.recv_from_target(&mut buf).await {
            Ok(x) => x,
            Err(error) => {
                tracing::debug!("dht proxy reader stopped: {error}");
                return;
            }
        };
        let from = match &source {
            ProxyDatagramSource::Ip(address) => DhtTarget::Addr(*address),
            ProxyDatagramSource::Host(host, port) => DhtTarget::Host(host.clone(), *port),
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
        let expected = pending.lock().get(&txn).map(|entry| entry.target.clone());
        let matches = match (&expected, &source) {
            (Some(DhtTarget::Addr(target)), ProxyDatagramSource::Host(..)) => {
                risuko_http::datagram_source_matches(&source, *target)
            }
            (Some(expected), _) => dht_targets_match(expected, &from),
            (None, _) => false,
        };
        if matches {
            let mut guard = pending.lock();
            if let Some(entry) = guard.get(&txn) {
                if expected
                    .as_ref()
                    .is_some_and(|target| target == &entry.target)
                {
                    if let Some(entry) = guard.remove(&txn) {
                        let _ = entry.tx.send(KrpcResponse { from, body: msg });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::decode_all;

    // Shared-DHT tests mutate process-wide state
    static SHARED_TEST_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
        std::sync::OnceLock::new();

    async fn reset_shared_state() {
        let previous = {
            let mut state = shared_dht_cell().lock().await;
            state.proxy_requested = None;
            state.last_error = None;
            state.dht.take()
        };
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
    }

    #[tokio::test]
    async fn proxied_dht_failure_blocks_implicit_direct_creation() {
        let lock = SHARED_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let _guard = lock.lock().await;
        reset_shared_state().await;

        // HTTP CONNECT can carry TCP but must fail DHT's UDP association
        let proxy = risuko_http::ProxyConnector::from_proxy(
            risuko_http::Proxy::all("http://127.0.0.1:8080").unwrap(),
        );
        assert!(Dht::replace_shared_with_proxy(Some(proxy)).await.is_none());
        assert!(Dht::current_shared().await.is_none());
        assert!(Dht::shared().await.is_none());

        reset_shared_state().await;
    }

    #[tokio::test]
    async fn dht_route_swap_surfaces_unavailable_http_udp_route() {
        let lock = SHARED_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let _guard = lock.lock().await;
        reset_shared_state().await;

        let previous = Dht::replace_shared_with_proxy(None)
            .await
            .expect("direct DHT should bind");
        let proxy = risuko_http::ProxyConnector::from_proxy(
            risuko_http::Proxy::all("http://127.0.0.1:8080").unwrap(),
        );

        let error = match Dht::prepare_shared_with_proxy(Some(proxy)).await {
            Ok(_) => panic!("HTTP UDP limitation must fail a checked route swap"),
            Err(error) => error,
        };
        assert!(error.to_ascii_lowercase().contains("socks5 required"));
        let restored = Dht::current_shared()
            .await
            .expect("failed route preparation must preserve the old DHT");
        assert!(Arc::ptr_eq(&restored, &previous));
        assert!(Dht::shared().await.is_some());
        drop(previous);

        reset_shared_state().await;
    }

    #[test]
    fn bootstrap_nodes_remain_host_targets_until_sent() {
        let targets = bootstrap_targets();
        assert_eq!(targets.len(), DEFAULT_BOOTSTRAP.len());
        assert!(matches!(
            targets.first(),
            Some(DhtTarget::Host(host, 6881)) if host == "router.bittorrent.com"
        ));
    }

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
                target: DhtTarget::Addr(target),
                resolved_addrs: None,
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
                target: DhtTarget::Addr(target),
                resolved_addrs: None,
                token: new_token,
            },
        );

        drop(guard);

        assert!(pending.lock().contains_key(&txn));
    }
}
