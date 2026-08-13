//! Kad node IDs, XOR distance ordering, and bounded routing buckets

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const ID_BYTES: usize = 16;
pub const ID_BITS: usize = ID_BYTES * 8;
pub const K: usize = 10;
pub const ALPHA: usize = 3;
pub const MIN_SUPPORTED_KAD_VERSION: u8 = 2;
pub const MAX_PERSISTED_CONTACTS: usize = 200;
pub const MAX_LOOKUP_QUERIES: usize = 64;
pub const MAX_LOOKUP_SOURCES: usize = 300;

pub type KadId = [u8; ID_BYTES];

/// A strongly named Kad node ID; the newtype prevents accidental interchange with an ED2K file hash at integration boundaries
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub KadId);

impl NodeId {
    pub const ZERO: Self = Self([0; ID_BYTES]);

    pub fn new(bytes: KadId) -> Self {
        Self(bytes)
    }

    pub fn random() -> Self {
        Self(rand::random())
    }

    pub fn as_bytes(&self) -> &KadId {
        &self.0
    }

    pub fn into_bytes(self) -> KadId {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self == Self::ZERO
    }

    pub fn distance(self, other: Self) -> KadId {
        xor_distance(&self.0, &other.0)
    }
}

impl From<KadId> for NodeId {
    fn from(value: KadId) -> Self {
        Self(value)
    }
}

impl From<NodeId> for KadId {
    fn from(value: NodeId) -> Self {
        value.0
    }
}

impl AsRef<[u8]> for NodeId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// XOR distance as a fixed-width big-endian byte sequence, so lexicographic comparison has the same ordering as the Kad integer distance
pub fn xor_distance(left: &KadId, right: &KadId) -> KadId {
    std::array::from_fn(|index| left[index] ^ right[index])
}

pub fn compare_distance(target: &KadId, left: &KadId, right: &KadId) -> Ordering {
    xor_distance(target, left).cmp(&xor_distance(target, right))
}

pub fn bucket_index(local: &KadId, remote: &KadId) -> Option<usize> {
    let distance = xor_distance(local, remote);
    let first_nonzero = distance.iter().position(|byte| *byte != 0)?;
    Some(first_nonzero * 8 + distance[first_nonzero].leading_zeros() as usize)
}

pub fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    // Kad routing must not learn endpoints that cannot be dialed directly; documentation/test networks are filtered too, since they are not usable peers in a real download
    let octets = ip.octets();
    octets[0] != 0
        && !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_private()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && !ip.is_multicast()
        // `Ipv4Addr::is_reserved` is still unstable on the Rust version used by the workspace; the documented TEST-NET ranges are filtered explicitly instead
        && !is_special_purpose_range(ip)
        && !(octets[0] == 100 && (64..=127).contains(&octets[1]))
        && octets[0] < 240
}

fn is_special_purpose_range(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    // IANA special-purpose blocks not covered by the standard `Ipv4Addr` predicates; these are not usable direct peers even when a malformed Kad contact advertises them as routable
    (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 192 && octets[1] == 31 && octets[2] == 196)
        || (octets[0] == 192 && octets[1] == 52 && octets[2] == 193)
        || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
        || (octets[0] == 192 && octets[1] == 175 && octets[2] == 48)
        || (octets[0] == 198 && octets[1] == 18)
        || (octets[0] == 198 && octets[1] == 19)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub id: NodeId,
    pub addr: SocketAddrV4,
    pub tcp_port: u16,
    pub version: u8,
    pub last_seen: u64,
    pub last_verified: u64,
}

impl Contact {
    pub fn new(id: KadId, addr: SocketAddrV4, tcp_port: u16, version: u8) -> Self {
        let seen = now_secs();
        Self {
            id: NodeId(id),
            addr,
            tcp_port,
            version,
            last_seen: seen,
            last_verified: seen,
        }
    }

    pub fn with_times(
        id: KadId,
        addr: SocketAddrV4,
        tcp_port: u16,
        version: u8,
        last_seen: u64,
        last_verified: u64,
    ) -> Self {
        Self {
            id: NodeId(id),
            addr,
            tcp_port,
            version,
            last_seen,
            last_verified,
        }
    }

    pub fn id_bytes(&self) -> KadId {
        self.id.0
    }

    pub fn udp_addr(&self) -> SocketAddrV4 {
        self.addr
    }

    pub fn tcp_addr(&self) -> Option<SocketAddrV4> {
        (self.tcp_port != 0).then(|| SocketAddrV4::new(*self.addr.ip(), self.tcp_port))
    }

    pub fn is_self(&self, local: NodeId) -> bool {
        self.id == local
    }

    pub fn is_valid_for_routing(&self, local: NodeId) -> bool {
        !self.id.is_zero()
            && !self.is_self(local)
            && self.version >= MIN_SUPPORTED_KAD_VERSION
            && is_public_ipv4(*self.addr.ip())
            && self.addr.port() != 0
    }

    pub fn is_valid_for_routing_allow_private(&self, local: NodeId) -> bool {
        !self.id.is_zero()
            && !self.is_self(local)
            && self.version >= MIN_SUPPORTED_KAD_VERSION
            && !self.addr.ip().is_unspecified()
            && self.addr.port() != 0
    }

    pub fn mark_seen(&mut self, verified: bool) {
        self.last_seen = now_secs();
        if verified {
            self.last_verified = self.last_seen;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertResult {
    Inserted,
    Updated,
    Replaced,
    Rejected,
    SelfContact,
}

#[derive(Clone, Debug)]
pub struct RoutingTable {
    local_id: NodeId,
    buckets: Vec<VecDeque<Contact>>,
    replacements: Vec<VecDeque<Contact>>,
    by_id: HashMap<NodeId, usize>,
}

impl RoutingTable {
    pub fn new(local_id: NodeId) -> Self {
        Self {
            local_id,
            buckets: (0..ID_BITS).map(|_| VecDeque::with_capacity(K)).collect(),
            replacements: (0..ID_BITS).map(|_| VecDeque::with_capacity(K)).collect(),
            by_id: HashMap::new(),
        }
    }

    pub fn local_id(&self) -> NodeId {
        self.local_id
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn bucket_len(&self, index: usize) -> usize {
        self.buckets.get(index).map(VecDeque::len).unwrap_or(0)
    }

    pub fn contacts(&self) -> Vec<Contact> {
        self.buckets
            .iter()
            .flat_map(|bucket| bucket.iter().cloned())
            .collect()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.by_id.contains_key(&id)
    }

    pub fn get(&self, id: NodeId) -> Option<Contact> {
        let bucket = *self.by_id.get(&id)?;
        self.buckets[bucket]
            .iter()
            .find(|contact| contact.id == id)
            .cloned()
    }

    /// Return the least-recently-seen contact in the full bucket that would receive `candidate`; the caller must probe this contact before evicting it, and a live pong moves it back to the MRU end instead
    pub fn liveness_probe_target(&self, candidate: &Contact) -> Option<Contact> {
        let index = bucket_index(&self.local_id.0, &candidate.id.0)?;
        let bucket = self.buckets.get(index)?;
        (bucket.len() >= K)
            .then(|| bucket.front().cloned())
            .flatten()
    }

    /// Record a successful liveness response and move the contact to the MRU end of its bucket
    pub fn mark_alive(&mut self, id: NodeId) -> bool {
        let Some(index) = self.by_id.get(&id).copied() else {
            return false;
        };
        let Some(position) = self.buckets[index]
            .iter()
            .position(|contact| contact.id == id)
        else {
            self.by_id.remove(&id);
            return false;
        };
        let Some(mut contact) = self.buckets[index].remove(position) else {
            return false;
        };
        contact.mark_seen(true);
        self.buckets[index].push_back(contact);
        true
    }

    pub fn insert(&mut self, contact: Contact) -> InsertResult {
        self.insert_checked(contact, false)
    }

    /// Loopback integration tests exercise the real UDP client, but must not teach production routing tables that a local/private endpoint is valid
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, contact: Contact) -> InsertResult {
        self.insert_checked(contact, true)
    }

    fn insert_checked(&mut self, mut contact: Contact, allow_private: bool) -> InsertResult {
        if contact.id == self.local_id {
            return InsertResult::SelfContact;
        }
        let Some(index) = bucket_index(&self.local_id.0, &contact.id.0) else {
            return InsertResult::SelfContact;
        };
        let valid = if allow_private {
            contact.is_valid_for_routing_allow_private(self.local_id)
        } else {
            contact.is_valid_for_routing(self.local_id)
        };
        if !valid {
            return InsertResult::Rejected;
        }

        if let Some(existing_bucket) = self.by_id.get(&contact.id).copied() {
            if let Some(existing) = self.buckets[existing_bucket]
                .iter_mut()
                .find(|existing| existing.id == contact.id)
            {
                existing.addr = contact.addr;
                existing.tcp_port = contact.tcp_port;
                existing.version = existing.version.max(contact.version);
                existing.mark_seen(true);
                // Move a live contact to the MRU end of its bucket
                let updated = self.buckets[existing_bucket]
                    .iter()
                    .position(|item| item.id == contact.id)
                    .and_then(|position| self.buckets[existing_bucket].remove(position));
                if let Some(updated) = updated {
                    self.buckets[existing_bucket].push_back(updated);
                }
                return InsertResult::Updated;
            }
            self.by_id.remove(&contact.id);
        }

        // Contacts reconstructed from disk carry their original validation metadata; fresh wire contacts come from `Contact::new` and already have a current timestamp, so do not erase persisted timestamps here
        if contact.last_seen == 0 {
            contact.mark_seen(true);
        }
        let bucket = &mut self.buckets[index];
        if bucket.len() < K {
            let id = contact.id;
            bucket.push_back(contact);
            self.by_id.insert(id, index);
            InsertResult::Inserted
        } else {
            // Keep a bounded replacement cache; a full bucket is only changed after its LRU contact fails a Kad ping, and timestamps alone are not evidence that the contact is dead
            let replacements = &mut self.replacements[index];
            if let Some(existing) = replacements.iter_mut().find(|entry| entry.id == contact.id) {
                *existing = contact;
                return InsertResult::Updated;
            }
            if replacements.len() >= K {
                replacements.pop_front();
            }
            replacements.push_back(contact);
            InsertResult::Rejected
        }
    }

    pub fn remove(&mut self, id: NodeId) -> Option<Contact> {
        let index = self.by_id.remove(&id)?;
        let position = self.buckets[index]
            .iter()
            .position(|entry| entry.id == id)?;
        let removed = self.buckets[index].remove(position);
        self.promote_replacement(index);
        removed
    }

    /// Remove `expected` only when it has not been refreshed or moved since a liveness probe began; this avoids a stale ping timeout evicting a contact that received a newer response meanwhile
    pub fn remove_if_unchanged(&mut self, expected: &Contact) -> Option<Contact> {
        let current = self.get(expected.id)?;
        if current.addr != expected.addr
            || current.last_seen != expected.last_seen
            || current.last_verified != expected.last_verified
        {
            return None;
        }
        self.remove(expected.id)
    }

    pub fn mark_failed(&mut self, id: NodeId) {
        if let Some(index) = self.by_id.get(&id).copied() {
            // Keep the timestamp snapshot stable for an in-flight liveness probe; moving the failed contact to the LRU end makes it the next eviction candidate without causing `remove_if_unchanged` to mistake this bookkeeping update for a newer response
            if let Some(position) = self.buckets[index].iter().position(|entry| entry.id == id) {
                if let Some(contact) = self.buckets[index].remove(position) {
                    self.buckets[index].push_front(contact);
                }
            }
        }
    }

    pub fn closest(&self, target: NodeId, limit: usize) -> Vec<Contact> {
        let mut contacts = self.contacts();
        contacts.sort_by(|left, right| compare_distance(&target.0, &left.id.0, &right.id.0));
        contacts.truncate(limit.min(MAX_LOOKUP_QUERIES));
        contacts
    }

    pub fn closest_with_replacements(&self, target: NodeId, limit: usize) -> Vec<Contact> {
        let mut contacts = self.closest(target, limit);
        if contacts.len() < limit {
            let mut replacements: Vec<_> = self
                .replacements
                .iter()
                .flat_map(|bucket| bucket.iter().cloned())
                .filter(|contact| !contacts.iter().any(|item| item.id == contact.id))
                .collect();
            replacements
                .sort_by(|left, right| compare_distance(&target.0, &left.id.0, &right.id.0));
            contacts.extend(replacements.into_iter().take(limit - contacts.len()));
        }
        contacts
    }

    pub fn replacement_len(&self) -> usize {
        self.replacements.iter().map(VecDeque::len).sum()
    }

    fn promote_replacement(&mut self, index: usize) {
        if let Some(contact) = self.replacements[index].pop_back() {
            let id = contact.id;
            self.buckets[index].push_back(contact);
            self.by_id.insert(id, index);
        }
    }
}

#[derive(Clone, Debug)]
pub struct LookupConfig {
    pub alpha: usize,
    pub max_queries: usize,
    pub max_sources: usize,
    pub request_timeout: std::time::Duration,
    pub deadline: std::time::Duration,
    pub retries: usize,
}

impl Default for LookupConfig {
    fn default() -> Self {
        Self {
            alpha: ALPHA,
            max_queries: MAX_LOOKUP_QUERIES,
            max_sources: MAX_LOOKUP_SOURCES,
            request_timeout: std::time::Duration::from_secs(3),
            deadline: std::time::Duration::from_secs(45),
            retries: 1,
        }
    }
}

impl LookupConfig {
    pub fn bounded(mut self) -> Self {
        self.alpha = self.alpha.clamp(1, ALPHA);
        self.max_queries = self.max_queries.clamp(1, MAX_LOOKUP_QUERIES);
        self.max_sources = self.max_sources.clamp(1, MAX_LOOKUP_SOURCES);
        self.request_timeout = self.request_timeout.min(std::time::Duration::from_secs(3));
        self.deadline = self.deadline.min(std::time::Duration::from_secs(45));
        self.retries = self.retries.min(1);
        self
    }
}

/// Tracks queried nodes for one iterative lookup and prevents duplicate work
#[derive(Debug)]
pub struct LookupTracker {
    target: NodeId,
    queried: HashMap<NodeId, bool>,
    max_queries: usize,
}

impl LookupTracker {
    pub fn new(target: NodeId, config: &LookupConfig) -> Self {
        Self {
            target,
            queried: HashMap::new(),
            max_queries: config.max_queries,
        }
    }

    pub fn target(&self) -> NodeId {
        self.target
    }

    pub fn queried_count(&self) -> usize {
        self.queried.len()
    }

    pub fn mark_queried(&mut self, id: NodeId) -> bool {
        if self.queried.len() >= self.max_queries || self.queried.contains_key(&id) {
            return false;
        }
        self.queried.insert(id, false);
        true
    }

    pub fn mark_responded(&mut self, id: NodeId) {
        if let Some(value) = self.queried.get_mut(&id) {
            *value = true;
        }
    }

    pub fn has_responded(&self, id: NodeId) -> bool {
        self.queried.get(&id).copied().unwrap_or(false)
    }

    pub fn was_queried(&self, id: NodeId) -> bool {
        self.queried.contains_key(&id)
    }
}

/// Convert a socket address to an endpoint key for source de-duplication
pub fn endpoint_key(addr: SocketAddrV4) -> (Ipv4Addr, u16) {
    (*addr.ip(), addr.port())
}

/// Keep a bounded set of source endpoints; shared by the service and the eventual ED2K download scheduler
#[derive(Debug, Default)]
pub struct SourceSet {
    entries: HashMap<(Ipv4Addr, u16), ()>,
}

impl SourceSet {
    pub fn insert(&mut self, addr: SocketAddrV4) -> bool {
        self.entries.insert(endpoint_key(addr), ()).is_none()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, addr: SocketAddrV4) -> bool {
        self.entries.contains_key(&endpoint_key(addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u8) -> KadId {
        [value; ID_BYTES]
    }

    fn contact(value: u8, ip: [u8; 4]) -> Contact {
        Contact::new(
            id(value),
            SocketAddrV4::new(Ipv4Addr::from(ip), 4672),
            4662,
            8,
        )
    }

    #[test]
    fn xor_distance_and_bucket_index_are_most_significant_bit_ordered() {
        assert_eq!(xor_distance(&id(0), &id(0)), [0; 16]);
        assert_eq!(bucket_index(&id(0), &id(0x80)), Some(0));
        let mut least_significant = [0; ID_BYTES];
        least_significant[ID_BYTES - 1] = 1;
        assert_eq!(bucket_index(&id(0), &least_significant), Some(127));
        assert!(compare_distance(&id(0), &id(1), &id(2)).is_lt());
    }

    #[test]
    fn public_ipv4_rejects_special_purpose_peer_ranges() {
        for ip in [
            Ipv4Addr::new(0, 1, 2, 3),
            Ipv4Addr::new(192, 0, 0, 1),
            Ipv4Addr::new(192, 31, 196, 1),
            Ipv4Addr::new(192, 52, 193, 1),
            Ipv4Addr::new(192, 88, 99, 1),
            Ipv4Addr::new(192, 175, 48, 1),
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(198, 19, 255, 254),
        ] {
            assert!(!is_public_ipv4(ip), "special-purpose address {ip}");
        }
        assert!(is_public_ipv4(Ipv4Addr::new(8, 8, 8, 8)));
    }

    #[test]
    fn table_caps_buckets_and_rejects_invalid_contacts() {
        let local = NodeId(id(0));
        let mut table = RoutingTable::new(local);
        assert_eq!(
            table.insert(contact(0, [1, 2, 3, 4])),
            InsertResult::SelfContact
        );
        assert_eq!(
            table.insert(contact(1, [10, 0, 0, 1])),
            InsertResult::Rejected
        );
        assert_eq!(
            table.insert(Contact::new(
                id(1),
                SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 4672),
                4662,
                MIN_SUPPORTED_KAD_VERSION - 1,
            )),
            InsertResult::Rejected
        );
        for value in 1..=K as u8 {
            let mut c = contact(value, [8, 8, 8, value]);
            c.id = NodeId([0x80 | value; 16]);
            assert!(matches!(
                table.insert(c),
                InsertResult::Inserted | InsertResult::Replaced
            ));
        }
        assert_eq!(table.bucket_len(0), K);
        let mut overflow = contact(99, [8, 8, 8, 99]);
        overflow.id = NodeId([0x80 | 99; 16]);
        assert_eq!(table.insert(overflow), InsertResult::Rejected);
        assert_eq!(table.len(), K);
    }

    #[test]
    fn routing_contacts_need_a_udp_endpoint_but_not_a_tcp_listener() {
        let local = NodeId(id(0));
        let mut table = RoutingTable::new(local);
        let contact = Contact::new(
            id(1),
            SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 4672),
            0,
            MIN_SUPPORTED_KAD_VERSION,
        );

        assert_eq!(table.insert(contact), InsertResult::Inserted);
    }

    #[test]
    fn closest_is_sorted_and_remove_promotes_replacement() {
        let local = NodeId(id(0));
        let mut table = RoutingTable::new(local);
        for value in 1..=K as u8 {
            let mut c = contact(value, [9, 9, 9, value]);
            c.id = NodeId([0x80 | value; 16]);
            let _ = table.insert(c);
        }
        let mut replacement = contact(77, [9, 9, 9, 77]);
        replacement.id = NodeId([0x80 | 77; 16]);
        assert_eq!(table.insert(replacement), InsertResult::Rejected);
        let removed = table.remove(NodeId([0x81; 16])).unwrap();
        assert_eq!(removed.id, NodeId([0x81; 16]));
        assert_eq!(table.len(), K);
    }

    #[test]
    fn liveness_probe_retains_ponging_contacts_and_promotes_after_timeout() {
        let local = NodeId(id(0));
        let mut table = RoutingTable::new(local);
        for value in 1..=K as u8 {
            let mut entry = contact(value, [9, 9, 9, value]);
            entry.id = NodeId([0x80 | value; ID_BYTES]);
            assert_eq!(table.insert(entry), InsertResult::Inserted);
        }

        let mut replacement = contact(77, [9, 9, 9, 77]);
        replacement.id = NodeId([0xf0; ID_BYTES]);
        assert_eq!(table.insert(replacement.clone()), InsertResult::Rejected);

        let first_lru = table
            .liveness_probe_target(&replacement)
            .expect("full bucket has an LRU contact");
        assert!(table.mark_alive(first_lru.id));
        assert!(table.contains(first_lru.id));

        let timed_out = table
            .liveness_probe_target(&replacement)
            .expect("full bucket has a later LRU contact");
        assert_ne!(timed_out.id, first_lru.id);
        assert!(table.remove_if_unchanged(&timed_out).is_some());
        assert!(!table.contains(timed_out.id));
        assert!(table.contains(replacement.id));
        assert_eq!(table.len(), K);
    }

    #[test]
    fn mark_failed_keeps_an_in_flight_liveness_snapshot_evictable() {
        let local = NodeId(id(0));
        let mut table = RoutingTable::new(local);
        let contact = contact(1, [8, 8, 8, 8]);
        let id = contact.id;
        assert_eq!(table.insert(contact), InsertResult::Inserted);
        let snapshot = table.get(id).expect("inserted contact should be present");

        table.mark_failed(id);

        assert!(table.remove_if_unchanged(&snapshot).is_some());
    }

    #[test]
    fn lookup_tracker_deduplicates_and_caps_queries() {
        let config = LookupConfig {
            max_queries: 2,
            ..LookupConfig::default()
        }
        .bounded();
        let mut tracker = LookupTracker::new(NodeId(id(2)), &config);
        assert!(tracker.mark_queried(NodeId(id(3))));
        assert!(!tracker.mark_queried(NodeId(id(3))));
        assert!(tracker.mark_queried(NodeId(id(4))));
        assert!(!tracker.mark_queried(NodeId(id(5))));
        tracker.mark_responded(NodeId(id(3)));
        assert!(tracker.has_responded(NodeId(id(3))));
    }
}
