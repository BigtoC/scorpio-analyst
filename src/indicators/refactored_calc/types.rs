use serde::{Deserialize, Serialize};

/// Per-bar output of a MACD calculation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacdResult {
    /// MACD line (fast EMA − slow EMA) per bar.
    pub macd_line: Vec<Option<f64>>,
    /// Signal line (EMA of MACD line) per bar.
    pub signal_line: Vec<Option<f64>>,
    /// Histogram (MACD line − signal line) per bar.
    pub histogram: Vec<Option<f64>>,
}

/// Per-bar output of a Bollinger Bands calculation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BollingerResult {
    /// Upper band per bar.
    pub upper: Vec<Option<f64>>,
    /// Middle band (SMA) per bar.
    pub middle: Vec<Option<f64>>,
    /// Lower band per bar.
    pub lower: Vec<Option<f64>>,
}

/// Output of the named-indicator selection API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedIndicatorOutput {
    /// The requested indicator name (e.g. `"rsi"`, `"close_50_sma"`).
    pub indicator: String,
    /// Per-bar values; `None` means the indicator was not yet valid at that bar.
    pub values: Vec<Option<f64>>,
}
