//! `rig` tool wrappers for the technical indicator calculation functions.
//!
//! Each struct implements [`rig::tool::Tool`] so the downstream Technical
//! Analyst agent can bind these calculations via the agent-builder helper.
//! Tools operate on pre-fetched candle data; they do **not** fetch market
//! data directly, preserving the separation between the `financial-data` and
//! `technical-analysis` capabilities.

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::data::yfinance::Candle;
use crate::error::TradingError;
use crate::state::TechnicalData;

use super::batch::{calculate_all_indicators, calculate_indicator_by_name};
use super::core_math::{calculate_atr, calculate_bollinger_bands, calculate_macd, calculate_rsi};
use super::types::{BollingerResult, MacdResult, NamedIndicatorOutput};

// ── CalculateAllIndicators ────────────────────────────────────────────────────

/// Args for the `calculate_all_indicators` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAllIndicatorsArgs {
    /// Pre-fetched OHLCV candles (output of the `get_ohlcv` tool).
    pub candles: Vec<Candle>,
}

/// `rig` tool: compute all technical indicators from pre-fetched OHLCV candles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAllIndicators;

impl Tool for CalculateAllIndicators {
    const NAME: &'static str = "calculate_all_indicators";
    type Error = TradingError;
    type Args = CalculateAllIndicatorsArgs;
    type Output = TechnicalData;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Compute all technical indicators (RSI, MACD, ATR, Bollinger Bands, \
                           SMA, EMA, VWMA, support/resistance) from pre-fetched OHLCV candle \
                           data and return a populated TechnicalData snapshot."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "candles": {
                        "type": "array",
                        "description": "OHLCV candles previously fetched via get_ohlcv",
                        "items": {
                            "type": "object",
                            "properties": {
                                "date":   { "type": "string" },
                                "open":   { "type": "number" },
                                "high":   { "type": "number" },
                                "low":    { "type": "number" },
                                "close":  { "type": "number" },
                                "volume": { "type": ["integer", "null"] }
                            },
                            "required": ["date", "open", "high", "low", "close"]
                        }
                    }
                },
                "required": ["candles"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        calculate_all_indicators(&args.candles)
    }
}

// ── CalculateRsi ──────────────────────────────────────────────────────────────

/// Args for the `calculate_rsi` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateRsiArgs {
    /// Pre-fetched OHLCV candles.
    pub candles: Vec<Candle>,
    /// RSI period (default: 14).
    #[serde(default = "default_rsi_period")]
    pub period: usize,
}

fn default_rsi_period() -> usize {
    14
}

/// `rig` tool: compute RSI from pre-fetched OHLCV candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateRsi;

impl Tool for CalculateRsi {
    const NAME: &'static str = "calculate_rsi";
    type Error = TradingError;
    type Args = CalculateRsiArgs;
    type Output = Vec<Option<f64>>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Compute RSI (Relative Strength Index, default period 14) from \
                           pre-fetched OHLCV candles. Returns per-bar RSI values; None indicates \
                           the indicator was not yet valid at that bar."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "candles": { "type": "array", "description": "OHLCV candles from get_ohlcv" },
                    "period":  { "type": "integer", "description": "RSI period (default 14)", "default": 14 }
                },
                "required": ["candles"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        calculate_rsi(&args.candles, args.period)
    }
}

// ── CalculateMacd ─────────────────────────────────────────────────────────────

/// Args for the `calculate_macd` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateMacdArgs {
    /// Pre-fetched OHLCV candles.
    pub candles: Vec<Candle>,
    /// Fast EMA period (default: 12).
    #[serde(default = "default_macd_fast")]
    pub fast: usize,
    /// Slow EMA period (default: 26).
    #[serde(default = "default_macd_slow")]
    pub slow: usize,
    /// Signal line period (default: 9).
    #[serde(default = "default_macd_signal")]
    pub signal: usize,
}

fn default_macd_fast() -> usize {
    12
}
fn default_macd_slow() -> usize {
    26
}
fn default_macd_signal() -> usize {
    9
}

/// `rig` tool: compute MACD from pre-fetched OHLCV candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateMacd;

impl Tool for CalculateMacd {
    const NAME: &'static str = "calculate_macd";
    type Error = TradingError;
    type Args = CalculateMacdArgs;
    type Output = MacdResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Compute MACD (Moving Average Convergence Divergence, default \
                           12/26/9) from pre-fetched OHLCV candles. Returns MACD line, signal \
                           line, and histogram as separate per-bar series."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "candles": { "type": "array", "description": "OHLCV candles from get_ohlcv" },
                    "fast":    { "type": "integer", "description": "Fast EMA period (default 12)", "default": 12 },
                    "slow":    { "type": "integer", "description": "Slow EMA period (default 26)", "default": 26 },
                    "signal":  { "type": "integer", "description": "Signal line period (default 9)", "default": 9 }
                },
                "required": ["candles"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        calculate_macd(&args.candles, args.fast, args.slow, args.signal)
    }
}

// ── CalculateAtr ──────────────────────────────────────────────────────────────

/// Args for the `calculate_atr` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAtrArgs {
    /// Pre-fetched OHLCV candles.
    pub candles: Vec<Candle>,
    /// ATR period (default: 14).
    #[serde(default = "default_atr_period")]
    pub period: usize,
}

fn default_atr_period() -> usize {
    14
}

/// `rig` tool: compute ATR from pre-fetched OHLCV candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAtr;

impl Tool for CalculateAtr {
    const NAME: &'static str = "calculate_atr";
    type Error = TradingError;
    type Args = CalculateAtrArgs;
    type Output = Vec<Option<f64>>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description:
                "Compute ATR (Average True Range, default period 14) from pre-fetched OHLCV \
                 candles. Returns per-bar ATR values; None before the lookback period."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "candles": { "type": "array", "description": "OHLCV candles from get_ohlcv" },
                    "period":  { "type": "integer", "description": "ATR period (default 14)", "default": 14 }
                },
                "required": ["candles"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        calculate_atr(&args.candles, args.period)
    }
}

// ── CalculateBollingerBands ───────────────────────────────────────────────────

/// Args for the `calculate_bollinger_bands` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateBollingerArgs {
    /// Pre-fetched OHLCV candles.
    pub candles: Vec<Candle>,
    /// Bollinger period (default: 20).
    #[serde(default = "default_boll_period")]
    pub period: usize,
    /// Standard-deviation multiplier (default: 2.0).
    #[serde(default = "default_boll_std_dev")]
    pub std_dev: f64,
}

fn default_boll_period() -> usize {
    20
}
fn default_boll_std_dev() -> f64 {
    2.0
}

/// `rig` tool: compute Bollinger Bands from pre-fetched OHLCV candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateBollingerBands;

impl Tool for CalculateBollingerBands {
    const NAME: &'static str = "calculate_bollinger_bands";
    type Error = TradingError;
    type Args = CalculateBollingerArgs;
    type Output = BollingerResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Compute Bollinger Bands (default 20/2.0) from pre-fetched OHLCV \
                           candles. Returns upper, middle (SMA), and lower band series."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "candles": { "type": "array", "description": "OHLCV candles from get_ohlcv" },
                    "period":  { "type": "integer", "description": "Bollinger period (default 20)", "default": 20 },
                    "std_dev": { "type": "number", "description": "Std-dev multiplier (default 2.0)", "default": 2.0 }
                },
                "required": ["candles"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        calculate_bollinger_bands(&args.candles, args.period, args.std_dev)
    }
}

// ── CalculateIndicatorByName ──────────────────────────────────────────────────

/// Args for the `calculate_indicator_by_name` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateIndicatorByNameArgs {
    /// Pre-fetched OHLCV candles.
    pub candles: Vec<Candle>,
    /// Prompt-compatible indicator name (e.g. `"rsi"`, `"close_50_sma"`).
    pub indicator: String,
}

/// `rig` tool: compute a single indicator by its prompt-compatible name.
///
/// Accepts the exact indicator names defined in the Technical Analyst prompt:
/// `close_50_sma`, `close_200_sma`, `close_10_ema`, `macd`, `macds`,
/// `macdh`, `rsi`, `boll`, `boll_ub`, `boll_lb`, `atr`, `vwma`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateIndicatorByName;

impl Tool for CalculateIndicatorByName {
    const NAME: &'static str = "calculate_indicator_by_name";
    type Error = TradingError;
    type Args = CalculateIndicatorByNameArgs;
    type Output = NamedIndicatorOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Compute a single technical indicator by its prompt-compatible name \
                           from pre-fetched OHLCV candles. Supported: close_50_sma, \
                           close_200_sma, close_10_ema, macd, macds, macdh, rsi, boll, \
                           boll_ub, boll_lb, atr, vwma."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "candles": {
                        "type": "array",
                        "description": "OHLCV candles from get_ohlcv"
                    },
                    "indicator": {
                        "type": "string",
                        "description": "Prompt-compatible indicator name",
                        "enum": [
                            "close_50_sma", "close_200_sma", "close_10_ema",
                            "macd", "macds", "macdh",
                            "rsi", "boll", "boll_ub", "boll_lb",
                            "atr", "vwma"
                        ]
                    }
                },
                "required": ["candles", "indicator"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        calculate_indicator_by_name(&args.indicator, &args.candles)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::test_utils::*;

    #[tokio::test]
    async fn tool_calculate_all_indicators_name() {
        let tool = CalculateAllIndicators;
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_all_indicators");
    }

    #[tokio::test]
    async fn tool_calculate_rsi_name() {
        let tool = CalculateRsi;
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_rsi");
    }

    #[tokio::test]
    async fn tool_calculate_macd_name() {
        let tool = CalculateMacd;
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_macd");
    }

    #[tokio::test]
    async fn tool_calculate_atr_name() {
        let tool = CalculateAtr;
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_atr");
    }

    #[tokio::test]
    async fn tool_calculate_bollinger_name() {
        let tool = CalculateBollingerBands;
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_bollinger_bands");
    }

    #[tokio::test]
    async fn tool_calculate_indicator_by_name_name() {
        let tool = CalculateIndicatorByName;
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_indicator_by_name");
    }

    #[tokio::test]
    async fn tool_call_calculate_all_indicators() {
        let tool = CalculateAllIndicators;
        let candles = rising_candles(200, 50.0, 1.0);
        let result = tool.call(CalculateAllIndicatorsArgs { candles }).await;
        assert!(result.is_ok(), "Tool call failed: {:?}", result.err());
        let td = result.unwrap();
        assert!(td.rsi.is_some());
    }

    #[tokio::test]
    async fn tool_call_calculate_rsi() {
        let tool = CalculateRsi;
        let candles = rising_candles(50, 100.0, 1.0);
        let result = tool
            .call(CalculateRsiArgs {
                candles,
                period: 14,
            })
            .await
            .unwrap();
        assert_eq!(result.len(), 50);
    }

    #[tokio::test]
    async fn tool_call_calculate_indicator_by_name() {
        let tool = CalculateIndicatorByName;
        let candles = rising_candles(50, 100.0, 1.0);
        let result = tool
            .call(CalculateIndicatorByNameArgs {
                candles,
                indicator: "rsi".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(result.indicator, "rsi");
        assert_eq!(result.values.len(), 50);
    }
}
