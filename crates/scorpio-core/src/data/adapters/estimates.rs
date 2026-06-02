//! Consensus-estimates evidence contract and concrete yfinance-rs provider.
//!
//! Declares the [`ConsensusEvidence`] payload struct, the [`EstimatesProvider`]
//! trait seam, and the concrete [`YFinanceEstimatesProvider`] that normalizes
//! yfinance-rs earnings-trend data into the adapter contract.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::warn;
use yfinance_rs::analysis::{
    PriceTarget as UpstreamPriceTarget, RecommendationSummary as UpstreamRecommendationSummary,
};

use crate::error::TradingError;

/// Analyst consensus-estimates evidence for a single ticker.
///
/// Stage 1: fields are defined for the full contract; live data population
/// is deferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Aggregated price-target distribution (mean / high / low / analyst count).
    /// Additive field — older snapshots will deserialize with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_target: Option<PriceTargetSummary>,
    /// Aggregated analyst recommendation distribution (strong-buy → strong-sell).
    /// Additive field — older snapshots will deserialize with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendations: Option<RecommendationsSummary>,
    /// Per-symbol counter for consecutive `ProviderDegraded` outcomes from
    /// [`EstimatesProvider::fetch_consensus`]. Used by the runtime hydration
    /// half-life policy in `workflow/pipeline/runtime.rs::hydrate_consensus`.
    /// Additive snapshot-safe field — older snapshots default to `0`.
    #[serde(default)]
    pub consecutive_provider_degraded_cycles: u32,
}

/// Summary statistics for analyst price targets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PriceTargetSummary {
    #[serde(default)]
    pub mean: Option<f64>,
    #[serde(default)]
    pub high: Option<f64>,
    #[serde(default)]
    pub low: Option<f64>,
    #[serde(default)]
    pub analyst_count: Option<u32>, // mapped from upstream `number_of_analysts`
}

/// Aggregated count of analyst recommendations across the standard buckets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct RecommendationsSummary {
    #[serde(default)]
    pub strong_buy: Option<u32>,
    #[serde(default)]
    pub buy: Option<u32>,
    #[serde(default)]
    pub hold: Option<u32>,
    #[serde(default)]
    pub sell: Option<u32>,
    #[serde(default)]
    pub strong_sell: Option<u32>,
}

/// Structured outcome of an analyst-consensus fetch.
///
/// Replaces the prior `Result<Option<ConsensusEvidence>, _>` shape so a
/// degraded provider (one branch errored, others empty) cannot be silently
/// confused with "no analyst coverage" (all branches succeeded but empty).
///
/// See `docs/superpowers/plans/2026-04-26-yfinance-news-options-consensus-implementation.md`
/// (Guardrails > No-data taxonomy) for the full semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsensusOutcome {
    /// At least one upstream branch produced usable data.
    Data(ConsensusEvidence),
    /// All upstream branches succeeded but none produced usable data.
    NoCoverage,
    /// At least one upstream branch errored AND no remaining successful
    /// branch yielded usable fields.
    ProviderDegraded,
}

/// Contract for any provider that can supply [`ConsensusEvidence`].
///
/// Implementations return a [`ConsensusOutcome`] so consumers can distinguish
/// "data available" / "no analyst coverage" / "provider degraded" without
/// collapsing them into a single `Option`.
#[async_trait]
pub trait EstimatesProvider: Send + Sync {
    /// Fetch the most recent consensus estimates for `symbol` as of `as_of_date`
    /// (`"YYYY-MM-DD"`).
    async fn fetch_consensus(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<ConsensusOutcome, TradingError>;
}

// ─── Concrete provider: yfinance-rs ─────────────────────────────────────────

use yfinance_rs::analysis::EarningsTrendRow;
use yfinance_rs::core::conversions::money_to_f64;

use std::sync::Arc;

use crate::data::YFinanceData;

/// Normalizes yfinance-rs [`EarningsTrendRow`] data into [`ConsensusEvidence`].
///
/// This provider:
/// - Fetches earnings trend data via [`YFinanceData::get_earnings_trend_result`].
/// - Takes the first (nearest-quarter) row from the trend data.
/// - Extracts `earnings_estimate.avg` as EPS and `revenue_estimate.avg` as
///   revenue (converted from raw to millions).
/// - Uses the `num_analysts` count from the earnings estimate.
///
/// The data source is held behind the [`YFinanceData`] trait so tests can
/// inject a `MockYFinanceData` and set the earnings-trend response directly,
/// rather than mocking the HTTP layer beneath the `yfinance-rs` library.
pub struct YFinanceEstimatesProvider {
    client: Arc<dyn YFinanceData>,
    /// Analyst price target lifted from the shared `Info` snapshot. The
    /// earnings-trend branch is still fetched live (it is not part of `Info`),
    /// but price target / recommendations are read from here so they are not
    /// fetched a second time.
    price_target: Option<UpstreamPriceTarget>,
    /// Analyst recommendation summary lifted from the shared `Info` snapshot.
    recommendations: Option<UpstreamRecommendationSummary>,
}

impl std::fmt::Debug for YFinanceEstimatesProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YFinanceEstimatesProvider")
            .field("price_target", &self.price_target)
            .field("recommendations", &self.recommendations)
            .finish_non_exhaustive()
    }
}

impl YFinanceEstimatesProvider {
    /// Construct a provider with no pre-fetched consensus inputs. The
    /// earnings-trend branch is fetched live; price target / recommendations
    /// are treated as absent.
    pub fn new(client: Arc<dyn YFinanceData>) -> Self {
        Self {
            client,
            price_target: None,
            recommendations: None,
        }
    }

    /// Construct a provider seeded with the price target / recommendation
    /// summary already fetched once via `YFinanceClient::get_info`, so
    /// `fetch_consensus` only issues the live earnings-trend call.
    pub fn with_consensus_inputs(
        client: Arc<dyn YFinanceData>,
        price_target: Option<UpstreamPriceTarget>,
        recommendations: Option<UpstreamRecommendationSummary>,
    ) -> Self {
        Self {
            client,
            price_target,
            recommendations,
        }
    }
}

#[async_trait]
impl EstimatesProvider for YFinanceEstimatesProvider {
    async fn fetch_consensus(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<ConsensusOutcome, TradingError> {
        // Only the earnings-trend branch is fetched live — price target and
        // recommendations are lifted from the shared `Info` snapshot fetched
        // once per cycle. The trend branch therefore carries the sole error
        // provenance; price target / recommendations are present-or-absent
        // (an empty upstream payload is collapsed to absent below).
        let trend_res = self.client.get_earnings_trend_result(symbol).await;
        let trend_branch = classify_branch(trend_res, |rows| {
            rows.as_ref().is_some_and(|r| !r.is_empty())
        });

        let price_target = self
            .price_target
            .as_ref()
            .filter(|pt| !price_target_is_empty(pt));
        let recommendations = self
            .recommendations
            .as_ref()
            .filter(|rs| !recommendations_is_empty(rs));

        let any_error = trend_branch.is_error();
        let any_data =
            trend_branch.is_data() || price_target.is_some() || recommendations.is_some();

        if !any_data {
            // No branch produced usable data. A live trend error with empty
            // cached consensus is "provider degraded"; otherwise no coverage.
            return if any_error {
                emit_trend_warn(&trend_branch);
                Ok(ConsensusOutcome::ProviderDegraded)
            } else {
                Ok(ConsensusOutcome::NoCoverage)
            };
        }

        // At least one branch has data. If the trend branch errored, log it and
        // still return Data with the cached consensus fields filled in.
        if any_error {
            emit_trend_warn(&trend_branch);
        }

        let mut evidence = if let BranchOutcome::Data(Some(trend)) = &trend_branch {
            normalize_earnings_trend(symbol, as_of_date, trend)
        } else {
            // Trend branch did not produce data. Build a stub with only the
            // identity fields set; price_target/recommendations are filled
            // below from the shared `Info` snapshot.
            ConsensusEvidence {
                symbol: symbol.to_ascii_uppercase(),
                eps_estimate: None,
                revenue_estimate_m: None,
                analyst_count: None,
                as_of_date: as_of_date.to_owned(),
                price_target: None,
                recommendations: None,
                consecutive_provider_degraded_cycles: 0,
            }
        };

        if let Some(pt) = price_target {
            evidence.price_target = Some(price_target_summary_from_upstream(pt));
        }
        if let Some(rs) = recommendations {
            evidence.recommendations = Some(recommendations_summary_from_upstream(rs));
        }

        Ok(ConsensusOutcome::Data(evidence))
    }
}

/// `true` when every aggregate field of the upstream price target is `None`
/// (an "empty" 200-OK payload), which `Info` does not collapse on its own.
fn price_target_is_empty(pt: &UpstreamPriceTarget) -> bool {
    pt.mean.is_none() && pt.high.is_none() && pt.low.is_none() && pt.number_of_analysts.is_none()
}

/// `true` when every recommendation bucket is `None`.
fn recommendations_is_empty(rs: &UpstreamRecommendationSummary) -> bool {
    rs.strong_buy.is_none()
        && rs.buy.is_none()
        && rs.hold.is_none()
        && rs.sell.is_none()
        && rs.strong_sell.is_none()
}

/// Classifies a branch result as "data" / "empty" / "error" for partial
/// fail-open analysis. `is_data` decides whether the `Ok(_)` payload counts
/// as having data — for `Vec<EarningsTrendRow>` we require non-empty;
/// for `Option<PriceTarget>` / `Option<RecommendationSummary>` we require
/// `Some(_)` (the wrappers already collapse all-empty payloads to `None`).
enum BranchOutcome<T> {
    Data(T),
    Empty,
    Error(TradingError),
}

impl<T> BranchOutcome<T> {
    fn is_data(&self) -> bool {
        matches!(self, Self::Data(_))
    }

    fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

fn classify_branch<T, F>(res: Result<T, TradingError>, has_data: F) -> BranchOutcome<T>
where
    F: FnOnce(&T) -> bool,
{
    match res {
        Ok(payload) => {
            if has_data(&payload) {
                BranchOutcome::Data(payload)
            } else {
                BranchOutcome::Empty
            }
        }
        Err(e) => BranchOutcome::Error(e),
    }
}

fn emit_trend_warn<T>(trend: &BranchOutcome<T>) {
    if let BranchOutcome::Error(e) = trend {
        warn!(provider = "yfinance", endpoint = "earnings_trend", reason = %e, "consensus branch failed");
    }
}

fn price_target_summary_from_upstream(pt: &UpstreamPriceTarget) -> PriceTargetSummary {
    PriceTargetSummary {
        mean: pt.mean.as_ref().map(money_to_f64),
        high: pt.high.as_ref().map(money_to_f64),
        low: pt.low.as_ref().map(money_to_f64),
        analyst_count: pt.number_of_analysts,
    }
}

fn recommendations_summary_from_upstream(
    rs: &UpstreamRecommendationSummary,
) -> RecommendationsSummary {
    RecommendationsSummary {
        strong_buy: rs.strong_buy,
        buy: rs.buy,
        hold: rs.hold,
        sell: rs.sell,
        strong_sell: rs.strong_sell,
    }
}

/// Normalize the nearest-quarter earnings trend row into a `ConsensusEvidence`.
fn normalize_earnings_trend(
    symbol: &str,
    as_of_date: &str,
    rows: &[EarningsTrendRow],
) -> ConsensusEvidence {
    let row = select_next_quarter_row(rows).unwrap_or(&rows[0]);

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
        price_target: None,
        recommendations: None,
        consecutive_provider_degraded_cycles: 0,
    }
}

fn quarter_period_priority(period: &str) -> Option<u8> {
    match period.trim().to_ascii_uppercase().as_str() {
        "+1Q" | "1Q" => Some(0),
        "0Q" => Some(1),
        "-1Q" => Some(2),
        "+2Q" | "2Q" => Some(3),
        _ => None,
    }
}

fn select_next_quarter_row(trend: &[EarningsTrendRow]) -> Option<&EarningsTrendRow> {
    trend
        .iter()
        .filter_map(|row| {
            quarter_period_priority(&row.period.to_string()).map(|priority| (priority, row))
        })
        .min_by_key(|(priority, _)| *priority)
        .map(|(_, row)| row)
        .or_else(|| trend.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::MockYFinanceData;
    use std::sync::Arc;

    #[test]
    fn consensus_evidence_serializes_and_deserializes() {
        let evidence = ConsensusEvidence {
            symbol: "MSFT".to_owned(),
            eps_estimate: Some(3.10),
            revenue_estimate_m: Some(65_500.0),
            analyst_count: Some(32),
            as_of_date: "2025-03-01".to_owned(),
            price_target: None,
            recommendations: None,
            consecutive_provider_degraded_cycles: 0,
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: ConsensusEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn consensus_evidence_missing_extended_fields_defaults_to_none() {
        // Backward-compat: pre-extended-consensus snapshots lack the
        // additive `price_target` and `recommendations` keys. Deserialization
        // must default both to None rather than failing.
        let json = r#"{
            "symbol": "AAPL",
            "eps_estimate": 2.5,
            "revenue_estimate_m": 95000.0,
            "analyst_count": 35,
            "as_of_date": "2026-03-15"
        }"#;
        let evidence: ConsensusEvidence = serde_json::from_str(json)
            .expect("legacy consensus payload without extended fields must deserialize");
        assert!(
            evidence.price_target.is_none(),
            "missing price_target should default to None"
        );
        assert!(
            evidence.recommendations.is_none(),
            "missing recommendations should default to None"
        );
    }

    #[test]
    fn consensus_evidence_all_optional_fields_none_roundtrips() {
        let evidence = ConsensusEvidence {
            symbol: "TSLA".to_owned(),
            eps_estimate: None,
            revenue_estimate_m: None,
            analyst_count: None,
            as_of_date: "2025-04-01".to_owned(),
            price_target: None,
            recommendations: None,
            consecutive_provider_degraded_cycles: 0,
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
        let rev = evidence
            .revenue_estimate_m
            .expect("revenue should be present");
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

    #[test]
    fn normalize_earnings_trend_prefers_next_quarter_row_over_first_row_position() {
        let annual = serde_json::json!({
            "period": "+1Y",
            "growth": null,
            "earnings_estimate": {
                "avg": null,
                "low": null,
                "high": null,
                "year_ago_eps": null,
                "num_analysts": null,
                "growth": null
            },
            "revenue_estimate": {
                "avg": null,
                "low": null,
                "high": null,
                "year_ago_revenue": null,
                "num_analysts": null,
                "growth": null
            },
            "eps_trend": { "current": null, "historical": [] },
            "eps_revisions": { "historical": [] }
        });
        let annual_row: EarningsTrendRow =
            serde_json::from_value(annual).expect("valid annual row");

        let quarterly_row = make_trend_row(Some(2.75), Some(80_000_000_000.0), Some(28));
        let rows = vec![annual_row, quarterly_row];

        let evidence = normalize_earnings_trend("AAPL", "2025-04-01", &rows);

        assert_eq!(evidence.eps_estimate, Some(2.75));
        assert_eq!(evidence.analyst_count, Some(28));
        assert_eq!(evidence.revenue_estimate_m, Some(80_000.0));
    }

    // ── ConsensusOutcome behavioral regressions (Task 2) ─────────────────

    use yfinance_rs::analysis::{PriceTarget, RecommendationSummary};

    fn priced_target() -> PriceTarget {
        use paft_money::{Currency, IsoCurrency, Price};
        // paft 0.8 analyst price-target fields are `Price` (per-unit), not `Money`.
        let to_price = |v: f64| {
            let d = rust_decimal::Decimal::try_from(v).unwrap();
            Price::new(d, Currency::Iso(IsoCurrency::USD))
        };
        PriceTarget {
            mean: Some(to_price(220.0)),
            high: Some(to_price(260.0)),
            low: Some(to_price(180.0)),
            number_of_analysts: Some(28),
        }
    }

    fn populated_recommendations() -> RecommendationSummary {
        RecommendationSummary {
            strong_buy: Some(8),
            buy: Some(15),
            hold: Some(5),
            sell: Some(1),
            strong_sell: Some(0),
            ..RecommendationSummary::default()
        }
    }

    #[tokio::test]
    async fn fetch_consensus_populates_price_target_and_recommendations() {
        // Trend is fetched live (stubbed on the client); price target and
        // recommendations are supplied from the shared `Info` snapshot.
        let mut mock = MockYFinanceData::new();
        mock.expect_get_earnings_trend_result().returning(|_| {
            Ok(Some(vec![make_trend_row(
                Some(2.50),
                Some(95_000_000_000.0),
                Some(35),
            )]))
        });
        let provider = YFinanceEstimatesProvider::with_consensus_inputs(
            Arc::new(mock),
            Some(priced_target()),
            Some(populated_recommendations()),
        );

        let outcome = provider
            .fetch_consensus("AAPL", "2025-04-01")
            .await
            .expect("fetch should succeed when all branches return data");

        let evidence = match outcome {
            ConsensusOutcome::Data(ev) => ev,
            other => panic!("expected ConsensusOutcome::Data, got {other:?}"),
        };

        assert_eq!(evidence.symbol, "AAPL");
        assert_eq!(evidence.eps_estimate, Some(2.50));
        let revenue_m = evidence.revenue_estimate_m.unwrap();
        assert!(
            (revenue_m - 95_000.0).abs() < 1.0,
            "expected ~95000M, got {revenue_m}"
        );
        assert_eq!(evidence.analyst_count, Some(35));

        let pt = evidence
            .price_target
            .expect("price_target must be populated");
        assert!(matches!(pt.mean, Some(m) if (m - 220.0).abs() < 0.01));
        assert!(matches!(pt.high, Some(h) if (h - 260.0).abs() < 0.01));
        assert!(matches!(pt.low, Some(l) if (l - 180.0).abs() < 0.01));
        assert_eq!(pt.analyst_count, Some(28));

        let rec = evidence
            .recommendations
            .expect("recommendations must be populated");
        assert_eq!(rec.strong_buy, Some(8));
        assert_eq!(rec.buy, Some(15));
        assert_eq!(rec.hold, Some(5));
        assert_eq!(rec.sell, Some(1));
        assert_eq!(rec.strong_sell, Some(0));
    }

    #[tokio::test]
    async fn fetch_consensus_classifies_partial_data_with_one_branch_error_as_data_with_warn() {
        // Earnings trend errors (live) but price_target + recommendations are
        // present from the shared `Info` snapshot.
        let mut mock = MockYFinanceData::new();
        mock.expect_get_earnings_trend_result().returning(|_| {
            Err(TradingError::SchemaViolation {
                message: "trend rate-limited".to_owned(),
            })
        });
        let provider = YFinanceEstimatesProvider::with_consensus_inputs(
            Arc::new(mock),
            Some(priced_target()),
            Some(populated_recommendations()),
        );

        let outcome = provider
            .fetch_consensus("AAPL", "2025-04-01")
            .await
            .expect("partial data should resolve to Ok(Data)");

        let evidence = match outcome {
            ConsensusOutcome::Data(ev) => ev,
            other => panic!("expected ConsensusOutcome::Data, got {other:?}"),
        };

        assert_eq!(evidence.symbol, "AAPL");
        assert!(evidence.eps_estimate.is_none());
        assert!(evidence.revenue_estimate_m.is_none());
        assert!(evidence.analyst_count.is_none());
        assert!(evidence.price_target.is_some());
        assert!(evidence.recommendations.is_some());
        // `tracing-test` is not in the dev-deps; warn-capture is documented as a concern.
    }

    #[tokio::test]
    async fn fetch_consensus_returns_no_coverage_when_all_endpoints_return_no_data() {
        // Trend succeeds empty; the shared-Info price target / recommendations
        // are all-empty payloads, which collapse to absent → no coverage.
        let mut mock = MockYFinanceData::new();
        mock.expect_get_earnings_trend_result()
            .returning(|_| Ok(Some(Vec::new())));
        let provider = YFinanceEstimatesProvider::with_consensus_inputs(
            Arc::new(mock),
            Some(PriceTarget {
                mean: None,
                high: None,
                low: None,
                number_of_analysts: None,
            }),
            Some(RecommendationSummary::default()),
        );

        let outcome = provider
            .fetch_consensus("AAPL", "2025-04-01")
            .await
            .expect("fetch should succeed even with all-empty branches");

        assert_eq!(outcome, ConsensusOutcome::NoCoverage);
    }

    #[tokio::test]
    async fn fetch_consensus_absent_consensus_inputs_with_empty_trend_is_no_coverage() {
        // With the shared-Info model, price target / recommendations carry no
        // error provenance — an unavailable endpoint surfaces as absent. With
        // an empty (non-error) trend and no cached consensus, the outcome is
        // NoCoverage (previously a price-target *error* yielded ProviderDegraded;
        // that distinction now lives only on the live trend branch).
        let mut mock = MockYFinanceData::new();
        mock.expect_get_earnings_trend_result()
            .returning(|_| Ok(Some(Vec::new())));
        let provider = YFinanceEstimatesProvider::with_consensus_inputs(Arc::new(mock), None, None);

        let outcome = provider
            .fetch_consensus("AAPL", "2025-04-01")
            .await
            .expect("absent consensus inputs with empty trend resolves to Ok");

        assert_eq!(outcome, ConsensusOutcome::NoCoverage);
    }

    #[tokio::test]
    async fn fetch_consensus_trend_error_with_no_cached_consensus_is_provider_degraded() {
        // The earnings-trend branch is the sole error-bearing branch now. A
        // live trend failure with no cached price target / recommendations
        // resolves to ProviderDegraded (fed to the half-life policy), not Err.
        let mut mock = MockYFinanceData::new();
        mock.expect_get_earnings_trend_result().returning(|_| {
            Err(TradingError::SchemaViolation {
                message: "trend down".to_owned(),
            })
        });
        let provider = YFinanceEstimatesProvider::with_consensus_inputs(Arc::new(mock), None, None);

        let outcome = provider
            .fetch_consensus("AAPL", "2025-04-01")
            .await
            .expect("trend failure with no cached consensus resolves to Ok(ProviderDegraded)");

        assert_eq!(outcome, ConsensusOutcome::ProviderDegraded);
    }
}
