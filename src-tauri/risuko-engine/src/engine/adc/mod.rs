//! ADC / Direct Connect (NMDC + ADC dialects) — Phase 1
//!
//! Schemes: `adc://`, `adcs://` (TLS), `dchub://`, `nmdc://`
//! - NMDC = TCP line-text protocol (`$Lock`/`$Key`, `$MyINFO`, `$Search`, `$SR`, `$ADCGET`)
//! - ADC = binary-tagged frame protocol (`CSUP`, `BINF`, `BGET`, `BRES`)
//!
//! A direct file URI (`dchub://hub.host/?TTH=…&xl=size&dn=name`) carries a
//! TTH hash, file size, and display name; the engine connects to the hub,
//! locates a peer holding that TTH, then downloads via `$ADCGET file …`
//! (NMDC) or `CGET file …` (ADC)

/// ADC binary-frame protocol implementation (`CSUP`, `BINF`, `BGET`, `BRES`)
#[cfg(feature = "adc-transfer")]
pub mod adc_proto;
/// Top-level download orchestrator dispatching by hub dialect
pub mod download;
/// NMDC (legacy DC++) line-text protocol client
#[cfg(feature = "adc-transfer")]
pub mod nmdc;
/// Streaming peer-side file transfer over both NMDC and ADC
#[cfg(feature = "adc-transfer")]
pub mod transfer;
/// Shared URI parsers and protocol-agnostic types
pub mod types;

pub use download::run_adc_download;
pub use types::{
    is_adc_uri, parse_adc_hub_uri, parse_dchub_file_uri, AdcError, FileEntry, HubInfo, PeerInfo,
};
