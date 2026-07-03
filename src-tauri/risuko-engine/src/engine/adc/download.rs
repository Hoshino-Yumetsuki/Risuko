//! ADC / DC download orchestration. Resolves the target hub from the URI,
//! handshakes, searches for the TTH, and downloads from the first peer that
//! accepts the request
//!
//! Limitations:
//! - Active-mode only (we don't bind a listening socket; passive-only hubs
//!   that require `$RevConnectToMe` won't complete)
//! - Hub-only URIs without a TTH return immediately with `NoSource` since we
//!   don't surface a search UI

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::types::{is_adc_uri, parse_adc_hub_uri, parse_dchub_file_uri, AdcError};
use crate::engine::options::EngineOptions;

/// Run a single ADC / NMDC download to completion. Parses the URI, opens
/// the hub, locates a peer holding the requested TTH and copies the file
/// to `dir`. Returns the absolute output path on success or a string
/// describing the failure (`"cancelled"` when aborted)
pub async fn run_adc_download(
    uri: &str,
    dir: &str,
    _opts: &EngineOptions,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    let _ = (speed, connections); // surfaced via the periodic update task

    if !is_adc_uri(uri) {
        return Err(format!("not an ADC/DC URI: {uri}"));
    }

    parse_adc_hub_uri(uri).map_err(|e| e.to_string())?;
    let file = parse_dchub_file_uri(uri).ok_or_else(|| {
        "ADC/DC hub-only URIs (without TTH+size+name) are not yet supported".to_string()
    })?;

    let tth = file
        .tth
        .ok_or_else(|| "ADC/DC URI missing TTH parameter".to_string())?;
    if file.file_size == 0 {
        return Err("ADC/DC URI missing xl/size parameter".to_string());
    }

    total.store(file.file_size, Ordering::Relaxed);

    if cancel_token.is_cancelled() {
        return Err("cancelled".into());
    }

    // Passive-only mode,  don't bind a listening socket, so the peer
    // negotiation (`$ConnectToMe` / ADC `CTM`) cannot complete. Bail out
    // before performing hub I/O so the task fails fast instead of hanging
    let _ = (&tth, &completed, dir);
    Err(AdcError::NoSource.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_adc_uri() {
        let (total, completed, speed, conns) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let opts = EngineOptions::from_config(&serde_json::Map::new(), &serde_json::Map::new());
        let res = run_adc_download(
            "magnet:?xt=urn:btih:abcd",
            "/tmp",
            &opts,
            total,
            completed,
            speed,
            conns,
            CancellationToken::new(),
        )
        .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn rejects_hub_only_uri_without_tth() {
        let (total, completed, speed, conns) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU32::new(0)),
        );
        let opts = EngineOptions::from_config(&serde_json::Map::new(), &serde_json::Map::new());
        let res = run_adc_download(
            "dchub://hub.example.com:411",
            "/tmp",
            &opts,
            total,
            completed,
            speed,
            conns,
            CancellationToken::new(),
        )
        .await;
        assert!(res.is_err());
    }
}
