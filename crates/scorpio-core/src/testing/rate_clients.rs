//! Fake rate-client helpers for unit/integration tests.
//!
//! These let tests inject closures in place of live FRED and yfinance HTTP
//! calls so the rate-fetch path in `PreflightTask` (and its `run_for_test`
//! shim) can be exercised without network I/O.
//!
//! Gated behind `#[cfg(any(test, feature = "test-helpers"))]` — not for
//! production use.

use async_trait::async_trait;

use crate::error::TradingError;
use crate::workflow::tasks::preflight::{FredSeriesClient, RiskFreeRateYFinanceClient};

// ── FRED fake ────────────────────────────────────────────────────────────────

/// Fake FRED client backed by a closure.
///
/// Construct via [`with_fake_fred_client`].
pub struct FakeFredClient<F> {
    pub(super) handler: F,
}

#[async_trait]
impl<F> FredSeriesClient for FakeFredClient<F>
where
    F: Fn(&str) -> Result<Option<f64>, TradingError> + Send + Sync,
{
    async fn get_series_latest(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
        (self.handler)(series_id)
    }
}

/// Create a fake FRED client that delegates to the given closure.
///
/// The closure receives the series ID (e.g. `"DGS3MO"`) and returns
/// `Ok(Some(rate_pct))` on success, `Ok(None)` when the series is absent,
/// or `Err(...)` to simulate a network failure.
///
/// Use `panic!(...)` inside the closure when the test asserts that the
/// client must not be called:
///
/// ```ignore
/// let fred = with_fake_fred_client(|s| panic!("must not call FRED, got series={s}"));
/// ```
pub fn with_fake_fred_client<F>(handler: F) -> FakeFredClient<F>
where
    F: Fn(&str) -> Result<Option<f64>, TradingError> + Send + Sync + 'static,
{
    FakeFredClient { handler }
}

// ── yfinance fake ─────────────────────────────────────────────────────────────

/// Fake yfinance client backed by a closure.
///
/// Construct via [`with_fake_yfinance_client`].
pub struct FakeYFinanceClient<F> {
    pub(super) handler: F,
}

#[async_trait]
impl<F> RiskFreeRateYFinanceClient for FakeYFinanceClient<F>
where
    F: Fn(&str) -> Result<Option<f64>, TradingError> + Send + Sync,
{
    async fn latest_risk_free_rate_pct(&self, symbol: &str) -> Result<Option<f64>, TradingError> {
        (self.handler)(symbol)
    }
}

/// Create a fake yfinance client that delegates to the given closure.
///
/// The closure receives the symbol (e.g. `"^IRX"`) and returns
/// `Ok(Some(rate_pct))` on success, `Ok(None)` when data is absent,
/// or `Err(...)` to simulate a failure.
pub fn with_fake_yfinance_client<F>(handler: F) -> FakeYFinanceClient<F>
where
    F: Fn(&str) -> Result<Option<f64>, TradingError> + Send + Sync + 'static,
{
    FakeYFinanceClient { handler }
}
