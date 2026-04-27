// traits/options.rs

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{domain::Symbol, error::TradingError};

#[async_trait]
pub trait OptionsProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    async fn fetch_snapshot(
        &self,
        symbol: &Symbol,
        target_date: &str,
    ) -> Result<OptionsOutcome, TradingError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OptionsOutcome {
    Snapshot(OptionsSnapshot),
    NoListedInstrument,
    SparseChain,
    HistoricalRun,
    MissingSpot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptionsSnapshot {
    pub spot_price: f64,
    pub atm_iv: f64,
    pub iv_term_structure: Vec<IvTermPoint>,
    pub put_call_volume_ratio: f64,
    pub put_call_oi_ratio: f64,
    pub max_pain_strike: f64,
    pub near_term_expiration: String, // ISO-8601 date
    pub near_term_strikes: Vec<NearTermStrike>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IvTermPoint {
    pub expiration: String, // ISO-8601 date
    pub atm_iv: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NearTermStrike {
    pub strike: f64,
    pub call_iv: Option<f64>,
    pub put_iv: Option<f64>,
    pub call_volume: Option<u64>,
    pub put_volume: Option<u64>,
    pub call_oi: Option<u64>,
    pub put_oi: Option<u64>,
}
