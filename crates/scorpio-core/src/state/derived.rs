//! Derived valuation state computed deterministically before trader inference.
//!
//! This module defines the typed output of the scenario valuation derivation step
//! that runs after analyst data is collected but before the Trader LLM reasons.
//!
//! # Key types
//!
//! - [`AssetShape`]: inferred asset type used to route the valuation model.
//! - [`ScenarioValuation`]: either a typed corporate equity valuation or an explicit
//!   `NotAssessed` outcome with a reason string.
//! - [`DerivedValuation`]: container that bundles asset shape and scenario valuation
//!   for storage on [`super::TradingState`].
//!
//! # Design invariants
//!
//! - All fields that are optional have `#[serde(default)]` so old snapshots without
//!   these fields still deserialize cleanly.
//! - Missing inputs produce `NotAssessed`, never fake numeric values.
//! - ETF/fund-style runs produce `NotAssessed { reason }` rather than a broken
//!   corporate-equity path.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ─── Asset-shape seam ─────────────────────────────────────────────────────────

/// The inferred type of asset being analysed.
///
/// Used to route the valuation model: corporate-equity instruments follow the
/// DCF/multiples path; fund-style instruments go straight to `NotAssessed`.
///
/// Populated from `yfinance_rs::profile::Profile` when available, falling back
/// to data-shape detection when the profile is absent or inconclusive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AssetShape {
    /// Standard corporate equity — EPS, revenue, FCF, and balance-sheet inputs
    /// are expected.
    CorporateEquity,
    /// Fund-style instrument (ETF, mutual fund, etc.) — corporate fundamentals
    /// may be structurally absent as a domain-valid state, not a data error.
    Fund,
    /// Asset shape could not be determined from profile or data signals.
    Unknown,
    /// A blockchain's native asset (e.g. BTC, ETH, SOL). Placeholder; crypto
    /// valuation logic lands with the crypto pack implementation.
    NativeChainAsset,
    /// An ERC-20-style fungible token on an EVM-compatible chain. Placeholder.
    Erc20Token,
    /// A stablecoin — fiat- or crypto-collateralised. Placeholder.
    Stablecoin,
    /// A liquidity-provider position (AMM LP token, vault receipt). Placeholder.
    LpToken,
}

// ─── Typed metric sub-structures (JsonSchema for proposal schema exposure) ────

/// DCF intrinsic value estimate.
///
/// Computed from trailing free cash flow and a discount rate. Absent when FCF
/// or share count inputs are unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DcfValuation {
    /// Trailing free cash flow used as the DCF base, in reporting currency.
    #[schemars(
        description = "Trailing free cash flow used as the DCF base, in reporting currency"
    )]
    pub free_cash_flow: f64,

    /// Discount rate assumed for present-value calculation, e.g. `10.0` for 10 %.
    #[schemars(description = "Discount rate (%) applied in the DCF model, e.g. 10.0 for 10%")]
    pub discount_rate_pct: f64,

    /// Derived intrinsic value per share.
    #[schemars(
        description = "Derived intrinsic value per share from the DCF model, in reporting currency"
    )]
    pub intrinsic_value_per_share: f64,
}

/// EV/EBITDA relative valuation.
///
/// Absent when EBITDA or enterprise value inputs are unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvEbitdaValuation {
    /// Trailing EV/EBITDA multiple.
    #[schemars(description = "Trailing EV/EBITDA multiple derived from financial statements")]
    pub ev_ebitda_ratio: f64,

    /// Implied value per share from the EV/EBITDA multiple, if derivable.
    #[schemars(description = "Implied value per share from the EV/EBITDA multiple, if derivable")]
    #[serde(default)]
    pub implied_value_per_share: Option<f64>,
}

/// Forward P/E relative valuation.
///
/// Absent when analyst forward EPS estimates are unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardPeValuation {
    /// Forward EPS from analyst consensus estimates.
    #[schemars(description = "Forward EPS from analyst consensus estimates")]
    pub forward_eps: f64,

    /// Forward P/E ratio (current price / forward EPS).
    #[schemars(description = "Forward P/E ratio derived as current price / forward EPS")]
    pub forward_pe: f64,
}

/// PEG ratio relative valuation.
///
/// Absent when forward P/E or EPS growth inputs are unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PegValuation {
    /// PEG ratio: forward P/E divided by the expected EPS growth rate.
    #[schemars(description = "PEG ratio: forward P/E divided by the expected EPS growth rate")]
    pub peg_ratio: f64,
}

// ─── Aggregate valuation containers ──────────────────────────────────────────

/// Typed output of the corporate equity valuation step.
///
/// All metric sub-fields are `Option<T>`: a metric is `None` when its required
/// inputs are not available. Having all sub-fields `None` is valid as a partial
/// result; the caller decides whether to treat it as `NotAssessed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CorporateEquityValuation {
    /// DCF intrinsic value estimate.
    #[schemars(
        description = "DCF intrinsic value estimate; absent when FCF or share count inputs are unavailable"
    )]
    #[serde(default)]
    pub dcf: Option<DcfValuation>,

    /// EV/EBITDA relative valuation.
    #[schemars(
        description = "EV/EBITDA relative valuation; absent when EBITDA or enterprise value inputs are unavailable"
    )]
    #[serde(default)]
    pub ev_ebitda: Option<EvEbitdaValuation>,

    /// Forward P/E relative valuation.
    #[schemars(
        description = "Forward P/E relative valuation; absent when analyst forward EPS estimates are unavailable"
    )]
    #[serde(default)]
    pub forward_pe: Option<ForwardPeValuation>,

    /// PEG ratio relative valuation.
    #[schemars(
        description = "PEG ratio relative valuation; absent when forward P/E or earnings growth inputs are unavailable"
    )]
    #[serde(default)]
    pub peg: Option<PegValuation>,
}

/// Outcome of the deterministic scenario valuation step.
///
/// Either a typed corporate equity valuation or an explicit `NotAssessed`
/// outcome with a reason string. Used in both [`DerivedValuation`] (state) and
/// [`super::TradeProposal`] (LLM-visible schema).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
// `ScenarioValuation` lives on `TradingState`, which is heap-allocated and
// constructed once per analysis run. The size delta between
// `CorporateEquityValuation` (~96 B) and `EtfValuation` (~376 B) is irrelevant
// at this allocation cadence, and boxing would force every callsite (including
// LLM-visible `TradeProposal` serde) to deal with an extra indirection that
// the task spec explicitly forbids.
#[allow(clippy::large_enum_variant)]
pub enum ScenarioValuation {
    /// Deterministic corporate equity valuation computed from financial statements.
    #[schemars(
        description = "Deterministic corporate equity valuation computed from financial statements"
    )]
    CorporateEquity(CorporateEquityValuation),

    /// ETF-native valuation: premium/discount band + composition + tracking.
    /// Phase 1 always omits `options_gex` (`None`).
    #[schemars(
        description = "ETF-native valuation: premium/discount band + composition + tracking"
    )]
    Etf(EtfValuation),

    /// Valuation was not assessed; includes the reason why.
    ///
    /// Example reasons: `"fund_style_asset"`, `"insufficient_corporate_fundamentals"`.
    #[schemars(
        description = "Valuation was not assessed; reason explains why (e.g. fund-style asset or insufficient corporate inputs)"
    )]
    NotAssessed {
        /// Human-readable reason for the absence of valuation.
        reason: String,
    },
}

// ─── Full derived valuation state (stored on TradingState) ───────────────────

/// Full derived valuation state persisted on [`super::TradingState`].
///
/// Carries both the inferred asset shape (used for routing decisions) and the
/// valuation outcome. Not part of the LLM's structured output schema; only
/// [`ScenarioValuation`] flows into [`super::TradeProposal`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedValuation {
    /// The inferred asset type used to route valuation logic.
    pub asset_shape: AssetShape,

    /// The scenario valuation outcome.
    pub scenario: ScenarioValuation,
}

// ─── ETF valuation types (Phase 1) ────────────────────────────────────────────

/// Premium-band classification anchored to category norms.
///
/// Populated by [`EtfValuation`]. `Unknown` means the band could not be
/// computed (NAV missing, category unknown, or both).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PremiumBand {
    Normal,
    Elevated,
    Extreme,
    Unknown,
}

/// Single holding row inside [`EtfComposition`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HoldingWeight {
    pub cusip: Option<String>,
    pub ticker: Option<String>,
    pub name: String,
    pub weight_pct: f64,
    #[serde(default)]
    pub value_usd: Option<f64>,
}

/// Single sector row inside [`EtfComposition`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SectorWeight {
    pub sector: String,
    pub weight_pct: f64,
}

/// Quote + premium snapshot. `premium_pct` is `None` when NAV is unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PremiumSnapshot {
    pub nav: Option<f64>,
    pub market_price: f64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub premium_pct: Option<f64>,
    pub category_band: PremiumBand,
    pub bid_ask_spread_pct: Option<f64>,
    pub as_of: chrono::DateTime<chrono::Utc>,
}

/// Provider that supplied ETF composition/profile rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EtfCompositionSource {
    #[default]
    SecNport,
    AlphaVantageEtfProfile,
}

/// Official textual benchmark-name source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkSource {
    SecRiskReturn,
    SecNport,
}

/// Status for ETF tracking-error computation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackingStatus {
    #[default]
    NotResolved,
    BenchmarkNameOnly,
    Computed,
}

/// Composition + cost snapshot derived from N-PORT-P + fund metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EtfComposition {
    #[serde(default)]
    pub source: EtfCompositionSource,
    pub top_holdings: Vec<HoldingWeight>,
    pub top10_concentration_pct: f64,
    pub sector_weights: Vec<SectorWeight>,
    #[serde(default)]
    pub expense_ratio_pct: Option<f64>,
    #[serde(default)]
    pub aum_usd: Option<f64>,
    #[serde(default)]
    pub fund_family: Option<String>,
    #[serde(default)]
    pub distribution_yield_ttm_pct: Option<f64>,
    pub holdings_filing_date: chrono::NaiveDate,
    #[serde(default)]
    pub holdings_report_date: Option<chrono::NaiveDate>,
    pub holdings_age_days: u32,
    #[serde(default)]
    pub portfolio_turnover_pct: Option<f64>,
    #[serde(default)]
    pub inception_date: Option<chrono::NaiveDate>,
}

/// Tracking error vs a source-provided benchmark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackingError {
    pub benchmark_symbol: String,
    pub te_pct_90d: f64,
    pub te_pct_1y: f64,
    pub sample_days: u32,
}

/// Dealer-positioning summary populated by `compute_gex_summary` from a live
/// `OptionsSnapshot`. Phase 1 always emitted `options_gex: None`; Phase 2
/// Stage 1/2 populates the legacy fields plus `strikes`. Stage 3 additionally
/// adds broad GEX and secondary VEX/CEX summaries. The added `strikes` field
/// carries `#[serde(default)]` so legacy snapshots remain readable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GexSummary {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub call_put_oi_ratio: f64,
    pub max_pain_strike: f64,
    pub near_term_expiration: chrono::NaiveDate,

    /// Top-N strikes by `|net_gex_usd_per_1pct_move|` — gamma walls.
    /// Populated by Stage 1/2.
    #[serde(default)]
    pub strikes: Vec<StrikeGex>,

    /// Broad dealer-positioning aggregate across NTM slices for all listed
    /// expirations. Populated by Stage 3.
    #[serde(default)]
    pub broad: Option<BroadGex>,

    /// Secondary sensitivity: dealer exposure to absolute IV moves.
    /// Populated by Stage 3.
    #[serde(default)]
    pub vex_summary: Option<VexSummary>,

    /// Secondary sensitivity: dealer exposure to one day of time decay.
    /// Populated by Stage 3.
    #[serde(default)]
    pub cex_summary: Option<CexSummary>,
}

/// Broad (all-expirations) GEX aggregate. Single-rate approximation — the
/// renderer/prompt always labels this as such.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BroadGex {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub expirations_used: u32,
    #[serde(default)]
    pub expirations_total_considered: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VexSummary {
    /// Per 1.0 vol-point change.
    pub net_vex_usd_per_volpt: f64,
    pub gross_vex_usd_per_volpt: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CexSummary {
    /// Per 1 calendar day of time decay.
    pub net_cex_usd_per_day: f64,
    pub gross_cex_usd_per_day: f64,
}

/// Single gamma-wall row inside `GexSummary.strikes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StrikeGex {
    pub strike: f64,
    pub net_gex_usd_per_1pct_move: f64,
}

/// Filing-age qualification for N-PORT-backed holdings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HoldingsAgeBand {
    Fresh,
    Aging,
    Stale,
    #[default]
    Unknown,
}

/// Per-signal availability flags plus holdings age-band qualification.
/// Availability flags default to `false`; `holdings_age_band` defaults to
/// `Unknown` until holdings land.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EtfDataAvailability {
    #[serde(default)]
    pub nav_available: bool,
    #[serde(default)]
    pub bid_ask_available: bool,
    #[serde(default)]
    pub holdings_present: bool,
    #[serde(default)]
    pub holdings_age_band: HoldingsAgeBand,
    #[serde(default)]
    pub benchmark_resolved: bool,
    #[serde(default)]
    pub options_chain_present: bool,
    #[serde(default)]
    pub expense_ratio_available: bool,
}

/// Aggregate ETF valuation output. Carried by [`ScenarioValuation::Etf`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EtfValuation {
    pub premium: PremiumSnapshot,
    #[serde(default)]
    pub composition: Option<EtfComposition>,
    #[serde(default)]
    pub tracking: Option<TrackingError>,
    #[serde(default)]
    pub tracking_status: TrackingStatus,
    #[serde(default)]
    pub official_benchmark_name: Option<String>,
    #[serde(default)]
    pub official_benchmark_source: Option<BenchmarkSource>,
    #[serde(default)]
    pub official_benchmark_metadata_age_days: Option<u32>,
    /// Phase 2 — always `None` in Phase 1.
    #[serde(default)]
    pub options_gex: Option<GexSummary>,
    #[serde(default)]
    pub category: Option<String>,
    /// `1.0` for plain ETFs; `2.0`, `3.0`, `-1.0`, `-2.0`, `-3.0` for
    /// leveraged/inverse products. `None` when not declared by the source.
    #[serde(default)]
    pub leverage_factor: Option<f64>,
    #[serde(default)]
    pub flags: EtfDataAvailability,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ScenarioValuation serde round-trips ──────────────────────────────

    #[test]
    fn scenario_valuation_corporate_equity_roundtrips_json() {
        let val = ScenarioValuation::CorporateEquity(CorporateEquityValuation {
            dcf: Some(DcfValuation {
                free_cash_flow: 1_200_000_000.0,
                discount_rate_pct: 10.0,
                intrinsic_value_per_share: 185.42,
            }),
            ev_ebitda: Some(EvEbitdaValuation {
                ev_ebitda_ratio: 22.5,
                implied_value_per_share: Some(192.0),
            }),
            forward_pe: Some(ForwardPeValuation {
                forward_eps: 7.25,
                forward_pe: 26.2,
            }),
            peg: Some(PegValuation { peg_ratio: 1.8 }),
        });

        let json = serde_json::to_string(&val).expect("serialize");
        let back: ScenarioValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn scenario_valuation_not_assessed_roundtrips_json() {
        let val = ScenarioValuation::NotAssessed {
            reason: "fund_style_asset".to_owned(),
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: ScenarioValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn scenario_valuation_not_assessed_serializes_with_snake_case_key() {
        let val = ScenarioValuation::NotAssessed {
            reason: "insufficient_corporate_fundamentals".to_owned(),
        };
        let json = serde_json::to_string(&val).expect("serialize");
        // The outer key must be snake_case per #[serde(rename_all = "snake_case")].
        assert!(
            json.contains("not_assessed"),
            "expected 'not_assessed' in JSON, got: {json}"
        );
    }

    #[test]
    fn scenario_valuation_corporate_equity_serializes_with_snake_case_key() {
        let val = ScenarioValuation::CorporateEquity(CorporateEquityValuation {
            dcf: None,
            ev_ebitda: None,
            forward_pe: None,
            peg: None,
        });
        let json = serde_json::to_string(&val).expect("serialize");
        assert!(
            json.contains("corporate_equity"),
            "expected 'corporate_equity' in JSON, got: {json}"
        );
    }

    // ── CorporateEquityValuation partial-data serde ───────────────────────

    #[test]
    fn corporate_equity_valuation_with_all_none_metrics_roundtrips() {
        let val = CorporateEquityValuation {
            dcf: None,
            ev_ebitda: None,
            forward_pe: None,
            peg: None,
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: CorporateEquityValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn corporate_equity_valuation_missing_optional_fields_deserialize_as_none() {
        // Old snapshot or minimal JSON with no metric sub-fields.
        let json = r#"{}"#;
        let val: CorporateEquityValuation = serde_json::from_str(json).expect("deserialize");
        assert!(val.dcf.is_none());
        assert!(val.ev_ebitda.is_none());
        assert!(val.forward_pe.is_none());
        assert!(val.peg.is_none());
    }

    // ── DerivedValuation serde round-trips ───────────────────────────────

    #[test]
    fn derived_valuation_with_fund_shape_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::NotAssessed {
                reason: "fund_style_asset".to_owned(),
            },
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn derived_valuation_with_corporate_equity_shape_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::CorporateEquity,
            scenario: ScenarioValuation::CorporateEquity(CorporateEquityValuation {
                dcf: Some(DcfValuation {
                    free_cash_flow: 500_000_000.0,
                    discount_rate_pct: 9.5,
                    intrinsic_value_per_share: 142.0,
                }),
                ev_ebitda: None,
                forward_pe: Some(ForwardPeValuation {
                    forward_eps: 5.80,
                    forward_pe: 24.5,
                }),
                peg: None,
            }),
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn derived_valuation_with_unknown_shape_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::Unknown,
            scenario: ScenarioValuation::NotAssessed {
                reason: "unsupported_asset_shape".to_owned(),
            },
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    // ── EvEbitdaValuation optional field backward compat ─────────────────

    #[test]
    fn ev_ebitda_valuation_missing_implied_value_deserializes_as_none() {
        let json = r#"{"ev_ebitda_ratio": 18.5}"#;
        let val: EvEbitdaValuation = serde_json::from_str(json).expect("deserialize");
        assert!((val.ev_ebitda_ratio - 18.5).abs() < f64::EPSILON);
        assert!(val.implied_value_per_share.is_none());
    }

    // ── Proposal validation: inconsistent valuation ───────────────────────

    // ── New AssetShape variants round-trip cleanly ───────────────────────

    #[test]
    fn derived_valuation_native_chain_asset_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::NativeChainAsset,
            scenario: ScenarioValuation::NotAssessed {
                reason: "unsupported_asset_shape".to_owned(),
            },
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn derived_valuation_erc20_token_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::Erc20Token,
            scenario: ScenarioValuation::NotAssessed {
                reason: "unsupported_asset_shape".to_owned(),
            },
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn derived_valuation_stablecoin_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::Stablecoin,
            scenario: ScenarioValuation::NotAssessed {
                reason: "unsupported_asset_shape".to_owned(),
            },
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn derived_valuation_lp_token_roundtrips_json() {
        let val = DerivedValuation {
            asset_shape: AssetShape::LpToken,
            scenario: ScenarioValuation::NotAssessed {
                reason: "unsupported_asset_shape".to_owned(),
            },
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: DerivedValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn asset_shape_crypto_variants_preserve_pascalcase_in_json() {
        // Guard against an accidental `rename_all` drift — old snapshots store
        // variant names in PascalCase, so any serde attribute change would
        // silently break snapshot compat. This test fails loudly if someone
        // adds `rename_all` to AssetShape.
        let val = AssetShape::NativeChainAsset;
        let json = serde_json::to_string(&val).expect("serialize");
        assert_eq!(json, "\"NativeChainAsset\"");
    }

    #[test]
    fn scenario_valuation_not_assessed_with_empty_reason_is_representable() {
        // The schema itself does not forbid empty reasons; validation is the
        // caller's responsibility. Here we verify only that serde round-trips.
        let val = ScenarioValuation::NotAssessed {
            reason: String::new(),
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: ScenarioValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    // ── ScenarioValuation::Etf variant (Phase 1) ─────────────────────────

    #[test]
    fn scenario_valuation_etf_variant_roundtrips_json() {
        let val = ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(621.18),
                market_price: 621.40,
                bid: Some(621.39),
                ask: Some(621.41),
                premium_pct: Some(0.04),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            tracking_status: TrackingStatus::NotResolved,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        });
        let json = serde_json::to_string(&val).expect("serialize");
        let back: ScenarioValuation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn scenario_valuation_etf_serializes_with_snake_case_key() {
        let val = ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: None,
                market_price: 100.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            tracking_status: TrackingStatus::NotResolved,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        });
        let json = serde_json::to_string(&val).expect("serialize");
        assert!(json.contains("\"etf\""), "expected 'etf' tag, got: {json}");
    }

    #[test]
    fn legacy_not_assessed_snapshot_still_deserializes_after_etf_variant_added() {
        let json = r#"{"not_assessed":{"reason":"fund_style_asset"}}"#;
        let back: ScenarioValuation = serde_json::from_str(json).expect("deserialize");
        assert!(matches!(back, ScenarioValuation::NotAssessed { .. }));
    }

    #[test]
    fn etf_data_availability_defaults_to_unavailable_and_unknown_age_band() {
        let flags = EtfDataAvailability::default();
        assert!(!flags.nav_available);
        assert!(!flags.bid_ask_available);
        assert!(!flags.holdings_present);
        assert_eq!(flags.holdings_age_band, HoldingsAgeBand::Unknown);
        assert!(!flags.benchmark_resolved);
        assert!(!flags.options_chain_present);
        assert!(!flags.expense_ratio_available);
    }

    #[test]
    fn legacy_etf_data_availability_with_benchmark_resolved_roundtrips() {
        let json = r#"{
            "nav_available": true,
            "bid_ask_available": true,
            "holdings_present": true,
            "holdings_age_band": "fresh",
            "benchmark_resolved": true,
            "options_chain_present": false,
            "expense_ratio_available": true
        }"#;

        let flags: EtfDataAvailability = serde_json::from_str(json).expect("legacy flags");
        assert!(flags.benchmark_resolved);

        let serialized = serde_json::to_string(&flags).expect("serialize flags");
        assert!(serialized.contains("\"benchmark_resolved\":true"));
    }

    #[test]
    fn gex_summary_with_strikes_field_roundtrips_json() {
        let val = GexSummary {
            net_gex_usd_per_1pct_move: 1_000_000.0,
            gross_gex_usd_per_1pct_move: 2_000_000.0,
            call_put_oi_ratio: 1.3,
            max_pain_strike: 100.0,
            near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            strikes: vec![
                StrikeGex {
                    strike: 100.0,
                    net_gex_usd_per_1pct_move: 500_000.0,
                },
                StrikeGex {
                    strike: 105.0,
                    net_gex_usd_per_1pct_move: -250_000.0,
                },
            ],
            broad: None,
            vex_summary: None,
            cex_summary: None,
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: GexSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn legacy_phase1_gex_summary_without_strikes_still_deserializes() {
        let json = r#"{
            "net_gex_usd_per_1pct_move": 0.0,
            "gross_gex_usd_per_1pct_move": 0.0,
            "call_put_oi_ratio": 0.0,
            "max_pain_strike": 0.0,
            "near_term_expiration": "2026-06-26"
        }"#;
        let back: GexSummary = serde_json::from_str(json).expect("deserialize");
        assert!(back.strikes.is_empty());
    }

    #[test]
    fn broad_gex_with_partial_expiration_coverage_roundtrips() {
        let val = BroadGex {
            net_gex_usd_per_1pct_move: 5_000_000.0,
            gross_gex_usd_per_1pct_move: 9_000_000.0,
            expirations_used: 3,
            expirations_total_considered: 5,
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: BroadGex = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn legacy_broad_gex_without_total_considered_defaults_to_zero() {
        let json = r#"{
            "net_gex_usd_per_1pct_move": 0.0,
            "gross_gex_usd_per_1pct_move": 0.0,
            "expirations_used": 0
        }"#;
        let back: BroadGex = serde_json::from_str(json).expect("deserialize");
        assert_eq!(back.expirations_total_considered, 0);
    }
}
