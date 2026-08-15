use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use super::chunks::ChunkManager;
use super::kad::{routing::is_public_ipv4, KadLookupStatus, KadService, KadState};
use super::peer::{PeerConnection, PeerEvent};
use super::server::{ServerConnection, ServerEvent};
use super::server_list::server_list;
use super::types::*;

const MAX_ACTIVE_PEER_DIALS: usize = 64;
const MAX_PENDING_PEER_SOURCES: usize = 300;
const SERVER_FALLBACK_WAIT: Duration = Duration::from_secs(30);
const KAD_FALLBACK_WAIT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceOrigin {
    Link,
    Server,
    Kad,
}

#[derive(Debug, Clone, Copy)]
struct SourceCandidate {
    addr: SocketAddrV4,
    client_id: u32,
    server_ip: u32,
    server_port: u16,
    origin: SourceOrigin,
}

type CandidateDispatcher =
    Arc<dyn Fn(SourceCandidate, OwnedSemaphorePermit) + Send + Sync + 'static>;

/// Per-download source gate: every discovery path (link, server, Kad) shares it so a source is never dialled twice and a Kad burst cannot spawn unbounded peer tasks
struct SourceScheduler {
    seen: parking_lot::Mutex<HashSet<SocketAddrV4>>,
    candidates: mpsc::Sender<SourceCandidate>,
    cancel_token: CancellationToken,
    pending: Arc<AtomicU32>,
}

struct DownloadWorkerGuard(CancellationToken);

impl Drop for DownloadWorkerGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn finish_kad_lookup(
    cancel: &CancellationToken,
    lookup_task: &mut Option<tokio::task::JoinHandle<()>>,
) {
    cancel.cancel();
    if let Some(task) = lookup_task.take() {
        let _ = task.await;
    }
}

fn fallback_wait(kad_lookup_running: bool) -> Duration {
    if kad_lookup_running {
        KAD_FALLBACK_WAIT
    } else {
        SERVER_FALLBACK_WAIT
    }
}

async fn collect_kad_sources(
    mut sources: mpsc::Receiver<super::kad::KadSource>,
    mut status: tokio::sync::watch::Receiver<KadLookupStatus>,
    completion: tokio::task::JoinHandle<Result<(), super::kad::KadError>>,
    scheduler: Arc<SourceScheduler>,
    kad_status: Arc<parking_lot::Mutex<Option<KadLookupStatus>>>,
    local_client_hash: [u8; 16],
) {
    let mut sources_open = true;
    let mut status_open = true;
    while sources_open || status_open {
        tokio::select! {
            source = sources.recv(), if sources_open => match source {
                Some(source) => {
                    // Kad source IDs are ED2K client hashes, not Kad node IDs; filter our own advertised source here where the per-download ED2K identity is available
                    if source.client_hash == local_client_hash {
                        continue;
                    }
                    scheduler.submit(SourceCandidate {
                        addr: source.addr,
                        client_id: 0,
                        server_ip: 0,
                        server_port: 0,
                        origin: SourceOrigin::Kad,
                    });
                }
                None => sources_open = false,
            },
            changed = status.changed(), if status_open => {
                if changed.is_ok() {
                    *kad_status.lock() = Some(status.borrow().clone());
                } else {
                    // The lookup task drops its watch sender as it finishes; buffered source records can still be waiting, so keep the source branch alive until drained
                    status_open = false;
                }
            }
        }
    }
    let _ = completion.await;
    *kad_status.lock() = Some(status.borrow().clone());
}

impl SourceScheduler {
    fn new(
        client_hash: [u8; 16],
        client_port: u16,
        file_hash: [u8; 16],
        chunks: Arc<Mutex<ChunkManager>>,
        completed: Arc<AtomicU64>,
        cancel_token: CancellationToken,
        peer_count: Arc<AtomicU32>,
    ) -> Arc<Self> {
        let peer_cancel = cancel_token.clone();
        let dispatcher: CandidateDispatcher = Arc::new(move |candidate, permit| {
            spawn_peer_task_with_permit(
                candidate.addr,
                client_hash,
                candidate.client_id,
                client_port,
                candidate.server_ip,
                candidate.server_port,
                file_hash,
                chunks.clone(),
                completed.clone(),
                peer_cancel.clone(),
                peer_count.clone(),
                permit,
            );
        });
        Self::with_dispatcher(
            cancel_token,
            MAX_PENDING_PEER_SOURCES,
            MAX_ACTIVE_PEER_DIALS,
            dispatcher,
        )
    }

    fn with_dispatcher(
        cancel_token: CancellationToken,
        queue_capacity: usize,
        max_active_dials: usize,
        dispatcher: CandidateDispatcher,
    ) -> Arc<Self> {
        let permits = Arc::new(Semaphore::new(max_active_dials));
        let (candidates, mut pending_candidates) = mpsc::channel::<SourceCandidate>(queue_capacity);
        let dispatcher_permits = permits.clone();
        let dispatcher_cancel = cancel_token.clone();
        let pending = Arc::new(AtomicU32::new(0));
        let dispatcher_pending = pending.clone();
        tokio::spawn(async move {
            loop {
                let candidate = tokio::select! {
                    _ = dispatcher_cancel.cancelled() => break,
                    candidate = pending_candidates.recv() => candidate,
                };
                let Some(candidate) = candidate else {
                    break;
                };
                let permit = tokio::select! {
                    _ = dispatcher_cancel.cancelled() => {
                        dispatcher_pending.fetch_sub(1, Ordering::Relaxed);
                        break;
                    }
                    permit = dispatcher_permits.clone().acquire_owned() => match permit {
                        Ok(permit) => permit,
                        Err(_) => {
                            dispatcher_pending.fetch_sub(1, Ordering::Relaxed);
                            break;
                        }
                    },
                };
                // Cancellation and permit availability can become ready in the same select round; do not launch a new dial after the download was already cancelled
                if dispatcher_cancel.is_cancelled() {
                    drop(permit);
                    dispatcher_pending.fetch_sub(1, Ordering::Relaxed);
                    break;
                }
                // Keep the candidate pending until its dial task is registered, so the download fallback can't decide there is no work between dequeue and dispatch
                dispatcher_pending.fetch_sub(1, Ordering::Relaxed);
                dispatcher(candidate, permit);
            }
        });

        Arc::new(Self {
            seen: parking_lot::Mutex::new(HashSet::new()),
            candidates,
            cancel_token,
            pending,
        })
    }

    fn submit(&self, candidate: SourceCandidate) -> bool {
        let usable_addr = if candidate.origin == SourceOrigin::Kad {
            usable_source_addr(candidate.addr)
        } else {
            candidate.addr.port() != 0
        };
        if self.cancel_token.is_cancelled() || !usable_addr {
            return false;
        }
        let mut seen = self.seen.lock();
        if !seen.insert(candidate.addr) {
            return false;
        }
        self.pending.fetch_add(1, Ordering::Relaxed);
        if self.candidates.try_send(candidate).is_ok() {
            return true;
        }

        // A candidate that could not be queued was never dialled, so allow a later discovery response to submit it again once capacity frees up
        self.pending.fetch_sub(1, Ordering::Relaxed);
        seen.remove(&candidate.addr);
        false
    }

    fn is_idle(&self) -> bool {
        self.pending.load(Ordering::Relaxed) == 0
    }
}

fn usable_source_addr(addr: SocketAddrV4) -> bool {
    addr.port() != 0 && is_public_ipv4(*addr.ip())
}

/// Run an ed2k download to completion or cancellation; like `http::run_http_download_multi`, updates atomic progress counters, returns Ok(final_path)/Err, and checks `cancel_token` for pause/stop
pub async fn run_ed2k_download(
    file_link: &Ed2kFileLink,
    dir: &str,
    ed2k_servers: Vec<String>,
    client_port: u16,
    kad_udp_port: Option<u16>,
    kad: Option<Arc<KadService>>,
    kad_status: Arc<parking_lot::Mutex<Option<KadLookupStatus>>>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    // Keep ED2K's detached source dispatcher and peer tasks in a child scope; external pause/stop still propagates from `cancel_token`, while a normal worker return also stops orphaned work
    let worker_cancel = cancel_token.child_token();
    let _worker_guard = DownloadWorkerGuard(worker_cancel.clone());
    let file_hash = file_link.file_hash_bytes;
    let safe = crate::engine::util::safe_filename(&file_link.file_name, "ed2k-download");
    let file_path = PathBuf::from(dir).join(safe);

    if file_link.file_size > u32::MAX as u64 {
        return Err(format!(
            "ed2k file too large ({} bytes): files over 4 GiB require the 64-bit \
             large-file extension, which is not supported",
            file_link.file_size
        ));
    }

    total.store(file_link.file_size, Ordering::Relaxed);

    let chunks = ChunkManager::new(file_path.clone(), file_link.file_size);
    chunks.init_file().await?;

    let chunks = Arc::new(Mutex::new(chunks));
    let peer_count = Arc::new(AtomicU32::new(0));

    let client_hash: [u8; 16] = rand::random();
    let scheduler = SourceScheduler::new(
        client_hash,
        client_port,
        file_hash,
        chunks.clone(),
        completed.clone(),
        worker_cancel.clone(),
        peer_count.clone(),
    );

    let servers = server_list(&ed2k_servers);

    // Add sources from the ed2k link itself
    let link_sources: Vec<(u32, u16)> = file_link
        .sources
        .iter()
        .filter_map(|s| {
            let ip: std::net::Ipv4Addr = s.ip.parse().ok()?;
            let octets = ip.octets();
            let ip_le = u32::from_le_bytes(octets);
            Some((ip_le, s.port))
        })
        .collect();

    // Connect to link-embedded sources immediately (they work without a server)
    for &(ip, port) in &link_sources {
        if is_high_id(ip) {
            let peer_addr = SocketAddrV4::new(client_id_to_ip(ip), port);
            scheduler.submit(SourceCandidate {
                addr: peer_addr,
                client_id: 0,
                server_ip: 0,
                server_port: 0,
                origin: SourceOrigin::Link,
            });
        }
    }

    // Kad is a discovery sidecar: it shares the download cancellation token but never determines whether the ED2K download itself succeeds
    let kad_cancel = worker_cancel.child_token();
    let mut kad_task = kad.map(|service| {
        let lookup = service.lookup_sources_for_client(
            file_hash,
            file_link.file_size,
            client_hash,
            kad_cancel.clone(),
        );
        let scheduler = scheduler.clone();
        let kad_status = kad_status.clone();
        tokio::spawn(async move {
            let (sources, status, completion) = lookup.into_parts();
            collect_kad_sources(
                sources,
                status,
                completion,
                scheduler,
                kad_status,
                client_hash,
            )
            .await;
        })
    });
    if kad_task.is_none() {
        let mut status = kad_status.lock();
        if status.is_none() {
            *status = Some(KadLookupStatus {
                state: KadState::Disabled,
                ..KadLookupStatus::default()
            });
        }
    }

    let mut progress_tick = interval(Duration::from_secs(1));
    let mut prev_completed: u64 = 0;

    // Outer loop: try each server, reconnect on disconnect
    let mut last_error = String::from("No servers available");

    for entry in &servers {
        if cancel_token.is_cancelled() {
            finish_kad_lookup(&kad_cancel, &mut kad_task).await;
            return Err("cancelled".to_string());
        }

        // Check completion before trying another server
        {
            let cm = chunks.lock().await;
            if cm.is_complete() {
                finish_kad_lookup(&kad_cancel, &mut kad_task).await;
                return Ok(file_path);
            }
        }

        let addr = match entry.to_socket_addr() {
            Some(a) => a,
            None => continue,
        };

        tracing::info!("[ed2k] Trying server {} ({})", entry.name, addr);
        let mut conn = ServerConnection::new(addr, client_hash, client_port, kad_udp_port);
        let (event_rx, _packet_tx) = match conn.connect().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("[ed2k] Failed to connect to {}: {}", entry.name, e);
                last_error = format!("Failed to connect to {}: {}", entry.name, e);
                continue;
            }
        };
        tracing::info!("[ed2k] Connected to server {}", entry.name);

        let server_ip = u32::from_le_bytes(addr.ip().octets());
        let server_port_val = addr.port();
        connections.store(1, Ordering::Relaxed);

        // Run the event loop for this server connection
        match run_server_session(
            &conn,
            event_rx,
            server_ip,
            server_port_val,
            file_hash,
            &file_path,
            &chunks,
            &completed,
            &speed,
            &worker_cancel,
            &peer_count,
            &connections,
            &scheduler,
            &mut progress_tick,
            &mut prev_completed,
        )
        .await
        {
            Ok(path) => {
                finish_kad_lookup(&kad_cancel, &mut kad_task).await;
                return Ok(path);
            }
            Err(e) if e == "cancelled" => {
                finish_kad_lookup(&kad_cancel, &mut kad_task).await;
                return Err(e);
            }
            Err(e) => {
                tracing::warn!("[ed2k] Server {} session ended: {}", entry.name, e);
                last_error = e;
                // Continue to next server
            }
        }
    }

    // All servers exhausted — wait for peers if any are active
    tracing::info!("[ed2k] All servers tried, waiting for active peers to finish");
    let fallback_wait = fallback_wait(kad_task.as_ref().is_some_and(|task| !task.is_finished()));
    let deadline = tokio::time::Instant::now() + fallback_wait;
    loop {
        if cancel_token.is_cancelled() {
            finish_kad_lookup(&kad_cancel, &mut kad_task).await;
            return Err("cancelled".to_string());
        }
        {
            let cm = chunks.lock().await;
            if cm.is_complete() {
                finish_kad_lookup(&kad_cancel, &mut kad_task).await;
                return Ok(file_path);
            }
        }
        let kad_finished = kad_task.as_ref().is_none_or(|task| task.is_finished());
        if (peer_count.load(Ordering::Relaxed) == 0 && scheduler.is_idle() && kad_finished)
            || tokio::time::Instant::now() >= deadline
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Final completion check
    {
        let cm = chunks.lock().await;
        if cm.is_complete() {
            finish_kad_lookup(&kad_cancel, &mut kad_task).await;
            return Ok(file_path);
        }
    }

    finish_kad_lookup(&kad_cancel, &mut kad_task).await;
    Err(last_error)
}

/// Run the event loop for a single server connection: Ok(path) on completion, Err on disconnect or failure
async fn run_server_session(
    server: &ServerConnection,
    mut event_rx: tokio::sync::mpsc::Receiver<ServerEvent>,
    server_ip: u32,
    server_port_val: u16,
    file_hash: [u8; 16],
    file_path: &Path,
    chunks: &Arc<Mutex<ChunkManager>>,
    completed: &Arc<AtomicU64>,
    speed: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
    peer_count: &Arc<AtomicU32>,
    connections: &Arc<AtomicU32>,
    scheduler: &Arc<SourceScheduler>,
    progress_tick: &mut tokio::time::Interval,
    prev_completed: &mut u64,
) -> Result<PathBuf, String> {
    let mut got_id = false;
    let mut sources_requested = false;
    let mut client_id: u32 = 0;
    let mut source_check = interval(Duration::from_secs(30));

    loop {
        if cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        // Check completion
        {
            let cm = chunks.lock().await;
            if cm.is_complete() {
                return Ok(file_path.to_path_buf());
            }
        }

        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Err("cancelled".to_string());
            }
            event = event_rx.recv() => {
                match event {
                    Some(ServerEvent::Connected { client_id: cid }) => {
                        tracing::info!("[ed2k] Got client ID: {} ({})",
                            cid,
                            if is_high_id(cid) { "High" } else { "Low" }
                        );
                        got_id = true;
                        client_id = cid;
                        server.request_sources(&file_hash).await?;
                        sources_requested = true;
                    }
                    Some(ServerEvent::FoundSources { file_hash: fh, sources }) => {
                        if fh == file_hash {
                            tracing::info!("[ed2k] Found {} sources", sources.len());
                            for &(ip, port) in &sources {
                                if !is_high_id(ip) {
                                    continue;
                                }
                                let peer_addr = SocketAddrV4::new(client_id_to_ip(ip), port);
                                scheduler.submit(SourceCandidate {
                                    addr: peer_addr,
                                    client_id,
                                    server_ip,
                                    server_port: server_port_val,
                                    origin: SourceOrigin::Server,
                                });
                            }
                        }
                    }
                    Some(ServerEvent::ServerMessage(msg)) => {
                        tracing::info!("[ed2k] Server message: {}", msg);
                    }
                    Some(ServerEvent::ServerStatus { users, files }) => {
                        tracing::info!("[ed2k] Server: {} users, {} files", users, files);
                    }
                    Some(ServerEvent::ServerList) => {}
                    Some(ServerEvent::Disconnected(reason)) => {
                        tracing::warn!("[ed2k] Server disconnected: {:?}", reason);
                        return Err(format!("Server disconnected: {:?}", reason));
                    }
                    None => {
                        return Err("Server event channel closed".to_string());
                    }
                }
            }
            _ = source_check.tick() => {
                if got_id && sources_requested {
                    let _ = server.request_sources(&file_hash).await;
                }
            }
            _ = progress_tick.tick() => {
                let cm = chunks.lock().await;
                let comp = cm.completed_length();
                let delta = comp.saturating_sub(*prev_completed);
                *prev_completed = comp;
                completed.store(comp, Ordering::Relaxed);
                speed.store(delta, Ordering::Relaxed);
                connections.store(1 + peer_count.load(Ordering::Relaxed), Ordering::Relaxed);
            }
        }
    }
}

fn spawn_peer_task_with_permit(
    addr: SocketAddrV4,
    client_hash: [u8; 16],
    client_id: u32,
    client_port: u16,
    server_ip: u32,
    server_port: u16,
    file_hash: [u8; 16],
    chunks: Arc<Mutex<ChunkManager>>,
    completed: Arc<AtomicU64>,
    cancel_token: CancellationToken,
    peer_count: Arc<AtomicU32>,
    permit: OwnedSemaphorePermit,
) {
    peer_count.fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        let _permit = permit;
        let result = run_peer_download(
            addr,
            client_hash,
            client_id,
            client_port,
            server_ip,
            server_port,
            &file_hash,
            &chunks,
            &completed,
            &cancel_token,
        )
        .await;
        peer_count.fetch_sub(1, Ordering::Relaxed);

        if let Err(e) = result {
            tracing::debug!("[ed2k] Peer {} finished: {}", addr, e);
        }
    });
}

/// Handle a single peer connection: handshake, request file, download chunks
async fn run_peer_download(
    addr: SocketAddrV4,
    client_hash: [u8; 16],
    client_id: u32,
    client_port: u16,
    server_ip: u32,
    server_port: u16,
    file_hash: &[u8; 16],
    chunks: &Arc<Mutex<ChunkManager>>,
    completed: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
) -> Result<(), String> {
    let mut peer = PeerConnection::new(
        addr,
        client_hash,
        client_id,
        client_port,
        server_ip,
        server_port,
    );
    let (mut event_rx, _packet_tx) = tokio::select! {
        _ = cancel_token.cancelled() => return Err("cancelled".to_string()),
        result = peer.connect() => result?,
    };

    let mut got_hello = false;
    let mut got_slot = false;

    loop {
        if cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        let event = tokio::select! {
            _ = cancel_token.cancelled() => return Err("cancelled".to_string()),
            event = event_rx.recv() => event,
        };
        match event {
            Some(PeerEvent::HelloAnswer) => {
                got_hello = true;
                peer.request_file(file_hash).await?;
                peer.request_file_status(file_hash).await?;
                peer.request_hashset(file_hash).await?;
            }
            Some(PeerEvent::FileStatus {
                file_hash: fh,
                parts,
            }) => {
                if fh == *file_hash {
                    let needs = {
                        let cm = chunks.lock().await;
                        cm.next_needed_chunk_excluding(&parts, &[]).is_some()
                    };
                    if needs && got_hello {
                        peer.request_slot(file_hash).await?;
                    } else {
                        return Ok(()); // Peer has nothing we need
                    }
                }
            }
            Some(PeerEvent::HashsetAnswer {
                file_hash: fh,
                hashes,
            }) => {
                if fh == *file_hash {
                    chunks.lock().await.set_chunk_hashes(hashes);
                }
            }
            Some(PeerEvent::SlotGiven) => {
                got_slot = true;
                let ranges = {
                    let cm = chunks.lock().await;
                    collect_needed_ranges(&cm, 3)
                };
                if !ranges.is_empty() {
                    peer.request_parts(file_hash, &ranges).await?;
                }
            }
            Some(PeerEvent::DataReceived { start, data, .. }) => {
                let (is_complete, ranges) = {
                    let mut cm = chunks.lock().await;
                    cm.write_data(start as u64, &data).await?;
                    let comp = cm.completed_length();
                    completed.store(comp, Ordering::Relaxed);

                    if cm.is_complete() {
                        (true, vec![])
                    } else if got_slot {
                        (false, collect_needed_ranges(&cm, 3))
                    } else {
                        (false, vec![])
                    }
                };

                if is_complete {
                    return Ok(());
                }
                if !ranges.is_empty() {
                    peer.request_parts(file_hash, &ranges).await?;
                }
            }
            Some(PeerEvent::SlotTaken) => {
                got_slot = false;
                tokio::select! {
                    _ = cancel_token.cancelled() => return Err("cancelled".to_string()),
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                }
                peer.request_slot(file_hash).await?;
            }
            Some(PeerEvent::QueueRanking(rank)) => {
                tracing::debug!("[ed2k] Peer {} queue rank: {}", addr, rank);
            }
            Some(PeerEvent::Disconnected(reason)) => {
                return Err(format!("disconnected: {:?}", reason));
            }
            None => return Ok(()),
        }
    }
}

fn collect_needed_ranges(cm: &ChunkManager, max: usize) -> Vec<(u32, u32)> {
    let mut ranges = Vec::with_capacity(max);
    let mut picked: Vec<u64> = Vec::with_capacity(max);
    for _ in 0..max {
        if let Some(idx) = cm.next_needed_chunk_excluding(&[], &picked) {
            picked.push(idx);
            let (s, e) = cm.chunk_range(idx);
            ranges.push((s as u32, e as u32));
        } else {
            break;
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Semaphore as TestGate;
    use tokio::time::timeout;

    fn source_candidate(addr: SocketAddrV4, client_id: u32, server_ip: u32) -> SourceCandidate {
        source_candidate_from(SourceOrigin::Server, addr, client_id, server_ip)
    }

    fn source_candidate_from(
        origin: SourceOrigin,
        addr: SocketAddrV4,
        client_id: u32,
        server_ip: u32,
    ) -> SourceCandidate {
        SourceCandidate {
            addr,
            client_id,
            server_ip,
            server_port: 4661,
            origin,
        }
    }

    fn public_addr(index: usize) -> SocketAddrV4 {
        let third = (index / 254 + 1) as u8;
        let fourth = (index % 254 + 1) as u8;
        SocketAddrV4::new(Ipv4Addr::new(8, 8, third, fourth), 4000 + index as u16)
    }

    fn recording_dispatcher() -> (
        CandidateDispatcher,
        tokio::sync::mpsc::UnboundedReceiver<SourceCandidate>,
    ) {
        let (started_tx, started_rx) = mpsc::unbounded_channel();
        let dispatcher: CandidateDispatcher = Arc::new(move |candidate, permit| {
            let _ = started_tx.send(candidate);
            drop(permit);
        });
        (dispatcher, started_rx)
    }

    fn gated_dispatcher(
        gate: Arc<TestGate>,
        active: Arc<AtomicUsize>,
    ) -> (
        CandidateDispatcher,
        tokio::sync::mpsc::UnboundedReceiver<SourceCandidate>,
    ) {
        let (started_tx, started_rx) = mpsc::unbounded_channel();
        let dispatcher: CandidateDispatcher = Arc::new(move |candidate, permit| {
            active.fetch_add(1, Ordering::SeqCst);
            let _ = started_tx.send(candidate);
            let gate = gate.clone();
            let active = active.clone();
            tokio::spawn(async move {
                let _gate_permit = gate
                    .acquire_owned()
                    .await
                    .expect("test gate should remain open");
                active.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
            });
        });
        (dispatcher, started_rx)
    }

    #[tokio::test]
    async fn finish_kad_lookup_cancels_and_joins_the_collector() {
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let mut task = Some(tokio::spawn(async move {
            task_cancel.cancelled().await;
            let _ = finished_tx.send(());
        }));

        finish_kad_lookup(&cancel, &mut task).await;

        assert!(cancel.is_cancelled());
        assert!(task.is_none());
        finished_rx.await.expect("collector completed");
    }

    #[test]
    fn fallback_wait_does_not_extend_after_kad_lookup_finishes() {
        assert_eq!(fallback_wait(false), SERVER_FALLBACK_WAIT);
    }

    #[test]
    fn fallback_wait_extends_only_while_kad_lookup_is_running() {
        assert_eq!(fallback_wait(true), KAD_FALLBACK_WAIT);
    }

    #[tokio::test]
    async fn kad_collector_drains_buffered_sources_after_status_closes() {
        let cancel = CancellationToken::new();
        let (dispatcher, mut started) = recording_dispatcher();
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 1, 1, dispatcher);
        let (source_tx, source_rx) = mpsc::channel(1);
        let (status_tx, status_rx) = tokio::sync::watch::channel(KadLookupStatus::default());
        let addr = public_addr(1);
        source_tx
            .send(super::super::kad::KadSource {
                client_hash: [1; 16],
                addr,
                source_type: 1,
            })
            .await
            .expect("source receiver should be open");
        drop(source_tx);
        drop(status_tx);

        let completion = tokio::spawn(async { Ok::<(), super::super::kad::KadError>(()) });
        let kad_status = Arc::new(parking_lot::Mutex::new(None));
        collect_kad_sources(
            source_rx, status_rx, completion, scheduler, kad_status, [9; 16],
        )
        .await;

        assert_eq!(
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("buffered Kad source should be dispatched")
                .expect("dispatcher should stay open")
                .addr,
            addr
        );
        cancel.cancel();
    }

    #[test]
    fn download_worker_guard_cancels_only_its_child_scope() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        {
            let _guard = DownloadWorkerGuard(child.clone());
        }

        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
    }

    #[tokio::test]
    async fn unavailable_kad_preserves_the_initial_failure_diagnostic() {
        let directory = tempfile::tempdir().unwrap();
        let link = super::super::parse_ed2k_link(
            "ed2k://|file|test.bin|1|0123456789abcdef0123456789abcdef|/",
        )
        .unwrap();
        let kad_status = Arc::new(parking_lot::Mutex::new(Some(KadLookupStatus {
            state: KadState::Error,
            queried_nodes: 0,
            discovered_sources: 0,
            contacts: 0,
            error: Some("UDP port is already in use".into()),
        })));

        let result = run_ed2k_download(
            &link,
            directory.path().to_str().unwrap(),
            vec!["127.0.0.1:1".into()],
            4662,
            None,
            None,
            kad_status.clone(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU32::new(0)),
            CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let status = kad_status.lock();
        assert_eq!(
            status.as_ref().map(|status| status.state),
            Some(KadState::Error)
        );
        assert_eq!(
            status.as_ref().and_then(|status| status.error.as_deref()),
            Some("UDP port is already in use")
        );
    }

    #[tokio::test]
    async fn source_scheduler_deduplicates_link_server_and_kad_candidates() {
        let cancel = CancellationToken::new();
        let (dispatcher, mut started) = recording_dispatcher();
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 8, 64, dispatcher);
        let addr = public_addr(1);

        assert!(scheduler.submit(source_candidate_from(SourceOrigin::Link, addr, 0, 0)));
        // Same endpoint learned from server and Kad; metadata must not defeat endpoint-level deduplication
        assert!(!scheduler.submit(source_candidate_from(
            SourceOrigin::Server,
            addr,
            123,
            0x0102_0304,
        )));
        assert!(!scheduler.submit(source_candidate_from(SourceOrigin::Kad, addr, 0, 0)));

        let dispatched = timeout(Duration::from_secs(1), started.recv())
            .await
            .expect("link source should be dispatched")
            .expect("dispatcher should stay open");
        assert_eq!(dispatched.addr, addr);
        assert!(timeout(Duration::from_millis(25), started.recv())
            .await
            .is_err());

        cancel.cancel();
    }

    #[tokio::test]
    async fn source_scheduler_rejects_non_global_kad_endpoints_only() {
        let cancel = CancellationToken::new();
        let (dispatcher, mut started) = recording_dispatcher();
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 8, 64, dispatcher);
        let invalid = [
            SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 4662),
            SocketAddrV4::new(Ipv4Addr::new(0, 1, 2, 3), 4662),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 4662),
            SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(172, 16, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(192, 168, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(192, 0, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(192, 88, 99, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(198, 18, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(198, 19, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(240, 0, 0, 1), 4662),
            SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 0),
        ];

        for addr in invalid {
            assert!(
                !usable_source_addr(addr),
                "unexpected usable endpoint {addr}"
            );
            assert!(!scheduler.submit(source_candidate_from(SourceOrigin::Kad, addr, 0, 0)));
        }
        assert!(usable_source_addr(SocketAddrV4::new(
            Ipv4Addr::new(8, 8, 8, 8),
            4662,
        )));

        let link_addr = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 4662);
        let server_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 4663);
        assert!(scheduler.submit(source_candidate_from(SourceOrigin::Link, link_addr, 0, 0,)));
        assert!(scheduler.submit(source_candidate_from(
            SourceOrigin::Server,
            server_addr,
            1,
            0x0102_0304,
        )));

        let dispatched = [
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("link source should be dispatched")
                .expect("dispatcher should stay open")
                .addr,
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("server source should be dispatched")
                .expect("dispatcher should stay open")
                .addr,
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(dispatched, HashSet::from([link_addr, server_addr]));
        cancel.cancel();
    }

    #[tokio::test]
    async fn source_scheduler_allows_retry_after_queue_full() {
        let cancel = CancellationToken::new();
        let gate = Arc::new(TestGate::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let (dispatcher, mut started) = gated_dispatcher(gate.clone(), active.clone());
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 1, 1, dispatcher);
        let first = source_candidate(public_addr(1), 0, 0);

        assert!(scheduler.submit(first));
        assert_eq!(
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("first candidate should start")
                .expect("dispatcher should stay open")
                .addr,
            first.addr
        );
        let mut accepted = Vec::new();
        let rejected = loop {
            let candidate = source_candidate(public_addr(accepted.len() + 2), 0, 0);
            if scheduler.submit(candidate) {
                accepted.push(candidate);
            } else {
                break candidate;
            }
        };
        assert!(!accepted.is_empty());

        // Release the first active dial, then drain every candidate admitted before the bounded queue reported full
        gate.add_permits(1);
        let accepted_addresses: HashSet<SocketAddrV4> =
            accepted.iter().map(|candidate| candidate.addr).collect();
        for _ in &accepted {
            let started_candidate = timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("queued candidate should start after permit release")
                .expect("dispatcher should stay open");
            assert!(accepted_addresses.contains(&started_candidate.addr));
            gate.add_permits(1);
        }
        timeout(Duration::from_secs(1), async {
            while active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all accepted candidates should release their permits");

        // The failed enqueue must not poison `seen`; a later response can retry the candidate once queue capacity is available
        assert!(scheduler.submit(rejected));
        gate.add_permits(1);
        assert_eq!(
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("retried candidate should start")
                .expect("dispatcher should stay open")
                .addr,
            rejected.addr
        );
        gate.add_permits(1);
        cancel.cancel();
    }

    #[tokio::test]
    async fn source_scheduler_caps_active_dials_at_64() {
        let cancel = CancellationToken::new();
        let gate = Arc::new(TestGate::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let (dispatcher, mut started) = gated_dispatcher(gate.clone(), active.clone());
        let scheduler = SourceScheduler::with_dispatcher(
            cancel.clone(),
            MAX_ACTIVE_PEER_DIALS + 1,
            MAX_ACTIVE_PEER_DIALS,
            dispatcher,
        );

        for index in 0..=MAX_ACTIVE_PEER_DIALS {
            assert!(scheduler.submit(source_candidate(public_addr(index), 0, 0)));
        }
        for _ in 0..MAX_ACTIVE_PEER_DIALS {
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("each of the first 64 candidates should start")
                .expect("dispatcher should stay open");
        }
        assert_eq!(active.load(Ordering::SeqCst), MAX_ACTIVE_PEER_DIALS);
        assert!(timeout(Duration::from_millis(25), started.recv())
            .await
            .is_err());

        gate.add_permits(MAX_ACTIVE_PEER_DIALS);
        let overflow = timeout(Duration::from_secs(1), started.recv())
            .await
            .expect("the 65th candidate should start after a permit releases")
            .expect("dispatcher should stay open");
        assert_eq!(overflow.addr, public_addr(MAX_ACTIVE_PEER_DIALS));
        gate.add_permits(1);
        cancel.cancel();
    }

    #[tokio::test]
    async fn source_scheduler_cancellation_drops_waiting_candidates() {
        let cancel = CancellationToken::new();
        let gate = Arc::new(TestGate::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let (dispatcher, mut started) = gated_dispatcher(gate.clone(), active);
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 2, 1, dispatcher);
        let first = source_candidate(public_addr(1), 0, 0);
        let waiting = source_candidate(public_addr(2), 0, 0);

        assert!(scheduler.submit(first));
        assert_eq!(
            timeout(Duration::from_secs(1), started.recv())
                .await
                .expect("first candidate should start")
                .expect("dispatcher should stay open")
                .addr,
            first.addr
        );
        assert!(scheduler.submit(waiting));

        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(matches!(
            timeout(Duration::from_millis(100), started.recv()).await,
            Err(_) | Ok(None)
        ));
        gate.add_permits(1);
    }

    #[tokio::test]
    async fn source_scheduler_keeps_queued_work_visible_until_dispatch() {
        let cancel = CancellationToken::new();
        let (dispatcher, _started) = recording_dispatcher();
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 1, 0, dispatcher);

        assert!(scheduler.submit(source_candidate(public_addr(1), 0, 0)));
        assert!(!scheduler.is_idle());
        tokio::task::yield_now().await;
        assert!(!scheduler.is_idle());

        cancel.cancel();
    }

    #[tokio::test]
    async fn source_scheduler_dispatches_server_and_kad_sources_in_parallel() {
        let cancel = CancellationToken::new();
        let gate = Arc::new(TestGate::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let (dispatcher, mut started) = gated_dispatcher(gate.clone(), active.clone());
        let scheduler = SourceScheduler::with_dispatcher(cancel.clone(), 2, 2, dispatcher);
        let server_source = source_candidate(public_addr(1), 0x0102_0304, 0x0506_0708);
        let kad_source = source_candidate(public_addr(2), 0, 0);

        let (server_accepted, kad_accepted) = tokio::join!(
            {
                let scheduler = scheduler.clone();
                async move { scheduler.submit(server_source) }
            },
            {
                let scheduler = scheduler.clone();
                async move { scheduler.submit(kad_source) }
            },
        );
        assert!(server_accepted);
        assert!(kad_accepted);

        let first = timeout(Duration::from_secs(1), started.recv())
            .await
            .expect("server/Kad candidates should be dispatched")
            .expect("dispatcher should stay open");
        let second = timeout(Duration::from_secs(1), started.recv())
            .await
            .expect("server/Kad candidates should be dispatched in parallel")
            .expect("dispatcher should stay open");
        let addresses = [first.addr, second.addr]
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(addresses.len(), 2);
        assert_eq!(active.load(Ordering::SeqCst), 2);

        gate.add_permits(2);
        cancel.cancel();
    }
}
