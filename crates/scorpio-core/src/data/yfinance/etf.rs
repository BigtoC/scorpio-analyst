//! ETF-specific yfinance types — quote, fund info, leverage detection.
//!
//! # Upstream coverage notes
//!
//! `yfinance-rs` 0.7 (via the `paft` re-exports) does **not** expose the
//! following ETF-relevant fields on its `Quote` / `Info` / `Profile::Fund`
//! types: `bid`, `ask`, `nav_price`, `category`, `expense_ratio` (net or
//! gross), `total_assets`, `tracked_index` / `benchmark`. We populate what is
//! available and leave the remaining fields as `None` so that downstream
//! consumers (Task 13 hydration, Task 10 valuator) can decide whether to
//! degrade gracefully or backfill via a different provider (e.g. SEC EDGAR
//! N-PORT for holdings / total assets in Task 9).
//!
//! When upstream begins exposing these fields, populate them here without
//! changing the public shape of [`EtfQuote`] / [`FundInfo`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;
use yfinance_rs::Range;
use yfinance_rs::core::conversions::{money_to_currency_str, money_to_f64};
use yfinance_rs::profile::Profile;
use yfinance_rs::ticker::Ticker;

use super::ohlcv::YFinanceClient;

/// ETF quote snapshot — extends the regular quote with NAV and bid/ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EtfQuote {
    pub symbol: String,
    pub regular_market_price: f64,
    pub previous_close: Option<f64>,
    pub nav: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub market_cap: Option<f64>,
    pub day_volume: Option<u64>,
    pub currency: Option<String>,
    pub as_of: DateTime<Utc>,
}

/// Fund-level metadata pulled from yfinance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundInfo {
    pub symbol: String,
    pub category: Option<String>,
    pub fund_family: Option<String>,
    pub expense_ratio: Option<f64>,
    pub total_assets: Option<f64>,
    /// `Some(1.0)` for plain ETFs; `Some(2.0)`, `Some(3.0)`, `Some(-1.0)`,
    /// etc. for leveraged/inverse. `None` when undetermined.
    pub leverage_factor: Option<f64>,
    /// e.g. "etf", "mutual_fund". Lowercased.
    pub fund_kind: Option<String>,
    /// Stated benchmark symbol or index name when present in fund metadata.
    pub stated_benchmark: Option<String>,
}

/// Subset of supported ETF kinds. Used by [`is_supported_etf_kind`] in
/// runtime classification.
#[must_use]
pub fn is_supported_etf_kind(kind: &str) -> bool {
    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "etf" | "exchange-traded fund" | "exchangetradedfund"
    )
}

/// Heuristic leverage detection from fund name and category.
///
/// Returns `Some(1.0)` for a plain ETF when neither the name nor the category
/// names a leverage multiplier; returns `Some(2.0)` / `Some(3.0)` / `Some(-1.0)`
/// when a known multiplier prefix is present. Defaults to `Some(1.0)`.
fn derive_leverage_factor(fund_name: Option<&str>, category: &Option<String>) -> Option<f64> {
    let haystack = format!(
        "{} {}",
        fund_name.unwrap_or(""),
        category.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();
    if haystack.contains("3x") || haystack.contains("ultra pro") {
        Some(3.0)
    } else if haystack.contains("2x") || haystack.contains("ultra") {
        Some(2.0)
    } else if haystack.contains("inverse") || haystack.contains("-1x") || haystack.contains("short")
    {
        Some(-1.0)
    } else {
        Some(1.0)
    }
}

impl YFinanceClient {
    // ── ETF quote ────────────────────────────────────────────────────────

    /// Fetch a quote snapshot for an ETF (or any) symbol.
    ///
    /// Fail-soft: returns `None` on network failure, missing payload, or any
    /// upstream parsing error. Network errors are recorded via `tracing::warn`.
    ///
    /// # Coverage caveats
    ///
    /// `yfinance-rs` 0.7 does not expose bid/ask or NAV via its `Quote` /
    /// `Info` types, so [`EtfQuote::bid`], [`EtfQuote::ask`], and
    /// [`EtfQuote::nav`] are always populated as `None` by this method.
    /// [`EtfQuote::market_cap`] is sourced from `Ticker::info()` and is
    /// `None` when that secondary call fails.
    pub async fn get_quote(&self, symbol: &str) -> Option<EtfQuote> {
        let ticker = Ticker::new(self.session.client(), symbol);

        let quote = match self.session.with_rate_limit(ticker.quote()).await {
            Ok(q) => q,
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance ETF quote");
                return None;
            }
        };

        // Market cap lives on `Info`, not `Quote`. Best-effort: log + carry
        // on if the secondary fetch fails so we still return the price snapshot.
        let info = match self.session.with_rate_limit(ticker.info()).await {
            Ok(i) => Some(i),
            Err(e) => {
                warn!(error = %e, symbol, "failed to fetch yfinance ETF info for market_cap");
                None
            }
        };

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
            symbol: quote.symbol.to_string(),
            regular_market_price,
            previous_close: quote.previous_close.as_ref().map(money_to_f64),
            nav: None,
            bid: None,
            ask: None,
            market_cap: info
                .as_ref()
                .and_then(|i| i.market_cap.as_ref().map(money_to_f64)),
            day_volume: quote.day_volume,
            currency,
            as_of: Utc::now(),
        })
    }

    // ── ETF fund info ────────────────────────────────────────────────────

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
    /// [`derive_leverage_factor`].
    pub async fn get_fund_info(&self, symbol: &str) -> Option<FundInfo> {
        let profile = self.get_profile(symbol).await?;
        match profile {
            Profile::Fund(fund) => {
                let category: Option<String> = None;
                let leverage_factor = derive_leverage_factor(Some(&fund.name), &category);
                Some(FundInfo {
                    symbol: symbol.to_owned(),
                    category,
                    fund_family: fund.family,
                    expense_ratio: None,
                    total_assets: None,
                    leverage_factor,
                    fund_kind: Some(fund.kind.to_string().to_ascii_lowercase()),
                    stated_benchmark: None,
                })
            }
            Profile::Company(_) => {
                warn!(symbol, "get_fund_info called on a Company profile");
                None
            }
        }
    }

    // ── ETF distribution yield (TTM) ─────────────────────────────────────

    /// Compute trailing-twelve-month distribution yield as
    /// `(sum of last 365 days of distributions) / current_price`.
    ///
    /// Returns `None` when distribution history or the current price cannot be
    /// fetched, or when the current price is non-positive (guarding against a
    /// divide-by-zero blow-up).
    pub async fn get_distribution_yield_ttm(&self, symbol: &str) -> Option<f64> {
        let ticker = Ticker::new(self.session.client(), symbol);

        // `Ticker::dividends` returns Vec<(unix_ts_seconds, amount_f64)>.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_supported_etf_kind_matches_known_variants() {
        assert!(is_supported_etf_kind("etf"));
        assert!(is_supported_etf_kind("ETF"));
        assert!(is_supported_etf_kind("Exchange-Traded Fund"));
        assert!(!is_supported_etf_kind("mutual_fund"));
        assert!(!is_supported_etf_kind(""));
    }

    #[test]
    fn derive_leverage_factor_detects_3x() {
        assert_eq!(
            derive_leverage_factor(Some("ProShares Ultra Pro QQQ"), &None),
            Some(3.0)
        );
    }

    #[test]
    fn derive_leverage_factor_detects_2x() {
        assert_eq!(
            derive_leverage_factor(Some("ProShares Ultra QQQ"), &None),
            Some(2.0)
        );
    }

    #[test]
    fn derive_leverage_factor_detects_inverse() {
        assert_eq!(
            derive_leverage_factor(Some("ProShares Short S&P 500"), &None),
            Some(-1.0)
        );
    }

    #[test]
    fn derive_leverage_factor_defaults_to_1x() {
        assert_eq!(
            derive_leverage_factor(Some("SPDR S&P 500 ETF Trust"), &None),
            Some(1.0)
        );
        assert_eq!(
            derive_leverage_factor(None, &Some("Large Blend".to_owned())),
            Some(1.0)
        );
    }
}
