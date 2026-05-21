//! Baseline equity pack — the default analysis profile.
//!
//! Reproduces current runtime behavior as the default analysis profile.
//! Corporate equities receive full deterministic valuation; ETFs and
//! unsupported shapes fall back to valuation-not-assessed.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::{prompts::PromptBundle, state::AssetShape, valuation::ValuatorId};

use super::super::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};

/// Cross-cutting evidence-discipline rules + analyst inference guards every
/// equity analyst's system prompt enforces. Loaded once at compile time and
/// appended to each analyst slot in [`baseline_prompt_bundle`].
const ANALYST_RUNTIME_CONTRACT: &str =
    include_str!("../common/prompts/analyst_runtime_contract.md");
const THEME_C_MANAGEMENT_RED_FLAGS: &str = include_str!("prompts/theme_c_management_red_flags.md");
const THEME_H_SOURCING_AND_UNTRUSTED: &str =
    include_str!("../common/prompts/theme_h_sourcing_and_untrusted.md");

fn trim_trailing_newline(content: &str) -> &str {
    content.strip_suffix('\n').unwrap_or(content)
}

fn theme_h_sourcing_and_untrusted(output_field: &str) -> String {
    THEME_H_SOURCING_AND_UNTRUSTED.replace("{output_field}", output_field)
}

fn compose_prompt_sections(raw: &str, sections: &[&str]) -> String {
    let mut composed = trim_trailing_newline(raw).to_owned();
    for section in sections {
        composed.push_str("\n\n");
        composed.push_str(trim_trailing_newline(section));
    }
    composed
}

fn with_sections(raw: &'static str, sections: &[&str]) -> Cow<'static, str> {
    Cow::Owned(compose_prompt_sections(raw, sections))
}

fn with_analyst_runtime_contract_sections(
    raw: &'static str,
    sections: &[&str],
) -> Cow<'static, str> {
    let mut composed = compose_prompt_sections(raw, sections);
    composed.push_str("\n\n");
    composed.push_str(trim_trailing_newline(ANALYST_RUNTIME_CONTRACT));
    Cow::Owned(composed)
}

fn baseline_prompt_bundle() -> PromptBundle {
    let theme_h_summary = theme_h_sourcing_and_untrusted("summary");
    let theme_h_rationale = theme_h_sourcing_and_untrusted("`rationale`");

    PromptBundle {
        fundamental_analyst: with_analyst_runtime_contract_sections(
            include_str!("prompts/fundamental_analyst.md"),
            &[theme_h_summary.as_str()],
        ),
        sentiment_analyst: with_analyst_runtime_contract_sections(
            include_str!("prompts/sentiment_analyst.md"),
            &[THEME_C_MANAGEMENT_RED_FLAGS, theme_h_summary.as_str()],
        ),
        news_analyst: with_analyst_runtime_contract_sections(
            include_str!("prompts/news_analyst.md"),
            &[THEME_C_MANAGEMENT_RED_FLAGS, theme_h_summary.as_str()],
        ),
        technical_analyst: with_analyst_runtime_contract_sections(
            include_str!("prompts/technical_analyst.md"),
            &[theme_h_summary.as_str()],
        ),
        bullish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/bullish_researcher.md"
        ))),
        bearish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/bearish_researcher.md"
        ))),
        debate_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/debate_moderator.md"
        ))),
        trader: with_sections(
            include_str!("prompts/trader.md"),
            &[theme_h_rationale.as_str()],
        ),
        aggressive_risk: Cow::Borrowed(trim_trailing_newline(include_str!(
            "prompts/aggressive_risk.md"
        ))),
        conservative_risk: Cow::Borrowed(trim_trailing_newline(include_str!(
            "prompts/conservative_risk.md"
        ))),
        neutral_risk: Cow::Borrowed(trim_trailing_newline(include_str!(
            "prompts/neutral_risk.md"
        ))),
        risk_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/risk_moderator.md"
        ))),
        fund_manager: Cow::Borrowed(trim_trailing_newline(include_str!(
            "prompts/fund_manager.md"
        ))),
        auditor: Cow::Borrowed(trim_trailing_newline(include_str!("prompts/auditor.md"))),
    }
}

/// Build the baseline pack manifest.
pub fn baseline_pack() -> AnalysisPackManifest {
    AnalysisPackManifest {
        id: PackId::Baseline,
        name: "Balanced Institutional".to_owned(),
        description: "Balanced institutional strategy utilizing DCF, multiples, \
                       options flow, and consensus estimates. Corporate equities \
                       receive full deterministic valuation; ETFs and unsupported \
                       shapes fall back to valuation-not-assessed."
            .to_owned(),
        required_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
        enrichment_intent: EnrichmentIntent {
            transcripts: true,
            consensus_estimates: true,
            event_news: true,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Weight all data sources equally. Use DCF and multiples for valuation \
             when available. Consider both fundamental quality and market sentiment."
            .to_owned(),
        report_strategy_label: "Balanced Institutional".to_owned(),
        default_valuation: ValuationAssessment::Full,
        prompt_bundle: baseline_prompt_bundle(),
        valuator_selection: {
            let mut m = HashMap::new();
            m.insert(AssetShape::CorporateEquity, ValuatorId::EquityDefault);
            m
        },
        auditor_enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AssetShape;

    fn resolve_pack(id: PackId) -> AnalysisPackManifest {
        super::super::super::registry::resolve_pack(id)
    }

    #[test]
    fn baseline_pack_validates_successfully() {
        let pack = resolve_pack(PackId::Baseline);
        assert!(
            pack.validate().is_ok(),
            "baseline pack must pass validation: {:?}",
            pack.validate()
        );
    }

    #[test]
    fn baseline_pack_has_correct_id() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(pack.id, PackId::Baseline);
    }

    #[test]
    fn baseline_pack_required_inputs_match_current_fixed_order() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(
            pack.required_inputs,
            vec!["fundamentals", "sentiment", "news", "technical"],
            "baseline pack must reproduce the current Stage 1 fixed input order"
        );
    }

    #[test]
    fn baseline_pack_enrichment_intent_preserves_current_defaults() {
        let pack = resolve_pack(PackId::Baseline);
        assert!(pack.enrichment_intent.transcripts);
        assert!(pack.enrichment_intent.consensus_estimates);
        assert!(pack.enrichment_intent.event_news);
    }

    #[test]
    fn baseline_pack_strategy_is_balanced() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(pack.strategy_focus, StrategyFocus::Balanced);
    }

    #[test]
    fn baseline_pack_default_valuation_is_full() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(pack.default_valuation, ValuationAssessment::Full);
    }

    #[test]
    fn baseline_pack_corporate_equity_gets_full_valuation() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::CorporateEquity),
            ValuationAssessment::Full
        );
    }

    #[test]
    fn baseline_pack_etf_gets_not_assessed() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::Fund),
            ValuationAssessment::NotAssessed,
            "ETF/fund shape should fall back to NotAssessed"
        );
    }

    #[test]
    fn baseline_pack_unknown_shape_gets_not_assessed() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::Unknown),
            ValuationAssessment::NotAssessed,
            "unknown shape should fall back to NotAssessed"
        );
    }

    #[test]
    fn baseline_pack_populates_prompt_bundle_slots_with_runtime_placeholders() {
        let pack = resolve_pack(PackId::Baseline);
        let slots = [
            (
                "fundamental",
                pack.prompt_bundle.fundamental_analyst.as_ref(),
            ),
            ("sentiment", pack.prompt_bundle.sentiment_analyst.as_ref()),
            ("news", pack.prompt_bundle.news_analyst.as_ref()),
            ("technical", pack.prompt_bundle.technical_analyst.as_ref()),
            (
                "bullish_researcher",
                pack.prompt_bundle.bullish_researcher.as_ref(),
            ),
            (
                "bearish_researcher",
                pack.prompt_bundle.bearish_researcher.as_ref(),
            ),
            (
                "debate_moderator",
                pack.prompt_bundle.debate_moderator.as_ref(),
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
            ("risk_moderator", pack.prompt_bundle.risk_moderator.as_ref()),
            ("fund_manager", pack.prompt_bundle.fund_manager.as_ref()),
        ];

        for (label, template) in slots {
            assert!(
                !template.is_empty(),
                "baseline pack should ship a non-empty {label} prompt template"
            );
            assert!(
                template.contains("{ticker}"),
                "baseline {label} prompt should preserve the {{ticker}} placeholder"
            );
            assert!(
                template.contains("{current_date}"),
                "baseline {label} prompt should preserve the {{current_date}} placeholder"
            );
            // `{analysis_emphasis}` is intentionally not in the baseline
            // assets today — renderers substitute it post-hoc for tests and
            // future packs that opt into it. The completeness predicate
            // (`is_effectively_empty`) handles the placeholder-only case
            // separately, so a future asset that adds `{analysis_emphasis}`
            // continues to render correctly.
        }
    }

    #[test]
    fn baseline_pack_uses_extracted_prompt_assets_not_empty_placeholders() {
        let pack = resolve_pack(PackId::Baseline);

        assert_ne!(pack.prompt_bundle, PromptBundle::empty());
    }

    #[test]
    fn resolve_pack_returns_matching_id() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(pack.id, PackId::Baseline);
    }
}
