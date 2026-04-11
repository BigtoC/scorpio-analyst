//! Analysis-pack schema and validation.
//!
//! Defines the declarative analysis-pack vocabulary: coverage, enrichment
//! intent, strategy focus, valuation policy, and pack metadata.
//! Packs are policy objects — they shape analysis behavior without owning
//! execution or graph topology.

use serde::{Deserialize, Serialize};

use crate::state::AssetShape;

// ─── Pack identifier ─────────────────────────────────────────────────────────

/// Built-in analysis pack identifier.
///
/// First-slice: only built-in packs selected by config/env string.
/// Serde support enables lightweight persistence in snapshot metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackId {
    /// Balanced institutional strategy — the default pack.
    Baseline,
}

impl PackId {
    /// Canonical string representation for config/env selection.
    pub fn as_str(self) -> &'static str {
        match self {
            PackId::Baseline => "baseline",
        }
    }
}

impl std::fmt::Display for PackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PackId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "baseline" => Ok(PackId::Baseline),
            unknown => Err(format!(
                "unknown analysis pack: \"{unknown}\" (available: baseline)"
            )),
        }
    }
}

// ─── Strategy focus ──────────────────────────────────────────────────────────

/// Strategy lens for prompt and report framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyFocus {
    /// Balanced institutional — weights all data sources equally.
    Balanced,
    /// Deep value — emphasizes DCF, earnings quality, margin of safety.
    DeepValue,
    /// Momentum — emphasizes price action, flow, and trend signals.
    Momentum,
}

// ─── Valuation assessment ────────────────────────────────────────────────────

/// Asset-shape valuation assessment policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuationAssessment {
    /// Full deterministic valuation (DCF, multiples) for corporate equities.
    Full,
    /// Valuation not assessed — explicit fallback for ETFs, indices, etc.
    NotAssessed,
}

// ─── Enrichment intent ───────────────────────────────────────────────────────

/// Optional enrichment data intent declared by the pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichmentIntent {
    pub transcripts: bool,
    pub consensus_estimates: bool,
    pub event_news: bool,
}

// ─── Pack manifest ───────────────────────────────────────────────────────────

/// Declarative analysis-pack manifest.
///
/// Encodes coverage, enrichment intent, strategy focus, valuation policy, and
/// metadata. Packs do not own execution, graph topology, or provider-factory
/// routing.
#[derive(Debug, Clone)]
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

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PackId parsing ──────────────────────────────────────────────────

    #[test]
    fn pack_id_parses_baseline() {
        let id: PackId = "baseline".parse().expect("should parse");
        assert_eq!(id, PackId::Baseline);
    }

    #[test]
    fn pack_id_parses_case_insensitive() {
        let id: PackId = "  Baseline  ".parse().expect("should parse");
        assert_eq!(id, PackId::Baseline);
    }

    #[test]
    fn pack_id_rejects_unknown() {
        let err = "momentum_turbo".parse::<PackId>().unwrap_err();
        assert!(
            err.contains("unknown analysis pack"),
            "error should describe the problem: {err}"
        );
        assert!(
            err.contains("momentum_turbo"),
            "error should include the unknown id: {err}"
        );
    }

    #[test]
    fn pack_id_rejects_empty_string() {
        let err = "".parse::<PackId>().unwrap_err();
        assert!(err.contains("unknown analysis pack"));
    }

    #[test]
    fn pack_id_display_matches_as_str() {
        assert_eq!(PackId::Baseline.to_string(), "baseline");
        assert_eq!(PackId::Baseline.as_str(), "baseline");
    }

    // ── PackId serde ────────────────────────────────────────────────────

    #[test]
    fn pack_id_serde_round_trip() {
        let id = PackId::Baseline;
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"baseline\"");
        let back: PackId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    // ── StrategyFocus serde ─────────────────────────────────────────────

    #[test]
    fn strategy_focus_serde_snake_case() {
        let json = serde_json::to_string(&StrategyFocus::DeepValue).unwrap();
        assert_eq!(json, "\"deep_value\"");
        let back: StrategyFocus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, StrategyFocus::DeepValue);
    }

    // ── ValuationAssessment serde ───────────────────────────────────────

    #[test]
    fn valuation_assessment_serde_round_trip() {
        for val in [ValuationAssessment::Full, ValuationAssessment::NotAssessed] {
            let json = serde_json::to_string(&val).unwrap();
            let back: ValuationAssessment = serde_json::from_str(&json).unwrap();
            assert_eq!(val, back);
        }
    }

    #[test]
    fn valuation_assessment_not_assessed_serializes_snake_case() {
        let json = serde_json::to_string(&ValuationAssessment::NotAssessed).unwrap();
        assert_eq!(json, "\"not_assessed\"");
    }

    // ── Manifest validation ─────────────────────────────────────────────

    fn valid_manifest() -> AnalysisPackManifest {
        AnalysisPackManifest {
            id: PackId::Baseline,
            name: "Test Pack".to_owned(),
            description: "A test pack".to_owned(),
            required_inputs: vec!["fundamentals".to_owned()],
            enrichment_intent: EnrichmentIntent {
                transcripts: false,
                consensus_estimates: false,
                event_news: false,
            },
            strategy_focus: StrategyFocus::Balanced,
            analysis_emphasis: "Test emphasis".to_owned(),
            report_strategy_label: "Test".to_owned(),
            default_valuation: ValuationAssessment::Full,
        }
    }

    #[test]
    fn valid_manifest_passes_validation() {
        assert!(valid_manifest().validate().is_ok());
    }

    #[test]
    fn manifest_rejects_empty_name() {
        let mut m = valid_manifest();
        m.name = "  ".to_owned();
        let err = m.validate().unwrap_err();
        assert!(err.contains("name"), "error should mention name: {err}");
    }

    #[test]
    fn manifest_rejects_empty_required_inputs() {
        let mut m = valid_manifest();
        m.required_inputs.clear();
        let err = m.validate().unwrap_err();
        assert!(
            err.contains("required input"),
            "error should mention required inputs: {err}"
        );
    }

    #[test]
    fn manifest_rejects_empty_analysis_emphasis() {
        let mut m = valid_manifest();
        m.analysis_emphasis = String::new();
        let err = m.validate().unwrap_err();
        assert!(
            err.contains("analysis_emphasis"),
            "error should mention analysis_emphasis: {err}"
        );
    }

    #[test]
    fn manifest_rejects_empty_report_strategy_label() {
        let mut m = valid_manifest();
        m.report_strategy_label = "   ".to_owned();
        let err = m.validate().unwrap_err();
        assert!(
            err.contains("report_strategy_label"),
            "error should mention report_strategy_label: {err}"
        );
    }

    // ── Valuation resolution by asset shape ──────────────────────────────

    #[test]
    fn resolve_valuation_corporate_equity_uses_pack_default() {
        let m = valid_manifest();
        assert_eq!(
            m.resolve_valuation(&AssetShape::CorporateEquity),
            ValuationAssessment::Full
        );
    }

    #[test]
    fn resolve_valuation_fund_always_not_assessed() {
        let m = valid_manifest();
        assert_eq!(
            m.resolve_valuation(&AssetShape::Fund),
            ValuationAssessment::NotAssessed
        );
    }

    #[test]
    fn resolve_valuation_unknown_always_not_assessed() {
        let m = valid_manifest();
        assert_eq!(
            m.resolve_valuation(&AssetShape::Unknown),
            ValuationAssessment::NotAssessed
        );
    }

    #[test]
    fn resolve_valuation_fund_ignores_pack_full_default() {
        // Even if the pack says Full, funds should always be NotAssessed
        let mut m = valid_manifest();
        m.default_valuation = ValuationAssessment::Full;
        assert_eq!(
            m.resolve_valuation(&AssetShape::Fund),
            ValuationAssessment::NotAssessed
        );
    }
}
