//! [`NewsProvider`] — company / market news feed.
use async_trait::async_trait;

use crate::{domain::Symbol, error::TradingError, state::NewsData};

/// Provides structured news for an asset.
#[async_trait]
pub trait NewsProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Fetch company-scoped news for `symbol`.
    ///
    /// # Errors
    ///
    /// - [`TradingError::NetworkTimeout`] on transport failures.
    /// - [`TradingError::SchemaViolation`] on malformed responses.
    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError>;
}
