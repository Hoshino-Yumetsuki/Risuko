//! Diagnostic: drive a real risuko-bt `Session` download of a magnet and
//! print progress/peer stats periodically. Mirrors how the app downloads.
//!
//! Usage: download_magnet <magnet> [plaintext|prefer|require] [seconds]

use std::time::Duration;

use risuko_bt::session::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ListenerOptions, Session, SessionOptions,
};
use risuko_bt::EncryptionPolicy;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    env_logger::init();

    let mut args = std::env::args().skip(1);
    let magnet = args
        .next()
        .expect("usage: download_magnet <magnet> [plaintext|prefer|require] [seconds]");
    let policy = match args.next().as_deref() {
        Some("prefer") => EncryptionPolicy::Prefer,
        Some("require") => EncryptionPolicy::RequireEncryption,
        _ => EncryptionPolicy::PlaintextOnly,
    };
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(90);
    // Optional 4th arg: path to a file with one tracker URL per line.
    let trackers: Option<Vec<String>> = args.next().map(|path| {
        let body = std::fs::read_to_string(&path).expect("read trackers file");
        body.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    });
    if let Some(t) = &trackers {
        eprintln!("loaded {} trackers", t.len());
    }

    let out = std::env::temp_dir().join("risuko-cmp-dl");
    std::fs::create_dir_all(&out).unwrap();

    let session = Session::new_with_opts(
        out.clone(),
        SessionOptions {
            listen: Some(ListenerOptions {
                listen_addr: Some("0.0.0.0:0".parse().unwrap()),
                enable_upnp_port_forwarding: false,
                listen_ipv6: true,
                ..Default::default()
            }),
            encryption: policy,
            ..Default::default()
        },
    )
    .await
    .expect("session");

    eprintln!(
        "session listen_port={} policy={:?} out={}",
        session.listen_port(),
        policy,
        out.display()
    );
    eprintln!("resolving + adding magnet (may take a few seconds)...");

    let add_opts = AddTorrentOptions {
        trackers,
        ..AddTorrentOptions::default()
    };
    // If the first arg is an existing file, treat it as a .torrent (skip
    // magnet resolution); otherwise treat it as a magnet URL.
    let which = if std::path::Path::new(&magnet).is_file() {
        eprintln!("loading .torrent file (skipping magnet resolution)");
        let bytes = std::fs::read(&magnet).expect("read torrent file");
        AddTorrent::TorrentFileBytes(bytes.into())
    } else {
        AddTorrent::Url(magnet)
    };
    let handle = match session.add_torrent(which, Some(add_opts)).await {
        Ok(AddTorrentResponse::Added(_, h)) => h,
        Ok(_) => {
            eprintln!("unexpected non-Added response");
            return;
        }
        Err(e) => {
            eprintln!("add_torrent failed: {e}");
            return;
        }
    };

    eprintln!("added; polling stats for {secs}s");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let s = handle.stats();
        let live = s
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live)
            .unwrap_or(0);
        let pct = if s.total_bytes > 0 {
            s.progress_bytes as f64 / s.total_bytes as f64 * 100.0
        } else {
            0.0
        };
        eprintln!(
            "progress {} / {} bytes ({pct:.3}%) live_peers={} known_peers={} finished={}",
            s.progress_bytes,
            s.total_bytes,
            live,
            s.peers.len(),
            s.finished
        );
        if s.finished || tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    eprintln!("DONE");
}
