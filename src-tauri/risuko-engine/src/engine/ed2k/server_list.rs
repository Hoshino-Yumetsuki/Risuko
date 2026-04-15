use serde::{Deserialize, Serialize};
use std::net::SocketAddrV4;

/// Known ed2k server entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub ip: String,
    pub port: u16,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub users: u32,
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub fail_count: u32,
}

impl ServerEntry {
    pub fn new(ip: &str, port: u16, name: &str) -> Self {
        Self {
            ip: ip.to_string(),
            port,
            name: name.to_string(),
            description: String::new(),
            users: 0,
            files: 0,
            fail_count: 0,
        }
    }

    pub fn to_socket_addr(&self) -> Option<SocketAddrV4> {
        let ip = self.ip.parse().ok()?;
        Some(SocketAddrV4::new(ip, self.port))
    }
}

/// Manages a list of known ed2k servers
pub struct ServerList {
    servers: Vec<ServerEntry>,
}

impl ServerList {
    pub fn new() -> Self {
        Self {
            servers: Self::default_servers(),
        }
    }

    /// Build from user-configured server strings (format: "ip:port").
    /// Falls back to defaults if the list is empty.
    pub fn from_config(entries: &[String]) -> Self {
        let mut servers: Vec<ServerEntry> = entries
            .iter()
            .filter_map(|s| {
                let parts: Vec<&str> = s.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let _: std::net::Ipv4Addr = parts[0].parse().ok()?;
                    let port: u16 = parts[1].parse().ok()?;
                    Some(ServerEntry::new(parts[0], port, "Custom"))
                } else {
                    None
                }
            })
            .collect();
        if servers.is_empty() {
            servers = Self::default_servers();
        }
        Self { servers }
    }

    /// Well-known public ed2k servers
    fn default_servers() -> Vec<ServerEntry> {
        vec![
            ServerEntry::new("176.123.5.89", 4725, "eMule Sunrise"),
            ServerEntry::new("45.82.80.155", 5687, "eMule Security"),
            ServerEntry::new("85.239.33.123", 4232, "!! Sharing-Devils No.2 !!"),
            ServerEntry::new("91.208.162.87", 4232, "!! Sharing-Devils No.1 !!"),
            ServerEntry::new("145.239.2.134", 4661, "GrupoTS Server"),
        ]
    }

    pub fn servers(&self) -> &[ServerEntry] {
        &self.servers
    }
}

impl Default for ServerList {
    fn default() -> Self {
        Self::new()
    }
}
