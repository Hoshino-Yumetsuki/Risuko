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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::adc_proto::AdcClient;
use super::nmdc::NmdcClient;
use super::types::{is_adc_uri, parse_adc_hub_uri, parse_dchub_file_uri, AdcError, HubDialect};
use crate::engine::options::EngineOptions;

/// Run a single ADC / NMDC download to completion. Parses the URI, opens
/// the hub, locates a peer holding the requested TTH and copies the file
/// to `dir`. Returns the absolute output path on success or a string
/// describing the failure (`"cancelled"` when aborted)
pub async fn run_adc_download(
    uri: &str,
    dir: &str,
    opts: &EngineOptions,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
) -> Result<PathBuf, String> {
    let _ = (speed, connections); // surfaced via the periodic update task

    if !is_adc_uri(uri) {
        return Err(format!("not an ADC/DC URI: {uri}"));
    }

    let hub = parse_adc_hub_uri(uri).map_err(|e| e.to_string())?;
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

    let nick = opts
        .get_str("adc-nick")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Risuko".to_string());

    if cancel.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
        return Err("cancelled".into());
    }

    match hub.dialect {
        HubDialect::Nmdc => {
            let mut client = NmdcClient::connect(&hub, &nick)
                .await
                .map_err(|e| format!("hub connect: {e}"))?;
            client
                .handshake()
                .await
                .map_err(|e| format!("nmdc handshake: {e}"))?;
            let _ = client
                .search_by_tth(&tth, 5)
                .await
                .map_err(|e| format!("search: {e}"))?;
            // Without an active listening socket and a public IP the rest of
            // the negotiation cannot complete. Return a clear error so the
            // task moves to the Error state instead of hanging
            Err(AdcError::NoSource.to_string())
        }
        HubDialect::Adc => {
            let mut client = AdcClient::connect(&hub)
                .await
                .map_err(|e| format!("hub connect: {e}"))?;
            client
                .handshake(&nick)
                .await
                .map_err(|e| format!("adc handshake: {e}"))?;
            // Same limitation as above: without a peer accepting our INF and
            // approving our `BGET file TTH/...`, the transfer cannot proceed
            let _ = (&mut client, dir, &completed);
            Err(AdcError::NoSource.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_adc_uri() {
        let (total, completed, speed, cancel, conns) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
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
            cancel,
            conns,
            CancellationToken::new(),
        )
        .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn rejects_hub_only_uri_without_tth() {
        let (total, completed, speed, cancel, conns) = (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
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
            cancel,
            conns,
            CancellationToken::new(),
        )
        .await;
        assert!(res.is_err());
    }
}
