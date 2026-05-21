//! ETF-specific yfinance types — quote, fund info, leverage detection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}
