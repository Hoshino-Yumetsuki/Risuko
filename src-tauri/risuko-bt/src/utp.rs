//! µTP (BEP-29) — Micro Transport Protocol over UDP.
//!
//! An additive, TCP-alternative transport for the BitTorrent peer wire. The
//! peer connection layer is already generic over `AsyncRead + AsyncWrite`
//! (see `peer::connection::finish_spawn`), so a [`stream::UtpStream`] can be
//! dialed in place of a `TcpStream` and the BT/MSE handshake runs unchanged
//! on top of it.
//!
//! Module layout:
//! - [`packet`] — the wire header + extension codec (pure, no I/O).
//! - [`socket`] — the shared UDP endpoint that demuxes datagrams to per-peer
//!   connection state machines.
//! - [`stream`] — the per-connection `AsyncRead`/`AsyncWrite` handle.

pub mod packet;
pub mod socket;
pub mod stream;

pub use socket::UtpSocket;
pub use stream::UtpStream;

/// Microsecond timestamp from a monotonic clock, truncated to the 32-bit
/// field µTP uses. Only differences matter, so wraparound is harmless.
pub fn now_micros() -> u32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    // Anchor to a fixed start so the value is a small monotonic microsecond
    // counter rather than nanoseconds-since-epoch truncated unpredictably.
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_micros() as u32
}
