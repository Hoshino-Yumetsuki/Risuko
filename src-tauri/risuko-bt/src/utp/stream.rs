//! Per-connection µTP state machine and the [`UtpStream`] `AsyncRead`/`AsyncWrite` handle: each connection is driven by a single background task ([`drive`]) that owns the connection's slice of the shared UDP socket's traffic (fed by the socket router over an mpsc channel) and is the only thing that touches the wire for this connection; the [`UtpStream`] handle shares a [`Mutex<ConnState>`] with the driver (reads drain `recv_ready`, writes append to `send_buf`, a [`Notify`] nudges the driver) and the driver wakes the stream's stored wakers when data arrives or buffer space frees up. Reliability model: in-order byte delivery with a reorder buffer for out-of-order data, cumulative + selective acknowledgements, RFC-6298-style RTO retransmission (Karn's algorithm for RTT sampling), and LEDBAT-lite delay-based congestion control bounded by the peer's advertised window

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Notify};

use risuko_http::ProxyDatagram;

use super::now_micros;
use super::packet::{PacketType, UtpHeader};
use super::socket::{ConnKey, ConnRegistry, ProxyConnRegistry};
const MSS: usize = 1200;
const RECV_BUF_MAX: usize = 1024 * 1024;
const SEND_BUF_MAX: usize = 512 * 1024;
const TARGET_MICROS: f64 = 100_000.0;
const CWND_GAIN: f64 = 1.0;
const MIN_CWND: usize = 2 * MSS;
const MAX_CWND: usize = 2 * 1024 * 1024;
const INITIAL_CWND: usize = 3 * MSS;
const MIN_RTO: Duration = Duration::from_millis(500);
const MAX_RTO: Duration = Duration::from_secs(10);
const INITIAL_RTO: Duration = Duration::from_secs(1);
const MAX_RETRANSMITS: u32 = 8;
const LINGER_TIMEOUT: Duration = Duration::from_secs(30);

/// `a` is strictly after `b` in 16-bit sequence space (within half the ring)
fn seq_after(a: u16, b: u16) -> bool {
    let d = a.wrapping_sub(b);
    d != 0 && d < 0x8000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    SynSent,
    Connected,
    FinSent,
    Closed,
}

/// A packet we've transmitted that is awaiting acknowledgement; stored un-encoded so retransmissions carry fresh timestamps / ack_nr / window
struct OutPacket {
    packet_type: PacketType,
    seq_nr: u16,
    payload: Vec<u8>,
    sent_at: Instant,
    transmissions: u32,
}

pub(crate) struct ConnState {
    state: State,
    remote: SocketAddr,
    conn_id_send: u16,

    seq_nr: u16,
    ack_nr: u16,

    send_buf: VecDeque<u8>,
    unacked: VecDeque<OutPacket>,

    recv_ready: VecDeque<u8>,
    reorder: BTreeMap<u16, Vec<u8>>,

    peer_wnd: u32,
    max_window: usize,
    base_delay: u32,

    rtt: f64,
    rtt_var: f64,
    rto: Duration,
    reply_micros: u32,

    needs_ack: bool,
    want_fin: bool,
    peer_fin: Option<u16>,
    eof: bool,
    error: Option<io::ErrorKind>,

    recovery_seq: Option<u16>,

    outbox: Vec<Vec<u8>>,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    connect_notify: Option<oneshot::Sender<io::Result<()>>>,
}

impl ConnState {
    fn advertised_window(&self) -> u32 {
        // Out-of-order bytes in `reorder` also consume receive buffer, so count them in the advertised window
        let reorder_bytes: usize = self.reorder.values().map(|b| b.len()).sum();
        let used = self.recv_ready.len().saturating_add(reorder_bytes);
        RECV_BUF_MAX.saturating_sub(used) as u32
    }

    /// Encode an outstanding packet with current ack/window/timestamps
    fn encode(&self, p: &OutPacket) -> Vec<u8> {
        // The SYN is special-cased to carry our *receive* id (send-1); every other packet carries our send id
        let conn_id = if p.packet_type == PacketType::Syn {
            self.conn_id_send.wrapping_sub(1)
        } else {
            self.conn_id_send
        };
        let header = UtpHeader {
            packet_type: p.packet_type,
            connection_id: conn_id,
            timestamp_micros: now_micros(),
            timestamp_diff_micros: self.reply_micros,
            wnd_size: self.advertised_window(),
            seq_nr: p.seq_nr,
            ack_nr: self.ack_nr,
            selective_ack: self.build_selective_ack(),
        };
        header.encode(&p.payload)
    }

    /// Build a standalone ST_STATE acknowledgement
    fn encode_state(&self) -> Vec<u8> {
        UtpHeader {
            packet_type: PacketType::State,
            connection_id: self.conn_id_send,
            timestamp_micros: now_micros(),
            timestamp_diff_micros: self.reply_micros,
            wnd_size: self.advertised_window(),
            // ST_STATE carries the next seq to be used but does not consume it
            seq_nr: self.seq_nr,
            ack_nr: self.ack_nr,
            selective_ack: self.build_selective_ack(),
        }
        .encode(&[])
    }

    /// Selective-ACK bitmask covering buffered out-of-order packets, if any; bit `i` (LSB-first per byte) acks `ack_nr + 2 + i`
    fn build_selective_ack(&self) -> Option<Vec<u8>> {
        if self.reorder.is_empty() {
            return None;
        }
        let base = self.ack_nr.wrapping_add(2);
        let mut mask = vec![0u8; 4];
        for &seq in self.reorder.keys() {
            let bit = seq.wrapping_sub(base) as usize;
            if bit < mask.len() * 8 {
                mask[bit / 8] |= 1 << (bit % 8);
            }
        }
        Some(mask)
    }

    fn bytes_in_flight(&self) -> usize {
        self.unacked.iter().map(|p| p.payload.len()).sum()
    }

    /// Move app bytes from `send_buf` into DATA packets while the congestion and flow-control windows allow, queuing them for transmission
    fn fill_send_window(&mut self) {
        if self.state != State::Connected {
            return;
        }
        loop {
            if self.send_buf.is_empty() {
                break;
            }
            let window = self.max_window.min(self.peer_wnd as usize);
            let room = window.saturating_sub(self.bytes_in_flight());
            if room == 0 {
                break;
            }
            let take = self.send_buf.len().min(MSS).min(room);
            if take == 0 {
                break;
            }
            let payload: Vec<u8> = self.send_buf.drain(..take).collect();
            self.transmit_new(PacketType::Data, payload);
        }
    }

    /// Assign a fresh sequence number to a DATA/FIN packet and queue it
    fn transmit_new(&mut self, packet_type: PacketType, payload: Vec<u8>) {
        let p = OutPacket {
            packet_type,
            seq_nr: self.seq_nr,
            payload,
            sent_at: Instant::now(),
            transmissions: 1,
        };
        self.seq_nr = self.seq_nr.wrapping_add(1);
        self.outbox.push(self.encode(&p));
        self.unacked.push_back(p);
        // A DATA/FIN packet carries our ack_nr, so it doubles as an ack
        self.needs_ack = false;
    }

    /// Process a cumulative ack: drop fully-acked packets and sample RTT
    fn process_ack(&mut self, ack_nr: u16, their_delay: u32) {
        let mut acked_any = false;
        let mut acked_bytes = 0usize;
        while let Some(front) = self.unacked.front() {
            if seq_after(front.seq_nr, ack_nr) {
                break; // front is beyond the ack point
            }
            let p = self.unacked.pop_front().unwrap();
            acked_any = true;
            acked_bytes += p.payload.len();
            // Karn: only sample RTT from packets sent exactly once
            if p.transmissions == 1 {
                self.update_rtt(p.sent_at.elapsed());
            }
        }
        if acked_any {
            // Recovery ends once its marker is cumulatively acked, re-arming decrease for the next loss
            if let Some(rseq) = self.recovery_seq {
                if !seq_after(rseq, ack_nr) {
                    self.recovery_seq = None;
                }
            }
            self.update_cwnd(their_delay, acked_bytes);
            self.notify_write();
        }
    }

    /// Remove packets named by a selective-ACK bitmask and fast-retransmit the oldest unacked packet if anything past it was selectively acked
    fn process_selective_ack(&mut self, ack_nr: u16, mask: &[u8]) {
        let base = ack_nr.wrapping_add(2);
        let mut sacked = false;
        for (byte_idx, byte) in mask.iter().enumerate() {
            for bit in 0..8 {
                if byte & (1 << bit) == 0 {
                    continue;
                }
                let seq = base.wrapping_add((byte_idx * 8 + bit) as u16);
                if let Some(pos) = self.unacked.iter().position(|p| p.seq_nr == seq) {
                    self.unacked.remove(pos);
                    sacked = true;
                }
            }
        }
        // If holes remain before SACKed packets, the front was likely lost; fast-retransmit it, extracting its fields first so the mutable borrow is released before we re-encode (which borrows &self)
        if sacked {
            // SACK past a hole signals loss; decrease once per `recovery_seq` episode
            if self.recovery_seq.is_none() {
                if let Some(back) = self.unacked.back() {
                    self.recovery_seq = Some(back.seq_nr);
                    self.max_window = (self.max_window / 2).max(MIN_CWND);
                }
            }
            let p = self.unacked.front_mut().map(|f| {
                f.sent_at = Instant::now();
                f.transmissions += 1;
                OutPacket {
                    packet_type: f.packet_type,
                    seq_nr: f.seq_nr,
                    payload: f.payload.clone(),
                    sent_at: f.sent_at,
                    transmissions: f.transmissions,
                }
            });
            if let Some(p) = p {
                self.outbox.push(self.encode(&p));
            }
        }
    }

    fn update_rtt(&mut self, sample: Duration) {
        let s = sample.as_secs_f64();
        if self.rtt == 0.0 {
            self.rtt = s;
            self.rtt_var = s / 2.0;
        } else {
            self.rtt_var = 0.75 * self.rtt_var + 0.25 * (self.rtt - s).abs();
            self.rtt = 0.875 * self.rtt + 0.125 * s;
        }
        let rto = Duration::from_secs_f64(self.rtt + 4.0 * self.rtt_var);
        self.rto = rto.clamp(MIN_RTO, MAX_RTO);
    }

    /// LEDBAT congestion-window update from the peer-measured one-way delay
    fn update_cwnd(&mut self, their_delay: u32, acked_bytes: usize) {
        if their_delay == 0 {
            return;
        }
        if self.base_delay == 0 || their_delay < self.base_delay {
            self.base_delay = their_delay;
        }
        let queuing = their_delay.saturating_sub(self.base_delay) as f64;
        let off_target = (TARGET_MICROS - queuing) / TARGET_MICROS;
        let gain = CWND_GAIN * off_target * (acked_bytes as f64) * (MSS as f64)
            / (self.max_window.max(MSS) as f64);
        let next = self.max_window as f64 + gain;
        self.max_window = (next as i64).clamp(MIN_CWND as i64, MAX_CWND as i64) as usize;
    }

    /// Ingest one decoded packet. Returns nothing; mutates buffers/outbox and arms wakers as appropriate
    fn handle_packet(&mut self, header: &UtpHeader, payload: &[u8]) {
        if self.state == State::Closed {
            return;
        }
        self.peer_wnd = header.wnd_size;
        // Measure the one-way delay of *this* packet so we can echo it back
        self.reply_micros = now_micros().wrapping_sub(header.timestamp_micros);

        // Handshake completion: first STATE after our SYN
        if self.state == State::SynSent && header.packet_type == PacketType::State {
            self.state = State::Connected;
            // Peer's STATE carries its next-data seq; we've received nothing yet
            self.ack_nr = header.seq_nr.wrapping_sub(1);
            if let Some(tx) = self.connect_notify.take() {
                let _ = tx.send(Ok(()));
            }
            self.notify_write();
        }

        self.process_ack(header.ack_nr, header.timestamp_diff_micros);
        if let Some(mask) = &header.selective_ack {
            self.process_selective_ack(header.ack_nr, mask);
        }

        match header.packet_type {
            PacketType::Reset => {
                self.fail(io::ErrorKind::ConnectionReset);
                return;
            }
            PacketType::Data | PacketType::Fin => {
                self.accept_inorder(header, payload);
            }
            PacketType::State | PacketType::Syn => {}
        }

        // After a FIN whose sequence we've now reached in order, signal EOF
        if let Some(fin) = self.peer_fin {
            if !seq_after(fin, self.ack_nr) {
                self.eof = true;
                self.notify_read();
            }
        }
        self.maybe_finish();
    }

    /// Place a DATA/FIN payload in order, buffering out-of-order arrivals
    fn accept_inorder(&mut self, header: &UtpHeader, payload: &[u8]) {
        let expected = self.ack_nr.wrapping_add(1);
        if header.seq_nr == expected {
            self.consume(header.packet_type, header.seq_nr, payload);
            // Drain any contiguous reorder-buffer entries
            loop {
                let next = self.ack_nr.wrapping_add(1);
                let Some(buf) = self.reorder.remove(&next) else {
                    break;
                };
                // A buffered FIN is recorded; its (empty) payload adds nothing
                let ty = if Some(next) == self.peer_fin {
                    PacketType::Fin
                } else {
                    PacketType::Data
                };
                self.consume(ty, next, &buf);
            }
            self.needs_ack = true;
        } else if seq_after(header.seq_nr, self.ack_nr) {
            // Record an out-of-order FIN regardless of buffer room: it carries no payload, so it costs nothing, and dropping it could stall EOF until a retransmission refills the reorder buffer
            if header.packet_type == PacketType::Fin {
                self.peer_fin = Some(header.seq_nr);
            }
            // Future packet: buffer it (bounded by the advertised window)
            if self.reorder.len() < RECV_BUF_MAX / MSS {
                self.reorder.insert(header.seq_nr, payload.to_vec());
            }
            self.needs_ack = true;
        } else {
            // Duplicate / already-acked: re-ack so the peer makes progress
            self.needs_ack = true;
        }
    }

    /// Advance `ack_nr` past `seq`, delivering DATA bytes to the reader and recording a FIN
    fn consume(&mut self, ty: PacketType, seq: u16, payload: &[u8]) {
        self.ack_nr = seq;
        if ty == PacketType::Fin {
            self.peer_fin = Some(seq);
        } else if !payload.is_empty() {
            self.recv_ready.extend(payload.iter().copied());
            self.notify_read();
        }
    }

    /// Retransmit timed-out packets; returns the deadline of the next timer
    fn check_retransmit(&mut self) {
        let Some(front) = self.unacked.front() else {
            return;
        };
        if front.sent_at.elapsed() < self.rto {
            return;
        }
        if front.transmissions >= MAX_RETRANSMITS {
            self.fail(io::ErrorKind::TimedOut);
            return;
        }
        // Timeout: collapse the congestion window (TCP-style) and back off RTO
        self.max_window = MIN_CWND;
        self.rto = (self.rto * 2).min(MAX_RTO);
        // Re-send the oldest unacked packet; cumulative acks pull the rest
        let (pt, seq, payload, tx) = {
            let f = self.unacked.front_mut().unwrap();
            f.sent_at = Instant::now();
            f.transmissions += 1;
            (f.packet_type, f.seq_nr, f.payload.clone(), f.transmissions)
        };
        let p = OutPacket {
            packet_type: pt,
            seq_nr: seq,
            payload,
            sent_at: Instant::now(),
            transmissions: tx,
        };
        self.outbox.push(self.encode(&p));
    }

    /// Emit a FIN once the send buffer has drained, then mark FinSent
    fn maybe_send_fin(&mut self) {
        if self.want_fin && self.state == State::Connected && self.send_buf.is_empty() {
            self.transmit_new(PacketType::Fin, Vec::new());
            self.state = State::FinSent;
        }
    }

    /// Transition a half-closed connection to fully closed once our FIN is acked and we've seen the peer's FIN, so the driver can wind down
    fn maybe_finish(&mut self) {
        if self.state == State::FinSent && self.unacked.is_empty() && self.eof {
            self.state = State::Closed;
        }
    }

    /// Next instant the driver must wake to do timer work, if any
    fn next_deadline(&self) -> Option<Instant> {
        self.unacked.front().map(|p| p.sent_at + self.rto)
    }

    fn fail(&mut self, kind: io::ErrorKind) {
        if self.error.is_none() {
            self.error = Some(kind);
        }
        self.state = State::Closed;
        self.eof = true;
        self.unacked.clear();
        self.send_buf.clear();
        if let Some(tx) = self.connect_notify.take() {
            let _ = tx.send(Err(io::Error::from(kind)));
        }
        self.notify_read();
        self.notify_write();
    }

    /// Seed responder state from the initiating SYN: it consumed `syn.seq_nr`, so our first expected DATA is the next sequence number
    pub(crate) fn seed_responder(&mut self, syn: &UtpHeader) {
        self.ack_nr = syn.seq_nr;
        self.peer_wnd = syn.wnd_size;
        self.reply_micros = now_micros().wrapping_sub(syn.timestamp_micros);
    }

    /// Abandon the connection immediately (e.g. on connect timeout) so the driver exits promptly instead of retransmitting to a dead peer
    pub(crate) fn force_close(&mut self) {
        self.state = State::Closed;
        self.error.get_or_insert(io::ErrorKind::TimedOut);
        self.unacked.clear();
        self.send_buf.clear();
    }

    fn notify_read(&mut self) {
        if let Some(w) = self.read_waker.take() {
            w.wake();
        }
    }

    fn notify_write(&mut self) {
        if let Some(w) = self.write_waker.take() {
            w.wake();
        }
    }
}

/// Shared between the [`UtpStream`] handle and its driver task
pub(crate) struct Shared {
    pub(crate) state: Mutex<ConnState>,
    /// Nudges the driver after the app writes / requests shutdown
    pub(crate) nudge: Notify,
}

/// Whether a freshly-created connection initiates (sends a SYN) or responds. Carries the establishment notifier for the initiator; consumed by [`drive`]
pub(crate) enum Role {
    /// Outgoing dial; the driver sends a SYN and reports establishment here
    Initiator(oneshot::Sender<io::Result<()>>),
    /// Inbound connection accepted from a peer's SYN; already Connected
    Responder,
}

/// `Copy` view of [`Role`] used to pick a connection's initial state without consuming the (non-`Clone`) establishment notifier
#[derive(Clone, Copy)]
pub(crate) enum RoleKind {
    Initiator,
    Responder,
}

/// Configuration handed to a connection driver by the socket layer
pub(crate) struct DriverConfig {
    pub transport: DatagramTransport,
    pub remote: SocketAddr,
    pub incoming: mpsc::UnboundedReceiver<(UtpHeader, Bytes)>,
    pub registry: ConnRegistry,
    pub key: ConnKey,
    pub proxy_registry: Option<ProxyConnRegistry>,
}

#[derive(Clone)]
pub(crate) enum DatagramTransport {
    Direct(Arc<UdpSocket>),
    Proxy(Arc<ProxyDatagram>),
}

/// Create the shared state for a new connection
pub(crate) fn new_shared(remote: SocketAddr, conn_id_send: u16, kind: RoleKind) -> Arc<Shared> {
    let state = match kind {
        RoleKind::Initiator => State::SynSent,
        RoleKind::Responder => State::Connected,
    };
    Arc::new(Shared {
        state: Mutex::new(ConnState {
            state,
            remote,
            conn_id_send,
            // Initiator's SYN consumes seq 1, so the next DATA is seq 2. Responder picks a random initial sequence
            seq_nr: match kind {
                RoleKind::Initiator => 2,
                RoleKind::Responder => rand::random::<u16>() | 1,
            },
            ack_nr: 0,
            send_buf: VecDeque::new(),
            unacked: VecDeque::new(),
            recv_ready: VecDeque::new(),
            reorder: BTreeMap::new(),
            peer_wnd: RECV_BUF_MAX as u32,
            max_window: INITIAL_CWND,
            base_delay: 0,
            rtt: 0.0,
            rtt_var: 0.0,
            rto: INITIAL_RTO,
            reply_micros: 0,
            needs_ack: false,
            want_fin: false,
            peer_fin: None,
            eof: false,
            error: None,
            recovery_seq: None,
            outbox: Vec::new(),
            read_waker: None,
            write_waker: None,
            connect_notify: None,
        }),
        nudge: Notify::new(),
    })
}

/// The single task that owns a connection's wire traffic for its lifetime
pub(crate) async fn drive(shared: Arc<Shared>, mut cfg: DriverConfig, role: Role) {
    // Kick off the handshake / initial ack and arm the connect notifier
    {
        let mut st = shared.state.lock();
        if let Role::Initiator(tx) = role {
            st.connect_notify = Some(tx);
            // Send the SYN (seq 1). It lives in `unacked` for retransmission
            let syn = OutPacket {
                packet_type: PacketType::Syn,
                seq_nr: 1,
                payload: Vec::new(),
                sent_at: Instant::now(),
                transmissions: 1,
            };
            let syn_bytes = st.encode(&syn);
            st.outbox.push(syn_bytes);
            st.unacked.push_back(syn);
        } else {
            // Responder: ack_nr was set by the socket from the SYN; send STATE
            let state_bytes = st.encode_state();
            st.outbox.push(state_bytes);
        }
    }
    flush(&shared, &cfg).await;

    let mut closed_since: Option<Instant> = None;
    loop {
        let deadline = {
            let st = shared.state.lock();
            if st.state == State::Closed && closed_since.is_none() {
                closed_since = Some(Instant::now());
            }
            st.next_deadline()
        };

        // Stop lingering once closed and drained
        if let Some(since) = closed_since {
            let drained = {
                let st = shared.state.lock();
                st.unacked.is_empty() && st.send_buf.is_empty()
            };
            if drained || since.elapsed() > LINGER_TIMEOUT {
                break;
            }
        }

        let sleep = async {
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if d > now {
                        tokio::time::sleep(d - now).await;
                    }
                }
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            pkt = cfg.incoming.recv() => {
                match pkt {
                    Some((header, payload)) => {
                        let mut st = shared.state.lock();
                        st.handle_packet(&header, &payload);
                    }
                    None => {
                        // Router dropped our channel; nothing more will arrive
                        shared.state.lock().fail(io::ErrorKind::ConnectionAborted);
                    }
                }
            }
            _ = shared.nudge.notified() => {}
            _ = sleep => {
                shared.state.lock().check_retransmit();
            }
        }

        // Do per-iteration work: drain app writes, emit FIN if requested, send a standalone ack if we owe one
        {
            let mut st = shared.state.lock();
            st.fill_send_window();
            st.maybe_send_fin();
            st.fill_send_window();
            if st.needs_ack && st.state != State::SynSent {
                let ack = st.encode_state();
                st.outbox.push(ack);
                st.needs_ack = false;
            }
            st.maybe_finish();
        }
        flush(&shared, &cfg).await;
    }

    cfg.registry.lock().remove(&cfg.key);
    if let Some(proxy_registry) = &cfg.proxy_registry {
        let mut proxy_registry = proxy_registry.lock();
        if proxy_registry
            .get(&cfg.key.1)
            .is_some_and(|key| *key == cfg.key)
        {
            proxy_registry.remove(&cfg.key.1);
        }
    }
}

/// Drain the outbox to the wire. Datagrams are collected under the lock and sent after releasing it so UDP I/O never blocks the state mutex
async fn flush(shared: &Arc<Shared>, cfg: &DriverConfig) {
    let datagrams: Vec<Vec<u8>> = {
        let mut st = shared.state.lock();
        std::mem::take(&mut st.outbox)
    };
    for d in datagrams {
        let result = match &cfg.transport {
            DatagramTransport::Direct(udp) => udp.send_to(&d, cfg.remote).await.map(|_| ()),
            DatagramTransport::Proxy(proxy) => proxy
                .send_to(&d, cfg.remote)
                .await
                .map(|_| ())
                .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string())),
        };
        if let Err(error) = result {
            shared.state.lock().fail(error.kind());
            break;
        }
    }
}

/// A µTP connection presented as an async byte stream. Plugs into the peer connection layer wherever a `TcpStream` would go
pub struct UtpStream {
    shared: Arc<Shared>,
}

impl UtpStream {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.shared.state.lock().remote
    }
}

impl std::fmt::Debug for UtpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UtpStream")
            .field("peer", &self.peer_addr())
            .finish()
    }
}

impl AsyncRead for UtpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut st = self.shared.state.lock();
        if !st.recv_ready.is_empty() {
            let n = st.recv_ready.len().min(buf.remaining());
            let (first, second) = st.recv_ready.as_slices();
            let first_n = first.len().min(n);
            buf.put_slice(&first[..first_n]);
            if first_n < n {
                buf.put_slice(&second[..n - first_n]);
            }
            st.recv_ready.drain(..n);
            // Reading frees receive-buffer space; the peer learns the larger window on our next outgoing packet, so nudge a fresh ack
            self.shared.nudge.notify_one();
            return Poll::Ready(Ok(()));
        }
        if let Some(kind) = st.error {
            return Poll::Ready(Err(io::Error::from(kind)));
        }
        if st.eof {
            return Poll::Ready(Ok(())); // clean EOF (empty read)
        }
        st.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl AsyncWrite for UtpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut st = self.shared.state.lock();
        if let Some(kind) = st.error {
            return Poll::Ready(Err(io::Error::from(kind)));
        }
        if matches!(st.state, State::FinSent | State::Closed) || st.want_fin {
            return Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)));
        }
        let room = SEND_BUF_MAX.saturating_sub(st.send_buf.len());
        if room == 0 {
            st.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = data.len().min(room);
        st.send_buf.extend(&data[..n]);
        drop(st);
        self.shared.nudge.notify_one();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut st = self.shared.state.lock();
        if let Some(kind) = st.error {
            return Poll::Ready(Err(io::Error::from(kind)));
        }
        if st.send_buf.is_empty() && st.bytes_in_flight() == 0 {
            return Poll::Ready(Ok(()));
        }
        st.write_waker = Some(cx.waker().clone());
        drop(st);
        self.shared.nudge.notify_one();
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut st = self.shared.state.lock();
        if let Some(kind) = st.error {
            return Poll::Ready(Err(io::Error::from(kind)));
        }
        if st.state == State::Closed {
            return Poll::Ready(Ok(()));
        }
        st.want_fin = true;
        // Shutdown completes once the FIN has been sent and acked (no more unacked packets) and the send buffer is empty
        if st.state == State::FinSent && st.unacked.is_empty() {
            return Poll::Ready(Ok(()));
        }
        st.write_waker = Some(cx.waker().clone());
        drop(st);
        self.shared.nudge.notify_one();
        Poll::Pending
    }
}

impl Drop for UtpStream {
    fn drop(&mut self) {
        let mut st = self.shared.state.lock();
        st.want_fin = true;
        st.read_waker = None;
        st.write_waker = None;
        drop(st);
        // Wake the driver so it can emit a FIN and tear down cleanly
        self.shared.nudge.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_after_handles_wraparound() {
        assert!(seq_after(5, 4));
        assert!(!seq_after(4, 5));
        assert!(!seq_after(4, 4));
        // Wraparound: 1 is after 65535
        assert!(seq_after(1, 65535));
        assert!(!seq_after(65535, 1));
    }

    #[test]
    fn fail_clears_in_flight_so_driver_stops_spinning() {
        let shared = new_shared("127.0.0.1:1".parse().unwrap(), 2, RoleKind::Initiator);
        let mut st = shared.state.lock();
        st.state = State::Connected;
        st.unacked.push_back(OutPacket {
            packet_type: PacketType::Data,
            seq_nr: 2,
            payload: vec![0u8; MSS],
            sent_at: Instant::now() - Duration::from_secs(60),
            transmissions: MAX_RETRANSMITS,
        });
        st.send_buf.extend(std::iter::repeat_n(0u8, 100));

        st.fail(io::ErrorKind::TimedOut);
        assert!(st.unacked.is_empty());
        assert!(st.send_buf.is_empty());
        assert!(st.next_deadline().is_none());
    }
}
