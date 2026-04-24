//! [`TokenomicsProvider`] placeholder — crypto pack implementation wires
//! this up to Messari / CoinGecko / similar.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{domain::Symbol, error::TradingError};

/// Opaque tokenomics payload shape — filled in by the crypto pack slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TokenomicsData {
    /// Placeholder; concrete fields land with the crypto pack.
    pub raw: String,
}

/// Provides token supply / unlock / treasury data for a crypto symbol.
#[async_trait]
pub trait TokenomicsProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Currently unimplemented — always returns
    /// [`TradingError::SchemaViolation`] so consumers can surface a typed
    /// error while the trait shape bakes in.
    async fn fetch(&self, symbol: &Symbol) -> Result<TokenomicsData, TradingError> {
        Err(TradingError::SchemaViolation {
            message: format!("TokenomicsProvider not implemented yet; refuse to fetch {symbol}"),
        })
    }
}
