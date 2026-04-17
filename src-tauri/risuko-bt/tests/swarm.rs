//! End-to-end swarm test for the in-tree BitTorrent implementation.
//!
//! Spins up two `Session`s sharing a 256 KiB random payload. The seeder
//! pre-populates storage and we verify the leecher receives a byte-exact
//! copy via direct peer connection (no tracker / DHT).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use risuko_bt::core::hash::Id20;
use risuko_bt::core::metainfo::{TorrentMeta, TorrentMetaInfo, ValidatedTorrentMetaV1Info};
use risuko_bt::session::{AddTorrentOptions, AddTorrentResponse, Session, SessionOptions};
use sha1::{Digest, Sha1};

const PIECE_LEN: u32 = 64 * 1024;
const TOTAL: u64 = 256 * 1024;

fn make_payload() -> Vec<u8> {
    // Deterministic non-trivial pattern: LCG output. Not uniform random,
    // but each piece is distinct which is all we need.
    let mut out = Vec::with_capacity(TOTAL as usize);
    let mut x: u32 = 0xDEADBEEF;
    for _ in 0..TOTAL {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        out.push((x >> 24) as u8);
    }
    out
}

fn build_meta(payload: &[u8]) -> TorrentMeta {
    let pieces: Vec<u8> = payload
        .chunks(PIECE_LEN as usize)
        .flat_map(|c| {
            let mut h = Sha1::new();
            h.update(c);
            let d: [u8; 20] = h.finalize().into();
            d.into_iter()
        })
        .collect();
    let info = ValidatedTorrentMetaV1Info {
        name: "payload.bin".to_string(),
        piece_length: PIECE_LEN,
        pieces,
        private: false,
        files: vec![TorrentMetaInfo {
            path: vec!["payload.bin".to_string()],
            length: payload.len() as u64,
        }],
        single_file_mode: true,
    };
    // Build a deterministic info-hash by SHA-1ing a concat of name+length+pieces —
    // the actual value doesn't matter for peer-direct tests as long as both
    // sessions agree. Handshake compares this byte-for-byte.
    let mut h = Sha1::new();
    h.update(&info.name);
    h.update((info.files[0].length as u64).to_be_bytes());
    h.update(&info.pieces);
    let ih: [u8; 20] = h.finalize().into();
    TorrentMeta {
        info,
        announce: None,
        announce_list: Vec::new(),
        comment: None,
        created_by: None,
        creation_date: None,
        encoding: None,
        info_hash: Id20::new(ih),
    }
}

fn write_payload(dir: &std::path::Path, name: &str, payload: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), payload).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leecher_downloads_from_seeder() {
    let _ = env_logger::builder().is_test(true).try_init();

    let payload = make_payload();
    let meta = build_meta(&payload);

    let seed_dir = tempfile::tempdir().unwrap();
    let leech_dir = tempfile::tempdir().unwrap();

    // Seed side: file is already on disk so scan_existing_pieces marks them.
    write_payload(seed_dir.path(), &meta.info.name, &payload);

    let seed = Session::new_with_opts(
        seed_dir.path().to_path_buf(),
        SessionOptions {
            disable_dht: true,
            disable_dht_persistence: true,
            listen: Some(risuko_bt::session::ListenerOptions {
                listen_addr: Some("127.0.0.1:0".parse().unwrap()),
                enable_upnp_port_forwarding: false,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let leech = Session::new_with_opts(
        leech_dir.path().to_path_buf(),
        SessionOptions {
            disable_dht: true,
            disable_dht_persistence: true,
            listen: Some(risuko_bt::session::ListenerOptions {
                listen_addr: Some("127.0.0.1:0".parse().unwrap()),
                enable_upnp_port_forwarding: false,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Add to seeder.
    let _seed_handle = match seed
        .add_from_meta(meta.clone(), AddTorrentOptions::default())
        .await
        .expect("seed add")
    {
        AddTorrentResponse::Added(_, h) => h,
        _ => panic!("expected Added"),
    };

    // Add to leecher.
    let leech_handle = match leech
        .add_from_meta(meta.clone(), AddTorrentOptions::default())
        .await
        .expect("leech add")
    {
        AddTorrentResponse::Added(_, h) => h,
        _ => panic!("expected Added"),
    };

    // Dial seeder from leecher.
    let seed_addr: SocketAddr = format!("127.0.0.1:{}", seed.listen_port()).parse().unwrap();
    leech
        .add_peer(meta.info_hash, seed_addr)
        .await
        .expect("add peer");

    // Poll for completion.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let s = leech_handle.stats();
        if s.finished {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "timed out: progress={} / {} bytes (peers={})",
                s.progress_bytes,
                s.total_bytes,
                s.live.map(|l| l.snapshot.peer_stats.live).unwrap_or(0),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify byte equality.
    let got = std::fs::read(leech_dir.path().join(&meta.info.name)).unwrap();
    assert_eq!(got.len(), payload.len(), "length mismatch");
    assert_eq!(got, payload, "payload mismatch");
}

#[test]
fn sanity_bytes_type_exists() {
    // Exercise the Bytes re-export so unused-import lints don't trip.
    let _ = Bytes::from_static(b"");
    let _ = PathBuf::new();
}
