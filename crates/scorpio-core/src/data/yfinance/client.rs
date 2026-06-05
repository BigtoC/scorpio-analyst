//! [`YFinanceClient`] — the Yahoo Finance client, trait, and all fetch implementations.
//!
//! [`YFinanceData`] is the consumer-facing abstraction: data-only consumers hold
//! `Arc<dyn YFinanceData>` and tests use the `mockall`-generated `MockYFinanceData`.
//! Consumers that need the raw `YfClient` session for building `rig` tools (e.g.
//! `TechnicalAnalyst`, options/news providers) use the concrete [`YFinanceClient`].

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tokio::sync::RwLock;
use tracing::warn;
use yfinance_rs::{
    FundamentalsBuilder, HistoryBuilder, Interval, Range,
    analysis::{AnalysisBuilder, EarningsTrendRow},
    core::conversions::{money_to_currency_str, money_to_f64},
    fundamentals::{BalanceSheetRow, CashflowRow, IncomeStatementRow, ShareCount},
    profile::{self, Profile},
    ticker::{Info, Ticker},
};

use super::etf::{EtfQuote, FundInfo, fund_info_from_profile};
use super::financials::TickerCalendar;
use super::ohlcv::{Candle, map_yf_err, parse_date};
use super::session::YfSession;
use crate::config::RateLimitConfig;
use crate::data::symbol::validate_symbol;
use crate::error::TradingError;
use crate::rate_limit::SharedRateLimiter;

type OhlcvCacheKey = (String, String, String);

// ─── Client ──────────────────────────────────────────────────────────────────

/// Thin async wrapper around `yfinance-rs`.
///
/// Caches OHLCV results in memory by `(symbol, start, end)` so repeated calls
/// within the same session skip the network. Implements [`YFinanceData`] for
/// the data-only fetch surface; also exposes `get_profile`, `fetch_calendar`,
/// and `get_fund_info` as inherent methods for non-trait consumers.
#[derive(Clone)]
pub struct YFinanceClient {
    pub(super) session: YfSession,
    cache: Arc<RwLock<HashMap<OhlcvCacheKey, Arc<Vec<Candle>>>>>,
}

impl std::fmt::Debug for YFinanceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache_len = self.cache.try_read().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("YFinanceClient")
            .field("session", &self.session)
            .field("cached_entries", &cache_len)
            .finish()
    }
}

impl YFinanceClient {
    /// Create a new client using a shared provider-scoped rate limiter.
    #[must_use]
    pub fn new(limiter: SharedRateLimiter) -> Self {
        Self {
            session: YfSession::new(limiter),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new client from [`RateLimitConfig`].
    ///
    /// When `cfg.yahoo_finance_rps == 0` the limiter is disabled (no blocking).
    #[must_use]
    pub fn from_config(cfg: &RateLimitConfig) -> Self {
        Self {
            session: YfSession::from_config(cfg),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for YFinanceClient {
    fn default() -> Self {
        Self::from_config(&RateLimitConfig::default())
    }
}

// ─── YFinanceData trait ───────────────────────────────────────────────────────

/// The set of Yahoo Finance fetches consumed by valuation and consensus code.
///
/// Consumers that only need data depend on `Arc<dyn YFinanceData>`; tests use
/// `MockYFinanceData` to inject controlled responses.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait YFinanceData: Send + Sync {
    /// The shared composed [`Info`] snapshot fetched once per cycle.
    async fn get_info(&self, symbol: &str) -> Option<Info>;
    /// Quarterly cash-flow statement rows.
    async fn get_quarterly_cashflow(&self, symbol: &str) -> Option<Vec<CashflowRow>>;
    /// Quarterly balance-sheet rows.
    async fn get_quarterly_balance_sheet(&self, symbol: &str) -> Option<Vec<BalanceSheetRow>>;
    /// Quarterly income-statement rows.
    async fn get_quarterly_income_stmt(&self, symbol: &str) -> Option<Vec<IncomeStatementRow>>;
    /// Quarterly shares-outstanding data.
    async fn get_quarterly_shares(&self, symbol: &str) -> Option<Vec<ShareCount>>;
    /// Analyst earnings-trend rows (fail-soft `None` on error).
    async fn get_earnings_trend(&self, symbol: &str) -> Option<Vec<EarningsTrendRow>>;
    /// Analyst earnings-trend rows, preserving the failure reason as `Err`.
    async fn get_earnings_trend_result(
        &self,
        symbol: &str,
    ) -> Result<Option<Vec<EarningsTrendRow>>, TradingError>;
    /// ETF NAV / bid / ask quote (best-effort).
    async fn get_quote(&self, symbol: &str) -> Option<EtfQuote>;
    /// Trailing-twelve-month distribution yield (percent), if available.
    async fn get_distribution_yield_ttm(&self, symbol: &str) -> Option<f64>;
    /// Daily OHLCV bars for `[start, end]` (inclusive, `YYYY-MM-DD`).
    async fn get_ohlcv(
        &self,
        symbol: &str,
        start: &str,
        end: &str,
    ) -> Result<Vec<Candle>, TradingError>;
}

// ─── YFinanceData impl ────────────────────────────────────────────────────────

#[async_trait]
impl YFinanceData for YFinanceClient {
    async fn get_info(&self, symbol: &str) -> Option<Info> {
        let ticker = Ticker::new(self.session.client(), symbol);
        match self.session.with_rate_limit(ticker.info()).await {
            Ok(info) => Some(info),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance Info");
                None
            }
        }
    }

    async fn get_quarterly_cashflow(&self, symbol: &str) -> Option<Vec<CashflowRow>> {
        match self
            .session
            .with_rate_limit(
                FundamentalsBuilder::new(self.session.client(), symbol).cashflow(true, None),
            )
            .await
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch quarterly cashflow");
                None
            }
        }
    }

    async fn get_quarterly_balance_sheet(&self, symbol: &str) -> Option<Vec<BalanceSheetRow>> {
        match self
            .session
            .with_rate_limit(
                FundamentalsBuilder::new(self.session.client(), symbol).balance_sheet(true, None),
            )
            .await
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch quarterly balance sheet");
                None
            }
        }
    }

    async fn get_quarterly_income_stmt(&self, symbol: &str) -> Option<Vec<IncomeStatementRow>> {
        match self
            .session
            .with_rate_limit(
                FundamentalsBuilder::new(self.session.client(), symbol)
                    .income_statement(true, None),
            )
            .await
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch quarterly income statement");
                None
            }
        }
    }

    async fn get_quarterly_shares(&self, symbol: &str) -> Option<Vec<ShareCount>> {
        match self
            .session
            .with_rate_limit(FundamentalsBuilder::new(self.session.client(), symbol).shares(true))
            .await
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch quarterly shares");
                None
            }
        }
    }

    async fn get_earnings_trend(&self, symbol: &str) -> Option<Vec<EarningsTrendRow>> {
        self.get_earnings_trend_result(symbol).await.ok().flatten()
    }

    async fn get_earnings_trend_result(
        &self,
        symbol: &str,
    ) -> Result<Option<Vec<EarningsTrendRow>>, TradingError> {
        match self
            .session
            .with_rate_limit(
                AnalysisBuilder::new(self.session.client(), symbol).earnings_trend(None),
            )
            .await
        {
            Ok(rows) => Ok(Some(rows)),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch earnings trend");
                Err(map_yf_err(e))
            }
        }
    }

    async fn get_quote(&self, symbol: &str) -> Option<EtfQuote> {
        let ticker = Ticker::new(self.session.client(), symbol);
        let quote = match self.session.with_rate_limit(ticker.quote()).await {
            Ok(q) => q,
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance ETF quote");
                return None;
            }
        };
        let summary = self
            .session
            .with_rate_limit(self.session.summary().fetch(symbol))
            .await
            .unwrap_or_default();
        let regular_market_price = quote.price.as_ref().map(money_to_f64)?;
        let currency = quote
            .price
            .as_ref()
            .and_then(money_to_currency_str)
            .or_else(|| {
                quote
                    .previous_close
                    .as_ref()
                    .and_then(money_to_currency_str)
            });
        Some(EtfQuote {
            symbol: quote.instrument.symbol.as_str().to_owned(),
            regular_market_price,
            previous_close: quote.previous_close.as_ref().map(money_to_f64),
            nav: summary.nav,
            bid: summary.bid,
            ask: summary.ask,
            day_volume: quote.day_volume,
            currency,
            as_of: Utc::now(),
        })
    }

    async fn get_distribution_yield_ttm(&self, symbol: &str) -> Option<f64> {
        let ticker = Ticker::new(self.session.client(), symbol);
        let dividends = match self
            .session
            .with_rate_limit(ticker.dividends(Some(Range::Y1)))
            .await
        {
            Ok(d) => d,
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance dividend history");
                return None;
            }
        };
        if dividends.is_empty() {
            return None;
        }
        let cutoff_ts = (Utc::now() - chrono::Duration::days(365)).timestamp();
        let ttm_sum: f64 = dividends
            .iter()
            .filter(|(ts, _)| *ts >= cutoff_ts)
            .map(|(_, amount)| *amount)
            .sum();
        if ttm_sum <= 0.0 {
            return None;
        }
        let quote = self.get_quote(symbol).await?;
        if quote.regular_market_price <= 0.0 {
            return None;
        }
        Some(ttm_sum / quote.regular_market_price)
    }

    async fn get_ohlcv(
        &self,
        symbol: &str,
        start: &str,
        end: &str,
    ) -> Result<Vec<Candle>, TradingError> {
        let symbol = validate_symbol(symbol)?;
        let start_date = parse_date(start)?;
        let end_date = parse_date(end)?;
        if end_date < start_date {
            return Err(TradingError::SchemaViolation {
                message: format!("invalid date range: end ({end}) is before start ({start})"),
            });
        }
        let cache_key: OhlcvCacheKey = (
            symbol.to_ascii_uppercase(),
            start.to_owned(),
            end.to_owned(),
        );
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok((**cached).clone());
            }
        }
        let start_dt = Utc
            .from_local_datetime(&start_date.and_hms_opt(0, 0, 0).ok_or_else(|| {
                TradingError::SchemaViolation {
                    message: format!("invalid start datetime for {start}"),
                }
            })?)
            .single()
            .ok_or_else(|| TradingError::SchemaViolation {
                message: format!("invalid start datetime for {start}"),
            })?;
        let end_dt = Utc
            .from_local_datetime(&end_date.and_hms_opt(23, 59, 59).ok_or_else(|| {
                TradingError::SchemaViolation {
                    message: format!("invalid end datetime for {end}"),
                }
            })?)
            .single()
            .ok_or_else(|| TradingError::SchemaViolation {
                message: format!("invalid end datetime for {end}"),
            })?;
        self.session.limiter().acquire().await;
        let candles = HistoryBuilder::new(self.session.client(), symbol)
            .between(start_dt, end_dt)
            .interval(Interval::D1)
            .fetch()
            .await
            .map_err(map_yf_err)?;
        let mut result: Vec<Candle> = candles.into_iter().map(Candle::from_yf).collect();
        result.sort_by(|a, b| a.date.cmp(&b.date));
        self.cache
            .write()
            .await
            .insert(cache_key, Arc::new(result.clone()));
        Ok(result)
    }
}

// ─── Non-trait inherent methods ──────────────────────────────────────────────

impl YFinanceClient {
    // ── Profile / asset-shape ────────────────────────────────────────────

    /// Fetch the Yahoo Finance profile for `symbol`.
    ///
    /// Returns `Profile::Company(_)` for corporate equities and `Profile::Fund(_)`
    /// for ETF/fund-style instruments. Returns `None` on network or parsing failures.
    ///
    /// Callers must treat a `None` result as "profile unavailable" rather than
    /// as proof that the symbol is an equity — absent profile data is not a
    /// discriminating signal for asset shape.
    pub async fn get_profile(&self, symbol: &str) -> Option<Profile> {
        match self
            .session
            .with_rate_limit(profile::load_profile(self.session.client(), symbol))
            .await
        {
            Ok(p) => Some(p),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch profile");
                None
            }
        }
    }

    // ── Calendar ─────────────────────────────────────────────────────────

    /// Fetch the corporate calendar for `symbol` (earnings dates, ex-dividend,
    /// dividend payment date).
    ///
    /// Returns `None` on network or parsing failures so the caller can degrade
    /// gracefully. The upstream type is converted to a thin [`TickerCalendar`]
    /// domain wrapper so callers don't depend on the `yfinance_rs` type directly.
    pub async fn fetch_calendar(&self, symbol: &str) -> Option<TickerCalendar> {
        match self
            .session
            .with_rate_limit(FundamentalsBuilder::new(self.session.client(), symbol).calendar())
            .await
        {
            Ok(cal) => Some(TickerCalendar::from(cal)),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance calendar");
                None
            }
        }
    }

    // ── ETF fund info ─────────────────────────────────────────────────────

    /// Fetch ETF-level metadata for `symbol`.
    ///
    /// Fail-soft: returns `None` when the upstream profile cannot be fetched
    /// or when the profile is not a fund (`Profile::Company(_)`).
    ///
    /// # Coverage caveats
    ///
    /// `yfinance-rs` 0.7 / `paft` 0.7 does not expose `category`,
    /// `expense_ratio`, `total_assets`, or the tracked benchmark on
    /// `Profile::Fund` (which only carries `name`, `family`, `kind`, `isin`).
    /// Those fields are therefore left as `None`. [`FundInfo::leverage_factor`]
    /// is heuristically derived from the fund name via
    /// [`derive_leverage_factor`][super::etf::derive_leverage_factor].
    pub async fn get_fund_info(&self, symbol: &str) -> Option<FundInfo> {
        let profile = self.get_profile(symbol).await?;
        match fund_info_from_profile(symbol, &profile) {
            Some(info) => Some(info),
            None => {
                warn!(symbol, "get_fund_info called on a Company profile");
                None
            }
        }
    }
}

// ─── Test helpers ────────────────────────────────────────────────────────────

#[cfg(test)]
impl YFinanceClient {
    /// Seed the in-memory OHLCV cache with pre-built candles.
    ///
    /// Call before a test that would otherwise hit the network via `get_ohlcv`.
    /// Inserts the candles under the normalized (uppercase) symbol key so the
    /// cache lookup in `get_ohlcv` finds them.
    pub(super) async fn cache_seed(
        &self,
        symbol: &str,
        start: &str,
        end: &str,
        candles: Vec<Candle>,
    ) {
        self.cache.write().await.insert(
            (
                symbol.to_ascii_uppercase(),
                start.to_owned(),
                end.to_owned(),
            ),
            Arc::new(candles),
        );
    }

    pub(super) fn limiter_label(&self) -> &str {
        self.session.limiter().label()
    }

    pub(super) fn limiter_is_enabled(&self) -> bool {
        self.session.limiter().is_enabled()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use crate::config::RateLimitConfig;
    use crate::rate_limit::SharedRateLimiter;

    // ── Session rate-limit passthrough ────────────────────────────────────

    #[tokio::test]
    async fn with_rate_limit_acquires_permit_before_running_fetch() {
        let client = YFinanceClient::new(SharedRateLimiter::disabled("yahoo_finance"));
        let acquired = Arc::new(AtomicBool::new(false));
        let acquired_for_fetch = acquired.clone();

        client
            .session
            .with_rate_limit(async move {
                acquired_for_fetch.store(true, Ordering::SeqCst);
                Some(())
            })
            .await;

        assert!(acquired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn with_rate_limit_returns_fetch_result_unchanged() {
        let client = YFinanceClient::default();
        let result = client.session.with_rate_limit(async { Some(42_u8) }).await;
        assert_eq!(result, Some(42));
    }

    // ── Date range validation ─────────────────────────────────────────────

    #[tokio::test]
    async fn end_before_start_returns_error() {
        let client = YFinanceClient::default();
        let result = client.get_ohlcv("AAPL", "2024-06-01", "2024-01-01").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::error::TradingError::SchemaViolation { ref message } if message.contains("before start"))
        );
    }

    #[tokio::test]
    async fn same_start_and_end_is_valid() {
        // Dates are equal — should not return an "invalid range" error.
        let client = YFinanceClient::default();
        client
            .cache_seed("AAPL", "2024-01-15", "2024-01-15", vec![])
            .await;
        let result = client.get_ohlcv("AAPL", "2024-01-15", "2024-01-15").await;
        assert!(
            result.is_ok(),
            "equal start/end should not fail with date-range error, got: {result:?}"
        );
    }

    // ── In-memory cache ───────────────────────────────────────────────────

    #[tokio::test]
    async fn get_ohlcv_returns_cached_result_on_second_call_with_same_params() {
        let client = YFinanceClient::default();
        let candles = vec![Candle {
            date: "2024-01-02".to_owned(),
            open: 180.0,
            high: 182.0,
            low: 179.0,
            close: 181.0,
            volume: Some(30_000_000),
        }];
        // Insert directly into the cache to simulate a prior successful fetch.
        client.cache.write().await.insert(
            (
                "AAPL".to_owned(),
                "2024-01-01".to_owned(),
                "2024-01-31".to_owned(),
            ),
            Arc::new(candles.clone()),
        );

        let first = client
            .get_ohlcv("AAPL", "2024-01-01", "2024-01-31")
            .await
            .expect("cache hit must succeed");
        let second = client
            .get_ohlcv("AAPL", "2024-01-01", "2024-01-31")
            .await
            .expect("cache hit must succeed");

        assert_eq!(first, candles);
        assert_eq!(second, candles);
        assert_eq!(client.cache.read().await.len(), 1);
    }

    #[tokio::test]
    async fn get_ohlcv_cache_is_case_insensitive_for_symbol() {
        let client = YFinanceClient::default();
        let candles = vec![Candle {
            date: "2024-03-01".to_owned(),
            open: 170.0,
            high: 172.0,
            low: 169.0,
            close: 171.0,
            volume: None,
        }];
        client.cache.write().await.insert(
            (
                "MSFT".to_owned(),
                "2024-03-01".to_owned(),
                "2024-03-31".to_owned(),
            ),
            Arc::new(candles.clone()),
        );

        let result = client
            .get_ohlcv("msft", "2024-03-01", "2024-03-31")
            .await
            .expect("case-insensitive cache hit must succeed");
        assert_eq!(result, candles);
    }

    #[tokio::test]
    async fn cache_seed_helper_populates_cache_for_tests() {
        let client = YFinanceClient::default();
        client
            .cache_seed(
                "aapl",
                "2024-01-01",
                "2024-01-31",
                vec![Candle {
                    date: "2024-01-02".to_owned(),
                    open: 100.0,
                    high: 101.0,
                    low: 99.0,
                    close: 100.5,
                    volume: Some(1),
                }],
            )
            .await;
        assert_eq!(client.cache.read().await.len(), 1);
    }

    // ── from_config constructor ───────────────────────────────────────────

    #[test]
    fn from_config_with_zero_rps_creates_client_without_panic() {
        let cfg = RateLimitConfig {
            finnhub_rps: 0,
            fred_rps: 0,
            yahoo_finance_rps: 0,
            alpha_vantage_rps: 0,
            reddit_rpm: 0,
            sec_edgar_rps: 0,
        };
        let client = YFinanceClient::from_config(&cfg);
        assert_eq!(client.limiter_label(), "yahoo_finance");
        assert!(
            !client.limiter_is_enabled(),
            "yahoo_finance_rps=0 should disable the limiter"
        );
    }

    #[test]
    fn from_config_with_nonzero_rps_creates_client_without_panic() {
        let cfg = RateLimitConfig {
            finnhub_rps: 0,
            fred_rps: 0,
            yahoo_finance_rps: 5,
            alpha_vantage_rps: 0,
            reddit_rpm: 0,
            sec_edgar_rps: 0,
        };
        let client = YFinanceClient::from_config(&cfg);
        assert_eq!(client.limiter_label(), "yahoo_finance");
        assert!(
            client.limiter_is_enabled(),
            "non-zero yahoo_finance_rps should enable the limiter"
        );
    }

    #[test]
    fn default_and_from_config_default_produce_same_limiter_label() {
        let default_client = YFinanceClient::default();
        let config_client = YFinanceClient::from_config(&RateLimitConfig::default());
        assert_eq!(default_client.limiter_label(), "yahoo_finance");
        assert_eq!(config_client.limiter_label(), "yahoo_finance");
        assert_eq!(
            default_client.limiter_is_enabled(),
            config_client.limiter_is_enabled()
        );
    }
}
