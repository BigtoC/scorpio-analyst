//! [`OnChainProvider`] placeholder — crypto pack implementation wires this
//! up to DeFiLlama / GeckoTerminal / similar.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{domain::Symbol, error::TradingError};

/// Opaque on-chain activity payload shape — filled in by the crypto pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OnChainData {
    pub raw: String,
}

/// Provides on-chain flow / holder-concentration data for a crypto symbol.
#[async_trait]
pub trait OnChainProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn fetch(&self, symbol: &Symbol) -> Result<OnChainData, TradingError> {
        Err(TradingError::SchemaViolation {
            message: format!("OnChainProvider not implemented yet; refuse to fetch {symbol}"),
        })
    }
}
