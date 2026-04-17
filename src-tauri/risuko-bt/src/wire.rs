//! Peer wire protocol (BEP-3) plus the bits of BEP-10 extension protocol and
//! ut_metadata / ut_pex that we need
//!
//! MSE/PE (encryption) is intentionally not implemented for v1; we accept
//! and connect with plain handshakes only.

pub mod extended;
pub mod handshake;
pub mod message;

pub use handshake::{Handshake, HANDSHAKE_LEN, PROTOCOL};
pub use message::{Message, MessageDecoder, MessageEncoder, MAX_MESSAGE_BYTES};
