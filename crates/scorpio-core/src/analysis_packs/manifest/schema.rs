use std::collections::HashMap;

use crate::{prompts::PromptBundle, state::AssetShape, valuation::ValuatorId};

use super::{PackId, StrategyFocus, ValuationAssessment};

/// Optional enrichment data intent declared by the pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichmentIntent {
    pub transcripts: bool,
    pub consensus_estimates: bool,
    pub event_news: bool,
}

/// Declarative analysis-pack manifest.
///
/// Encodes coverage, enrichment intent, strategy focus, valuation policy, and
/// metadata. Packs do not own execution, graph topology, or provider-factory
/// routing.
// Eq is intentionally not derived: the `valuator_selection` HashMap
// carries runtime-only ordering and HashMap's PartialEq impl is sufficient
// for the `assert_eq!` comparisons tests rely on. Manifests aren't used as
// HashMap keys, so dropping `Eq` carries no behavioural impact.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisPackManifest {
    /// Unique pack identifier.
    pub id: PackId,
    /// Human-readable pack name.
    pub name: String,
    /// Description of the pack's analytical strategy.
    pub description: String,
    /// Required evidence inputs for this pack (e.g. "fundamentals", "news").
    pub required_inputs: Vec<String>,
    /// Optional enrichment data the pack wants fetched.
    pub enrichment_intent: EnrichmentIntent,
    /// Strategy lens for prompt and report framing.
    pub strategy_focus: StrategyFocus,
    /// Short emphasis description injected into analysis prompts.
    pub analysis_emphasis: String,
    /// Label shown in the report header for the selected strategy.
    pub report_strategy_label: String,
    /// Default valuation assessment for supported asset shapes (corporate equity).
    pub default_valuation: ValuationAssessment,
    /// Per-role system prompts supplied by the pack.
    ///
    /// Introduced in Phase 4 of the asset-class generalization refactor.
    /// The baseline equity pack ships extracted prompt assets for every live
    /// role, while stub packs may still use [`PromptBundle::empty`] so runtime
    /// code falls back to the legacy in-module prompt constants.
    pub prompt_bundle: PromptBundle,
    /// Manifest-selected valuation strategy per asset shape.
    ///
    /// Introduced in Phase 5: packs declare which [`ValuatorId`] handles
    /// each [`AssetShape`] they care about. Shapes not listed here fall
    /// through to `ValuationReport::NotAssessed` with reason
    /// `"no_valuator_selected"`. For the baseline equity pack the map
    /// holds `CorporateEquity → EquityDefault`.
    pub valuator_selection: HashMap<AssetShape, ValuatorId>,
}

impl AnalysisPackManifest {
    /// Validate the manifest for internal consistency.
    ///
    /// Returns `Err` with a description if any invariant is violated.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("pack name must not be empty".to_owned());
        }
        if self.required_inputs.is_empty() {
            return Err("pack must declare at least one required input".to_owned());
        }
        if self.analysis_emphasis.trim().is_empty() {
            return Err("pack analysis_emphasis must not be empty".to_owned());
        }
        if self.report_strategy_label.trim().is_empty() {
            return Err("pack report_strategy_label must not be empty".to_owned());
        }
        Ok(())
    }

    /// Resolve the effective valuation assessment for a given asset shape.
    ///
    /// Corporate equities use the pack's `default_valuation` policy.
    /// Fund-style and unknown shapes always resolve to `NotAssessed`.
    pub fn resolve_valuation(&self, shape: &AssetShape) -> ValuationAssessment {
        match shape {
            AssetShape::CorporateEquity => self.default_valuation,
            AssetShape::Fund | AssetShape::Unknown => ValuationAssessment::NotAssessed,
            _ => ValuationAssessment::NotAssessed,
        }
    }
}
