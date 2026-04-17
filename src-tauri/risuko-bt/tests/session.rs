//! Session behaviour tests: add_torrent dedup and shutdown semantics.

use std::path::PathBuf;

use risuko_bt::bencode::{encode_to_vec, Value};
use risuko_bt::session::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, SessionOptions,
};

fn make_torrent_bytes(name: &str) -> Vec<u8> {
    let pieces = vec![0u8; 20];
    let info = Value::Dict(vec![
        (b"length".to_vec(), Value::Int(1024)),
        (b"name".to_vec(), Value::Bytes(name.as_bytes().to_vec())),
        (b"piece length".to_vec(), Value::Int(1024)),
        (b"pieces".to_vec(), Value::Bytes(pieces)),
    ]);
    let top = Value::Dict(vec![(b"info".to_vec(), info)]);
    encode_to_vec(&top)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_same_torrent_twice_returns_already_managed() {
    let tmp = tempfile::tempdir().unwrap();
    let session = Session::new_with_opts(PathBuf::from(tmp.path()), SessionOptions::default())
        .await
        .unwrap();

    let bytes = make_torrent_bytes("dedup-test");

    let first = session
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.clone().into()),
            Some(AddTorrentOptions::default()),
        )
        .await
        .expect("first add");
    let first_id = match first {
        AddTorrentResponse::Added(id, _) => id,
        other => panic!("expected Added, got {:?}", std::mem::discriminant(&other)),
    };

    let second = session
        .add_torrent(
            AddTorrent::TorrentFileBytes(bytes.into()),
            Some(AddTorrentOptions::default()),
        )
        .await
        .expect("second add");
    match second {
        AddTorrentResponse::AlreadyManaged(id, handle) => {
            assert_eq!(id, first_id);
            assert_eq!(handle.id, first_id);
        }
        _ => panic!("expected AlreadyManaged on duplicate add"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_only_returns_metadata_without_adding() {
    let tmp = tempfile::tempdir().unwrap();
    let session = Session::new_with_opts(PathBuf::from(tmp.path()), SessionOptions::default())
        .await
        .unwrap();

    let bytes = make_torrent_bytes("list-only");
    let opts = AddTorrentOptions {
        list_only: true,
        ..Default::default()
    };
    let resp = session
        .add_torrent(AddTorrent::TorrentFileBytes(bytes.into()), Some(opts))
        .await
        .expect("list-only add");

    match resp {
        AddTorrentResponse::ListOnly(r) => {
            assert_eq!(r.info.name, "list-only");
            assert_eq!(r.files.len(), 1);
        }
        _ => panic!("expected ListOnly"),
    }

    // list_only must NOT register the torrent.
    let count = session.with_torrents(|iter| iter.count());
    assert_eq!(count, 0);
}
