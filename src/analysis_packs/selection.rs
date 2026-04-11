//! Pack selection and runtime-policy hydration.
//!
//! Resolves a config-level pack identifier into a typed [`RuntimePolicy`] that
//! downstream consumers (prompts, reports, evidence) read instead of raw pack
//! manifests. This is the single resolution boundary: raw pack structure does
//! not leak past this module.

use serde::{Deserialize, Serialize};

use super::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
    builtin::resolve_pack,
};

/// Typed runtime policy derived from a resolved analysis pack.
///
/// Downstream consumers read this instead of raw pack manifests.
/// Serializable for context propagation through graph-flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimePolicy {
    /// Which pack produced this policy.
    pub pack_id: PackId,
    /// Human-readable pack name for reports.
    pub pack_name: String,
    /// Required evidence inputs for the pipeline.
    pub required_inputs: Vec<String>,
    /// Enrichment feature intent from the pack.
    pub enrichment_intent: RuntimeEnrichmentIntent,
    /// Strategy focus lens.
    pub strategy_focus: StrategyFocus,
    /// Short emphasis description for analysis prompts.
    pub analysis_emphasis: String,
    /// Label for the report header.
    pub report_strategy_label: String,
    /// Default valuation assessment for corporate equities.
    pub default_valuation: ValuationAssessment,
}

/// Serializable enrichment intent (mirrors [`EnrichmentIntent`] but with serde).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEnrichmentIntent {
    pub transcripts: bool,
    pub consensus_estimates: bool,
    pub event_news: bool,
}

impl From<&EnrichmentIntent> for RuntimeEnrichmentIntent {
    fn from(intent: &EnrichmentIntent) -> Self {
        RuntimeEnrichmentIntent {
            transcripts: intent.transcripts,
            consensus_estimates: intent.consensus_estimates,
            event_news: intent.event_news,
        }
    }
}

/// Resolve a pack identifier string into typed [`RuntimePolicy`].
///
/// # Errors
///
/// Returns an error string if the pack id is unknown or the resolved
/// manifest fails validation.
pub fn resolve_runtime_policy(pack_id_str: &str) -> Result<RuntimePolicy, String> {
    let pack_id: PackId = pack_id_str.parse()?;
    let manifest: AnalysisPackManifest = resolve_pack(pack_id);

    manifest.validate()?;

    Ok(hydrate_policy(&manifest))
}

/// Hydrate a [`RuntimePolicy`] from a validated manifest.
fn hydrate_policy(manifest: &AnalysisPackManifest) -> RuntimePolicy {
    RuntimePolicy {
        pack_id: manifest.id,
        pack_name: manifest.name.clone(),
        required_inputs: manifest.required_inputs.clone(),
        enrichment_intent: RuntimeEnrichmentIntent::from(&manifest.enrichment_intent),
        strategy_focus: manifest.strategy_focus,
        analysis_emphasis: manifest.analysis_emphasis.clone(),
        report_strategy_label: manifest.report_strategy_label.clone(),
        default_valuation: manifest.default_valuation,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Happy path ──────────────────────────────────────────────────────

    #[test]
    fn resolve_baseline_produces_valid_runtime_policy() {
        let policy = resolve_runtime_policy("baseline").expect("should resolve");
        assert_eq!(policy.pack_id, PackId::Baseline);
        assert_eq!(policy.pack_name, "Balanced Institutional");
        assert_eq!(policy.strategy_focus, StrategyFocus::Balanced);
        assert_eq!(policy.default_valuation, ValuationAssessment::Full);
    }

    #[test]
    fn resolve_baseline_required_inputs_match_stage1_fixed_order() {
        let policy = resolve_runtime_policy("baseline").expect("should resolve");
        assert_eq!(
            policy.required_inputs,
            vec!["fundamentals", "sentiment", "news", "technical"]
        );
    }

    #[test]
    fn resolve_baseline_enrichment_intent_matches_current_defaults() {
        let policy = resolve_runtime_policy("baseline").expect("should resolve");
        assert!(!policy.enrichment_intent.transcripts);
        assert!(!policy.enrichment_intent.consensus_estimates);
        assert!(!policy.enrichment_intent.event_news);
    }

    #[test]
    fn resolve_baseline_has_non_empty_analysis_emphasis() {
        let policy = resolve_runtime_policy("baseline").expect("should resolve");
        assert!(
            !policy.analysis_emphasis.is_empty(),
            "analysis_emphasis must not be empty"
        );
    }

    #[test]
    fn resolve_baseline_has_non_empty_report_strategy_label() {
        let policy = resolve_runtime_policy("baseline").expect("should resolve");
        assert!(
            !policy.report_strategy_label.is_empty(),
            "report_strategy_label must not be empty"
        );
    }

    // ── Error paths ─────────────────────────────────────────────────────

    #[test]
    fn resolve_unknown_pack_fails() {
        let err = resolve_runtime_policy("nonexistent").unwrap_err();
        assert!(
            err.contains("unknown analysis pack"),
            "error should mention unknown pack: {err}"
        );
    }

    #[test]
    fn resolve_empty_string_fails() {
        assert!(resolve_runtime_policy("").is_err());
    }

    // ── Serde round-trip ────────────────────────────────────────────────

    #[test]
    fn runtime_policy_serde_round_trip() {
        let policy = resolve_runtime_policy("baseline").expect("should resolve");
        let json = serde_json::to_string(&policy).expect("serialize");
        let back: RuntimePolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(policy, back);
    }

    #[test]
    fn runtime_enrichment_intent_from_enrichment_intent() {
        let intent = EnrichmentIntent {
            transcripts: true,
            consensus_estimates: false,
            event_news: true,
        };
        let runtime = RuntimeEnrichmentIntent::from(&intent);
        assert!(runtime.transcripts);
        assert!(!runtime.consensus_estimates);
        assert!(runtime.event_news);
    }
}
