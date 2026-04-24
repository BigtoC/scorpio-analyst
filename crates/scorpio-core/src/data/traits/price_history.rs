//! [`PriceHistoryProvider`] — OHLCV time-series feed.
use async_trait::async_trait;

use crate::{domain::Symbol, error::TradingError};

/// A single OHLCV (open / high / low / close / volume) bar in the provider's
/// native timescale.
///
/// Mirrors [`crate::data::yfinance::Candle`] but lives at the trait boundary
/// so the existing `Candle` can stay a concrete yfinance type while the
/// trait stays provider-agnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceBar {
    /// ISO-8601 timestamp or `YYYY-MM-DD` date marker for the bar.
    pub timestamp: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Provides historical price bars for an asset.
#[async_trait]
pub trait PriceHistoryProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Fetch OHLCV bars for `symbol` between `start` and `end` (inclusive),
    /// both `"YYYY-MM-DD"`.
    ///
    /// # Errors
    ///
    /// - [`TradingError::NetworkTimeout`] on transport failures.
    /// - [`TradingError::SchemaViolation`] on bad date strings, reversed
    ///   ranges, or malformed responses.
    async fn fetch(
        &self,
        symbol: &Symbol,
        start: &str,
        end: &str,
    ) -> Result<Vec<PriceBar>, TradingError>;
}
