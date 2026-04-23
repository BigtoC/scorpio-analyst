//! Shared Yahoo Finance session: [`YfClient`] + [`SharedRateLimiter`].
//!
//! Extracted so that sibling modules (`ohlcv`, `financials`, `price`) depend on
//! this common building block rather than one reaching into another's internals.

use yfinance_rs::YfClient;

use crate::config::RateLimitConfig;
use crate::rate_limit::SharedRateLimiter;

/// Low-level Yahoo Finance session holding the HTTP client and a shared rate
/// limiter.
///
/// [`super::ohlcv::YFinanceClient`] wraps this together with an OHLCV cache;
/// [`super::financials`] accesses it via the `pub(super)` field on
/// `YFinanceClient` to build `FundamentalsBuilder` / `AnalysisBuilder` queries.
#[derive(Clone)]
pub(super) struct YfSession {
    client: YfClient,
    limiter: SharedRateLimiter,
}

impl std::fmt::Debug for YfSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YfSession")
            .field("limiter", &self.limiter.label())
            .finish()
    }
}

impl YfSession {
    /// Create a session with the given rate limiter.
    pub(super) fn new(limiter: SharedRateLimiter) -> Self {
        Self {
            client: YfClient::default(),
            limiter,
        }
    }

    /// Create a session from [`RateLimitConfig`].
    ///
    /// When `cfg.yahoo_finance_rps == 0` the limiter is disabled (no blocking).
    pub(super) fn from_config(cfg: &RateLimitConfig) -> Self {
        let limiter = SharedRateLimiter::yahoo_finance_from_config(cfg)
            .unwrap_or_else(|| SharedRateLimiter::disabled("yahoo_finance"));
        Self::new(limiter)
    }

    /// Borrow the underlying `YfClient` for building queries.
    pub(super) fn client(&self) -> &YfClient {
        &self.client
    }

    /// Borrow the rate limiter (for direct acquire or inspection).
    pub(super) fn limiter(&self) -> &SharedRateLimiter {
        &self.limiter
    }

    /// Acquire a rate-limit permit, then run `fetch`.
    pub(super) async fn with_rate_limit<F, T>(&self, fetch: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.limiter.acquire().await;
        fetch.await
    }
}

impl Default for YfSession {
    fn default() -> Self {
        Self::from_config(&RateLimitConfig::default())
    }
}
