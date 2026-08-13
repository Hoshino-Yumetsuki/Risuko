//! Peer id generation: Azureus-style `-<CLIENT><VERSION>-` plus 12 random bytes; `RS` (Risuko) is the client tag

use rand::Rng;

use super::hash::Id20;

pub const PEER_ID_PREFIX: &str = "-RS0001-";

/// Generate a fresh peer id for this client instance
pub fn generate_peer_id() -> Id20 {
    let mut raw = [0u8; 20];
    let prefix = PEER_ID_PREFIX.as_bytes();
    raw[..prefix.len()].copy_from_slice(prefix);
    rand::rng().fill_bytes(&mut raw[prefix.len()..]);
    Id20(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_matches() {
        let id = generate_peer_id();
        assert!(id.0.starts_with(PEER_ID_PREFIX.as_bytes()));
    }

    #[test]
    fn randomness_differs() {
        let a = generate_peer_id();
        let b = generate_peer_id();
        // Prefix bytes match, suffix should (almost certainly) differ
        assert_ne!(a, b);
    }
}
