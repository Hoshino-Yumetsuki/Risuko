//! Peer wire protocol (BEP-3) plus the bits of BEP-10 extension protocol and
//! ut_metadata / ut_pex that we need
//!
//! MSE/PE (BEP-8 Message Stream Encryption) primitives live under `mse`,
//! and the peer connection layer negotiates it when possible.

pub mod extended;
pub mod handshake;
pub mod message;
pub mod mse;

pub use handshake::{Handshake, HANDSHAKE_LEN, PROTOCOL};
pub use message::{Message, MessageDecoder, MessageEncoder, MAX_MESSAGE_BYTES};
