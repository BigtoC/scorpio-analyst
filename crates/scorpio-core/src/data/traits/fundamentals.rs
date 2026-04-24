//! [`FundamentalsProvider`] — upstream fundamentals feed.
use async_trait::async_trait;

use crate::{domain::Symbol, error::TradingError, state::FundamentalData};

/// Provides corporate fundamentals (ratios, earnings, insider activity) for
/// an asset identified by a typed [`Symbol`].
///
/// Today only equity symbols are supported; crypto / other asset classes
/// will return [`TradingError::SchemaViolation`] from the concrete provider
/// until their analyst slice lands.
#[async_trait]
pub trait FundamentalsProvider: Send + Sync {
    /// Name used for evidence / provenance tagging (`"finnhub"`, etc.).
    fn provider_name(&self) -> &'static str;

    /// Fetch the fundamentals payload for `symbol`.
    ///
    /// # Errors
    ///
    /// - [`TradingError::NetworkTimeout`] on transport failures.
    /// - [`TradingError::SchemaViolation`] on malformed responses or
    ///   unsupported asset classes.
    async fn fetch(&self, symbol: &Symbol) -> Result<FundamentalData, TradingError>;
}
