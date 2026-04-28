use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::data::traits::options::OptionsOutcome;

// SCHEMA EVOLUTION WARNING: `TechnicalOptionsContext::Available { outcome }` embeds
// `OptionsOutcome` directly. Adding a new `OptionsOutcome` variant in a future PR is a
// backward-incompatible snapshot change (serde unknown-tag deserialization fails for
// externally-tagged enums). Any future variant addition MUST bump `THESIS_MEMORY_SCHEMA_VERSION`.

/// Persisted outcome of the options-snapshot fetch performed during the
/// Technical Analyst phase. Carried forward so downstream agents can consume
/// options evidence without re-fetching.
///
/// Additive field — older snapshots produced before this field existed will
/// deserialize with `options_context: None` via `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TechnicalOptionsContext {
    /// A valid options snapshot was obtained from the provider.
    Available { outcome: OptionsOutcome },
    /// The options fetch was attempted but failed (network, no instrument, etc.).
    FetchFailed {
        #[serde(default)]
        reason: String,
    },
}

/// Pre-calculated technical indicators derived from OHLCV data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TechnicalData {
    pub rsi: Option<f64>,
    pub macd: Option<MacdValues>,
    pub atr: Option<f64>,
    pub sma_20: Option<f64>,
    pub sma_50: Option<f64>,
    pub ema_12: Option<f64>,
    pub ema_26: Option<f64>,
    pub bollinger_upper: Option<f64>,
    pub bollinger_lower: Option<f64>,
    pub support_level: Option<f64>,
    pub resistance_level: Option<f64>,
    pub volume_avg: Option<f64>,
    pub summary: String,
    /// Optional snapshot of the equity options chain (IV, put/call ratio,
    /// expiry distribution). Additive field — older snapshots produced
    /// before the Yahoo options integration will deserialize with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_summary: Option<String>,
    /// Persisted result of the options-snapshot fetch performed during the
    /// Technical Analyst phase. Additive — older snapshots deserialize with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_context: Option<TechnicalOptionsContext>,
}

/// MACD indicator components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MacdValues {
    pub macd_line: f64,
    pub signal_line: f64,
    pub histogram: f64,
}
