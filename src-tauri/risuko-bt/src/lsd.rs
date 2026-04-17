//! BEP-14 Local Service Discovery — stub
//!
//! Full LSD uses UDP multicast to announce torrent info-hashes on the LAN
//! Stubbed for v1; returns an empty peer stream

use tokio::sync::mpsc;

use super::core::Id20;

pub struct Lsd;

impl Lsd {
    pub async fn spawn(
        _info_hashes: Vec<Id20>,
        _port: u16,
    ) -> std::io::Result<(Self, mpsc::Receiver<(Id20, std::net::SocketAddr)>)> {
        let (_tx, rx) = mpsc::channel(1);
        Ok((Self, rx))
    }
}
