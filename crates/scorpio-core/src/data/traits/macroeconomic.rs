//! [`MacroProvider`] — macroeconomic indicator feed (rates, inflation,
//! employment, …).
use async_trait::async_trait;

use crate::{error::TradingError, state::MacroEvent};

/// Provides macroeconomic indicators that aren't scoped to a single asset.
///
/// No [`crate::domain::Symbol`] parameter because macro data is market-wide.
#[async_trait]
pub trait MacroProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Fetch a snapshot of macroeconomic events / indicators.
    ///
    /// # Errors
    ///
    /// - [`TradingError::NetworkTimeout`] on transport failures.
    /// - [`TradingError::SchemaViolation`] on malformed responses.
    async fn fetch_indicators(&self) -> Result<Vec<MacroEvent>, TradingError>;
}
