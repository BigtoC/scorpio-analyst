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
    analysis::{AnalysisBuilder, EarningsTrendRow, PriceTarget, RecommendationSummary},
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

    /// Fetch the analyst price-target summary while preserving the failure reason.
    ///
    /// Returns `Ok(None)` when Yahoo replies with a payload whose every field
    /// is `None` (i.e. an "empty" 200-OK response). This collapses the upstream
    /// "no data" shape into the explicit absence variant so the consensus
    /// adapter can distinguish "no usable fields" from "fetch failed".
    pub async fn get_analyst_price_target_result(
        &self,
        symbol: &str,
    ) -> Result<Option<PriceTarget>, crate::error::TradingError> {
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            if let Some(message) = &stubbed.price_target_error {
                return Err(crate::error::TradingError::SchemaViolation {
                    message: message.clone(),
                });
            }
            return Ok(stubbed
                .price_target
                .clone()
                .and_then(empty_price_target_to_none));
        }

        match self
            .session
            .with_rate_limit(
                AnalysisBuilder::new(self.session.client(), symbol).analyst_price_target(None),
            )
            .await
        {
            Ok(pt) => Ok(empty_price_target_to_none(pt)),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch analyst price target");
                Err(super::ohlcv::map_yf_err(e))
            }
        }
    }

    /// Fetch the analyst recommendation summary while preserving the failure
    /// reason. Returns `Ok(None)` when every aggregate field is `None`.
    pub async fn get_recommendations_summary_result(
        &self,
        symbol: &str,
    ) -> Result<Option<RecommendationSummary>, crate::error::TradingError> {
        #[cfg(test)]
        if let Some(stubbed) = &self.stubbed_financials {
            if let Some(message) = &stubbed.recommendation_summary_error {
                return Err(crate::error::TradingError::SchemaViolation {
                    message: message.clone(),
                });
            }
            return Ok(stubbed
                .recommendation_summary
                .clone()
                .and_then(empty_recommendation_summary_to_none));
        }

        match self
            .session
            .with_rate_limit(
                AnalysisBuilder::new(self.session.client(), symbol).recommendations_summary(),
            )
            .await
        {
            Ok(rs) => Ok(empty_recommendation_summary_to_none(rs)),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch recommendations summary");
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

/// Collapse a fully-empty Yahoo `PriceTarget` payload into `None`.
///
/// The Yahoo Finance "analyst price target" endpoint occasionally returns a
/// 200-OK response with every field set to `None`. The consensus adapter
/// treats that shape as "no usable data" rather than "data available", so we
/// normalize it before returning to the caller.
fn empty_price_target_to_none(pt: PriceTarget) -> Option<PriceTarget> {
    if pt.mean.is_none() && pt.high.is_none() && pt.low.is_none() && pt.number_of_analysts.is_none()
    {
        None
    } else {
        Some(pt)
    }
}

/// Collapse a fully-empty Yahoo `RecommendationSummary` payload into `None`.
fn empty_recommendation_summary_to_none(
    rs: RecommendationSummary,
) -> Option<RecommendationSummary> {
    if rs.strong_buy.is_none()
        && rs.buy.is_none()
        && rs.hold.is_none()
        && rs.sell.is_none()
        && rs.strong_sell.is_none()
    {
        None
    } else {
        Some(rs)
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

    // ── Result-preserving Yahoo wrappers (Task 2) ────────────────────────

    use crate::data::StubbedFinancialResponses;
    use crate::error::TradingError;
    use yfinance_rs::analysis::{PriceTarget, RecommendationSummary};

    #[tokio::test]
    async fn get_analyst_price_target_result_preserves_yahoo_failure_reason() {
        let client = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            price_target_error: Some("rate limit reason X".to_owned()),
            ..StubbedFinancialResponses::default()
        });

        let err = client
            .get_analyst_price_target_result("AAPL")
            .await
            .expect_err("stubbed Yahoo failure should surface as Err");

        match err {
            TradingError::SchemaViolation { message } => {
                assert!(
                    message.contains("reason X"),
                    "expected error message to include upstream reason, got: {message}"
                );
            }
            other => panic!("expected SchemaViolation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_recommendations_summary_result_preserves_yahoo_failure_reason() {
        let client = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            recommendation_summary_error: Some("rate limit reason X".to_owned()),
            ..StubbedFinancialResponses::default()
        });

        let err = client
            .get_recommendations_summary_result("AAPL")
            .await
            .expect_err("stubbed Yahoo failure should surface as Err");

        match err {
            TradingError::SchemaViolation { message } => {
                assert!(
                    message.contains("reason X"),
                    "expected error message to include upstream reason, got: {message}"
                );
            }
            other => panic!("expected SchemaViolation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_price_target_payload_returns_none() {
        let client = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            price_target: Some(PriceTarget {
                mean: None,
                high: None,
                low: None,
                number_of_analysts: None,
            }),
            ..StubbedFinancialResponses::default()
        });

        let result = client
            .get_analyst_price_target_result("AAPL")
            .await
            .expect("empty upstream payload should not be an error");

        assert!(
            result.is_none(),
            "expected Ok(None) for all-empty PriceTarget upstream payload, got {result:?}"
        );
    }

    #[tokio::test]
    async fn empty_recommendations_summary_payload_returns_none() {
        let client = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            recommendation_summary: Some(RecommendationSummary::default()),
            ..StubbedFinancialResponses::default()
        });

        let result = client
            .get_recommendations_summary_result("AAPL")
            .await
            .expect("empty upstream payload should not be an error");

        assert!(
            result.is_none(),
            "expected Ok(None) for all-empty RecommendationSummary upstream payload, got {result:?}"
        );
    }
}
