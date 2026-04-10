//! Consensus-estimates evidence contract and concrete yfinance-rs provider.
//!
//! Declares the [`ConsensusEvidence`] payload struct, the [`EstimatesProvider`]
//! trait seam, and the concrete [`YFinanceEstimatesProvider`] that normalizes
//! yfinance-rs earnings-trend data into the adapter contract.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// Analyst consensus-estimates evidence for a single ticker.
///
/// Stage 1: fields are defined for the full contract; live data population
/// is deferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusEvidence {
    /// Ticker symbol (canonical uppercase).
    pub symbol: String,
    /// Consensus EPS estimate for the next reported quarter.
    pub eps_estimate: Option<f64>,
    /// Consensus revenue estimate (USD millions) for the next reported quarter.
    pub revenue_estimate_m: Option<f64>,
    /// Number of analysts contributing to this consensus.
    pub analyst_count: Option<u32>,
    /// ISO-8601 date of the estimate snapshot (`"YYYY-MM-DD"`).
    pub as_of_date: String,
}

/// Contract for any provider that can supply [`ConsensusEvidence`].
///
/// Stage 1 seam only.  Implementations are introduced in Milestone 7.
#[async_trait]
pub trait EstimatesProvider: Send + Sync {
    /// Fetch the most recent consensus estimates for `symbol` as of `as_of_date`
    /// (`"YYYY-MM-DD"`).
    ///
    /// Returns `Ok(None)` when no estimates are available.
    async fn fetch_consensus(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<Option<ConsensusEvidence>, TradingError>;
}

// ─── Concrete provider: yfinance-rs ─────────────────────────────────────────

use yfinance_rs::analysis::EarningsTrendRow;
use yfinance_rs::core::conversions::money_to_f64;

use crate::data::YFinanceClient;

/// Normalizes yfinance-rs [`EarningsTrendRow`] data into [`ConsensusEvidence`].
///
/// This provider:
/// - Fetches earnings trend data via `YFinanceClient::get_earnings_trend`.
/// - Takes the first (nearest-quarter) row from the trend data.
/// - Extracts `earnings_estimate.avg` as EPS and `revenue_estimate.avg` as
///   revenue (converted from raw to millions).
/// - Uses the `num_analysts` count from the earnings estimate.
pub struct YFinanceEstimatesProvider {
    client: YFinanceClient,
}

impl YFinanceEstimatesProvider {
    /// Construct a new provider backed by the given Yahoo Finance client.
    pub fn new(client: YFinanceClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EstimatesProvider for YFinanceEstimatesProvider {
    async fn fetch_consensus(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<Option<ConsensusEvidence>, TradingError> {
        let rows = self.client.get_earnings_trend(symbol).await;

        match rows {
            Some(trend) if !trend.is_empty() => {
                Ok(Some(normalize_earnings_trend(symbol, as_of_date, &trend)))
            }
            _ => Ok(None),
        }
    }
}

/// Normalize the nearest-quarter earnings trend row into a `ConsensusEvidence`.
fn normalize_earnings_trend(
    symbol: &str,
    as_of_date: &str,
    rows: &[EarningsTrendRow],
) -> ConsensusEvidence {
    // Take the first row — yfinance-rs returns rows ordered by period,
    // with the nearest quarter first.
    let row = &rows[0];

    let eps_estimate = row.earnings_estimate.avg.as_ref().map(money_to_f64);

    let revenue_estimate_m = row
        .revenue_estimate
        .avg
        .as_ref()
        .map(|m| money_to_f64(m) / 1_000_000.0);

    let analyst_count = row.earnings_estimate.num_analysts;

    ConsensusEvidence {
        symbol: symbol.to_ascii_uppercase(),
        eps_estimate,
        revenue_estimate_m,
        analyst_count,
        as_of_date: as_of_date.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_evidence_serializes_and_deserializes() {
        let evidence = ConsensusEvidence {
            symbol: "MSFT".to_owned(),
            eps_estimate: Some(3.10),
            revenue_estimate_m: Some(65_500.0),
            analyst_count: Some(32),
            as_of_date: "2025-03-01".to_owned(),
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: ConsensusEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn consensus_evidence_all_optional_fields_none_roundtrips() {
        let evidence = ConsensusEvidence {
            symbol: "TSLA".to_owned(),
            eps_estimate: None,
            revenue_estimate_m: None,
            analyst_count: None,
            as_of_date: "2025-04-01".to_owned(),
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: ConsensusEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    // ── Normalization tests ──────────────────────────────────────────────

    /// Build a test `EarningsTrendRow` with just the fields we care about.
    fn make_trend_row(
        eps_avg: Option<f64>,
        revenue_avg: Option<f64>,
        num_analysts: Option<u32>,
    ) -> EarningsTrendRow {
        use paft_money::{Currency, IsoCurrency, Money};

        let to_money = |v: f64| {
            let d = rust_decimal::Decimal::try_from(v).unwrap();
            Money::new(d, Currency::Iso(IsoCurrency::USD)).unwrap()
        };

        // Build a minimal earnings-trend row via serde round-trip to avoid
        // needing `paft_fundamentals` types directly.
        let json = serde_json::json!({
            "period": "0Q",
            "growth": null,
            "earnings_estimate": {
                "avg": eps_avg.map(&to_money),
                "low": null,
                "high": null,
                "year_ago_eps": null,
                "num_analysts": num_analysts,
                "growth": null
            },
            "revenue_estimate": {
                "avg": revenue_avg.map(&to_money),
                "low": null,
                "high": null,
                "year_ago_revenue": null,
                "num_analysts": null,
                "growth": null
            },
            "eps_trend": {
                "current": null,
                "historical": []
            },
            "eps_revisions": {
                "historical": []
            }
        });
        serde_json::from_value(json).expect("valid test EarningsTrendRow")
    }

    #[test]
    fn normalize_earnings_trend_maps_eps_and_revenue() {
        let rows = vec![make_trend_row(Some(2.50), Some(65_000_000_000.0), Some(35))];

        let evidence = normalize_earnings_trend("AAPL", "2025-04-01", &rows);

        assert_eq!(evidence.symbol, "AAPL");
        assert_eq!(evidence.as_of_date, "2025-04-01");
        let eps = evidence.eps_estimate.expect("EPS should be present");
        assert!((eps - 2.50).abs() < 0.01, "EPS should be ~2.50, got {eps}");
        let rev = evidence.revenue_estimate_m.expect("revenue should be present");
        assert!(
            (rev - 65_000.0).abs() < 1.0,
            "revenue should be ~65000M, got {rev}"
        );
        assert_eq!(evidence.analyst_count, Some(35));
    }

    #[test]
    fn normalize_earnings_trend_handles_all_none() {
        let rows = vec![make_trend_row(None, None, None)];

        let evidence = normalize_earnings_trend("TSLA", "2025-04-01", &rows);

        assert_eq!(evidence.symbol, "TSLA");
        assert!(evidence.eps_estimate.is_none());
        assert!(evidence.revenue_estimate_m.is_none());
        assert!(evidence.analyst_count.is_none());
    }

    #[test]
    fn normalize_earnings_trend_preserves_uppercase_symbol() {
        let rows = vec![make_trend_row(Some(1.0), None, None)];
        let evidence = normalize_earnings_trend("msft", "2025-01-01", &rows);
        assert_eq!(evidence.symbol, "MSFT");
    }

    #[test]
    fn normalize_earnings_trend_converts_revenue_to_millions() {
        // Revenue in raw units = 50 billion
        let rows = vec![make_trend_row(None, Some(50_000_000_000.0), None)];
        let evidence = normalize_earnings_trend("AAPL", "2025-04-01", &rows);

        // After normalization: should be 50,000 (millions)
        let rev = evidence.revenue_estimate_m.unwrap();
        assert!(
            (rev - 50_000.0).abs() < 1.0,
            "revenue should be ~50000M, got {rev}"
        );
    }
}
