//! Durable Kad identity and contact cache

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::routing::{Contact, NodeId, RoutingTable, MAX_PERSISTED_CONTACTS};

pub const STATE_VERSION: u32 = 1;
pub const STATE_FILENAME: &str = "ed2k-kad-state.json";

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedKadState {
    pub version: u32,
    pub node_id: NodeId,
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateLoadKind {
    Existing,
    Created,
    Recovered,
}

#[derive(Debug, Clone)]
pub struct LoadedKadState {
    pub node_id: NodeId,
    pub contacts: Vec<Contact>,
    pub kind: StateLoadKind,
}

pub fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STATE_FILENAME)
}

/// Load the persisted identity and bounded contact cache; missing/corrupt/unsupported/invalid state is treated as recoverable (new identity generated, invalid contacts discarded), and only filesystem permission failures are returned as errors
pub fn load(config_dir: &Path, local_hint: Option<NodeId>) -> io::Result<LoadedKadState> {
    let path = state_path(config_dir);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LoadedKadState {
                node_id: local_hint.unwrap_or_else(NodeId::random),
                contacts: Vec::new(),
                kind: StateLoadKind::Created,
            });
        }
        Err(error) => return Err(error),
    };

    let parsed = serde_json::from_slice::<PersistedKadState>(&bytes);
    let Ok(state) = parsed else {
        return Ok(LoadedKadState {
            node_id: local_hint.unwrap_or_else(NodeId::random),
            contacts: Vec::new(),
            kind: StateLoadKind::Recovered,
        });
    };

    if state.version != STATE_VERSION || state.node_id.is_zero() {
        return Ok(LoadedKadState {
            node_id: local_hint.unwrap_or_else(NodeId::random),
            contacts: Vec::new(),
            kind: StateLoadKind::Recovered,
        });
    }

    // Contacts are validated by RoutingTable::insert (which also applies local-ID and K-bucket constraints); keep only the first bounded set so an oversized cache can't cause startup work amplification
    let mut routing = RoutingTable::new(state.node_id);
    for contact in state.contacts.into_iter().take(MAX_PERSISTED_CONTACTS) {
        let _ = routing.insert(contact);
    }
    Ok(LoadedKadState {
        node_id: state.node_id,
        contacts: routing.contacts(),
        kind: StateLoadKind::Existing,
    })
}

pub fn load_or_create(config_dir: &Path) -> io::Result<LoadedKadState> {
    load(config_dir, None)
}

pub fn serialize(node_id: NodeId, contacts: &[Contact]) -> Result<Vec<u8>, serde_json::Error> {
    let state = PersistedKadState {
        version: STATE_VERSION,
        node_id,
        contacts: contacts
            .iter()
            .filter(|contact| contact.is_valid_for_routing(node_id))
            .take(MAX_PERSISTED_CONTACTS)
            .cloned()
            .collect(),
    };
    serde_json::to_vec_pretty(&state)
}

/// Atomically persist state via a sibling temp file and rename (atomic on supported filesystems when source and destination share a directory)
pub fn save(config_dir: &Path, node_id: NodeId, contacts: &[Contact]) -> io::Result<()> {
    fs::create_dir_all(config_dir)?;
    let path = state_path(config_dir);
    let temp_path = config_dir.join(format!(
        ".{STATE_FILENAME}.{}.{}.tmp",
        std::process::id(),
        TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let payload = serialize(node_id, contacts).map_err(io::Error::other)?;

    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(&payload)?;
        file.sync_all()?;
        fs::rename(&temp_path, &path)?;
        // Best effort directory sync; some platforms disallow opening a directory, and the durable file was already atomically replaced
        if let Ok(directory) = File::open(config_dir) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn contact(value: u8) -> Contact {
        Contact::with_times(
            [value; 16],
            SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, value), 4672),
            4662,
            8,
            20,
            20,
        )
    }

    #[test]
    fn first_run_creates_identity_and_restart_preserves_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create(dir.path()).unwrap();
        assert_ne!(first.node_id, NodeId::ZERO);
        assert_eq!(first.kind, StateLoadKind::Created);
        save(dir.path(), first.node_id, &[contact(1)]).unwrap();
        let second = load_or_create(dir.path()).unwrap();
        assert_eq!(second.node_id, first.node_id);
        assert_eq!(second.kind, StateLoadKind::Existing);
        assert_eq!(second.contacts.len(), 1);
    }

    #[test]
    fn corrupt_and_unsupported_state_recover_without_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(state_path(dir.path()), b"not json").unwrap();
        let recovered = load_or_create(dir.path()).unwrap();
        assert_eq!(recovered.kind, StateLoadKind::Recovered);
        let unsupported = PersistedKadState {
            version: STATE_VERSION + 1,
            node_id: NodeId::random(),
            contacts: Vec::new(),
        };
        std::fs::write(
            state_path(dir.path()),
            serde_json::to_vec(&unsupported).unwrap(),
        )
        .unwrap();
        let recovered = load_or_create(dir.path()).unwrap();
        assert_eq!(recovered.kind, StateLoadKind::Recovered);
    }

    #[test]
    fn save_is_bounded_and_json_has_version() {
        let dir = tempfile::tempdir().unwrap();
        let contacts: Vec<_> = (1..=250).map(contact).collect();
        save(dir.path(), NodeId::random(), &contacts).unwrap();
        let state: PersistedKadState =
            serde_json::from_slice(&std::fs::read(state_path(dir.path())).unwrap()).unwrap();
        assert_eq!(state.version, STATE_VERSION);
        assert!(state.contacts.len() <= MAX_PERSISTED_CONTACTS);
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp")));
    }

    #[test]
    fn save_discards_invalid_contacts_before_serializing() {
        let dir = tempfile::tempdir().unwrap();
        let node_id = NodeId::random();
        let contacts = vec![
            contact(1),
            Contact::with_times(
                [2; 16],
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, 4672),
                4662,
                8,
                20,
                20,
            ),
        ];

        save(dir.path(), node_id, &contacts).unwrap();
        let state: PersistedKadState =
            serde_json::from_slice(&std::fs::read(state_path(dir.path())).unwrap()).unwrap();
        assert_eq!(state.contacts.len(), 1);
        assert_eq!(state.contacts[0].id, NodeId([1; 16]));
    }
}
