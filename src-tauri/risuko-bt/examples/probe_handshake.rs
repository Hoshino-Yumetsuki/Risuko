//! Diagnostic probe: dial a single peer via risuko-bt's real `connect()`
//! path and report the handshake outcome. Used to reproduce/inspect the
//! plaintext-handshake rejection seen on certain swarms.
//!
//! Usage: probe_handshake <ip:port> <info_hash_hex> [v2]

use std::time::Duration;

use risuko_bt::generate_peer_id;
use risuko_bt::peer::{connect, EncryptionPolicy, PeerCommand, PeerEvent, SpawnPeer};
use risuko_bt::wire::Message;
use risuko_bt::Id20;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let addr: std::net::SocketAddr = args
        .next()
        .expect("usage: probe_handshake <ip:port> <info_hash_hex> [v2]")
        .parse()
        .expect("valid socket addr");
    let ih_hex = args.next().expect("info_hash hex required");
    let advertise_v2 = args.next().map(|s| s == "v2").unwrap_or(false);
    // Optional 4th arg: override the local peer_id (e.g. "-qB5011-" or
    // "-rQ0011-") to test whether a peer rejects us based on our peer_id.
    let peer_id_override = args.next();

    let ih = hex::decode(ih_hex.trim()).expect("valid hex info hash");
    let info_hash = Id20::from_slice(&ih).expect("info hash must be 20 bytes");
    let our_peer_id = match peer_id_override {
        Some(prefix) => {
            let mut raw = [0u8; 20];
            let pb = prefix.as_bytes();
            let n = pb.len().min(20);
            raw[..n].copy_from_slice(&pb[..n]);
            // fill the remainder with pseudo-random-ish bytes
            for (i, b) in raw.iter_mut().enumerate().skip(n) {
                *b = (i as u8).wrapping_mul(37).wrapping_add(11);
            }
            Id20::from_slice(&raw).unwrap()
        }
        None => generate_peer_id(),
    };

    eprintln!(
        "dialing {addr} info_hash={ih_hex} advertise_v2={advertise_v2} peer_id={:02x?}",
        our_peer_id.0
    );

    let spawn = SpawnPeer {
        addr,
        info_hash,
        our_peer_id,
        connect_timeout: Duration::from_secs(10),
        read_timeout: Duration::from_secs(15),
        encryption: EncryptionPolicy::PlaintextOnly,
        advertise_v2,
        ext_handshake_builder: None,
    };

    match connect(spawn).await {
        Ok((handle, mut rx)) => {
            eprintln!("CONNECTED (tcp + handshake write ok)");
            // Mimic rqbit: eagerly send Unchoke + Interested right after the
            // handshake so the peer sees a complete, well-behaved client.
            let _ = handle.tx.send(PeerCommand::Send(Message::Unchoke)).await;
            let _ = handle.tx.send(PeerCommand::Send(Message::Interested)).await;
            eprintln!("sent Unchoke + Interested");
            let mut idle_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            loop {
                match tokio::time::timeout_at(idle_deadline, rx.recv()).await {
                    Ok(Some(PeerEvent::Handshook {
                        peer_id,
                        reserved,
                        encrypted,
                        ..
                    })) => {
                        idle_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                        eprintln!(
                            "HANDSHOOK reserved={:02x?} encrypted={encrypted} remote_peer_id={:02x?}",
                            reserved, peer_id.0
                        );
                    }
                    Ok(Some(PeerEvent::Message(m))) => {
                        idle_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                        eprintln!("MSG {m:?}")
                    }
                    Ok(Some(PeerEvent::Disconnected { reason })) => {
                        eprintln!("DISCONNECTED: {reason}");
                        break;
                    }
                    Ok(None) => {
                        eprintln!("event channel closed");
                        break;
                    }
                    Err(_) => {
                        eprintln!("idle timeout (still connected, no further events)");
                        break;
                    }
                }
            }
        }
        Err(e) => eprintln!("CONNECT FAILED: {e}"),
    }
}
