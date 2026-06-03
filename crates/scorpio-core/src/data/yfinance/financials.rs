//! Financial statement and profile fetchers built on top of [`YFinanceClient`].
//!
//! Exposes quarterly financial data (cashflow, balance sheet, income statement,
//! shares outstanding), earnings trend data, and Yahoo Finance profile information.
//! All functions degrade gracefully to `None` so the pipeline can continue when
//! upstream data is unavailable or a symbol is not a corporate equity.
//!
//! # Asset-shape detection
//!
//! [`YFinanceClient::get_profile`] returns the raw `Profile` enum from
//! `yfinance_rs`. Callers should treat `Profile::Fund(_)` as a signal that
//! corporate-equity valuation inputs may be structurally absent (not a data error).
//!
//! # Degradation contract
//!
//! Every method in this module returns `Option<T>`. A `None` result means the
//! data was unavailable or could not be parsed — not that the pipeline should
//! abort. Callers are responsible for deciding whether to emit `NotAssessed`.

use chrono::NaiveDate;
use tracing::warn;
use yfinance_rs::{
    FundamentalsBuilder,
    analysis::{AnalysisBuilder, EarningsTrendRow},
    fundamentals::{BalanceSheetRow, Calendar, CashflowRow, IncomeStatementRow, ShareCount},
    profile::{self, Profile},
    ticker::{Info, Ticker},
};

/// Thin domain wrapper over the upstream yfinance `Calendar` type.
///
/// Uses `NaiveDate` (date-only) because providers disagree on time-of-day and
/// the catalyst calendar only needs the day.
#[derive(Debug, Clone, PartialEq)]
pub struct TickerCalendar {
    /// Upcoming or historical earnings dates (UTC).
    pub earnings_dates: Vec<NaiveDate>,
    /// Ex-dividend date, if declared.
    pub ex_dividend_date: Option<NaiveDate>,
    /// Dividend payment date, if declared.
    pub dividend_payment_date: Option<NaiveDate>,
}

impl From<Calendar> for TickerCalendar {
    fn from(c: Calendar) -> Self {
        Self {
            earnings_dates: c
                .earnings_dates
                .into_iter()
                .map(|dt| dt.date_naive())
                .collect(),
            ex_dividend_date: c.ex_dividend_date.map(|dt| dt.date_naive()),
            dividend_payment_date: c.dividend_payment_date.map(|dt| dt.date_naive()),
        }
    }
}

use super::ohlcv::YFinanceClient;

impl YFinanceClient {
    // ── Financial statements ─────────────────────────────────────────────

    /// Fetch quarterly cash flow statement rows for `symbol`.
    ///
    /// Returns `None` on network or parsing failures so the caller can degrade
    /// gracefully without aborting the pipeline.
    pub async fn get_quarterly_cashflow(&self, symbol: &str) -> Option<Vec<CashflowRow>> {
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

    /// Fetch quarterly balance sheet rows for `symbol`.
    ///
    /// Returns `None` on network or parsing failures.
    pub async fn get_quarterly_balance_sheet(&self, symbol: &str) -> Option<Vec<BalanceSheetRow>> {
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

    /// Fetch quarterly income statement rows for `symbol`.
    ///
    /// Returns `None` on network or parsing failures.
    pub async fn get_quarterly_income_stmt(&self, symbol: &str) -> Option<Vec<IncomeStatementRow>> {
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

    /// Fetch quarterly shares-outstanding data for `symbol`.
    ///
    /// Returns `None` on network or parsing failures.
    pub async fn get_quarterly_shares(&self, symbol: &str) -> Option<Vec<ShareCount>> {
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

    // ── Analyst data ─────────────────────────────────────────────────────

    /// Fetch earnings trend data (analyst EPS / revenue estimates) for `symbol`.
    ///
    /// Returns `None` on network or parsing failures.
    pub async fn get_earnings_trend(&self, symbol: &str) -> Option<Vec<EarningsTrendRow>> {
        self.get_earnings_trend_result(symbol).await.ok().flatten()
    }

    /// Fetch earnings trend data while preserving the failure reason.
    pub async fn get_earnings_trend_result(
        &self,
        symbol: &str,
    ) -> Result<Option<Vec<EarningsTrendRow>>, crate::error::TradingError> {
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
                Err(super::ohlcv::map_yf_err(e))
            }
        }
    }

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

    // ── Shared Info snapshot ─────────────────────────────────────────────

    /// Fetch the composed yfinance [`Info`] once via a single
    /// `Ticker::info()` call (which itself fans out to the underlying
    /// endpoints concurrently).
    ///
    /// Stored once per cycle on `TradingState::yfinance_info` and read by pack
    /// classification (`profile`), valuation + the ETF path (`profile`,
    /// `key_statistics.market_cap`), the catalyst adapter (`calendar`), and the
    /// consensus provider (`price_target`, `recommendation_summary`). Sharing
    /// the single `Info` eliminates the duplicate profile fetches and the
    /// separate price-target / recommendation / calendar calls.
    ///
    /// Fail-soft: returns `None` when the core profile fetch errors (the only
    /// hard-error path in `Ticker::info()`); individual sub-fields already
    /// degrade to `None` inside `info()`.
    pub async fn get_info(&self, symbol: &str) -> Option<Info> {
        let ticker = Ticker::new(self.session.client(), symbol);
        match self.session.with_rate_limit(ticker.info()).await {
            Ok(info) => Some(info),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance Info");
                None
            }
        }
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

    use crate::rate_limit::SharedRateLimiter;

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

    // ── TickerCalendar conversion tests ──────────────────────────────────

    #[test]
    fn ticker_calendar_from_upstream_with_all_fields() {
        use chrono::TimeZone;
        let upstream = yfinance_rs::fundamentals::Calendar {
            earnings_dates: vec![
                chrono::Utc.with_ymd_and_hms(2026, 7, 15, 20, 0, 0).unwrap(),
                chrono::Utc
                    .with_ymd_and_hms(2026, 10, 14, 20, 0, 0)
                    .unwrap(),
            ],
            ex_dividend_date: Some(chrono::Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 0).unwrap()),
            dividend_payment_date: Some(
                chrono::Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap(),
            ),
        };
        let cal = TickerCalendar::from(upstream);
        assert_eq!(cal.earnings_dates.len(), 2);
        assert_eq!(
            cal.earnings_dates[0],
            chrono::NaiveDate::from_ymd_opt(2026, 7, 15).unwrap()
        );
        assert_eq!(
            cal.ex_dividend_date,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 9).unwrap())
        );
        assert_eq!(
            cal.dividend_payment_date,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 15).unwrap())
        );
    }

    #[test]
    fn ticker_calendar_from_upstream_with_empty_fields() {
        let upstream = yfinance_rs::fundamentals::Calendar {
            earnings_dates: vec![],
            ex_dividend_date: None,
            dividend_payment_date: None,
        };
        let cal = TickerCalendar::from(upstream);
        assert!(cal.earnings_dates.is_empty());
        assert!(cal.ex_dividend_date.is_none());
        assert!(cal.dividend_payment_date.is_none());
    }
}
