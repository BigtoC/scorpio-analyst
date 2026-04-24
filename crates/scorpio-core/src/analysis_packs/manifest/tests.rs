use crate::state::AssetShape;

use super::{AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment};

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

#[test]
fn pack_id_serde_round_trip() {
    let id = PackId::Baseline;
    let json = serde_json::to_string(&id).expect("serialize");
    assert_eq!(json, "\"baseline\"");
    let back: PackId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, back);
}

#[test]
fn strategy_focus_serde_snake_case() {
    let json = serde_json::to_string(&StrategyFocus::DeepValue).unwrap();
    assert_eq!(json, "\"deep_value\"");
    let back: StrategyFocus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, StrategyFocus::DeepValue);
}

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

#[test]
fn valid_manifest_passes_validation() {
    assert!(valid_manifest().validate().is_ok());
}

#[test]
fn manifest_rejects_empty_name() {
    let mut manifest = valid_manifest();
    manifest.name = "  ".to_owned();
    let err = manifest.validate().unwrap_err();
    assert!(err.contains("name"), "error should mention name: {err}");
}

#[test]
fn manifest_rejects_empty_required_inputs() {
    let mut manifest = valid_manifest();
    manifest.required_inputs.clear();
    let err = manifest.validate().unwrap_err();
    assert!(
        err.contains("required input"),
        "error should mention required inputs: {err}"
    );
}

#[test]
fn manifest_rejects_empty_analysis_emphasis() {
    let mut manifest = valid_manifest();
    manifest.analysis_emphasis = String::new();
    let err = manifest.validate().unwrap_err();
    assert!(
        err.contains("analysis_emphasis"),
        "error should mention analysis_emphasis: {err}"
    );
}

#[test]
fn manifest_rejects_empty_report_strategy_label() {
    let mut manifest = valid_manifest();
    manifest.report_strategy_label = "   ".to_owned();
    let err = manifest.validate().unwrap_err();
    assert!(
        err.contains("report_strategy_label"),
        "error should mention report_strategy_label: {err}"
    );
}

#[test]
fn resolve_valuation_corporate_equity_uses_pack_default() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::CorporateEquity),
        ValuationAssessment::Full
    );
}

#[test]
fn resolve_valuation_fund_always_not_assessed() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::Fund),
        ValuationAssessment::NotAssessed
    );
}

#[test]
fn resolve_valuation_unknown_always_not_assessed() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::Unknown),
        ValuationAssessment::NotAssessed
    );
}

#[test]
fn resolve_valuation_native_chain_asset_not_assessed() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::NativeChainAsset),
        ValuationAssessment::NotAssessed
    );
}

#[test]
fn resolve_valuation_erc20_token_not_assessed() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::Erc20Token),
        ValuationAssessment::NotAssessed
    );
}

#[test]
fn resolve_valuation_stablecoin_not_assessed() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::Stablecoin),
        ValuationAssessment::NotAssessed
    );
}

#[test]
fn resolve_valuation_lp_token_not_assessed() {
    let manifest = valid_manifest();
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::LpToken),
        ValuationAssessment::NotAssessed
    );
}

#[test]
fn resolve_valuation_fund_ignores_pack_full_default() {
    let mut manifest = valid_manifest();
    manifest.default_valuation = ValuationAssessment::Full;
    assert_eq!(
        manifest.resolve_valuation(&AssetShape::Fund),
        ValuationAssessment::NotAssessed
    );
}
