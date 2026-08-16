//! Gnutella download orchestration Strategy: For a content-direct URI (`gnutella://host:port/uri-res/N2R?...`) we connect directly to the named peer over HTTP/1.1 and fetch by URN

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::peer::fetch_by_urn;
use super::types::{is_gnutella_uri, parse_gnutella_uri, GnutellaError};
use crate::engine::options::EngineOptions;

/// Run a single Gnutella download to completion. Connects directly to the peer in the URI and fetches by URN over HTTP/1.1. Returns the absolute output path on success or a string describing the failure (`"cancelled"` when aborted)
pub async fn run_gnutella_download(
    uri: &str,
    dir: &str,
    _opts: &EngineOptions,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    let _ = (speed, connections);
    if !is_gnutella_uri(uri) {
        return Err(format!("not a Gnutella URI: {uri}"));
    }
    let link = parse_gnutella_uri(uri).ok_or_else(|| "invalid gnutella URI".to_string())?;
    let urn = link
        .urn
        .as_deref()
        .ok_or_else(|| "Gnutella URI missing urn:sha1: parameter".to_string())?;
    if link.file_size == 0 {
        return Err("Gnutella URI missing xl/size parameter".into());
    }
    total.store(link.file_size, Ordering::Relaxed);

    if cancel_token.is_cancelled() {
        return Err("cancelled".into());
    }

    let safe = crate::engine::util::safe_filename(
        if link.file_name.is_empty() {
            urn.trim_start_matches("urn:sha1:")
        } else {
            &link.file_name
        },
        "gnutella-download",
    );
    let out_path = PathBuf::from(dir).join(safe);

    fetch_by_urn(
        &link.host,
        link.port,
        if link.n2r_path.is_empty() {
            "/uri-res/N2R"
        } else {
            &link.n2r_path
        },
        urn,
        link.file_size,
        &out_path,
        completed,
        cancel_token,
    )
    .await
    .map_err(|e: GnutellaError| e.to_string())?;
    Ok(out_path)
}
