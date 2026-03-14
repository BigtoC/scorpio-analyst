//! `rig` tool wrappers for the technical indicator calculation functions.
//!
//! Each struct implements [`rig::tool::Tool`] so the downstream Technical
//! Analyst agent can bind these calculations via the agent-builder helper.
//! Tools operate on pre-fetched candle data; they do **not** fetch market
//! data directly, preserving the separation between the `financial-data` and
//! `technical-analysis` capabilities.

use crate::data::yfinance::OhlcvToolContext;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::TradingError;
use crate::state::TechnicalData;

use super::batch::{calculate_all_indicators, calculate_indicator_by_name};
use super::core_math::{calculate_atr, calculate_bollinger_bands, calculate_macd, calculate_rsi};
use super::types::{BollingerResult, MacdResult, NamedIndicatorOutput};

// ── CalculateAllIndicators ────────────────────────────────────────────────────

/// Args for the `calculate_all_indicators` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAllIndicatorsArgs {}

/// `rig` tool: compute all technical indicators from pre-fetched OHLCV candles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAllIndicators {
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl CalculateAllIndicators {
    #[must_use]
    pub fn new(context: OhlcvToolContext) -> Self {
        Self {
            context: Some(context),
        }
    }
}

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
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let _ = args;
        let candles = self
            .context
            .as_ref()
            .ok_or_else(|| TradingError::Config(anyhow::anyhow!("missing OHLCV tool context")))?
            .load()
            .await?;
        calculate_all_indicators(&candles)
    }
}

// ── CalculateRsi ──────────────────────────────────────────────────────────────

/// Args for the `calculate_rsi` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateRsiArgs {
    /// RSI period (default: 14).
    #[serde(default = "default_rsi_period")]
    pub period: usize,
}

fn default_rsi_period() -> usize {
    14
}

/// `rig` tool: compute RSI from pre-fetched OHLCV candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateRsi {
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl CalculateRsi {
    #[must_use]
    pub fn new(context: OhlcvToolContext) -> Self {
        Self {
            context: Some(context),
        }
    }
}

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
                    "period":  { "type": "integer", "description": "RSI period (default 14)", "default": 14 }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let candles = self
            .context
            .as_ref()
            .ok_or_else(|| TradingError::Config(anyhow::anyhow!("missing OHLCV tool context")))?
            .load()
            .await?;
        calculate_rsi(&candles, args.period)
    }
}

// ── CalculateMacd ─────────────────────────────────────────────────────────────

/// Args for the `calculate_macd` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateMacdArgs {
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
pub struct CalculateMacd {
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl CalculateMacd {
    #[must_use]
    pub fn new(context: OhlcvToolContext) -> Self {
        Self {
            context: Some(context),
        }
    }
}

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
                    "fast":    { "type": "integer", "description": "Fast EMA period (default 12)", "default": 12 },
                    "slow":    { "type": "integer", "description": "Slow EMA period (default 26)", "default": 26 },
                    "signal":  { "type": "integer", "description": "Signal line period (default 9)", "default": 9 }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let candles = self
            .context
            .as_ref()
            .ok_or_else(|| TradingError::Config(anyhow::anyhow!("missing OHLCV tool context")))?
            .load()
            .await?;
        calculate_macd(&candles, args.fast, args.slow, args.signal)
    }
}

// ── CalculateAtr ──────────────────────────────────────────────────────────────

/// Args for the `calculate_atr` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAtrArgs {
    /// ATR period (default: 14).
    #[serde(default = "default_atr_period")]
    pub period: usize,
}

fn default_atr_period() -> usize {
    14
}

/// `rig` tool: compute ATR from pre-fetched OHLCV candle data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateAtr {
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl CalculateAtr {
    #[must_use]
    pub fn new(context: OhlcvToolContext) -> Self {
        Self {
            context: Some(context),
        }
    }
}

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
                    "period":  { "type": "integer", "description": "ATR period (default 14)", "default": 14 }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let candles = self
            .context
            .as_ref()
            .ok_or_else(|| TradingError::Config(anyhow::anyhow!("missing OHLCV tool context")))?
            .load()
            .await?;
        calculate_atr(&candles, args.period)
    }
}

// ── CalculateBollingerBands ───────────────────────────────────────────────────

/// Args for the `calculate_bollinger_bands` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateBollingerArgs {
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
pub struct CalculateBollingerBands {
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl CalculateBollingerBands {
    #[must_use]
    pub fn new(context: OhlcvToolContext) -> Self {
        Self {
            context: Some(context),
        }
    }
}

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
                    "period":  { "type": "integer", "description": "Bollinger period (default 20)", "default": 20 },
                    "std_dev": { "type": "number", "description": "Std-dev multiplier (default 2.0)", "default": 2.0 }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let candles = self
            .context
            .as_ref()
            .ok_or_else(|| TradingError::Config(anyhow::anyhow!("missing OHLCV tool context")))?
            .load()
            .await?;
        calculate_bollinger_bands(&candles, args.period, args.std_dev)
    }
}

// ── CalculateIndicatorByName ──────────────────────────────────────────────────

/// Args for the `calculate_indicator_by_name` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateIndicatorByNameArgs {
    /// Prompt-compatible indicator name (e.g. `"rsi"`, `"close_50_sma"`).
    pub indicator: String,
}

/// `rig` tool: compute a single indicator by its prompt-compatible name.
///
/// Accepts the exact indicator names defined in the Technical Analyst prompt:
/// `close_50_sma`, `close_200_sma`, `close_10_ema`, `macd`, `macds`,
/// `macdh`, `rsi`, `boll`, `boll_ub`, `boll_lb`, `atr`, `vwma`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateIndicatorByName {
    #[serde(skip)]
    context: Option<OhlcvToolContext>,
}

impl CalculateIndicatorByName {
    #[must_use]
    pub fn new(context: OhlcvToolContext) -> Self {
        Self {
            context: Some(context),
        }
    }
}

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
                "required": ["indicator"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let candles = self
            .context
            .as_ref()
            .ok_or_else(|| TradingError::Config(anyhow::anyhow!("missing OHLCV tool context")))?
            .load()
            .await?;
        calculate_indicator_by_name(&args.indicator, &candles)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::OhlcvToolContext;
    use crate::indicators::test_utils::*;

    async fn seeded_context(count: usize) -> OhlcvToolContext {
        let context = OhlcvToolContext::new();
        let _ = context.store(rising_candles(count, 100.0, 1.0)).await;
        context
    }

    #[tokio::test]
    async fn tool_calculate_all_indicators_name() {
        let tool = CalculateAllIndicators::new(seeded_context(200).await);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_all_indicators");
    }

    #[tokio::test]
    async fn tool_calculate_rsi_name() {
        let tool = CalculateRsi::new(seeded_context(50).await);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_rsi");
    }

    #[tokio::test]
    async fn tool_calculate_macd_name() {
        let tool = CalculateMacd::new(seeded_context(50).await);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_macd");
    }

    #[tokio::test]
    async fn tool_calculate_atr_name() {
        let tool = CalculateAtr::new(seeded_context(50).await);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_atr");
    }

    #[tokio::test]
    async fn tool_calculate_bollinger_name() {
        let tool = CalculateBollingerBands::new(seeded_context(50).await);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_bollinger_bands");
    }

    #[tokio::test]
    async fn tool_calculate_indicator_by_name_name() {
        let tool = CalculateIndicatorByName::new(seeded_context(50).await);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "calculate_indicator_by_name");
    }

    #[tokio::test]
    async fn tool_call_calculate_all_indicators() {
        let tool = CalculateAllIndicators::new(seeded_context(200).await);
        let result = tool.call(CalculateAllIndicatorsArgs {}).await;
        assert!(result.is_ok(), "Tool call failed: {:?}", result.err());
        let td = result.unwrap();
        assert!(td.rsi.is_some());
    }

    #[tokio::test]
    async fn tool_call_calculate_rsi() {
        let tool = CalculateRsi::new(seeded_context(50).await);
        let result = tool.call(CalculateRsiArgs { period: 14 }).await.unwrap();
        assert_eq!(result.len(), 50);
    }

    #[tokio::test]
    async fn tool_call_calculate_indicator_by_name() {
        let tool = CalculateIndicatorByName::new(seeded_context(50).await);
        let result = tool
            .call(CalculateIndicatorByNameArgs {
                indicator: "rsi".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(result.indicator, "rsi");
        assert_eq!(result.values.len(), 50);
    }
}
