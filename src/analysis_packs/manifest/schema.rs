use crate::state::AssetShape;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
        }
    }
}
