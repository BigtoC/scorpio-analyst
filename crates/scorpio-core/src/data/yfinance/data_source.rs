//! [`YFinanceData`] — the consumer-facing fetch surface of [`YFinanceClient`].
//!
//! `YFinanceClient` is a thin wrapper over the `yfinance-rs` library, so the
//! right test seam is the **function boundary** — what the fetch methods return
//! — not the HTTP layer beneath the library. Consumers that only need data
//! (not the raw `YfClient` session) depend on this trait, and tests use the
//! `mockall`-generated `MockYFinanceData` to set per-method responses.
//!
//! This mirrors the [`EdgarHttp`](crate::data::sec_edgar) trait pattern. HTTP-level
//! mocking (`wiremock`) is reserved for code that hand-rolls `reqwest` requests
//! without a client library (e.g. the Reddit client and [`super::summary`]).
//!
//! Consumers that also need the raw `yfinance-rs` session to build tools or the
//! options/news providers (the analyst fan-out, `TechnicalAnalyst`) keep using
//! the concrete [`YFinanceClient`]; only the data-only consumers
//! ([`fetch_valuation_inputs`](crate::workflow) and the estimates provider)
//! depend on this trait.

use async_trait::async_trait;
use yfinance_rs::{
    analysis::EarningsTrendRow,
    fundamentals::{BalanceSheetRow, CashflowRow, IncomeStatementRow, ShareCount},
    ticker::Info,
};

use super::etf::EtfQuote;
use super::ohlcv::{Candle, YFinanceClient};
use crate::error::TradingError;

/// The set of Yahoo Finance fetches consumed by valuation and consensus code.
///
/// Each method mirrors the identically-named inherent method on
/// [`YFinanceClient`]; the blanket impl below just delegates. Implementors must
/// be `Send + Sync` so the trait object can flow through the async pipeline.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait YFinanceData: Send + Sync {
    /// The shared composed [`Info`] snapshot (profile, price target,
    /// recommendation summary, calendar, key statistics) fetched once per cycle.
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

#[async_trait]
impl YFinanceData for YFinanceClient {
    async fn get_info(&self, symbol: &str) -> Option<Info> {
        YFinanceClient::get_info(self, symbol).await
    }

    async fn get_quarterly_cashflow(&self, symbol: &str) -> Option<Vec<CashflowRow>> {
        YFinanceClient::get_quarterly_cashflow(self, symbol).await
    }

    async fn get_quarterly_balance_sheet(&self, symbol: &str) -> Option<Vec<BalanceSheetRow>> {
        YFinanceClient::get_quarterly_balance_sheet(self, symbol).await
    }

    async fn get_quarterly_income_stmt(&self, symbol: &str) -> Option<Vec<IncomeStatementRow>> {
        YFinanceClient::get_quarterly_income_stmt(self, symbol).await
    }

    async fn get_quarterly_shares(&self, symbol: &str) -> Option<Vec<ShareCount>> {
        YFinanceClient::get_quarterly_shares(self, symbol).await
    }

    async fn get_earnings_trend(&self, symbol: &str) -> Option<Vec<EarningsTrendRow>> {
        YFinanceClient::get_earnings_trend(self, symbol).await
    }

    async fn get_earnings_trend_result(
        &self,
        symbol: &str,
    ) -> Result<Option<Vec<EarningsTrendRow>>, TradingError> {
        YFinanceClient::get_earnings_trend_result(self, symbol).await
    }

    async fn get_quote(&self, symbol: &str) -> Option<EtfQuote> {
        YFinanceClient::get_quote(self, symbol).await
    }

    async fn get_distribution_yield_ttm(&self, symbol: &str) -> Option<f64> {
        YFinanceClient::get_distribution_yield_ttm(self, symbol).await
    }

    async fn get_ohlcv(
        &self,
        symbol: &str,
        start: &str,
        end: &str,
    ) -> Result<Vec<Candle>, TradingError> {
        YFinanceClient::get_ohlcv(self, symbol, start, end).await
    }
}
