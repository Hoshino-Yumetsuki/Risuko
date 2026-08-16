//! ADC / Direct Connect (NMDC + ADC dialects) — Phase 1. Schemes `adc://`, `adcs://` (TLS), `dchub://`, `nmdc://`; NMDC is the legacy TCP line-text protocol (`$Lock`/`$Key`, `$MyINFO`, `$Search`, `$SR`, `$ADCGET`), ADC the binary-tagged frame protocol (`CSUP`, `BINF`, `BGET`, `BRES`). A direct file URI (`dchub://hub.host/?TTH=…&xl=size&dn=name`) carries TTH, size and name; the engine connects to the hub, finds a peer holding that TTH, then downloads via `$ADCGET file …` (NMDC) or `CGET file …` (ADC)

/// Top-level download orchestrator dispatching by hub dialect
pub mod download;
/// Shared URI parsers and protocol-agnostic types
pub mod types;

pub use download::run_adc_download;
pub use types::{
    is_adc_uri, parse_adc_hub_uri, parse_dchub_file_uri, AdcError, FileEntry, HubInfo,
};
