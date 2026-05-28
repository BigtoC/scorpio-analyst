//! ETF baseline pack manifest + prompt composition.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::{prompts::PromptBundle, state::AssetShape, valuation::ValuatorId};

use super::super::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};

// ─── ETF scaffolding ──────────────────────────────────────────────────────────

const ETF_RUNTIME_CONTRACT: &str = include_str!("prompts/etf_runtime_contract.md");
const ETF_FAILURE_MODES: &str = include_str!("prompts/etf_failure_modes.md");

// Shared scaffolding reused from the cross-pack `common/` prompt directory.
const COMMON_ANALYST_CONTRACT: &str = include_str!("../common/prompts/analyst_runtime_contract.md");
const RISK_REPORT_OUTPUT_CONTRACT: &str =
    include_str!("../common/prompts/risk_report_output_contract.md");
const TRADE_PROPOSAL_OUTPUT_CONTRACT: &str =
    include_str!("../common/prompts/trade_proposal_output_contract.md");
const EXECUTION_STATUS_OUTPUT_CONTRACT: &str =
    include_str!("../common/prompts/execution_status_output_contract.md");
const ETF_LEVERAGE_WARNING: &str = include_str!("prompts/etf_leverage_warning.md");

fn trim_trailing_newline(content: &str) -> &str {
    content.strip_suffix('\n').unwrap_or(content)
}

fn compose_prompt_sections(raw: &str, sections: &[&str]) -> String {
    let mut composed = trim_trailing_newline(raw).to_owned();
    for section in sections {
        composed.push_str("\n\n");
        composed.push_str(trim_trailing_newline(section));
    }
    composed
}

/// Compose a fully ETF-native analyst slot: raw prompt + common contract +
/// ETF runtime contract + ETF failure modes.
fn compose_etf_analyst(raw: &'static str) -> Cow<'static, str> {
    Cow::Owned(compose_prompt_sections(
        raw,
        &[
            COMMON_ANALYST_CONTRACT,
            ETF_RUNTIME_CONTRACT,
            ETF_FAILURE_MODES,
        ],
    ))
}

/// Compose a Tier-2 reuse: shared prompt verbatim + small ETF deltas.
fn compose_etf_section(raw: &'static str, deltas: &[&str]) -> Cow<'static, str> {
    let mut sections = vec![ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES];
    sections.extend_from_slice(deltas);
    Cow::Owned(compose_prompt_sections(raw, &sections))
}

/// Compose a risk-agent slot: ETF-specific raw prompt + ETF scaffolding +
/// the shared `RiskReport` output contract with `{stance}` substituted in.
fn compose_etf_risk(raw: &'static str, stance: &str) -> Cow<'static, str> {
    let output_contract = RISK_REPORT_OUTPUT_CONTRACT.replace("{stance}", stance);
    Cow::Owned(compose_prompt_sections(
        raw,
        &[ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES, &output_contract],
    ))
}

const LEVERAGE_TOLERANCE: f64 = 1e-6;

/// Runtime-only helper invoked after placeholder substitution. Risk and
/// auditor prompts append the leverage warning when `leverage_factor`
/// diverges from 1.0 beyond the tolerance. Substitutes `{leverage_factor}`
/// in the warning body with a human-friendly representation of the factor.
pub(crate) fn append_leverage_warning_if_needed(
    rendered: String,
    leverage_factor: Option<f64>,
) -> String {
    match leverage_factor {
        Some(factor) if (factor - 1.0).abs() > LEVERAGE_TOLERANCE => {
            let warning = trim_trailing_newline(ETF_LEVERAGE_WARNING)
                .replace("{leverage_factor}", &format_leverage_factor(factor));
            format!("{rendered}\n\n---\n\n{warning}")
        }
        _ => rendered,
    }
}

fn format_leverage_factor(factor: f64) -> String {
    if (factor - factor.round()).abs() < LEVERAGE_TOLERANCE {
        format!("{:.0}", factor)
    } else {
        format!("{:.1}", factor)
    }
}

fn etf_baseline_prompt_bundle() -> PromptBundle {
    PromptBundle {
        // Tier 3 — fully new ETF analysts.
        fundamental_analyst: compose_etf_analyst(include_str!("prompts/composition_analyst.md")),
        sentiment_analyst: compose_etf_analyst(include_str!("prompts/flow_premium_analyst.md")),

        // Tier 2 — shared prompt + ETF delta.
        news_analyst: compose_etf_section(
            include_str!("../common/prompts/news_analyst.md"),
            &[include_str!("prompts/etf_macro_sector_focus.md")],
        ),
        technical_analyst: compose_etf_section(
            include_str!("../common/prompts/technical_analyst.md"),
            &[include_str!("prompts/etf_tracking_options_focus.md")],
        ),
        auditor: compose_etf_section(
            include_str!("../common/prompts/auditor.md"),
            &[include_str!("prompts/etf_landmines.md")],
        ),

        // Tier 1 — verbatim reuse from the shared cross-pack `common/`
        // prompt directory.
        bullish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/bullish_researcher.md"
        ))),
        bearish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/bearish_researcher.md"
        ))),
        debate_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/debate_moderator.md"
        ))),
        risk_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/risk_moderator.md"
        ))),

        // Tier 3 — fully new ETF roles (trader, risk, fund manager).
        trader: compose_etf_section(
            include_str!("prompts/trader.md"),
            &[TRADE_PROPOSAL_OUTPUT_CONTRACT],
        ),
        aggressive_risk: compose_etf_risk(include_str!("prompts/aggressive_risk.md"), "Aggressive"),
        conservative_risk: compose_etf_risk(
            include_str!("prompts/conservative_risk.md"),
            "Conservative",
        ),
        neutral_risk: compose_etf_risk(include_str!("prompts/neutral_risk.md"), "Neutral"),
        fund_manager: compose_etf_section(
            include_str!("prompts/fund_manager.md"),
            &[EXECUTION_STATUS_OUTPUT_CONTRACT],
        ),
    }
}

/// Build the ETF baseline pack manifest.
pub fn etf_baseline_pack() -> AnalysisPackManifest {
    AnalysisPackManifest {
        id: PackId::EtfBaseline,
        name: "ETF Baseline".to_owned(),
        description: "Phase 1 ETF-native analysis: premium/discount band, \
                        composition/sector tilt with filing-age qualification when N-PORT data is available, \
                        and tracking error vs a source-provided benchmark. \
                        Sources: yfinance + SEC EDGAR N-PORT-P (free tier)."
            .to_owned(),
        required_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
        enrichment_intent: EnrichmentIntent {
            transcripts: false,
            consensus_estimates: false,
            event_news: true,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Premium/discount band classification anchors the assessment. \
                            Weight composition and tracking equally; flag leverage decay \
                            and AP arbitrage breakdown explicitly."
            .to_owned(),
        report_strategy_label: "ETF Baseline".to_owned(),
        default_valuation: ValuationAssessment::Etf,
        prompt_bundle: etf_baseline_prompt_bundle(),
        valuator_selection: {
            let mut m = HashMap::new();
            m.insert(AssetShape::Fund, ValuatorId::EtfPremiumDiscount);
            m
        },
        auditor_enabled: true,
        // ETFs are not in scope for the Reddit sentiment sidecar: r/stocks
        // and r/investing chatter rarely names a specific fund ticker, so
        // queries return mostly noise. Empty vec opts the pack out — the
        // sentiment lane carries on with vetted sources only.
        reddit_subreddits: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve_pack(id: PackId) -> AnalysisPackManifest {
        super::super::super::registry::resolve_pack(id)
    }

    #[test]
    fn etf_baseline_pack_validates_successfully() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert!(
            pack.validate().is_ok(),
            "validation failed: {:?}",
            pack.validate()
        );
    }

    #[test]
    fn etf_baseline_pack_has_correct_id() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(pack.id, PackId::EtfBaseline);
    }

    #[test]
    fn etf_baseline_required_inputs_drive_four_analyst_slots() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.required_inputs,
            vec!["fundamentals", "sentiment", "news", "technical"]
        );
    }

    #[test]
    fn etf_baseline_default_valuation_is_etf() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(pack.default_valuation, ValuationAssessment::Etf);
    }

    #[test]
    fn etf_baseline_fund_shape_resolves_to_etf_valuation() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::Fund),
            ValuationAssessment::Etf
        );
    }

    #[test]
    fn etf_baseline_corporate_equity_falls_through_to_full_per_resolve_rule() {
        // Sanity: the ETF pack doesn't list CorporateEquity in its
        // valuator_selection, but resolve_valuation maps it to Full when
        // default_valuation = Etf per the schema rule from Task 2.
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::CorporateEquity),
            ValuationAssessment::Full
        );
    }

    #[test]
    fn etf_baseline_valuator_selection_maps_fund_shape() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.valuator_selection.get(&AssetShape::Fund).copied(),
            Some(ValuatorId::EtfPremiumDiscount)
        );
    }

    #[test]
    fn etf_baseline_populates_every_prompt_slot_with_runtime_placeholders() {
        let pack = resolve_pack(PackId::EtfBaseline);
        let slots = [
            (
                "fundamental_analyst",
                pack.prompt_bundle.fundamental_analyst.as_ref(),
            ),
            (
                "sentiment_analyst",
                pack.prompt_bundle.sentiment_analyst.as_ref(),
            ),
            ("news_analyst", pack.prompt_bundle.news_analyst.as_ref()),
            (
                "technical_analyst",
                pack.prompt_bundle.technical_analyst.as_ref(),
            ),
            ("trader", pack.prompt_bundle.trader.as_ref()),
            (
                "aggressive_risk",
                pack.prompt_bundle.aggressive_risk.as_ref(),
            ),
            (
                "conservative_risk",
                pack.prompt_bundle.conservative_risk.as_ref(),
            ),
            ("neutral_risk", pack.prompt_bundle.neutral_risk.as_ref()),
            ("fund_manager", pack.prompt_bundle.fund_manager.as_ref()),
        ];
        for (label, template) in slots {
            assert!(!template.is_empty(), "{label} must not be empty");
            assert!(
                template.contains("{ticker}"),
                "{label} must contain {{ticker}}"
            );
            assert!(
                template.contains("{current_date}"),
                "{label} must contain {{current_date}}"
            );
        }
    }

    #[test]
    fn etf_baseline_auditor_slot_is_non_empty() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert!(pack.auditor_enabled);
        assert!(!pack.prompt_bundle.auditor.is_empty());
    }

    #[test]
    fn append_leverage_warning_uses_divider_when_factor_diverges() {
        let rendered = "BASE PROMPT".to_owned();
        let result = append_leverage_warning_if_needed(rendered, Some(2.0));
        assert!(
            result.starts_with("BASE PROMPT"),
            "rendered base prompt must remain at the head"
        );
        assert!(
            result.contains("\n\n---\n\n"),
            "must insert the explicit --- divider: {result}"
        );
        assert!(result.len() > "BASE PROMPT".len() + 8);
    }

    #[test]
    fn append_leverage_warning_uses_1e_minus_6_tolerance() {
        let base = "PROMPT".to_owned();
        let untouched = append_leverage_warning_if_needed(base.clone(), Some(1.0 + f64::EPSILON));
        assert_eq!(untouched, base, "EPSILON drift must not trigger warning");

        let with_drift = append_leverage_warning_if_needed(base.clone(), Some(1.0 + 1e-5));
        assert_ne!(with_drift, base, "1e-5 drift must trigger warning");
    }

    #[test]
    fn append_leverage_warning_skips_for_unit_and_none() {
        let base = "PROMPT".to_owned();
        assert_eq!(append_leverage_warning_if_needed(base.clone(), None), base);
        assert_eq!(
            append_leverage_warning_if_needed(base.clone(), Some(1.0)),
            base
        );
    }

    #[test]
    fn append_leverage_warning_triggers_for_leveraged_and_inverse() {
        let base = "PROMPT".to_owned();
        for factor in [2.0, 3.0, -1.0, -2.0, -3.0] {
            let result = append_leverage_warning_if_needed(base.clone(), Some(factor));
            assert!(
                result.len() > base.len(),
                "factor {factor} should append warning"
            );
        }
    }

    #[test]
    fn append_leverage_warning_substitutes_leverage_factor_placeholder() {
        let base = "PROMPT".to_owned();
        let triple = append_leverage_warning_if_needed(base.clone(), Some(3.0));
        assert!(triple.contains("3x"), "must substitute 3x: {triple}");
        assert!(
            !triple.contains("{leverage_factor}"),
            "placeholder must be gone"
        );

        let inverse = append_leverage_warning_if_needed(base.clone(), Some(-1.0));
        assert!(inverse.contains("-1x"), "must substitute -1x: {inverse}");

        let half = append_leverage_warning_if_needed(base.clone(), Some(1.5));
        assert!(half.contains("1.5x"), "must substitute 1.5x: {half}");
    }
}
