//! [`DerivativesProvider`] placeholder — crypto pack wires this up to
//! Binance / OKX / aggregator APIs.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{domain::Symbol, error::TradingError};

/// Opaque derivatives-market payload shape — filled in by the crypto pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DerivativesData {
    pub raw: String,
}

/// Provides derivatives data (funding rates, open interest, basis).
#[async_trait]
pub trait DerivativesProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn fetch(&self, symbol: &Symbol) -> Result<DerivativesData, TradingError> {
        Err(TradingError::SchemaViolation {
            message: format!("DerivativesProvider not implemented yet; refuse to fetch {symbol}"),
        })
    }
}
