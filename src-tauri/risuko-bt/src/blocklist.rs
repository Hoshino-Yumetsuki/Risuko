//! BitTorrent IP/CIDR blocklist

use std::collections::HashSet;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlocklistApplyResult {
    pub revision: u32,
    pub rule_count: u32,
    pub disconnected_peers: u32,
    pub removed_peers: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prefix {
    V4 { network: u32, mask: u32 },
    V6 { network: u128, mask: u128 },
}

impl Prefix {
    fn contains(self, ip: IpAddr) -> bool {
        match (self, ip) {
            (Prefix::V4 { network, mask }, IpAddr::V4(addr)) => u32::from(addr) & mask == network,
            (Prefix::V4 { network, mask }, IpAddr::V6(addr)) => match addr.to_ipv4_mapped() {
                Some(v4) => u32::from(v4) & mask == network,
                None => false,
            },
            (Prefix::V6 { network, mask }, IpAddr::V6(addr)) => u128::from(addr) & mask == network,
            (Prefix::V6 { .. }, IpAddr::V4(_)) => false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BlockList {
    exact: HashSet<IpAddr>,
    prefixes: Vec<Prefix>,
    revision: u32,
}

impl BlockList {
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.prefixes.is_empty()
    }

    pub fn revision(&self) -> u32 {
        self.revision
    }

    pub fn rule_count(&self) -> u32 {
        (self.exact.len() + self.prefixes.len()) as u32
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        if self.is_empty() {
            return false;
        }
        let canonical = canonicalize_ip(ip);
        if self.exact.contains(&canonical) {
            return true;
        }
        self.prefixes.iter().any(|prefix| prefix.contains(ip))
    }

    /// Full-replace semantics: parse `entries` (plain IPs or CIDR strings) and bump `revision`
    pub fn replace(&mut self, entries: &[String]) -> BlocklistApplyResult {
        self.exact.clear();
        self.prefixes.clear();
        for entry in entries {
            match parse_entry(entry) {
                Some(ParsedEntry::Exact(ip)) => {
                    self.exact.insert(canonicalize_ip(ip));
                }
                Some(ParsedEntry::Prefix(prefix)) => self.prefixes.push(prefix),
                None => {}
            }
        }
        self.revision = self.revision.wrapping_add(1);
        BlocklistApplyResult {
            revision: self.revision,
            rule_count: self.rule_count(),
            disconnected_peers: 0,
            removed_peers: 0,
        }
    }
}

enum ParsedEntry {
    Exact(IpAddr),
    Prefix(Prefix),
}

fn canonicalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        IpAddr::V4(_) => ip,
    }
}

fn parse_entry(raw: &str) -> Option<ParsedEntry> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((addr, prefix_str)) = trimmed.split_once('/') {
        let prefix: u8 = prefix_str.parse().ok()?;
        let ip: IpAddr = addr.parse().ok()?;
        return Some(ParsedEntry::Prefix(parse_prefix(ip, prefix)?));
    }
    let ip: IpAddr = trimmed.parse().ok()?;
    Some(ParsedEntry::Exact(ip))
}

fn parse_prefix(ip: IpAddr, prefix: u8) -> Option<Prefix> {
    match ip {
        IpAddr::V4(addr) => {
            if prefix > 32 {
                return None;
            }
            let mask = ipv4_mask(prefix);
            Some(Prefix::V4 {
                network: u32::from(addr) & mask,
                mask,
            })
        }
        IpAddr::V6(addr) => {
            if prefix > 128 {
                return None;
            }
            let mask = ipv6_mask(prefix);
            Some(Prefix::V6 {
                network: u128::from(addr) & mask,
                mask,
            })
        }
    }
}

fn ipv4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        !0u32 << (32 - prefix)
    }
}

fn ipv6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        !0u128 << (128 - prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_ipv4_and_mapped_v6_match() {
        let mut list = BlockList::default();
        list.replace(&["1.2.3.4".into()]);
        assert!(list.contains("1.2.3.4".parse().unwrap()));
        assert!(list.contains("::ffff:1.2.3.4".parse().unwrap()));
        assert!(!list.contains("1.2.3.5".parse().unwrap()));
        assert_eq!(list.rule_count(), 1);
        assert_eq!(list.revision(), 1);
    }

    #[test]
    fn ipv4_cidr_matches_hosts_in_range() {
        let mut list = BlockList::default();
        list.replace(&["10.0.0.0/8".into()]);
        assert!(list.contains("10.1.2.3".parse().unwrap()));
        assert!(!list.contains("11.0.0.1".parse().unwrap()));
    }

    #[test]
    fn ipv6_cidr_matches_prefix() {
        let mut list = BlockList::default();
        list.replace(&["2001:db8::/32".into()]);
        assert!(list.contains("2001:db8:1::1".parse().unwrap()));
        assert!(!list.contains("2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn replace_is_full_swap_and_bumps_revision() {
        let mut list = BlockList::default();
        list.replace(&["1.1.1.1".into()]);
        list.replace(&["8.8.8.8/32".into(), "not-an-ip".into(), "".into()]);
        assert!(!list.contains("1.1.1.1".parse().unwrap()));
        assert!(list.contains("8.8.8.8".parse().unwrap()));
        assert_eq!(list.revision(), 2);
        assert_eq!(list.rule_count(), 1);
    }

    #[test]
    fn empty_list_contains_nothing() {
        let list = BlockList::default();
        assert!(!list.contains("127.0.0.1".parse().unwrap()));
        assert!(list.is_empty());
    }

    #[test]
    fn ipv6_unspecified_cidr_does_not_block_ipv4_peers() {
        let mut list = BlockList::default();
        list.replace(&["::/0".into()]);
        assert!(list.contains("2001:db8::1".parse().unwrap()));
        assert!(!list.contains("1.2.3.4".parse().unwrap()));
        assert!(list.contains("::ffff:1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn ipv4_cidr_still_matches_mapped_ipv6_peers() {
        let mut list = BlockList::default();
        list.replace(&["10.0.0.0/8".into()]);
        assert!(list.contains("10.1.2.3".parse().unwrap()));
        assert!(list.contains("::ffff:10.1.2.3".parse().unwrap()));
    }

    #[test]
    fn mapped_ipv6_cidr_keeps_ipv6_prefix_length() {
        let mut list = BlockList::default();
        list.replace(&["::ffff:10.1.2.0/120".into()]);
        assert!(list.contains("::ffff:10.1.2.1".parse().unwrap()));
        assert!(list.contains("::ffff:10.1.2.255".parse().unwrap()));
        assert!(!list.contains("::ffff:10.1.3.1".parse().unwrap()));
        assert!(!list.contains("10.1.2.1".parse().unwrap()));
        assert_eq!(list.rule_count(), 1);
    }
}
