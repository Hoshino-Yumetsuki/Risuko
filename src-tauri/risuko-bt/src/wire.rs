//! Peer wire protocol (BEP-3) plus needed bits of BEP-10 extension protocol and ut_metadata / ut_pex; MSE/PE (BEP-8) primitives live under `mse`, negotiated by the peer connection layer when possible

pub mod extended;
pub mod handshake;
pub mod message;
pub mod mse;
pub mod rc4;

pub use handshake::{Handshake, HANDSHAKE_LEN, PROTOCOL};
pub use message::{Message, MessageDecoder, MessageEncoder, MAX_MESSAGE_BYTES};
