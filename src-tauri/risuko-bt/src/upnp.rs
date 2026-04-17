//! SSDP + UPnP WANIPConnection port mapping — stub
//!
//! A correct implementation discovers the IGD via SSDP M-SEARCH, parses the
//! service description XML, and issues `AddPortMapping` SOAP requests. This
//! is deferred; `map_port` reports `Unsupported` so enabling UPnP at the
//! session level is not silently ignored.

use std::net::SocketAddr;

#[derive(Debug)]
pub struct PortMapping {
    pub external: Option<SocketAddr>,
}

pub async fn map_port(port: u16) -> std::io::Result<PortMapping> {
    log::warn!("upnp port forwarding is not implemented; skipped for port {port}");
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "UPnP port forwarding not implemented",
    ))
}
