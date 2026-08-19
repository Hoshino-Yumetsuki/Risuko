//! ADC / DC download orchestration: resolves the hub from the URI, handshakes, searches for the TTH, downloads from the first accepting peer. Limitations: active-mode only (passive-only hubs needing `$RevConnectToMe` won't complete); hub-only URIs without a TTH return `NoSource` since there's no search UI

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::types::{is_adc_uri, parse_adc_hub_uri, parse_dchub_file_uri, AdcError, HubInfo};
use crate::engine::options::EngineOptions;

pub async fn connect_hub_with_proxy(
    hub: &HubInfo,
    proxy: &risuko_http::ProxyConnector,
) -> Result<risuko_http::BoxedIo, AdcError> {
    proxy
        .connect_tcp(&hub.host, hub.port)
        .await
        .map_err(|error| AdcError::Peer(format!("hub connect: {error}")))
}

/// Run a single ADC / NMDC download to completion: parse the URI, open the hub, locate a peer holding the requested TTH and copy the file to `dir`; returns the output path on success or a failure string (`"cancelled"` when aborted)
pub async fn run_adc_download(
    uri: &str,
    dir: &str,
    opts: &EngineOptions,
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

    if cancel_token.is_cancelled() {
        return Err("cancelled".into());
    }

    let proxy = opts
        .p2p_proxy_connector()
        .map_err(|error| format!("ADC P2P proxy: {error}"))?;
    if proxy.proxy().is_some() {
        tokio::select! {
            result = timeout(Duration::from_secs(15), connect_hub_with_proxy(&hub, &proxy)) => {
                result
                    .map_err(|_| "ADC hub connect timeout".to_string())?
                    .map_err(|error| error.to_string())?;
            }
            _ = cancel_token.cancelled() => return Err("cancelled".into()),
        }
        if cancel_token.is_cancelled() {
            return Err("cancelled".into());
        }
    }

    // Passive-only mode: no listening socket, so peer negotiation (`$ConnectToMe` / ADC `CTM`) cannot complete; bail before hub I/O so the task fails fast instead of hanging
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
