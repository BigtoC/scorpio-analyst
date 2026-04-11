//! Built-in analysis pack definitions.
//!
//! First-slice: only compile-time built-in packs. External manifests or
//! hybrid loading can follow in a later slice if needed.

use super::manifest::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};

/// Resolve a [`PackId`] into its full [`AnalysisPackManifest`].
///
/// First-slice: only built-in packs. This is a pure function with
/// negligible cost (no I/O, no file loading).
pub fn resolve_pack(id: PackId) -> AnalysisPackManifest {
    match id {
        PackId::Baseline => baseline_pack(),
    }
}

/// The baseline pack: balanced institutional strategy.
///
/// Reproduces current runtime behavior as the default analysis profile.
/// Corporate equities receive full deterministic valuation; ETFs and
/// unsupported shapes fall back to valuation-not-assessed.
fn baseline_pack() -> AnalysisPackManifest {
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
            transcripts: false,
            consensus_estimates: false,
            event_news: false,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Weight all data sources equally. Use DCF and multiples for valuation \
             when available. Consider both fundamental quality and market sentiment."
            .to_owned(),
        report_strategy_label: "Balanced Institutional".to_owned(),
        default_valuation: ValuationAssessment::Full,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AssetShape;

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
        assert!(!pack.enrichment_intent.transcripts);
        assert!(!pack.enrichment_intent.consensus_estimates);
        assert!(!pack.enrichment_intent.event_news);
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
    fn resolve_pack_returns_matching_id() {
        // Exhaustive: every PackId variant should resolve to a manifest with that id.
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(pack.id, PackId::Baseline);
    }
}
