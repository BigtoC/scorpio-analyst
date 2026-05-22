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

        DerivedValuation {
            asset_shape: shape.clone(),
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: snapshot,
                composition,
                tracking,
                options_gex: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

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
