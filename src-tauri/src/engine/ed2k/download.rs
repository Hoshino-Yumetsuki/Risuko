use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use super::chunks::ChunkManager;
use super::peer::{PeerConnection, PeerEvent};
use super::server::{ServerConnection, ServerEvent};
use super::server_list::ServerList;
use super::types::*;

/// Run an ed2k download to completion or cancellation
///
/// Follows the same contract as `http::run_http_download`
/// - Updates atomic counters for progress tracking
/// - Returns Ok(final_path) on success, Err on failure
/// - Checks `cancel` flag and `cancel_token` for pause/stop
pub async fn run_ed2k_download(
    file_link: &Ed2kFileLink,
    dir: &str,
    ed2k_servers: Vec<String>,
    client_port: u16,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    let file_hash = file_link.file_hash_bytes;
    let file_path = PathBuf::from(dir).join(&file_link.file_name);

    total.store(file_link.file_size, Ordering::Relaxed);

    let chunks = ChunkManager::new(file_path.clone(), file_link.file_size);
    chunks.init_file().await?;

    // Shared mutable state for the download
    let chunks = Arc::new(Mutex::new(chunks));
    let peer_count = Arc::new(AtomicU32::new(0));

    // Generate a random client hash for this session
    let client_hash: [u8; 16] = rand::random();

    let server_list = ServerList::from_config(&ed2k_servers);
    let servers = server_list.servers().to_vec();

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
            spawn_peer_task(
                peer_addr,
                client_hash,
                0,
                client_port,
                0,
                0,
                file_hash,
                chunks.clone(),
                completed.clone(),
                cancel.clone(),
                cancel_token.clone(),
                peer_count.clone(),
                connections.clone(),
            );
        }
    }

    let mut progress_tick = interval(Duration::from_secs(1));
    let mut prev_completed: u64 = 0;

    // Outer loop: try each server, reconnect on disconnect
    let mut last_error = String::from("No servers available");

    for entry in &servers {
        if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        // Check completion before trying another server
        {
            let cm = chunks.lock().await;
            if cm.is_complete() {
                return Ok(file_path);
            }
        }

        let addr = match entry.to_socket_addr() {
            Some(a) => a,
            None => continue,
        };

        log::info!("[ed2k] Trying server {} ({})", entry.name, addr);
        let mut conn = ServerConnection::new(addr, client_hash, client_port);
        let (event_rx, _packet_tx) = match conn.connect().await {
            Ok(pair) => pair,
            Err(e) => {
                log::warn!("[ed2k] Failed to connect to {}: {}", entry.name, e);
                last_error = format!("Failed to connect to {}: {}", entry.name, e);
                continue;
            }
        };
        log::info!("[ed2k] Connected to server {}", entry.name);

        let server_ip = u32::from_le_bytes(addr.ip().octets());
        let server_port_val = addr.port();
        connections.store(1, Ordering::Relaxed);

        // Run the event loop for this server connection
        match run_server_session(
            &conn,
            event_rx,
            client_hash,
            client_port,
            server_ip,
            server_port_val,
            file_hash,
            &file_path,
            &chunks,
            &completed,
            &speed,
            &cancel,
            &cancel_token,
            &peer_count,
            &connections,
            &mut progress_tick,
            &mut prev_completed,
        )
        .await
        {
            Ok(path) => return Ok(path),
            Err(e) if e == "cancelled" => return Err(e),
            Err(e) => {
                log::warn!("[ed2k] Server {} session ended: {}", entry.name, e);
                last_error = e;
                // Continue to next server
            }
        }
    }

    // All servers exhausted — wait for peers if any are active
    log::info!("[ed2k] All servers tried, waiting for active peers to finish");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }
        {
            let cm = chunks.lock().await;
            if cm.is_complete() {
                return Ok(file_path);
            }
        }
        if peer_count.load(Ordering::Relaxed) == 0 || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Final completion check
    {
        let cm = chunks.lock().await;
        if cm.is_complete() {
            return Ok(file_path);
        }
    }

    Err(last_error)
}

/// Run the event loop for a single server connection.
/// Returns Ok(path) on completion, Err on disconnect or failure.
async fn run_server_session(
    server: &ServerConnection,
    mut event_rx: tokio::sync::mpsc::Receiver<ServerEvent>,
    client_hash: [u8; 16],
    client_port: u16,
    server_ip: u32,
    server_port_val: u16,
    file_hash: [u8; 16],
    file_path: &PathBuf,
    chunks: &Arc<Mutex<ChunkManager>>,
    completed: &Arc<AtomicU64>,
    speed: &Arc<AtomicU64>,
    cancel: &Arc<AtomicBool>,
    cancel_token: &CancellationToken,
    peer_count: &Arc<AtomicU32>,
    connections: &Arc<AtomicU32>,
    progress_tick: &mut tokio::time::Interval,
    prev_completed: &mut u64,
) -> Result<PathBuf, String> {
    let mut got_id = false;
    let mut sources_requested = false;
    let mut client_id: u32 = 0;
    let mut source_check = interval(Duration::from_secs(30));

    loop {
        if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        // Check completion
        {
            let cm = chunks.lock().await;
            if cm.is_complete() {
                return Ok(file_path.clone());
            }
        }

        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Err("cancelled".to_string());
            }
            event = event_rx.recv() => {
                match event {
                    Some(ServerEvent::Connected { client_id: cid }) => {
                        log::info!("[ed2k] Got client ID: {} ({})",
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
                            log::info!("[ed2k] Found {} sources", sources.len());
                            for &(ip, port) in &sources {
                                if !is_high_id(ip) {
                                    continue;
                                }
                                let peer_addr = SocketAddrV4::new(client_id_to_ip(ip), port);
                                spawn_peer_task(
                                    peer_addr,
                                    client_hash,
                                    client_id,
                                    client_port,
                                    server_ip,
                                    server_port_val,
                                    file_hash,
                                    chunks.clone(),
                                    completed.clone(),
                                    cancel.clone(),
                                    cancel_token.clone(),
                                    peer_count.clone(),
                                    connections.clone(),
                                );
                            }
                        }
                    }
                    Some(ServerEvent::ServerMessage(msg)) => {
                        log::info!("[ed2k] Server message: {}", msg);
                    }
                    Some(ServerEvent::ServerStatus { users, files }) => {
                        log::info!("[ed2k] Server: {} users, {} files", users, files);
                    }
                    Some(ServerEvent::ServerList) => {}
                    Some(ServerEvent::Disconnected(reason)) => {
                        log::warn!("[ed2k] Server disconnected: {:?}", reason);
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

/// Spawn a peer connection task for downloading chunks
fn spawn_peer_task(
    addr: SocketAddrV4,
    client_hash: [u8; 16],
    client_id: u32,
    client_port: u16,
    server_ip: u32,
    server_port: u16,
    file_hash: [u8; 16],
    chunks: Arc<Mutex<ChunkManager>>,
    completed: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    cancel_token: CancellationToken,
    peer_count: Arc<AtomicU32>,
    _connections: Arc<AtomicU32>,
) {
    tokio::spawn(async move {
        peer_count.fetch_add(1, Ordering::Relaxed);
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
            &cancel,
            &cancel_token,
        )
        .await;
        peer_count.fetch_sub(1, Ordering::Relaxed);

        if let Err(e) = result {
            log::debug!("[ed2k] Peer {} finished: {}", addr, e);
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
    cancel: &Arc<AtomicBool>,
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
    let (mut event_rx, _packet_tx) = peer.connect().await?;

    let mut got_hello = false;
    let mut got_slot = false;

    loop {
        if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        match event_rx.recv().await {
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
                        cm.next_needed_chunk(&parts).is_some()
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
                tokio::time::sleep(Duration::from_secs(60)).await;
                if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
                    return Err("cancelled".to_string());
                }
                peer.request_slot(file_hash).await?;
            }
            Some(PeerEvent::QueueRanking(rank)) => {
                log::debug!("[ed2k] Peer {} queue rank: {}", addr, rank);
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
    for _ in 0..max {
        if let Some(idx) = cm.next_needed_chunk(&[]) {
            let (s, e) = cm.chunk_range(idx);
            ranges.push((s as u32, e as u32));
        } else {
            break;
        }
    }
    ranges
}
