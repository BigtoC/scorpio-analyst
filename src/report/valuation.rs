//! Scenario Valuation section for the final terminal report.

use std::fmt::Write;

use crate::state::{CorporateEquityValuation, DerivedValuation, ScenarioValuation, TradingState};

/// Render the `Scenario Valuation` section into `out`.
///
/// Branches:
/// - `state.derived_valuation = None` → emits an explicit "not computed" line.
/// - `ScenarioValuation::NotAssessed { reason }` → emits "not assessed" with the reason.
/// - `ScenarioValuation::CorporateEquity(_)` → renders only the metrics that are `Some`.
///
/// Never panics — all `Option` accesses use pattern matching.
pub(crate) fn write_scenario_valuation(out: &mut String, state: &TradingState) {
    super::final_report::section_header(out, "Scenario Valuation");

    match state.derived_valuation.as_ref() {
        None => {
            let _ = writeln!(out, "Not computed for this run.");
        }
        Some(dv) => {
            write_valuation_body(out, dv);
        }
    }
}

fn write_valuation_body(out: &mut String, dv: &DerivedValuation) {
    let _ = writeln!(out, "Asset shape: {:?}", dv.asset_shape);

    match &dv.scenario {
        ScenarioValuation::NotAssessed { reason } => {
            let _ = writeln!(out, "Valuation: not assessed for this asset shape.");
            let _ = writeln!(out, "Reason: {reason}");
        }
        ScenarioValuation::CorporateEquity(equity) => {
            let _ = writeln!(out, "Valuation model: Corporate Equity");
            write_equity_metrics(out, equity);
        }
    }
}

fn write_equity_metrics(out: &mut String, equity: &CorporateEquityValuation) {
    let mut any_metric = false;

    if let Some(dcf) = &equity.dcf {
        let _ = writeln!(
            out,
            "  DCF intrinsic value: {:.2} (FCF: {:.0}, discount rate: {:.1}%)",
            dcf.intrinsic_value_per_share, dcf.free_cash_flow, dcf.discount_rate_pct
        );
        any_metric = true;
    }

    if let Some(ev) = &equity.ev_ebitda {
        let implied = ev
            .implied_value_per_share
            .map(|v| format!(" (implied: {v:.2})"))
            .unwrap_or_default();
        let _ = writeln!(out, "  EV/EBITDA: {:.1}{implied}", ev.ev_ebitda_ratio);
        any_metric = true;
    }

    if let Some(fpe) = &equity.forward_pe {
        let _ = writeln!(
            out,
            "  Forward P/E: {:.1} (forward EPS: {:.2})",
            fpe.forward_pe, fpe.forward_eps
        );
        any_metric = true;
    }

    if let Some(peg) = &equity.peg {
        let _ = writeln!(out, "  PEG ratio: {:.2}", peg.peg_ratio);
        any_metric = true;
    }

    if !any_metric {
        let _ = writeln!(
            out,
            "  No valuation metrics computed (insufficient inputs)."
        );
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        AssetShape, CorporateEquityValuation, DcfValuation, DerivedValuation, EvEbitdaValuation,
        ForwardPeValuation, PegValuation, ScenarioValuation, TradingState,
    };

    fn state_with_valuation(dv: DerivedValuation) -> TradingState {
        let mut state = TradingState::new("AAPL", "2026-04-03");
        state.derived_valuation = Some(dv);
        state
    }

    fn full_corporate_valuation() -> DerivedValuation {
        DerivedValuation {
            asset_shape: AssetShape::CorporateEquity,
            scenario: ScenarioValuation::CorporateEquity(CorporateEquityValuation {
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
            }),
        }
    }

    // ── None case ────────────────────────────────────────────────────────

    #[test]
    fn write_scenario_valuation_shows_heading_when_none() {
        let state = TradingState::new("AAPL", "2026-04-03");
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(
            out.contains("Scenario Valuation"),
            "section heading must always appear"
        );
    }

    #[test]
    fn write_scenario_valuation_shows_not_computed_when_none() {
        let state = TradingState::new("AAPL", "2026-04-03");
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(
            out.contains("Not computed"),
            "must render 'Not computed' when derived_valuation is None"
        );
    }

    // ── NotAssessed case ─────────────────────────────────────────────────

    #[test]
    fn write_scenario_valuation_shows_not_assessed_with_reason_for_fund() {
        let dv = DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::NotAssessed {
                reason: "fund_style_asset".to_owned(),
            },
        };
        let state = state_with_valuation(dv);
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(
            out.contains("not assessed"),
            "must render 'not assessed' for NotAssessed variant"
        );
        assert!(
            out.contains("fund_style_asset"),
            "must include the reason string"
        );
    }

    #[test]
    fn write_scenario_valuation_shows_not_assessed_with_insufficient_inputs_reason() {
        let dv = DerivedValuation {
            asset_shape: AssetShape::Unknown,
            scenario: ScenarioValuation::NotAssessed {
                reason: "insufficient_corporate_fundamentals".to_owned(),
            },
        };
        let state = state_with_valuation(dv);
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(out.contains("insufficient_corporate_fundamentals"));
    }

    // ── CorporateEquity full metrics ─────────────────────────────────────

    #[test]
    fn write_scenario_valuation_renders_all_metrics_when_present() {
        let state = state_with_valuation(full_corporate_valuation());
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(out.contains("DCF intrinsic value"), "must show DCF metric");
        assert!(out.contains("EV/EBITDA"), "must show EV/EBITDA metric");
        assert!(out.contains("Forward P/E"), "must show forward P/E metric");
        assert!(out.contains("PEG ratio"), "must show PEG metric");
    }

    #[test]
    fn write_scenario_valuation_renders_implied_value_in_ev_ebitda() {
        let state = state_with_valuation(full_corporate_valuation());
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(
            out.contains("implied"),
            "must render implied value per share when present"
        );
    }

    // ── Partial metrics ───────────────────────────────────────────────────

    #[test]
    fn write_scenario_valuation_renders_only_present_metrics() {
        let dv = DerivedValuation {
            asset_shape: AssetShape::CorporateEquity,
            scenario: ScenarioValuation::CorporateEquity(CorporateEquityValuation {
                dcf: Some(DcfValuation {
                    free_cash_flow: 500_000_000.0,
                    discount_rate_pct: 9.5,
                    intrinsic_value_per_share: 142.0,
                }),
                ev_ebitda: None,
                forward_pe: None,
                peg: None,
            }),
        };
        let state = state_with_valuation(dv);
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(out.contains("DCF intrinsic value"), "DCF must appear");
        assert!(
            !out.contains("EV/EBITDA"),
            "EV/EBITDA must be absent when None"
        );
        assert!(
            !out.contains("Forward P/E"),
            "Forward P/E must be absent when None"
        );
        assert!(!out.contains("PEG ratio"), "PEG must be absent when None");
    }

    // ── All metrics None case ─────────────────────────────────────────────

    #[test]
    fn write_scenario_valuation_shows_no_metrics_message_when_all_none() {
        let dv = DerivedValuation {
            asset_shape: AssetShape::CorporateEquity,
            scenario: ScenarioValuation::CorporateEquity(CorporateEquityValuation {
                dcf: None,
                ev_ebitda: None,
                forward_pe: None,
                peg: None,
            }),
        };
        let state = state_with_valuation(dv);
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state);
        assert!(
            out.contains("No valuation metrics computed"),
            "must emit fallback message when all metric fields are None"
        );
    }

    // ── Never panics ──────────────────────────────────────────────────────

    #[test]
    fn write_scenario_valuation_never_panics_on_absent_valuation() {
        // This must not panic — full regression guard.
        let state = TradingState::new("SPY", "2026-04-03");
        let mut out = String::new();
        write_scenario_valuation(&mut out, &state); // no panic expected
        assert!(!out.is_empty());
    }
}
