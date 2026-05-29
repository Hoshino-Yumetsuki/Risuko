//! Per-peer connection actor
//!
//! A connected peer is modelled as a pair of tokio tasks communicating over
//! mpsc channels with the torrent state machine:
//!
//!  - **reader task** reads the TCP stream, decodes `Message` frames, and
//!    forwards them to the torrent via `PeerEvent`.
//!  - **writer task** pulls `PeerCommand`s from the torrent and writes the
//!    encoded messages to the stream
//!
//! The actor owns the peer's local BitTorrent state (am_choking,
//! am_interested, peer_choking, peer_interested, peer bitfield). Higher-level
//! policy (what to request, when to unchoke) lives in `torrent::`

pub mod connection;
pub mod state;

pub use connection::{
    accept, accept_utp_plaintext, accept_with_policy, accept_with_policy_and_capabilities, connect,
    connect_utp_plaintext, connect_with_utp_fallback, EncryptionPolicy, ExtHandshakeBuilder,
    KnownInfoHash, PeerCommand, PeerEvent, PeerHandle, SpawnPeer,
};
pub use state::{PeerFlags, PeerState};
