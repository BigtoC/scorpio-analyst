//! [`SocialProvider`] placeholder — crypto pack wires this up to social
//! listening / community sentiment APIs.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{domain::Symbol, error::TradingError};

/// Opaque social-signal payload shape — filled in by the crypto pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SocialData {
    pub raw: String,
}

/// Provides social / community-sentiment data for an asset.
#[async_trait]
pub trait SocialProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn fetch(&self, symbol: &Symbol) -> Result<SocialData, TradingError> {
        Err(TradingError::SchemaViolation {
            message: format!("SocialProvider not implemented yet; refuse to fetch {symbol}"),
        })
    }
}
