//! Gnutella 0.6 — Phase 3 URI schemes: `gnutella://`, `gnet://`, plus magnet links with `xt=urn:sha1:` (handled separately by the Magnet pipeline) This module implements: - HTTP/1.1 range download with `X-Gnutella-Content-URN` request header The URI we accept is `gnutella://host:port/uri-res/N2R?urn:sha1:<base32>` which is the standard Gnutella content-direct URL format

/// Top-level Gnutella download orchestrator
pub mod download;
/// HTTP/1.1 peer fetch (`/uri-res/N2R?<urn>`)
pub mod peer;
/// URI parsing and shared error / link types
pub mod types;

pub use download::run_gnutella_download;
pub use types::{is_gnutella_uri, parse_gnutella_uri, GnutellaLink};
