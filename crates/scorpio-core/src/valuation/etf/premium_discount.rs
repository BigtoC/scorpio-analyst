//! Premium/discount valuator entry point.

use crate::data::sec_edgar_nport::NPortHoldings;
use crate::data::yfinance::etf::{EtfQuote, FundInfo};
use crate::state::{
    AssetShape, DerivedValuation, EtfComposition, EtfDataAvailability, EtfValuation, HoldingWeight,
    HoldingsAgeBand, PremiumSnapshot, ScenarioValuation, SectorWeight,
};
use crate::valuation::{ValuationInputs, ValuationReport, Valuator, ValuatorId};

use super::category_norms::{band_for_category, classify_band};
use super::tracking_error::compute_tracking_error;

/// ETF-native valuator — composes premium/discount band, composition snapshot,
/// and tracking error against a stated benchmark.
pub struct EtfPremiumDiscountValuator;

impl Valuator for EtfPremiumDiscountValuator {
    fn id(&self) -> ValuatorId {
        ValuatorId::EtfPremiumDiscount
    }

    fn assess(&self, inputs: ValuationInputs<'_>, shape: &AssetShape) -> ValuationReport {
        if !matches!(shape, AssetShape::Fund) {
            return DerivedValuation {
                asset_shape: shape.clone(),
                scenario: ScenarioValuation::NotAssessed {
                    reason: "etf_valuator_wrong_shape".to_owned(),
                },
            };
        }

        let mut flags = EtfDataAvailability::default();

        let Some(snapshot) =
            build_premium_snapshot(inputs.etf_quote, inputs.etf_fund_info, &mut flags)
        else {
            return DerivedValuation {
                asset_shape: shape.clone(),
                scenario: ScenarioValuation::NotAssessed {
                    reason: "etf_quote_unavailable".to_owned(),
                },
            };
        };

        let composition = inputs
            .etf_holdings
            .and_then(|h| build_composition(h, inputs.etf_fund_info, &mut flags));

        let tracking = match (
            inputs.etf_fund_info,
            inputs.etf_ohlcv,
            inputs.etf_benchmark_ohlcv,
        ) {
            (Some(info), Some(etf_ohlcv), Some(bench)) if info.stated_benchmark.is_some() => {
                let symbol = info.stated_benchmark.as_deref().unwrap_or("^GSPC");
                let te = compute_tracking_error(etf_ohlcv, bench, symbol);
                if te.is_some() {
                    flags.benchmark_resolved = true;
                }
                te
            }
            _ => None,
        };

        let category = inputs.etf_fund_info.and_then(|f| f.category.clone());
        let leverage_factor = inputs.etf_fund_info.and_then(|f| f.leverage_factor);

        // Phase 2 dealer-positioning will compute once a live risk-free rate
        // is sourced (Stage 2 / Task 20). No hardcoded rate fallback is allowed,
        // so Stage 1 keeps the derived overlay absent.
        let r: Option<f64> = None;
        let q = composition
            .as_ref()
            .and_then(|c| c.distribution_yield_ttm_pct)
            .filter(|y| *y > 0.0)
            .map(|y_pct| y_pct / 100.0)
            .unwrap_or(0.0);
        flags.options_chain_present = inputs.etf_options.is_some();
        let options_gex = match (inputs.etf_options, r) {
            (Some(snap), Some(rate)) => compute_gex_summary(snap, rate, q, inputs.as_of),
            (Some(_), None) => {
                tracing::warn!(
                    target: "scorpio_core::valuation::etf::gex",
                    "ETF dealer-positioning skipped — risk-free rate unavailable"
                );
                None
            }
            (None, _) => None,
        };

        DerivedValuation {
            asset_shape: shape.clone(),
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: snapshot,
                composition,
                tracking,
                options_gex,
                category,
                leverage_factor,
                flags,
            }),
        }
    }
}

fn build_premium_snapshot(
    quote: Option<&EtfQuote>,
    fund_info: Option<&FundInfo>,
    flags: &mut EtfDataAvailability,
) -> Option<PremiumSnapshot> {
    let quote = quote?;
    let market_price = quote.regular_market_price;
    if market_price <= 0.0 {
        return None;
    }
    flags.nav_available = quote.nav.is_some();
    flags.bid_ask_available = quote.bid.is_some() && quote.ask.is_some();
    flags.expense_ratio_available = fund_info.and_then(|f| f.expense_ratio).is_some();
    let premium_pct = quote
        .nav
        .filter(|&nav| nav > 0.0)
        .map(|nav| (market_price - nav) / nav * 100.0);
    let spread = match (quote.bid, quote.ask) {
        (Some(b), Some(a)) if a > 0.0 => Some((a - b) / a * 100.0),
        _ => None,
    };
    let band_cfg = band_for_category(fund_info.and_then(|f| f.category.as_deref()));
    let band = classify_band(premium_pct, band_cfg);
    Some(PremiumSnapshot {
        nav: quote.nav,
        market_price,
        bid: quote.bid,
        ask: quote.ask,
        premium_pct,
        category_band: band,
        bid_ask_spread_pct: spread,
        as_of: quote.as_of,
    })
}

fn build_composition(
    nport: &NPortHoldings,
    fund_info: Option<&FundInfo>,
    flags: &mut EtfDataAvailability,
) -> Option<EtfComposition> {
    flags.holdings_present = !nport.holdings.is_empty();
    let today = chrono::Utc::now().date_naive();
    let age_days = (today - nport.filing_date).num_days().max(0) as u32;
    flags.holdings_age_band = match age_days {
        0..=45 => HoldingsAgeBand::Fresh,
        46..=90 => HoldingsAgeBand::Aging,
        _ => HoldingsAgeBand::Stale,
    };
    if age_days > 180 {
        return None;
    }
    let mut sorted: Vec<&_> = nport.holdings.iter().collect();
    sorted.sort_by(|a, b| {
        b.weight_pct
            .partial_cmp(&a.weight_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top10: Vec<HoldingWeight> = sorted
        .iter()
        .take(10)
        .map(|row| HoldingWeight {
            cusip: row.cusip.clone(),
            ticker: row.ticker.clone(),
            name: row.name.clone(),
            weight_pct: row.weight_pct,
            value_usd: row.value_usd,
        })
        .collect();
    let top10_concentration_pct = top10.iter().map(|h| h.weight_pct).sum();
    let sector_weights: Vec<SectorWeight> = nport
        .sector_breakdown
        .iter()
        .map(|s| SectorWeight {
            sector: s.sector.clone(),
            weight_pct: s.weight_pct,
        })
        .collect();
    Some(EtfComposition {
        top_holdings: top10,
        top10_concentration_pct,
        sector_weights,
        expense_ratio_pct: fund_info.and_then(|f| f.expense_ratio),
        aum_usd: fund_info.and_then(|f| f.total_assets),
        fund_family: fund_info.and_then(|f| f.fund_family.clone()),
        distribution_yield_ttm_pct: None, // filled by AnalystSyncTask (Task 13)
        holdings_filing_date: nport.filing_date,
        holdings_age_days: age_days,
    })
}

use crate::data::traits::options::OptionsSnapshot;
use crate::indicators::gex::{self, AggregateInputs};
use crate::state::{GexSummary, StrikeGex};

const MAX_GAMMA_WALLS: usize = 3;

/// Map a live options snapshot into the persistent `GexSummary` shape.
/// Returns `None` when the front-month near-term aggregate is unusable.
pub fn compute_gex_summary(
    snapshot: &OptionsSnapshot,
    r: f64,
    q: f64,
    as_of: chrono::NaiveDate,
) -> Option<GexSummary> {
    let agg = gex::aggregate(AggregateInputs {
        spot: snapshot.spot_price,
        r,
        q,
        as_of,
        near_term_expiration: &snapshot.near_term_expiration,
        near_term_strikes: &snapshot.near_term_strikes,
        atm_iv_fallback: snapshot.atm_iv,
    });

    let near = agg.near_term?;

    if agg.iv_fallback_count > agg.strikes_used.saturating_div(2) {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            iv_fallback_count = agg.iv_fallback_count,
            strikes_used = agg.strikes_used,
            "GEX computed with majority ATM-IV fallbacks — gamma skew may be understated"
        );
    }

    let mut walls: Vec<StrikeGex> = near
        .per_strike
        .iter()
        .map(|p| StrikeGex {
            strike: p.strike,
            net_gex_usd_per_1pct_move: p.net_gex_usd_per_1pct_move,
        })
        .collect();
    walls.sort_by(|a, b| {
        b.net_gex_usd_per_1pct_move
            .abs()
            .partial_cmp(&a.net_gex_usd_per_1pct_move.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    walls.truncate(MAX_GAMMA_WALLS);

    let call_put_oi_ratio = if snapshot.put_call_oi_ratio > 0.0 {
        1.0 / snapshot.put_call_oi_ratio
    } else {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            "put_call_oi_ratio is zero — call_put_oi_ratio set to 0.0"
        );
        0.0
    };

    Some(GexSummary {
        net_gex_usd_per_1pct_move: near.net_gex_usd_per_1pct_move,
        gross_gex_usd_per_1pct_move: near.gross_gex_usd_per_1pct_move,
        call_put_oi_ratio,
        max_pain_strike: snapshot.max_pain_strike,
        near_term_expiration: near.expiration,
        strikes: walls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::traits::options::{IvTermPoint, NearTermStrike, OptionsSnapshot};
    use chrono::Utc;

    fn sample_options_snapshot() -> OptionsSnapshot {
        OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 0.8, // call-heavy → call_put_oi_ratio = 1.25
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes: vec![
                NearTermStrike {
                    strike: 95.0,
                    call_iv: Some(0.22),
                    put_iv: Some(0.24),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(1_500),
                    put_oi: Some(500),
                },
                NearTermStrike {
                    strike: 100.0,
                    call_iv: Some(0.20),
                    put_iv: Some(0.20),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(3_000),
                    put_oi: Some(2_500),
                },
                NearTermStrike {
                    strike: 105.0,
                    call_iv: Some(0.21),
                    put_iv: Some(0.23),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(800),
                    put_oi: Some(2_000),
                },
                NearTermStrike {
                    strike: 110.0,
                    call_iv: Some(0.25),
                    put_iv: Some(0.27),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(200),
                    put_oi: Some(1_200),
                },
            ],
        }
    }

    #[test]
    fn compute_gex_summary_returns_none_when_expiration_is_unparseable() {
        let mut snap = sample_options_snapshot();
        snap.near_term_expiration = "not-a-date".to_owned();
        let result = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn compute_gex_summary_emits_top_3_strikes_sorted_by_abs_net_gex() {
        let snap = sample_options_snapshot();
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary present");
        assert_eq!(summary.strikes.len(), 3, "must truncate to top-3");
        let w = &summary.strikes;
        assert!(
            w[0].net_gex_usd_per_1pct_move.abs() >= w[1].net_gex_usd_per_1pct_move.abs(),
            "strikes[0] must dominate strikes[1]: {w:?}"
        );
        assert!(
            w[1].net_gex_usd_per_1pct_move.abs() >= w[2].net_gex_usd_per_1pct_move.abs(),
            "strikes[1] must dominate strikes[2]: {w:?}"
        );
    }

    #[test]
    fn compute_gex_summary_inverts_put_call_oi_ratio_correctly() {
        let snap = sample_options_snapshot();
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary present");
        // 1 / 0.8 = 1.25
        assert!(
            (summary.call_put_oi_ratio - 1.25).abs() < 1e-9,
            "expected 1.25, got {}",
            summary.call_put_oi_ratio
        );
    }

    #[test]
    fn compute_gex_summary_returns_zero_call_put_when_put_oi_ratio_is_zero() {
        let mut snap = sample_options_snapshot();
        snap.put_call_oi_ratio = 0.0;
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary present");
        assert_eq!(summary.call_put_oi_ratio, 0.0);
    }

    fn quote_with(market_price: f64, nav: Option<f64>) -> EtfQuote {
        EtfQuote {
            symbol: "SPY".into(),
            regular_market_price: market_price,
            previous_close: None,
            nav,
            bid: Some(market_price - 0.01),
            ask: Some(market_price + 0.01),
            market_cap: None,
            day_volume: None,
            currency: Some("USD".into()),
            as_of: Utc::now(),
        }
    }

    fn fund_info_with(category: Option<&str>, leverage: Option<f64>) -> FundInfo {
        FundInfo {
            symbol: "SPY".into(),
            category: category.map(str::to_owned),
            fund_family: None,
            expense_ratio: Some(0.09),
            total_assets: None,
            leverage_factor: leverage,
            fund_kind: Some("etf".into()),
            stated_benchmark: Some("^GSPC".into()),
        }
    }

    fn empty_inputs<'a>() -> ValuationInputs<'a> {
        ValuationInputs {
            profile: None,
            cashflow: None,
            balance: None,
            income: None,
            shares: None,
            earnings_trend: None,
            current_price: None,
            etf_quote: None,
            etf_fund_info: None,
            etf_holdings: None,
            etf_ohlcv: None,
            etf_benchmark_ohlcv: None,
            etf_options: None,
            as_of: chrono::Utc::now().date_naive(),
        }
    }

    #[test]
    fn assess_returns_not_assessed_when_quote_absent() {
        let info = fund_info_with(Some("Large Blend"), Some(1.0));
        let mut inputs = empty_inputs();
        inputs.etf_fund_info = Some(&info);
        let result = EtfPremiumDiscountValuator.assess(inputs, &AssetShape::Fund);
        assert!(matches!(
            result.scenario,
            ScenarioValuation::NotAssessed { ref reason } if reason == "etf_quote_unavailable"
        ));
    }

    #[test]
    fn assess_emits_unknown_band_when_nav_missing() {
        let q = quote_with(621.40, None);
        let i = fund_info_with(Some("Large Blend"), Some(1.0));
        let mut inputs = empty_inputs();
        inputs.etf_quote = Some(&q);
        inputs.etf_fund_info = Some(&i);
        let result = EtfPremiumDiscountValuator.assess(inputs, &AssetShape::Fund);
        let etf = match result.scenario {
            ScenarioValuation::Etf(e) => e,
            other => panic!("expected Etf variant, got {other:?}"),
        };
        assert!(!etf.flags.nav_available);
        assert!(etf.premium.premium_pct.is_none());
        assert_eq!(
            etf.premium.category_band,
            crate::state::PremiumBand::Unknown
        );
    }

    #[test]
    fn assess_classifies_normal_band_at_005_premium() {
        let q = quote_with(621.40, Some(621.18));
        let i = fund_info_with(Some("Large Blend"), Some(1.0));
        let mut inputs = empty_inputs();
        inputs.etf_quote = Some(&q);
        inputs.etf_fund_info = Some(&i);
        let result = EtfPremiumDiscountValuator.assess(inputs, &AssetShape::Fund);
        let etf = match result.scenario {
            ScenarioValuation::Etf(e) => e,
            other => panic!("{:?}", other),
        };
        // 0.04% < 0.05% Large-Blend elevated threshold → Normal.
        assert_eq!(etf.premium.category_band, crate::state::PremiumBand::Normal);
        assert!(etf.flags.nav_available);
        assert!(etf.flags.bid_ask_available);
    }

    #[test]
    fn assess_leverage_factor_passes_through() {
        let q = quote_with(50.0, Some(50.0));
        let i = fund_info_with(Some("Trading--Leveraged Equity"), Some(3.0));
        let mut inputs = empty_inputs();
        inputs.etf_quote = Some(&q);
        inputs.etf_fund_info = Some(&i);
        let result = EtfPremiumDiscountValuator.assess(inputs, &AssetShape::Fund);
        match result.scenario {
            ScenarioValuation::Etf(e) => assert_eq!(e.leverage_factor, Some(3.0)),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn assess_rejects_wrong_shape_with_specific_reason() {
        let q = quote_with(100.0, Some(100.0));
        let mut inputs = empty_inputs();
        inputs.etf_quote = Some(&q);
        let result = EtfPremiumDiscountValuator.assess(inputs, &AssetShape::CorporateEquity);
        assert!(matches!(
            result.scenario,
            ScenarioValuation::NotAssessed { ref reason } if reason == "etf_valuator_wrong_shape"
        ));
    }
}
