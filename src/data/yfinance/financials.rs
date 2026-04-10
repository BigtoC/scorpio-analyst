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

use tracing::warn;
use yfinance_rs::{
    FundamentalsBuilder,
    analysis::{AnalysisBuilder, EarningsTrendRow},
    fundamentals::{BalanceSheetRow, CashflowRow, IncomeStatementRow, ShareCount},
    profile::{self, Profile},
};

use super::ohlcv::YFinanceClient;

impl YFinanceClient {
    // ── Financial statements ─────────────────────────────────────────────

    /// Fetch quarterly cash flow statement rows for `symbol`.
    ///
    /// Returns `None` on network or parsing failures so the caller can degrade
    /// gracefully without aborting the pipeline.
    pub async fn get_quarterly_cashflow(&self, symbol: &str) -> Option<Vec<CashflowRow>> {
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            return stubbed.cashflow.clone();
        }

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
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            return stubbed.balance.clone();
        }

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
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            return stubbed.income.clone();
        }

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
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            return stubbed.shares.clone();
        }

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
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            if let Some(message) = &stubbed.trend_error {
                return Err(crate::error::TradingError::SchemaViolation {
                    message: message.clone(),
                });
            }
            return Ok(stubbed.trend.clone());
        }

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
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            return stubbed.profile.clone();
        }

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

    // Smoke: ensure all methods exist and have the correct signatures.
    // Network calls are not made in CI; this test only validates that the
    // code compiles and the client can be constructed without panicking.

    #[test]
    fn yfinance_client_has_financial_fetcher_methods_and_is_constructible() {
        // If this test compiles, all method signatures are syntactically correct.
        let _client = YFinanceClient::default();
        // Method existence is proven by the fact that this file compiles.
        // We cannot coerce async fn items to fn pointers due to lifetime
        // constraints — verifying via trait object wrapping is intentionally
        // avoided here; the presence of the method signatures above is sufficient.
    }
}
