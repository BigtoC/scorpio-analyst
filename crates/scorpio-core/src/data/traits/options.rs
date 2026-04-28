// traits/options.rs

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{domain::Symbol, error::TradingError};

/// Shared interface for providers that return a normalized live options snapshot.
#[async_trait]
pub trait OptionsProvider: Send + Sync {
    /// Stable provider identifier used in logs and diagnostics.
    fn provider_name(&self) -> &'static str;

    /// Fetch an options snapshot for `symbol` on `target_date`.
    async fn fetch_snapshot(
        &self,
        symbol: &Symbol,
        target_date: &str,
    ) -> Result<OptionsOutcome, TradingError>;
}

/// Result of an options snapshot request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OptionsOutcome {
    /// A normalized snapshot was produced successfully.
    Snapshot(OptionsSnapshot),
    /// The symbol has no listed options instrument.
    NoListedInstrument,
    /// Listed options exist, but the chain is too sparse to normalize safely.
    SparseChain,
    /// Live options were intentionally skipped because `target_date` is not today.
    HistoricalRun,
    /// The underlying spot price could not be resolved.
    MissingSpot,
}

/// Normalized options-chain snapshot used by downstream analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptionsSnapshot {
    /// Underlying spot price used for normalization.
    pub spot_price: f64,
    /// At-the-money implied volatility for the front-month expiration.
    pub atm_iv: f64,
    /// ATM implied volatility across available expirations.
    pub iv_term_structure: Vec<IvTermPoint>,
    /// Aggregate put volume divided by aggregate call volume.
    pub put_call_volume_ratio: f64,
    /// Aggregate put open interest divided by aggregate call open interest.
    pub put_call_oi_ratio: f64,
    /// Front-month max-pain strike.
    pub max_pain_strike: f64,
    /// Front-month expiration date as an ISO-8601 date string.
    pub near_term_expiration: String,
    /// Near-the-money strikes selected from the front-month chain.
    pub near_term_strikes: Vec<NearTermStrike>,
}

/// A single point in the implied-volatility term structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IvTermPoint {
    /// Expiration date as an ISO-8601 date string.
    pub expiration: String,
    /// At-the-money implied volatility for this expiration.
    pub atm_iv: f64,
}

/// Near-the-money option metrics for a specific strike.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NearTermStrike {
    /// Option strike price.
    pub strike: f64,
    /// Call implied volatility at this strike, if available.
    pub call_iv: Option<f64>,
    /// Put implied volatility at this strike, if available.
    pub put_iv: Option<f64>,
    /// Call volume at this strike, if available.
    pub call_volume: Option<u64>,
    /// Put volume at this strike, if available.
    pub put_volume: Option<u64>,
    /// Call open interest at this strike, if available.
    pub call_oi: Option<u64>,
    /// Put open interest at this strike, if available.
    pub put_oi: Option<u64>,
}
