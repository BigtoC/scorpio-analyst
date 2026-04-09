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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetShape {
    /// Standard corporate equity — EPS, revenue, FCF, and balance-sheet inputs
    /// are expected.
    CorporateEquity,
    /// Fund-style instrument (ETF, mutual fund, etc.) — corporate fundamentals
    /// may be structurally absent as a domain-valid state, not a data error.
    Fund,
    /// Asset shape could not be determined from profile or data signals.
    Unknown,
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
pub enum ScenarioValuation {
    /// Deterministic corporate equity valuation computed from financial statements.
    #[schemars(
        description = "Deterministic corporate equity valuation computed from financial statements"
    )]
    CorporateEquity(CorporateEquityValuation),

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
}
