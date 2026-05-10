//! Gnutella 0.6 — Phase 3
//!
//! URI schemes: `gnutella://`, `gnet://`, plus magnet links with
//! `xt=urn:sha1:` (handled separately by the Magnet pipeline)
//!
//! This module implements:
//! - GWebCache HTTP bootstrap to discover ultrapeers
//! - The `GNUTELLA CONNECT/0.6` handshake
//! - Binary message header (16-byte GUID + type/ttl/hops/payload-len)
//! - QUERY broadcast + QUERYHIT collection by SHA-1 URN
//! - HTTP/1.1 range download with `X-Gnutella-Content-URN` request header
//!
//! The URI we accept is `gnutella://host:port/uri-res/N2R?urn:sha1:<base32>`
//! which is the standard Gnutella content-direct URL format

/// Top-level Gnutella download orchestrator
pub mod download;
/// GWebCache HTTP bootstrap for ultrapeer discovery
pub mod gwcache;
/// `GNUTELLA CONNECT/0.6` three-step HTTP-style handshake
pub mod handshake;
/// 23-byte binary descriptor header and `QUERY` payload helpers
pub mod message;
/// HTTP/1.1 peer fetch (`/uri-res/N2R?<urn>`)
pub mod peer;
/// URI parsing and shared error / link types
pub mod types;

pub use download::run_gnutella_download;
pub use types::{is_gnutella_uri, parse_gnutella_uri, GnutellaLink};
