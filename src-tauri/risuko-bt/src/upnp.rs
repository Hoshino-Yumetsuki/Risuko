//! SSDP + UPnP IGD WANIPConnection port mapping: discovers an Internet Gateway Device via SSDP M-SEARCH, parses its description XML, and issues `AddPortMapping` SOAP against `WANIPConnection:1` (or `WANPPPConnection:1`), renewing leases periodically from a background task until cancelled; scope is IPv4 only (IPv6 has no UPnP IGD equivalent)

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use network_interface::{NetworkInterface, NetworkInterfaceConfig};
use parking_lot::Mutex;
use serde::Deserialize;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use url::Url;

const SSDP_MCAST: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
const ST_WAN_IP: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";
const ST_WAN_PPP: &str = "urn:schemas-upnp-org:service:WANPPPConnection:1";
const ST_ROOT: &str = "upnp:rootdevice";

/// Protocol argument of AddPortMapping
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapProto {
    Tcp,
    Udp,
}

impl MapProto {
    fn as_str(self) -> &'static str {
        match self {
            MapProto::Tcp => "TCP",
            MapProto::Udp => "UDP",
        }
    }
}

/// How often to re-run SSDP to discover new/restarted IGDs
const DISCOVER_INTERVAL: Duration = Duration::from_secs(60);
/// How long M-SEARCH responses are collected on each discover pass
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(3);
/// Description shown in the router's port-mapping table
const MAPPING_DESCRIPTION: &str = "risuko";

/// Forwarder spec: builds a handle that runs discovery and renewal in the background
pub struct UpnpPortForwarder {
    ports: Vec<(u16, MapProto)>,
    /// How long a single mapping lease lives on the router; renewed at `lease / 2` so brief network blips don't drop the mapping
    lease: Duration,
}

impl UpnpPortForwarder {
    pub fn new(ports: Vec<(u16, MapProto)>, lease: Duration) -> Self {
        Self { ports, lease }
    }

    /// Spawn the forwarder; the returned handle aborts all background tasks and best-effort deletes active mappings when dropped
    pub fn spawn(self) -> UpnpHandle {
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
        let active = Arc::new(Mutex::new(Vec::<ActiveMapping>::new()));
        let attempts = Arc::new(AtomicUsize::new(0));
        let active_for_task = active.clone();
        let attempts_for_task = attempts.clone();
        let ports = self.ports.clone();
        let lease = self.lease;
        let join = tokio::spawn(async move {
            run_forever(
                ports,
                lease,
                active_for_task,
                attempts_for_task,
                shutdown_rx,
            )
            .await;
        });
        UpnpHandle {
            shutdown: Some(shutdown_tx),
            join: Some(join),
            active,
            attempts,
        }
    }
}

/// Handle to a running forwarder; dropping it stops the forwarder and issues a best-effort `DeletePortMapping` for any still-active mappings
pub struct UpnpHandle {
    shutdown: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
    active: Arc<Mutex<Vec<ActiveMapping>>>,
    attempts: Arc<AtomicUsize>,
}

impl UpnpHandle {
    /// Number of currently confirmed mappings without cloning them
    pub fn mapping_count(&self) -> usize {
        self.active.lock().len()
    }

    /// Number of completed discover/map passes since spawn; health checks use this to distinguish "haven't tried yet" (== 0, still negotiating) from "tried and got no IGD response" (>= 1, router likely lacks or has disabled UPnP)
    pub fn discovery_attempts(&self) -> usize {
        self.attempts.load(Ordering::Relaxed)
    }
}

impl Drop for UpnpHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.try_send(());
        }
        // Let run_forever finish its cleanup (DeletePortMapping) instead of aborting immediately; a timeout wrapper caps how long we wait
        if let Some(h) = self.join.take() {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(3), h).await;
            });
        }
    }
}

// Core loop

#[derive(Debug, Clone)]
struct ActiveMapping {
    control_url: Url,
    service_type: String,
    port: u16,
    proto: MapProto,
}

async fn run_forever(
    ports: Vec<(u16, MapProto)>,
    lease: Duration,
    active: Arc<Mutex<Vec<ActiveMapping>>>,
    attempts: Arc<AtomicUsize>,
    mut shutdown: mpsc::Receiver<()>,
) {
    // Re-map at lease/2 so a single missed pass doesn't expire the mapping
    let renewal = lease
        .checked_div(2)
        .unwrap_or(DISCOVER_INTERVAL)
        .max(Duration::from_secs(30));
    let mut tick = tokio::time::interval(renewal.min(DISCOVER_INTERVAL));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(e) = discover_and_map(&ports, lease, &active).await {
                    tracing::debug!("upnp discover/map pass failed: {e}");
                }
                attempts.fetch_add(1, Ordering::Relaxed);
            }
            _ = shutdown.recv() => {
                break;
            }
        }
    }
    // Best-effort cleanup; capped in time so shutdown stays responsive
    let snapshot: Vec<ActiveMapping> = active.lock().drain(..).collect();
    for m in snapshot {
        let _ = tokio::time::timeout(
            Duration::from_millis(500),
            delete_port_mapping(&m.control_url, &m.service_type, m.port, m.proto),
        )
        .await;
    }
}

async fn discover_and_map(
    ports: &[(u16, MapProto)],
    lease: Duration,
    active: &Arc<Mutex<Vec<ActiveMapping>>>,
) -> std::io::Result<()> {
    let endpoints = ssdp_discover(DISCOVER_TIMEOUT).await?;
    let ifaces = NetworkInterface::show().unwrap_or_default();
    for ep in endpoints {
        let root = match fetch_root_desc(&ep.location).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("upnp fetch_root_desc {}: {e}", ep.location);
                continue;
            }
        };
        let Some((control_url, service_type)) = find_wan_service(&root, &ep.location) else {
            continue;
        };
        let Some(local_ip) = pick_local_ipv4(&ifaces, *ep.received_from.ip()) else {
            continue;
        };
        let external_ip = get_external_ip(&control_url, &service_type).await.ok();
        for &(port, proto) in ports {
            match add_port_mapping(
                &control_url,
                &service_type,
                &local_ip,
                port,
                proto,
                lease,
                MAPPING_DESCRIPTION,
            )
            .await
            {
                Ok(()) => {
                    tracing::info!(
                        "upnp mapped {} {} via {} (external {:?})",
                        proto.as_str(),
                        port,
                        control_url,
                        external_ip
                    );
                    let mut g = active.lock();
                    let key = (control_url.clone(), port, proto);
                    g.retain(|m| (m.control_url.clone(), m.port, m.proto) != key);
                    g.push(ActiveMapping {
                        control_url: control_url.clone(),
                        service_type: service_type.clone(),
                        port,
                        proto,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        "upnp AddPortMapping {} {} failed via {}: {e}",
                        proto.as_str(),
                        port,
                        control_url
                    );
                }
            }
        }
    }
    Ok(())
}

// SSDP M-SEARCH

#[derive(Debug, Clone)]
struct DiscoverResponse {
    location: Url,
    received_from: SocketAddrV4,
}

async fn ssdp_discover(timeout: Duration) -> std::io::Result<Vec<DiscoverResponse>> {
    let sock = bind_ssdp_socket()?;
    let sock = UdpSocket::from_std(sock.into())?;

    for st in [ST_WAN_IP, ST_ROOT] {
        let msg = ssdp_m_search(st);
        let _ = sock.send_to(msg.as_bytes(), SSDP_MCAST).await;
    }

    let mut seen = std::collections::HashSet::<String>::new();
    let mut responses = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = vec![0u8; 2048];
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                let SocketAddr::V4(from) = from else { continue };
                let Ok(parsed) = parse_ssdp_response(&buf[..n], from) else {
                    continue;
                };
                if seen.insert(parsed.location.to_string()) {
                    responses.push(parsed);
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }
    Ok(responses)
}

fn bind_ssdp_socket() -> std::io::Result<Socket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&SocketAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).into())?;
    sock.set_multicast_ttl_v4(2)?;
    Ok(sock)
}

fn ssdp_m_search(st: &str) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 2\r\n\
         ST: {st}\r\n\
         \r\n"
    )
}

fn parse_ssdp_response(buf: &[u8], from: SocketAddrV4) -> std::io::Result<DiscoverResponse> {
    let mut headers = [httparse::EMPTY_HEADER; 24];
    let mut resp = httparse::Response::new(&mut headers);
    resp.parse(buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    if resp.code != Some(200) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("SSDP status {:?}", resp.code),
        ));
    }
    let mut location: Option<&[u8]> = None;
    for h in resp.headers.iter() {
        if h.name.eq_ignore_ascii_case("location") {
            location = Some(h.value);
        }
    }
    let location = location.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing LOCATION header")
    })?;
    let s = std::str::from_utf8(location)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-utf8 LOCATION"))?;
    let url = Url::parse(s.trim())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    Ok(DiscoverResponse {
        location: url,
        received_from: from,
    })
}

// Device description + service walk

#[derive(Debug, Deserialize)]
struct RootDesc {
    device: DeviceXml,
}

#[derive(Debug, Deserialize, Default)]
struct DeviceXml {
    #[serde(rename = "serviceList", default)]
    service_list: ServiceList,
    #[serde(rename = "deviceList", default)]
    device_list: DeviceList,
}

#[derive(Debug, Deserialize, Default)]
struct DeviceList {
    #[serde(rename = "device", default)]
    devices: Vec<DeviceXml>,
}

#[derive(Debug, Deserialize, Default)]
struct ServiceList {
    #[serde(rename = "service", default)]
    services: Vec<ServiceXml>,
}

#[derive(Debug, Deserialize, Default)]
struct ServiceXml {
    #[serde(rename = "serviceType", default)]
    service_type: String,
    #[serde(rename = "controlURL", default)]
    control_url: String,
}

/// Shared HTTP client for UPnP description fetches and SOAP calls so connection pools, TLS context and timeouts are reused across `discover_and_map` calls; returns an `io::Error` instead of panicking when client init fails, so callers surface it through their `io::Result` plumbing
fn upnp_http_client() -> std::io::Result<&'static risuko_http::Client> {
    static CLIENT: std::sync::OnceLock<risuko_http::Client> = std::sync::OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let client = risuko_http::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(io_other)?;
    // If another thread won the race, our `client` is dropped
    let _ = CLIENT.set(client);
    Ok(CLIENT.get().expect("client just initialized"))
}

async fn fetch_root_desc(location: &Url) -> std::io::Result<RootDesc> {
    let body = upnp_http_client()?
        .get(location.clone())
        .send()
        .await
        .map_err(io_other)?
        .text()
        .await
        .map_err(io_other)?;
    quick_xml::de::from_str::<RootDesc>(&body).map_err(io_other)
}

fn find_wan_service(root: &RootDesc, base: &Url) -> Option<(Url, String)> {
    fn walk<'a>(d: &'a DeviceXml, out: &mut Vec<&'a ServiceXml>) {
        for s in &d.service_list.services {
            out.push(s);
        }
        for c in &d.device_list.devices {
            walk(c, out);
        }
    }
    let mut all = Vec::new();
    walk(&root.device, &mut all);
    // Prefer WANIPConnection; fall back to WANPPPConnection
    for target in [ST_WAN_IP, ST_WAN_PPP] {
        for s in &all {
            if s.service_type == target {
                if let Ok(url) = base.join(&s.control_url) {
                    return Some((url, s.service_type.clone()));
                }
            }
        }
    }
    None
}

fn pick_local_ipv4(ifaces: &[NetworkInterface], gateway: Ipv4Addr) -> Option<Ipv4Addr> {
    let gw_bits = u32::from_be_bytes(gateway.octets());
    let mut fallback: Option<Ipv4Addr> = None;
    for nic in ifaces {
        for addr in &nic.addr {
            if let std::net::IpAddr::V4(ip) = addr.ip() {
                if ip.is_loopback() {
                    continue;
                }
                // Same /24 heuristic: good enough on home LANs, and works on platforms that don't expose netmasks via network-interface
                let ip_bits = u32::from_be_bytes(ip.octets());
                if (ip_bits & 0xffff_ff00) == (gw_bits & 0xffff_ff00) {
                    return Some(ip);
                }
                fallback.get_or_insert(ip);
            }
        }
    }
    fallback
}

// SOAP

async fn add_port_mapping(
    control_url: &Url,
    service_type: &str,
    internal_ip: &Ipv4Addr,
    port: u16,
    proto: MapProto,
    lease: Duration,
    description: &str,
) -> std::io::Result<()> {
    let body = format!(
        "<?xml version=\"1.0\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body>\
         <u:AddPortMapping xmlns:u=\"{svc}\">\
         <NewRemoteHost></NewRemoteHost>\
         <NewExternalPort>{port}</NewExternalPort>\
         <NewProtocol>{proto}</NewProtocol>\
         <NewInternalPort>{port}</NewInternalPort>\
         <NewInternalClient>{internal_ip}</NewInternalClient>\
         <NewEnabled>1</NewEnabled>\
         <NewPortMappingDescription>{desc}</NewPortMappingDescription>\
         <NewLeaseDuration>{lease}</NewLeaseDuration>\
         </u:AddPortMapping>\
         </s:Body>\
         </s:Envelope>",
        svc = service_type,
        proto = proto.as_str(),
        lease = lease.as_secs(),
        desc = xml_escape(description),
    );
    soap_call(control_url, service_type, "AddPortMapping", body).await?;
    Ok(())
}

async fn delete_port_mapping(
    control_url: &Url,
    service_type: &str,
    port: u16,
    proto: MapProto,
) -> std::io::Result<()> {
    let body = format!(
        "<?xml version=\"1.0\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body>\
         <u:DeletePortMapping xmlns:u=\"{svc}\">\
         <NewRemoteHost></NewRemoteHost>\
         <NewExternalPort>{port}</NewExternalPort>\
         <NewProtocol>{proto}</NewProtocol>\
         </u:DeletePortMapping>\
         </s:Body>\
         </s:Envelope>",
        svc = service_type,
        proto = proto.as_str(),
    );
    soap_call(control_url, service_type, "DeletePortMapping", body).await?;
    Ok(())
}

async fn get_external_ip(control_url: &Url, service_type: &str) -> std::io::Result<Ipv4Addr> {
    let body = format!(
        "<?xml version=\"1.0\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body>\
         <u:GetExternalIPAddress xmlns:u=\"{svc}\"/>\
         </s:Body>\
         </s:Envelope>",
        svc = service_type,
    );
    let text = soap_call(control_url, service_type, "GetExternalIPAddress", body).await?;
    let ip = extract_tag(&text, "NewExternalIPAddress").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing NewExternalIPAddress",
        )
    })?;
    ip.parse::<Ipv4Addr>().map_err(io_other)
}

async fn soap_call(
    control_url: &Url,
    service_type: &str,
    action: &str,
    body: String,
) -> std::io::Result<String> {
    let resp = upnp_http_client()?
        .post(control_url.clone())
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPAction", format!("\"{service_type}#{action}\""))
        .body(body)
        .send()
        .await
        .map_err(io_other)?;
    let status = resp.status();
    let text = resp.text().await.map_err(io_other)?;
    if !status.is_success() {
        return Err(std::io::Error::other(format!(
            "SOAP {action} HTTP {status}: {text}"
        )));
    }
    Ok(text)
}

fn extract_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(xml[s..e].trim())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn io_other<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssdp_ok() {
        let buf = b"HTTP/1.1 200 OK\r\n\
                    CACHE-CONTROL: max-age=1800\r\n\
                    LOCATION: http://192.168.1.1:5000/rootDesc.xml\r\n\
                    SERVER: test/1.0\r\n\
                    ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
                    \r\n";
        let r = parse_ssdp_response(buf, SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 1900))
            .unwrap();
        assert_eq!(r.location.as_str(), "http://192.168.1.1:5000/rootDesc.xml");
    }

    #[test]
    fn parse_rootdesc_finds_service() {
        let xml = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
    <deviceList>
      <device>
        <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
        <deviceList>
          <device>
            <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
            <serviceList>
              <service>
                <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
                <controlURL>/ctl/IPConn</controlURL>
              </service>
            </serviceList>
          </device>
        </deviceList>
      </device>
    </deviceList>
  </device>
</root>"#;
        let root: RootDesc = quick_xml::de::from_str(xml).unwrap();
        let base = Url::parse("http://192.168.1.1:5000/rootDesc.xml").unwrap();
        let ctrl = find_wan_service(&root, &base).unwrap();
        assert_eq!(ctrl.0.as_str(), "http://192.168.1.1:5000/ctl/IPConn");
        assert_eq!(ctrl.1, ST_WAN_IP);
    }

    #[test]
    fn extract_tag_works() {
        let xml = "<foo>bar</foo><NewExternalIPAddress>203.0.113.7</NewExternalIPAddress>";
        assert_eq!(
            extract_tag(xml, "NewExternalIPAddress"),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn xml_escape_ok() {
        assert_eq!(xml_escape("a<b&c>d"), "a&lt;b&amp;c&gt;d");
    }
}
