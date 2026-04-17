//! SSDP + UPnP WANIPConnection port mapping — stub
//!
//! A correct implementation discovers the IGD via SSDP M-SEARCH, parses the
//! service description XML, and issues `AddPortMapping` SOAP requests. This
//! is deferred; for now `map_port` returns `Ok(())` without forwarding so
//! sessions configured with UPnP enabled still start

use std::net::SocketAddr;

#[derive(Debug)]
pub struct PortMapping {
    pub external: Option<SocketAddr>,
}

pub async fn map_port(port: u16) -> std::io::Result<PortMapping> {
    log::debug!("upnp stub: skipping forward for port {port}");
    Ok(PortMapping { external: None })
}
