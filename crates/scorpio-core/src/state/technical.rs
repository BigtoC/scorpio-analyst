use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub options_summary: Option<String>,
}

/// MACD indicator components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MacdValues {
    pub macd_line: f64,
    pub signal_line: f64,
    pub histogram: f64,
}
