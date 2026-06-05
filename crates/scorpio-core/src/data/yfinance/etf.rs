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
//! [`EtfQuote::nav`], [`EtfQuote::bid`], and [`EtfQuote::ask`] are
//! backfilled via a direct call to Yahoo's `quoteSummary` endpoint (see
//! [`super::summary`]).
//!
//! Stated benchmark names are not resolved to market-data symbols in this
//! module. Official textual benchmark metadata is carried separately by the ETF
//! valuation path; benchmark OHLCV is intentionally disabled until a trusted
//! source can resolve daily benchmark history.
//!
//! When upstream begins exposing these fields, populate them here without
//! changing the public shape of [`EtfQuote`] / [`FundInfo`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use yfinance_rs::profile::Profile;

/// ETF quote snapshot — extends the regular quote with NAV and bid/ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EtfQuote {
    pub symbol: String,
    pub regular_market_price: f64,
    pub previous_close: Option<f64>,
    pub nav: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
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

#[must_use]
pub(crate) fn fund_info_from_profile(symbol: &str, profile: &Profile) -> Option<FundInfo> {
    match profile {
        Profile::Fund(fund) => {
            let category: Option<String> = None;
            let leverage_factor = derive_leverage_factor(Some(&fund.name), &category);
            Some(FundInfo {
                symbol: symbol.to_owned(),
                category,
                fund_family: fund.family.clone(),
                expense_ratio: None,
                total_assets: None,
                leverage_factor,
                fund_kind: Some(fund.kind.to_string().to_ascii_lowercase()),
                stated_benchmark: None,
            })
        }
        Profile::Company(_) => None,
        // `Profile` is `#[non_exhaustive]` in paft 0.8; any future variant is
        // not a recognized fund shape, so it carries no ETF fund info.
        _ => None,
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
