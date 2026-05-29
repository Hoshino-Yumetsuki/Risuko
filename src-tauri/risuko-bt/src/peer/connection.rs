//! Per-peer async actor

use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::super::core::Id20;
use super::super::wire::mse::{self, DhKeys, MseRc4, DH_LEN};
use super::super::wire::{Handshake, Message, MessageDecoder, MessageEncoder, HANDSHAKE_LEN};
use rc4::StreamCipher;

/// Encryption behaviour for a single peer connection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncryptionPolicy {
    /// Only plaintext BEP-3 handshakes. Reject MSE
    PlaintextOnly,
    /// Try plaintext first; if the remote rejects it, fall back to MSE
    /// For inbound: accept either (auto-detect by first byte)
    #[default]
    Prefer,
    /// Only accept/initiate encrypted connections
    RequireEncryption,
}

/// Events emitted by the peer task toward the torrent
#[derive(Debug)]
pub enum PeerEvent {
    /// Handshake finished successfully
    Handshook {
        peer_id: Id20,
        reserved: [u8; 8],
        /// Info hash negotiated during the handshake. For outbound
        /// connections this matches the requested hash; for inbound ones it
        /// identifies which torrent the peer wants
        info_hash: Id20,
        /// `true` if the wire is encrypted under MSE/PE
        encrypted: bool,
    },
    /// Decoded peer message
    Message(Message),
    /// Connection ended (clean EOF or error)
    Disconnected { reason: String },
}

/// Commands sent from the torrent to the peer writer task
#[derive(Debug)]
pub enum PeerCommand {
    Send(Message),
    /// Abort the connection
    Disconnect,
}

/// Opaque handle to a running peer task
pub struct PeerHandle {
    pub addr: SocketAddr,
    pub tx: mpsc::Sender<PeerCommand>,
}

/// Builds the BEP-10 extended handshake bytes for a peer given that peer's
/// IP. The connection layer invokes this once per dial / accept after
/// reading the remote BT handshake, so the resulting message can include
/// the `yourip` field set to the peer's actual IP
pub type ExtHandshakeBuilder = Arc<dyn Fn(IpAddr) -> Bytes + Send + Sync>;

/// Parameters for spawning an outbound peer connection
#[derive(Clone)]
pub struct SpawnPeer {
    pub addr: SocketAddr,
    pub info_hash: Id20,
    pub our_peer_id: Id20,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub encryption: EncryptionPolicy,
    pub advertise_v2: bool,
    /// Optional builder for the BEP-10 extended handshake bytes. Invoked by
    /// the connection layer immediately after the BT handshake exchange
    /// completes, inside the same task. Eliminates the multi-hop tokio
    /// latency between "BT handshake done" and "ext handshake on the wire"
    /// and lets callers populate `yourip`. The bytes are only sent when the
    /// remote advertises the BEP-10 reserved bit
    pub ext_handshake_builder: Option<ExtHandshakeBuilder>,
}

#[derive(Clone)]
pub struct KnownInfoHash {
    pub info_hash: Id20,
    pub advertise_v2: bool,
    /// See `SpawnPeer::ext_handshake_builder` — same builder, applied to
    /// the inbound accept path so seeders also get our extended handshake
    /// on the wire as quickly as possible (and with `yourip` set)
    pub ext_handshake_builder: Option<ExtHandshakeBuilder>,
}

impl From<Id20> for KnownInfoHash {
    fn from(info_hash: Id20) -> Self {
        Self {
            info_hash,
            advertise_v2: true,
            ext_handshake_builder: None,
        }
    }
}

/// Connect to a peer, perform the BEP-3 handshake, and split the socket into
/// reader/writer tasks. Returns (handle, event receiver)
pub async fn connect(spawn: SpawnPeer) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let stream = timeout(spawn.connect_timeout, TcpStream::connect(spawn.addr))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"))??;
    drive_handshake(stream, spawn).await
}

/// Run the plaintext BT handshake over an already-established µTP (BEP-29) connection,
/// mirroring [`connect`] for the µTP transport. The peer connection layer is
/// transport-agnostic (see [`finish_spawn`]), so the resulting reader/writer tasks
/// behave identically to the TCP path
///
/// MSE-over-µTP is intentionally not attempted: µTP peers in the swarms this client
/// targets speak plaintext, and TCP remains the path for encrypted peers. The caller
/// obtains `stream` from [`crate::utp::UtpSocket::connect`]
pub async fn connect_utp_plaintext(
    stream: crate::utp::UtpStream,
    spawn: SpawnPeer,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let addr = spawn.addr;
    let (reader, writer) = tokio::io::split(stream);
    connect_plaintext(reader, writer, addr, &spawn).await
}

/// Dial a peer, preferring TCP and falling back to µTP (BEP-29) when the TCP attempt
/// fails (refused, filtered, or timed out). Many peers are reachable over only one
/// transport—notably those behind NATs/ISPs that drop inbound TCP SYNs but pass UDP—so
/// the fallback widens connectivity. With `utp = None` this is exactly [`connect`], so
/// callers without a µTP socket keep the unchanged TCP path
///
/// TCP is tried first rather than racing both at once: TCP carries our fast path today,
/// and a pure fallback avoids opening (then cancelling) a µTP connection for every peer
/// that TCP already reaches
pub async fn connect_with_utp_fallback(
    spawn: SpawnPeer,
    utp: Option<std::sync::Arc<crate::utp::UtpSocket>>,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let Some(utp) = utp else {
        return connect(spawn).await;
    };
    let addr = spawn.addr;
    let utp_timeout = spawn.connect_timeout;
    match connect(spawn.clone()).await {
        Ok(v) => Ok(v),
        Err(tcp_err) => match utp.connect_timeout(addr, utp_timeout).await {
            Ok(stream) => {
                log::debug!("tcp dial to {addr} failed ({tcp_err}); connected via µTP");
                connect_utp_plaintext(stream, spawn).await
            }
            // Surface the TCP error — it's usually the more actionable one.
            Err(_) => Err(tcp_err),
        },
    }
}

/// Accept an inbound peer connection: peer sends handshake first, we reply
/// `known_hashes` is the list of info-hashes the responder currently hosts;
/// it is used both to validate plaintext handshakes and to resolve the
/// obfuscated req2 field in MSE handshakes
pub async fn accept(
    stream: TcpStream,
    our_peer_id: Id20,
    known_hashes: Vec<Id20>,
    read_timeout: Duration,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    accept_with_policy(
        stream,
        our_peer_id,
        known_hashes,
        read_timeout,
        EncryptionPolicy::Prefer,
    )
    .await
}

pub async fn accept_with_policy_and_capabilities(
    stream: TcpStream,
    our_peer_id: Id20,
    known_hashes: Vec<KnownInfoHash>,
    read_timeout: Duration,
    policy: EncryptionPolicy,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    accept_with_policy_inner(stream, our_peer_id, known_hashes, read_timeout, policy).await
}

/// Accept with an explicit encryption policy. The first byte is peeked:
/// `0x13` means plaintext BEP-3; any other byte is treated as the start of
/// an MSE handshake (Ya)
pub async fn accept_with_policy(
    stream: TcpStream,
    our_peer_id: Id20,
    known_hashes: Vec<Id20>,
    read_timeout: Duration,
    policy: EncryptionPolicy,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    accept_with_policy_inner(
        stream,
        our_peer_id,
        known_hashes.into_iter().map(Into::into).collect(),
        read_timeout,
        policy,
    )
    .await
}

async fn accept_with_policy_inner(
    stream: TcpStream,
    our_peer_id: Id20,
    known_hashes: Vec<KnownInfoHash>,
    read_timeout: Duration,
    policy: EncryptionPolicy,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let addr = stream.peer_addr()?;

    // Peek the first 20 bytes (1 pstrlen + 19-byte "BitTorrent protocol")
    // so we can reliably distinguish plaintext from MSE. A single-byte check
    // would misclassify ~1/256 MSE connections whose Ya happens to start with 0x13.
    //
    // `TcpStream::peek` can return fewer than 20 bytes if the peer's handshake
    // arrives slowly, so we loop until we have 20 bytes, see a definitive
    // non-plaintext first byte, or hit the read timeout.
    let mut probe = [0u8; 20];
    let plaintext_first_byte = timeout(read_timeout, async {
        loop {
            let n = stream.peek(&mut probe).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof before handshake",
                ));
            }
            // If the very first byte isn't 0x13, this is definitely not a
            // plaintext BT handshake — no need to wait for more data.
            if probe[0] != 0x13 {
                return Ok(false);
            }
            if n >= 20 {
                return Ok(&probe[1..20] == b"BitTorrent protocol");
            }
            // We have a 0x13 lead but not enough bytes yet; yield and retry.
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "peek timeout"))??;

    if plaintext_first_byte {
        if matches!(policy, EncryptionPolicy::RequireEncryption) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "policy requires encryption; rejecting plaintext",
            ));
        }
        accept_plaintext(stream, addr, our_peer_id, known_hashes, read_timeout).await
    } else {
        if matches!(policy, EncryptionPolicy::PlaintextOnly) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "policy forbids encryption; rejecting MSE",
            ));
        }
        accept_mse(
            stream,
            addr,
            our_peer_id,
            known_hashes,
            read_timeout,
            policy,
        )
        .await
    }
}

async fn accept_plaintext(
    stream: TcpStream,
    addr: SocketAddr,
    our_peer_id: Id20,
    known_hashes: Vec<KnownInfoHash>,
    read_timeout: Duration,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let (reader, writer) = stream.into_split();
    accept_plaintext_generic(
        reader,
        writer,
        addr,
        our_peer_id,
        known_hashes,
        read_timeout,
    )
    .await
}

/// Accept an inbound µTP peer: run the plaintext BEP-3 responder handshake directly
/// over an established [`crate::utp::UtpStream`]. µTP carries no MSE layer (it is its
/// own transport), so unlike the TCP accept path there is no first-byte probe—the BT
/// handshake runs straight on the stream. `known_hashes` lists the
/// info-hashes we currently host; it is used both to validate the peer's handshake and
/// to choose the matching ext-handshake capabilities
pub async fn accept_utp_plaintext(
    stream: crate::utp::UtpStream,
    our_peer_id: Id20,
    known_hashes: Vec<KnownInfoHash>,
    read_timeout: Duration,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let addr = stream.peer_addr();
    let (reader, writer) = tokio::io::split(stream);
    accept_plaintext_generic(
        reader,
        writer,
        addr,
        our_peer_id,
        known_hashes,
        read_timeout,
    )
    .await
}

/// Responder side of the plaintext BEP-3 handshake, generic over the transport
/// so TCP (`into_split` halves) and µTP (`tokio::io::split` halves) share the
/// exact same logic: read the peer's handshake, validate the info-hash against
/// `known_hashes`, reply with ours, optionally write the ext-handshake, then
/// hand off to `finish_spawn`.
async fn accept_plaintext_generic<R, W>(
    mut reader: R,
    mut writer: W,
    addr: SocketAddr,
    our_peer_id: Id20,
    known_hashes: Vec<KnownInfoHash>,
    read_timeout: Duration,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut buf = [0u8; HANDSHAKE_LEN];
    timeout(read_timeout, reader.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "hs read timeout"))??;
    let remote_hs = Handshake::parse(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    let Some(known) = known_hashes
        .iter()
        .find(|known| known.info_hash == remote_hs.info_hash)
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unknown info hash",
        ));
    };
    let our_hs = Handshake::new_with_v2(remote_hs.info_hash, our_peer_id, known.advertise_v2);
    writer.write_all(&our_hs.to_bytes()).await?;
    write_ext_handshake_if_supported(
        &mut writer,
        &remote_hs,
        addr.ip(),
        known.ext_handshake_builder.as_ref(),
    )
    .await?;
    finish_spawn(
        addr,
        our_peer_id,
        remote_hs,
        false,
        Box::new(reader),
        Box::new(writer),
    )
}

async fn drive_handshake(
    stream: TcpStream,
    spawn: SpawnPeer,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let addr = spawn.addr;
    match spawn.encryption {
        EncryptionPolicy::PlaintextOnly => {
            let (reader, writer) = stream.into_split();
            connect_plaintext(reader, writer, addr, &spawn).await
        }
        EncryptionPolicy::RequireEncryption => connect_mse(stream, addr, &spawn).await,
        EncryptionPolicy::Prefer => {
            // Try plaintext first: it's a single 68-byte exchange and
            // succeeds for the overwhelming majority of public-tracker
            // peers. MSE-first wasted a full connect_timeout on every
            // plaintext-only peer because the failed handshake leaves us
            // unable to reuse the socket; we'd have to redial for the
            // fallback. With plaintext-first we only redial for the rare
            // ISP-blocked case.
            //
            // Bound the plaintext attempt by connect_timeout so a peer that
            // accepts the TCP connect but never sends the handshake cannot
            // tie us up for the much longer read_timeout before we fall
            // back to MSE.
            let (reader, writer) = stream.into_split();
            let plaintext = timeout(
                spawn.connect_timeout,
                connect_plaintext(reader, writer, addr, &spawn),
            )
            .await;
            let fallback_err: std::io::Error = match plaintext {
                Ok(Ok(v)) => return Ok(v),
                Ok(Err(e)) => e,
                Err(_) => {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "plaintext handshake timeout")
                }
            };
            log::debug!("plaintext handshake to {addr} failed: {fallback_err}; trying mse");
            let stream = timeout(spawn.connect_timeout, TcpStream::connect(spawn.addr))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout")
                })??;
            // Log the MSE outcome so a debug-level log captures whether the
            // encrypted fallback ever succeeds. Without this, an operator
            // troubleshooting "0 KB/s" only sees N "trying mse" lines and
            // cannot tell whether (a) every peer also rejects MSE so no
            // outbound peer ever connects, or (b) MSE works fine and the
            // download is slow for some other reason
            match connect_mse(stream, addr, &spawn).await {
                Ok(v) => {
                    log::debug!("mse handshake to {addr} succeeded");
                    Ok(v)
                }
                Err(e) => {
                    log::debug!("mse handshake to {addr} failed: {e}");
                    Err(e)
                }
            }
        }
    }
}

/// Run the plaintext BEP-3 handshake over an already-split byte stream and
/// spawn the reader/writer tasks. Generic over the transport so both TCP
/// (`OwnedReadHalf`/`OwnedWriteHalf`) and µTP (`tokio::io::split` halves) can
/// share the exact same handshake logic.
async fn connect_plaintext<R, W>(
    mut reader: R,
    mut writer: W,
    addr: SocketAddr,
    spawn: &SpawnPeer,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let our_hs = Handshake::new_with_v2(spawn.info_hash, spawn.our_peer_id, spawn.advertise_v2);
    writer.write_all(&our_hs.to_bytes()).await?;

    let mut buf = [0u8; HANDSHAKE_LEN];
    timeout(spawn.read_timeout, reader.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "hs read timeout"))??;
    let remote_hs = Handshake::parse(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    if remote_hs.info_hash != spawn.info_hash {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "info hash mismatch",
        ));
    }
    write_ext_handshake_if_supported(
        &mut writer,
        &remote_hs,
        addr.ip(),
        spawn.ext_handshake_builder.as_ref(),
    )
    .await?;
    finish_spawn(
        addr,
        spawn.our_peer_id,
        remote_hs,
        false,
        Box::new(reader),
        Box::new(writer),
    )
}

/// Perform the BEP-8 MSE handshake as the initiator (A), then send the BEP-3
/// handshake over the now-encrypted stream
async fn connect_mse(
    stream: TcpStream,
    addr: SocketAddr,
    spawn: &SpawnPeer,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let (mut read_h, mut write_h) = stream.into_split();
    let read_timeout = spawn.read_timeout;

    // Step 1: A -> B: Ya || PadA (0..512 bytes)
    let keys = DhKeys::generate();
    let pad_a = mse::gen_pad(512);
    let mut out = Vec::with_capacity(DH_LEN + pad_a.len());
    out.extend_from_slice(&keys.public_be);
    out.extend_from_slice(&pad_a);
    write_h.write_all(&out).await?;

    // Step 2: read Yb (96 bytes); PadB follows but we don't know its length
    // until we sync on HASH('req1', S) — which is how PadB is implicitly
    // delimited on our side
    let mut yb = [0u8; DH_LEN];
    timeout(read_timeout, read_h.read_exact(&mut yb))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse yb timeout"))??;
    let s = keys.shared_secret(&yb)?;

    // Derive RC4 keys now that we have S
    let skey: [u8; 20] = spawn.info_hash.0;
    let key_a = mse::rc4_key(b"keyA", &s, &skey); // initiator writes with this
    let key_b = mse::rc4_key(b"keyB", &s, &skey); // initiator reads with this
    let mut enc_out = mse::init_rc4(&key_a);
    let mut dec_in = mse::init_rc4(&key_b);

    // Step 3: A -> B: HASH('req1',S) || HASH('req2',SKEY)^HASH('req3',S)
    //                  || ENCRYPT(VC || crypto_provide || len(PadC) || PadC
    //                           || len(IA) || IA)
    let req1 = mse::req1(&s);
    let req2 = mse::req2(&skey);
    let req3 = mse::req3(&s);
    let req23 = mse::xor20(&req2, &req3);

    let pad_c = mse::gen_pad(512);
    let mut crypto_provide = mse::crypto::RC4;
    if !matches!(spawn.encryption, EncryptionPolicy::RequireEncryption) {
        crypto_provide |= mse::crypto::PLAINTEXT;
    }
    // Send IA = our BT handshake immediately so the responder can begin on
    // its very first reply packet — saves a round trip
    let our_hs_bytes =
        Handshake::new_with_v2(spawn.info_hash, spawn.our_peer_id, spawn.advertise_v2).to_bytes();
    let mut payload = mse::build_initiator_payload(crypto_provide, &pad_c, &our_hs_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    enc_out.apply_keystream(&mut payload);

    let mut to_send = Vec::with_capacity(20 + 20 + payload.len());
    to_send.extend_from_slice(&req1);
    to_send.extend_from_slice(&req23);
    to_send.extend_from_slice(&payload);
    write_h.write_all(&to_send).await?;

    // Step 4: scan B's reply for encrypted VC. Because rc4 0.2 does not
    // expose Clone, and we cannot "rewind" an RC4 stream, we instead
    // materialise the first `max_scan` bytes of our decrypt keystream up
    // front: XORing zero bytes through `dec_in` produces the keystream,
    // which we consume in-place while scanning for any offset where
    // ciphertext XOR keystream[off..off+8] == VC (all-zero), i.e. where
    // ciphertext[off..off+8] == keystream[off..off+8]
    let max_scan = 1100usize;
    let mut keystream = vec![0u8; max_scan];
    dec_in.apply_keystream(&mut keystream);
    // dec_in has now advanced by max_scan bytes; we'll reset it below once
    // we know the offset. To do that we recreate dec_in from the key
    let mut recv = Vec::with_capacity(max_scan);
    let mut chunk = [0u8; 256];
    let mut found_offset: Option<usize> = None;
    let deadline = tokio::time::Instant::now() + read_timeout;
    while recv.len() < max_scan {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse vc scan timeout",
            ));
        }
        let rn = match timeout(remaining, read_h.read(&mut chunk)).await {
            Ok(Ok(0)) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof during mse vc scan",
                ));
            }
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "mse vc read timeout",
                ));
            }
        };
        recv.extend_from_slice(&chunk[..rn]);

        // Scan new candidate offsets. The responder encrypts with keyB
        // starting at its keystream byte 0, so the first 8 encrypted bytes
        // of its reply (which are VC = 00..00) equal keystream[0..8]. PadB
        // is unencrypted and precedes the encrypted region, so the match
        // offset in `recv` is `pad_b.len()`. We detect it by scanning for
        // any offset `off` where recv[off..off+8] == keystream[0..8]
        if recv.len() >= 8 && keystream.len() >= 8 {
            let needle = &keystream[..8];
            for off in 0..=(recv.len() - 8) {
                if &recv[off..off + 8] == needle {
                    found_offset = Some(off);
                    break;
                }
            }
        }
        if found_offset.is_some() {
            break;
        }
    }
    let off = found_offset
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "mse vc not found"))?;

    // Rebuild dec_in from the key. The encrypted region begins at `off` in
    // `recv`, and B's cipher starts at its keystream byte 0 there — so we
    // do NOT fast-forward; instead we simply discard the `off` bytes of
    // PadB that preceded it
    let mut dec_in = mse::init_rc4(&key_b);
    // We have recv[off..] pending; need at least 8 (VC) + 4 (select) + 2 (len) = 14 bytes
    let tail_start = off;
    let mut tail = recv[tail_start..].to_vec();
    // If we don't yet have 14 bytes, read more encrypted bytes
    while tail.len() < 14 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse header timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse header"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in mse header",
            ));
        }
        tail.extend_from_slice(&chunk[..n]);
    }
    dec_in.apply_keystream(&mut tail[..14]);
    // tail[0..8] now equals VC (sanity check already done)
    let crypto_select = u32::from_be_bytes([tail[8], tail[9], tail[10], tail[11]]);
    let pad_d_len = u16::from_be_bytes([tail[12], tail[13]]) as usize;
    if pad_d_len > 512 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pad_d too long",
        ));
    }

    // Ensure we have PadD buffered (and decrypt it), then anything after is
    // already-plaintext-in-our-BT-sense data
    while tail.len() < 14 + pad_d_len {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse padd timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse padd"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in pad_d",
            ));
        }
        tail.extend_from_slice(&chunk[..n]);
    }
    if pad_d_len > 0 {
        dec_in.apply_keystream(&mut tail[14..14 + pad_d_len]);
    }
    let leftover = tail[14 + pad_d_len..].to_vec();

    // Decide whether to use RC4 or plaintext for the ongoing stream
    let use_rc4 = if crypto_select & mse::crypto::RC4 != 0 {
        true
    } else if crypto_select & mse::crypto::PLAINTEXT != 0 {
        if matches!(spawn.encryption, EncryptionPolicy::RequireEncryption) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "peer selected plaintext but policy requires encryption",
            ));
        }
        false
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("peer returned unsupported crypto_select {crypto_select:#x}"),
        ));
    };

    // Now read the peer's BT handshake from the still-encrypted (or now
    // plaintext) stream, starting with any leftover bytes we already buffered
    let (reader_boxed, mut writer_boxed, remote_hs): (
        Box<dyn AsyncRead + Unpin + Send>,
        Box<dyn AsyncWrite + Unpin + Send>,
        Handshake,
    ) = if use_rc4 {
        let mut pending = leftover;
        // Read BT handshake (68 bytes) from pending + stream, decrypting as
        // we go.
        let mut hs_bytes = Vec::with_capacity(HANDSHAKE_LEN);
        while pending.len() < HANDSHAKE_LEN {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "mse hs timeout",
                ));
            }
            let n = timeout(remaining, read_h.read(&mut chunk))
                .await
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse hs"))??;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof in encrypted bt hs",
                ));
            }
            pending.extend_from_slice(&chunk[..n]);
        }
        hs_bytes.extend_from_slice(&pending[..HANDSHAKE_LEN]);
        dec_in.apply_keystream(&mut hs_bytes);
        let rest = pending[HANDSHAKE_LEN..].to_vec();
        let mut hs_arr = [0u8; HANDSHAKE_LEN];
        hs_arr.copy_from_slice(&hs_bytes);
        let remote_hs = Handshake::parse(&hs_arr)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
        if remote_hs.info_hash != spawn.info_hash {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "info hash mismatch after mse",
            ));
        }
        let r = Rc4ReadHalf::new(read_h, dec_in, rest);
        let w = Rc4WriteHalf::new(write_h, enc_out);
        (Box::new(r), Box::new(w), remote_hs)
    } else {
        // Plaintext after MSE negotiation. Leftover bytes are already BT
        // handshake beginning
        let mut pending = leftover;
        while pending.len() < HANDSHAKE_LEN {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "mse/plain hs timeout",
                ));
            }
            let n = timeout(remaining, read_h.read(&mut chunk))
                .await
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse/plain hs"))??;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "eof in plain bt hs",
                ));
            }
            pending.extend_from_slice(&chunk[..n]);
        }
        let hs_bytes = &pending[..HANDSHAKE_LEN];
        let rest = pending[HANDSHAKE_LEN..].to_vec();
        let mut hs_arr = [0u8; HANDSHAKE_LEN];
        hs_arr.copy_from_slice(hs_bytes);
        let remote_hs = Handshake::parse(&hs_arr)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
        if remote_hs.info_hash != spawn.info_hash {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "info hash mismatch after mse",
            ));
        }
        let r = PrefixedReadHalf::new(read_h, rest);
        (Box::new(r), Box::new(write_h), remote_hs)
    };

    write_ext_handshake_if_supported(
        &mut *writer_boxed,
        &remote_hs,
        addr.ip(),
        spawn.ext_handshake_builder.as_ref(),
    )
    .await?;
    finish_spawn(
        addr,
        spawn.our_peer_id,
        remote_hs,
        use_rc4,
        reader_boxed,
        writer_boxed,
    )
}

/// Accept an MSE connection as responder (B)
async fn accept_mse(
    stream: TcpStream,
    addr: SocketAddr,
    our_peer_id: Id20,
    known_hashes: Vec<KnownInfoHash>,
    read_timeout: Duration,
    policy: EncryptionPolicy,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let (mut read_h, mut write_h) = stream.into_split();
    let deadline = tokio::time::Instant::now() + read_timeout;

    // Read Ya (96 bytes)
    let mut ya = [0u8; DH_LEN];
    timeout(read_timeout, read_h.read_exact(&mut ya))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse ya timeout"))??;

    // Generate our keys and send Yb || PadB
    let keys = DhKeys::generate();
    let pad_b = mse::gen_pad(512);
    let mut out = Vec::with_capacity(DH_LEN + pad_b.len());
    out.extend_from_slice(&keys.public_be);
    out.extend_from_slice(&pad_b);
    write_h.write_all(&out).await?;

    let s = keys.shared_secret(&ya)?;
    // Search A's stream for HASH('req1', S) marker — this delimits PadA
    let req1 = mse::req1(&s);
    let mut recv: Vec<u8> = Vec::with_capacity(1200);
    let mut chunk = [0u8; 256];
    let req1_off = loop {
        if recv.len() > 512 + 20 + 20 + 8 + 4 + 2 + 512 + HANDSHAKE_LEN + 256 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mse req1 not found in window",
            ));
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse req1 timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse req1"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof before req1",
            ));
        }
        recv.extend_from_slice(&chunk[..n]);
        if let Some(off) = mse::find_subsequence(&recv, &req1) {
            break off;
        }
    };

    // Fetch req2^req3 (20 bytes right after req1)
    while recv.len() < req1_off + 40 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse req2 timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse req2"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in req2",
            ));
        }
        recv.extend_from_slice(&chunk[..n]);
    }
    let mut req23 = [0u8; 20];
    req23.copy_from_slice(&recv[req1_off + 20..req1_off + 40]);
    let req3_s = mse::req3(&s);
    let want_req2 = mse::xor20(&req23, &req3_s);
    // Resolve SKEY by brute force over known info hashes
    let mut known_info: Option<KnownInfoHash> = None;
    for known in &known_hashes {
        if mse::req2(&known.info_hash.0) == want_req2 {
            known_info = Some(known.clone());
            break;
        }
    }
    let known_info = known_info.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "mse: unknown info hash")
    })?;
    let skey = known_info.info_hash;

    // Now derive RC4 keys and decrypt the rest of A's third message
    let key_a = mse::rc4_key(b"keyA", &s, &skey.0); // A writes (we decrypt with keyA)
    let key_b = mse::rc4_key(b"keyB", &s, &skey.0); // we write with keyB
    let mut dec_in = mse::init_rc4(&key_a);
    let mut enc_out = mse::init_rc4(&key_b);

    // After req1||req23 comes: ENCRYPT(VC||crypto_provide||len(PadC)||PadC||len(IA)||IA)
    let enc_start = req1_off + 40;
    let mut enc_buf = recv[enc_start..].to_vec();
    // Need at least 8+4+2 = 14 bytes before we can learn PadC length
    while enc_buf.len() < 14 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse ia-head timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse ia-head"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in ia-head",
            ));
        }
        enc_buf.extend_from_slice(&chunk[..n]);
    }
    // Decrypt the 14-byte header
    dec_in.apply_keystream(&mut enc_buf[..14]);
    if enc_buf[0..8] != mse::vc() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mse: vc mismatch after req2 resolve",
        ));
    }
    let crypto_provide = u32::from_be_bytes([enc_buf[8], enc_buf[9], enc_buf[10], enc_buf[11]]);
    let pad_c_len = u16::from_be_bytes([enc_buf[12], enc_buf[13]]) as usize;
    if pad_c_len > 512 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pad_c too long",
        ));
    }
    // Need PadC + len(IA) (2 bytes)
    while enc_buf.len() < 14 + pad_c_len + 2 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse padc timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse padc"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in padc",
            ));
        }
        enc_buf.extend_from_slice(&chunk[..n]);
    }
    dec_in.apply_keystream(&mut enc_buf[14..14 + pad_c_len + 2]);
    let ia_len_off = 14 + pad_c_len;
    let ia_len = u16::from_be_bytes([enc_buf[ia_len_off], enc_buf[ia_len_off + 1]]) as usize;
    if ia_len > 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ia too long",
        ));
    }
    let ia_off = ia_len_off + 2;
    while enc_buf.len() < ia_off + ia_len {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "mse ia timeout",
            ));
        }
        let n = timeout(remaining, read_h.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "mse ia"))??;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in ia",
            ));
        }
        enc_buf.extend_from_slice(&chunk[..n]);
    }
    if ia_len > 0 {
        dec_in.apply_keystream(&mut enc_buf[ia_off..ia_off + ia_len]);
    }
    let ia_bytes = enc_buf[ia_off..ia_off + ia_len].to_vec();
    let leftover = enc_buf[ia_off + ia_len..].to_vec();

    // Choose crypto_select: prefer RC4; fall back to plaintext if allowed
    // and requested. Require encryption policy forces RC4
    let allow_plain = !matches!(policy, EncryptionPolicy::RequireEncryption);
    let crypto_select = if crypto_provide & mse::crypto::RC4 != 0 {
        mse::crypto::RC4
    } else if allow_plain && crypto_provide & mse::crypto::PLAINTEXT != 0 {
        mse::crypto::PLAINTEXT
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("no acceptable crypto (provide={crypto_provide:#x})"),
        ));
    };

    // Send ENCRYPT(VC || crypto_select || len(PadD) || PadD)
    let pad_d = mse::gen_pad(512);
    let mut reply = mse::build_responder_payload(crypto_select, &pad_d)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    enc_out.apply_keystream(&mut reply);
    write_h.write_all(&reply).await?;

    // IA may contain A's BT handshake. MSE peers are allowed to send an
    // empty IA and defer the BT handshake to the encrypted stream.
    if !ia_bytes.is_empty() && ia_bytes.len() < HANDSHAKE_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ia shorter than bt handshake",
        ));
    }
    let remote_hs_from_ia = if !ia_bytes.is_empty() {
        let mut ia_arr = [0u8; HANDSHAKE_LEN];
        ia_arr.copy_from_slice(&ia_bytes[..HANDSHAKE_LEN]);
        let hs = Handshake::parse(&ia_arr)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
        if hs.info_hash != skey {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "info hash mismatch between req2 and ia",
            ));
        }
        Some(hs)
    } else {
        None
    };

    // Send our BT handshake, encrypted if RC4 selected
    let our_hs = Handshake::new_with_v2(skey, our_peer_id, known_info.advertise_v2).to_bytes();
    if crypto_select == mse::crypto::RC4 {
        let mut encoded = our_hs.to_vec();
        enc_out.apply_keystream(&mut encoded);
        write_h.write_all(&encoded).await?;
        let mut r = Rc4ReadHalf::new(
            read_h, dec_in,
            // Any bytes A sent after IA belong on the encrypted stream;
            // they are already decrypted up to ia_off+ia_len (in enc_buf)
            // but any after that are still encrypted. Wait — leftover is
            // the raw, still-encrypted tail; the Rc4ReadHalf will decrypt
            // as it streams
            leftover,
        );
        let remote_hs = match remote_hs_from_ia {
            Some(hs) => hs,
            None => {
                // Peer deferred BT handshake; read it from the encrypted stream
                let mut buf = [0u8; HANDSHAKE_LEN];
                timeout(read_timeout, r.read_exact(&mut buf))
                    .await
                    .map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "mse deferred hs timeout")
                    })??;
                let hs = Handshake::parse(&buf).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}"))
                })?;
                if hs.info_hash != skey {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "info hash mismatch in deferred handshake",
                    ));
                }
                hs
            }
        };
        let mut w = Rc4WriteHalf::new(write_h, enc_out);
        write_ext_handshake_if_supported(
            &mut w,
            &remote_hs,
            addr.ip(),
            known_info.ext_handshake_builder.as_ref(),
        )
        .await?;
        finish_spawn(addr, our_peer_id, remote_hs, true, Box::new(r), Box::new(w))
    } else {
        write_h.write_all(&our_hs).await?;
        let r = PrefixedReadHalf::new(read_h, leftover);
        let remote_hs = match remote_hs_from_ia {
            Some(hs) => hs,
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "empty ia with plaintext crypto not supported",
                ));
            }
        };
        write_ext_handshake_if_supported(
            &mut write_h,
            &remote_hs,
            addr.ip(),
            known_info.ext_handshake_builder.as_ref(),
        )
        .await?;
        finish_spawn(
            addr,
            our_peer_id,
            remote_hs,
            false,
            Box::new(r),
            Box::new(write_h),
        )
    }
}

/// Write our BEP-10 extended handshake to the wire if the remote advertised
/// the extension-protocol reserved bit. The bytes are produced by `builder`
/// per-peer so the message can include the peer's IP in the `yourip` field
/// Called inline by every connection-establishment path (plaintext + MSE,
/// inbound + outbound) so the message ships in the same async frame that
/// just completed the BT handshake — no event-channel hop, no writer-task
/// hop. Some real-world peers RST the connection if our follow-up doesn't
/// arrive promptly
async fn write_ext_handshake_if_supported<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    remote_hs: &Handshake,
    peer_ip: IpAddr,
    builder: Option<&ExtHandshakeBuilder>,
) -> std::io::Result<()> {
    if !remote_hs.has_ext_protocol() {
        return Ok(());
    }
    let Some(builder) = builder else {
        return Ok(());
    };
    let bytes = builder(peer_ip);
    writer.write_all(&bytes).await
}

fn finish_spawn(
    addr: SocketAddr,
    our_peer_id: Id20,
    remote_hs: Handshake,
    encrypted: bool,
    reader: Box<dyn AsyncRead + Unpin + Send>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    // Reject self-connections: the DHT can hand us our own externally-mapped
    // address, and without this we complete a full handshake with ourselves,
    // burning a peer slot on a connection that can never serve data
    if remote_hs.peer_id == our_peer_id {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "self-connection (peer_id matches ours)",
        ));
    }
    // Sized for high-throughput pipelining: with up to ~64 outstanding
    // chunk requests and Piece replies of 16 KiB arriving back-to-back,
    // a 64-slot channel would backpressure the reader and cap throughput
    let (event_tx, event_rx) = mpsc::channel(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel(1024);

    // Deliver the handshake synchronously before spawning the reader so that
    // consumers always observe `Handshook` before any peer messages. The
    // channel was just created with capacity 1024 so this cannot block
    event_tx
        .try_send(PeerEvent::Handshook {
            peer_id: remote_hs.peer_id,
            reserved: remote_hs.reserved,
            info_hash: remote_hs.info_hash,
            encrypted,
        })
        .map_err(|e| std::io::Error::other(format!("{e}")))?;

    tokio::spawn(reader_task(reader, event_tx));
    tokio::spawn(writer_task(writer, cmd_rx));

    Ok((PeerHandle { addr, tx: cmd_tx }, event_rx))
}

/// Read half with a replay prefix: yields `prefix` bytes before delegating
/// to the underlying stream. Used to "un-peek" handshake tail bytes we had
/// to buffer during MSE negotiation
struct PrefixedReadHalf {
    inner: tokio::net::tcp::OwnedReadHalf,
    prefix: Vec<u8>,
    pos: usize,
}

impl PrefixedReadHalf {
    fn new(inner: tokio::net::tcp::OwnedReadHalf, prefix: Vec<u8>) -> Self {
        Self {
            inner,
            prefix,
            pos: 0,
        }
    }
}

impl AsyncRead for PrefixedReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.prefix.len() {
            let remaining = &self.prefix[self.pos..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

/// RC4-encrypting read half. Any residual bytes we already pulled from the
/// socket while scanning the MSE handshake are replayed via `prefix`; they
/// are *still encrypted* under the peer's key, so the same cipher applies
struct Rc4ReadHalf {
    inner: tokio::net::tcp::OwnedReadHalf,
    cipher: MseRc4,
    prefix: Vec<u8>,
    pos: usize,
}

impl Rc4ReadHalf {
    fn new(inner: tokio::net::tcp::OwnedReadHalf, cipher: MseRc4, prefix: Vec<u8>) -> Self {
        Self {
            inner,
            cipher,
            prefix,
            pos: 0,
        }
    }
}

impl AsyncRead for Rc4ReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.prefix.len() {
            let remaining = self.prefix.len() - self.pos;
            let n = remaining.min(buf.remaining());
            let start = self.pos;
            let end = start + n;
            // Decrypt in place on a scratch copy, then copy into buf
            let mut scratch = self.prefix[start..end].to_vec();
            self.cipher.apply_keystream(&mut scratch);
            buf.put_slice(&scratch);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        let filled_before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let filled_after = buf.filled().len();
                if filled_after > filled_before {
                    let me = &mut *self;
                    me.cipher
                        .apply_keystream(&mut buf.filled_mut()[filled_before..filled_after]);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// RC4-encrypting write half. Writes are buffered into a small scratch
/// vector, encrypted, then written through. We keep scratch sized to each
/// call to avoid a long-lived heap buffer
struct Rc4WriteHalf {
    inner: tokio::net::tcp::OwnedWriteHalf,
    cipher: MseRc4,
    pending: Vec<u8>,
    pending_off: usize,
}

impl Rc4WriteHalf {
    fn new(inner: tokio::net::tcp::OwnedWriteHalf, cipher: MseRc4) -> Self {
        Self {
            inner,
            cipher,
            pending: Vec::new(),
            pending_off: 0,
        }
    }
}

impl AsyncWrite for Rc4WriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        // Flush any pending ciphertext first so we always commit bytes in order
        while self.pending_off < self.pending.len() {
            let me = &mut *self;
            let slice = &me.pending[me.pending_off..];
            match Pin::new(&mut me.inner).poll_write(cx, slice) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "rc4 write zero",
                    )));
                }
                Poll::Ready(Ok(n)) => {
                    me.pending_off += n;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        if self.pending_off >= self.pending.len() && !self.pending.is_empty() {
            self.pending.clear();
            self.pending_off = 0;
        }

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // Encrypt the caller's buffer into pending, then try to write in the same poll
        let mut scratch = buf.to_vec();
        self.cipher.apply_keystream(&mut scratch);
        self.pending = scratch;
        self.pending_off = 0;
        let consumed = buf.len();
        // Opportunistically flush
        while self.pending_off < self.pending.len() {
            let me = &mut *self;
            let slice = &me.pending[me.pending_off..];
            match Pin::new(&mut me.inner).poll_write(cx, slice) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "rc4 write zero",
                    )));
                }
                Poll::Ready(Ok(n)) => {
                    me.pending_off += n;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    // Report the caller's bytes as consumed; the ciphertext
                    // is safely queued in `pending`
                    return Poll::Ready(Ok(consumed));
                }
            }
        }
        self.pending.clear();
        self.pending_off = 0;
        Poll::Ready(Ok(consumed))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        while self.pending_off < self.pending.len() {
            let me = &mut *self;
            let slice = &me.pending[me.pending_off..];
            match Pin::new(&mut me.inner).poll_write(cx, slice) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "rc4 flush zero",
                    )));
                }
                Poll::Ready(Ok(n)) => me.pending_off += n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.pending.clear();
        self.pending_off = 0;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // Drain any ciphertext that poll_write queued in `pending`.
        while self.pending_off < self.pending.len() {
            let me = &mut *self;
            let slice = &me.pending[me.pending_off..];
            match Pin::new(&mut me.inner).poll_write(cx, slice) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "rc4 shutdown flush zero",
                    )));
                }
                Poll::Ready(Ok(n)) => me.pending_off += n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        self.pending.clear();
        self.pending_off = 0;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn reader_task(mut reader: Box<dyn AsyncRead + Unpin + Send>, tx: mpsc::Sender<PeerEvent>) {
    // Larger temp buffer => fewer read syscalls per Piece message. Each
    // Piece reply is up to 16 KiB of payload + 13 B header; 64 KiB lets us
    // ingest several pipelined replies per syscall
    let mut buf = BytesMut::with_capacity(256 * 1024);
    let mut tmp = vec![0u8; 64 * 1024];
    loop {
        // Try to decode any complete frame already buffered
        loop {
            match MessageDecoder::try_decode(&mut buf) {
                Ok(Some(msg)) => {
                    if tx.send(PeerEvent::Message(msg)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx
                        .send(PeerEvent::Disconnected {
                            reason: format!("decode: {e}"),
                        })
                        .await;
                    return;
                }
            }
        }
        match reader.read(&mut tmp).await {
            Ok(0) => {
                let _ = tx
                    .send(PeerEvent::Disconnected {
                        reason: "eof".into(),
                    })
                    .await;
                return;
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) => {
                let _ = tx
                    .send(PeerEvent::Disconnected {
                        reason: format!("io: {e}"),
                    })
                    .await;
                return;
            }
        }
    }
}

async fn writer_task(
    mut writer: Box<dyn AsyncWrite + Unpin + Send>,
    mut rx: mpsc::Receiver<PeerCommand>,
) {
    // Coalesce all commands currently queued into a single write_all so a
    // burst of 128 pipelined Request frames becomes one syscall instead of
    // 128. Per-message write_all + write_all overhead was a major
    // bottleneck on high-fan-in torrents
    let mut batch = BytesMut::with_capacity(64 * 1024);
    while let Some(first) = rx.recv().await {
        batch.clear();
        let mut disconnect = false;
        match first {
            PeerCommand::Send(msg) => batch.extend_from_slice(&MessageEncoder::encode(&msg)),
            PeerCommand::Disconnect => disconnect = true,
        }
        // Drain anything else already queued without awaiting
        while !disconnect {
            match rx.try_recv() {
                Ok(PeerCommand::Send(msg)) => {
                    batch.extend_from_slice(&MessageEncoder::encode(&msg));
                    // Cap batch size so a flood doesn't grow unbounded
                    if batch.len() >= 256 * 1024 {
                        break;
                    }
                }
                Ok(PeerCommand::Disconnect) => {
                    disconnect = true;
                    break;
                }
                Err(_) => break,
            }
        }
        if !batch.is_empty() && writer.write_all(&batch).await.is_err() {
            return;
        }
        if disconnect {
            let _ = writer.shutdown().await;
            return;
        }
    }
    let _ = writer.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Message;
    use tokio::net::TcpListener;

    async fn run_pair() -> (
        PeerHandle,
        mpsc::Receiver<PeerEvent>,
        PeerHandle,
        mpsc::Receiver<PeerEvent>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let info_hash = Id20([1u8; 20]);
        let peer_a = Id20([2u8; 20]);
        let peer_b = Id20([3u8; 20]);

        let accept_fut = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept(stream, peer_b, vec![info_hash], Duration::from_secs(5))
                .await
                .unwrap()
        });

        let (handle_a, rx_a) = connect(SpawnPeer {
            addr,
            info_hash,
            our_peer_id: peer_a,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
            encryption: EncryptionPolicy::PlaintextOnly,
            advertise_v2: true,
            ext_handshake_builder: None,
        })
        .await
        .unwrap();

        let (handle_b, rx_b) = accept_fut.await.unwrap();
        (handle_a, rx_a, handle_b, rx_b)
    }

    #[tokio::test]
    async fn handshake_and_message() {
        let (a, mut rx_a, b, mut rx_b) = run_pair().await;

        // Both sides should see a Handshook event
        assert!(matches!(
            rx_a.recv().await.unwrap(),
            PeerEvent::Handshook { .. }
        ));
        assert!(matches!(
            rx_b.recv().await.unwrap(),
            PeerEvent::Handshook { .. }
        ));

        a.tx.send(PeerCommand::Send(Message::Interested))
            .await
            .unwrap();
        let ev = rx_b.recv().await.unwrap();
        match ev {
            PeerEvent::Message(Message::Interested) => {}
            _ => panic!("expected Interested, got {ev:?}"),
        }
        b.tx.send(PeerCommand::Send(Message::Have { piece_index: 42 }))
            .await
            .unwrap();
        let ev = rx_a.recv().await.unwrap();
        match ev {
            PeerEvent::Message(Message::Have { piece_index }) => assert_eq!(piece_index, 42),
            _ => panic!("expected Have"),
        }
    }

    #[tokio::test]
    async fn rejects_wrong_info_hash() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hash_a = Id20([1u8; 20]);
        let hash_b = Id20([9u8; 20]);
        let peer_a = Id20([2u8; 20]);
        let peer_b = Id20([3u8; 20]);

        let accept_fut = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept(stream, peer_b, vec![hash_b], Duration::from_secs(5)).await
        });

        let res = connect(SpawnPeer {
            addr,
            info_hash: hash_a,
            our_peer_id: peer_a,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
            encryption: EncryptionPolicy::PlaintextOnly,
            advertise_v2: true,
            ext_handshake_builder: None,
        })
        .await;
        assert!(res.is_err() || accept_fut.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn mse_end_to_end_handshake_and_messages() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let info_hash = Id20([7u8; 20]);
        let peer_a = Id20([8u8; 20]);
        let peer_b = Id20([9u8; 20]);

        let accept_fut = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept_with_policy(
                stream,
                peer_b,
                vec![info_hash],
                Duration::from_secs(10),
                EncryptionPolicy::RequireEncryption,
            )
            .await
            .unwrap()
        });

        let (a, mut rx_a) = connect(SpawnPeer {
            addr,
            info_hash,
            our_peer_id: peer_a,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(10),
            encryption: EncryptionPolicy::RequireEncryption,
            advertise_v2: true,
            ext_handshake_builder: None,
        })
        .await
        .unwrap();
        let (b, mut rx_b) = accept_fut.await.unwrap();

        // Both sides observe an encrypted handshake
        match rx_a.recv().await.unwrap() {
            PeerEvent::Handshook { encrypted, .. } => assert!(encrypted),
            e => panic!("unexpected event: {e:?}"),
        }
        match rx_b.recv().await.unwrap() {
            PeerEvent::Handshook { encrypted, .. } => assert!(encrypted),
            e => panic!("unexpected event: {e:?}"),
        }

        a.tx.send(PeerCommand::Send(Message::Interested))
            .await
            .unwrap();
        match rx_b.recv().await.unwrap() {
            PeerEvent::Message(Message::Interested) => {}
            e => panic!("unexpected: {e:?}"),
        }
        b.tx.send(PeerCommand::Send(Message::Have { piece_index: 7 }))
            .await
            .unwrap();
        match rx_a.recv().await.unwrap() {
            PeerEvent::Message(Message::Have { piece_index }) => assert_eq!(piece_index, 7),
            e => panic!("unexpected: {e:?}"),
        }
    }

    #[tokio::test]
    async fn utp_plaintext_handshake_and_message_over_loopback() {
        use crate::utp::UtpSocket;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let info_hash = Id20([7u8; 20]);
        let client_id = Id20([1u8; 20]);
        let server_id = Id20([2u8; 20]);

        let server_sock = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let client_sock = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let server_addr = server_sock.local_addr();

        // Minimal peer: accept a µTP stream, exchange BT handshakes, then push
        // one peer message so the client's reader task surfaces it.
        let server = tokio::spawn(async move {
            let mut s = server_sock.accept().await.unwrap();
            let mut buf = [0u8; HANDSHAKE_LEN];
            s.read_exact(&mut buf).await.unwrap();
            let remote = Handshake::parse(&buf).unwrap();
            assert_eq!(remote.info_hash, info_hash);
            assert_eq!(remote.peer_id, client_id);
            let reply = Handshake::new_with_v2(remote.info_hash, server_id, false);
            s.write_all(&reply.to_bytes()).await.unwrap();
            s.write_all(&MessageEncoder::encode(&Message::Have { piece_index: 7 }))
                .await
                .unwrap();
            s.flush().await.unwrap();
            // Keep the connection alive until the client has read everything.
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let result = tokio::time::timeout(Duration::from_secs(10), async move {
            let utp = client_sock.connect(server_addr).await.unwrap();
            let (_handle, mut rx) = connect_utp_plaintext(
                utp,
                SpawnPeer {
                    addr: server_addr,
                    info_hash,
                    our_peer_id: client_id,
                    connect_timeout: Duration::from_secs(5),
                    read_timeout: Duration::from_secs(5),
                    encryption: EncryptionPolicy::PlaintextOnly,
                    advertise_v2: false,
                    ext_handshake_builder: None,
                },
            )
            .await
            .unwrap();

            match rx.recv().await.unwrap() {
                PeerEvent::Handshook {
                    peer_id,
                    info_hash: ih,
                    encrypted,
                    ..
                } => {
                    assert_eq!(peer_id, server_id);
                    assert_eq!(ih, info_hash);
                    assert!(!encrypted);
                }
                e => panic!("expected Handshook, got {e:?}"),
            }
            match rx.recv().await.unwrap() {
                PeerEvent::Message(Message::Have { piece_index }) => assert_eq!(piece_index, 7),
                e => panic!("expected Have over µTP, got {e:?}"),
            }
        })
        .await;
        result.expect("µTP handshake/message exchange timed out");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn utp_fallback_engages_when_tcp_refused() {
        use crate::utp::UtpSocket;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let info_hash = Id20([7u8; 20]);
        let client_id = Id20([1u8; 20]);
        let server_id = Id20([2u8; 20]);

        // The peer listens on µTP only — no TCP listener exists on this UDP
        // port number — so the client's TCP dial is refused and it must fall
        // back to µTP to connect.
        let server_sock = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let client_utp = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let server_addr = server_sock.local_addr();

        let server = tokio::spawn(async move {
            let mut s = server_sock.accept().await.unwrap();
            let mut buf = [0u8; HANDSHAKE_LEN];
            s.read_exact(&mut buf).await.unwrap();
            let remote = Handshake::parse(&buf).unwrap();
            let reply = Handshake::new_with_v2(remote.info_hash, server_id, false);
            s.write_all(&reply.to_bytes()).await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let spawn = SpawnPeer {
            addr: server_addr,
            info_hash,
            our_peer_id: client_id,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
            encryption: EncryptionPolicy::PlaintextOnly,
            advertise_v2: false,
            ext_handshake_builder: None,
        };
        let (_handle, mut rx) = tokio::time::timeout(
            Duration::from_secs(10),
            connect_with_utp_fallback(spawn, Some(client_utp)),
        )
        .await
        .expect("utp fallback timed out")
        .expect("utp fallback connect failed");
        match rx.recv().await.unwrap() {
            PeerEvent::Handshook { peer_id, .. } => assert_eq!(peer_id, server_id),
            e => panic!("expected Handshook via µTP fallback, got {e:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn utp_inbound_accept_completes_handshake() {
        use crate::utp::UtpSocket;

        let info_hash = Id20([9u8; 20]);
        let client_id = Id20([1u8; 20]);
        let server_id = Id20([2u8; 20]);

        let server_sock = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let client_sock = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let server_addr = server_sock.local_addr();

        // Responder: accept the inbound µTP stream and run the plaintext BT
        // handshake through the production `accept_utp_plaintext` path, then
        // push one message so the initiator's reader surfaces it.
        let server = tokio::spawn(async move {
            let s = server_sock.accept().await.unwrap();
            let known = vec![KnownInfoHash {
                info_hash,
                advertise_v2: false,
                ext_handshake_builder: None,
            }];
            let (handle, mut rx) =
                accept_utp_plaintext(s, server_id, known, Duration::from_secs(5))
                    .await
                    .unwrap();
            match rx.recv().await.unwrap() {
                PeerEvent::Handshook {
                    peer_id,
                    info_hash: ih,
                    ..
                } => {
                    assert_eq!(peer_id, client_id);
                    assert_eq!(ih, info_hash);
                }
                e => panic!("server expected Handshook, got {e:?}"),
            }
            handle
                .tx
                .send(PeerCommand::Send(Message::Have { piece_index: 9 }))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let result = tokio::time::timeout(Duration::from_secs(10), async move {
            let utp = client_sock.connect(server_addr).await.unwrap();
            let (_handle, mut rx) = connect_utp_plaintext(
                utp,
                SpawnPeer {
                    addr: server_addr,
                    info_hash,
                    our_peer_id: client_id,
                    connect_timeout: Duration::from_secs(5),
                    read_timeout: Duration::from_secs(5),
                    encryption: EncryptionPolicy::PlaintextOnly,
                    advertise_v2: false,
                    ext_handshake_builder: None,
                },
            )
            .await
            .unwrap();
            match rx.recv().await.unwrap() {
                PeerEvent::Handshook { peer_id, .. } => assert_eq!(peer_id, server_id),
                e => panic!("client expected Handshook, got {e:?}"),
            }
            match rx.recv().await.unwrap() {
                PeerEvent::Message(Message::Have { piece_index }) => assert_eq!(piece_index, 9),
                e => panic!("client expected Have over µTP, got {e:?}"),
            }
        })
        .await;
        result.expect("inbound µTP accept handshake timed out");
        server.await.unwrap();
    }
}
