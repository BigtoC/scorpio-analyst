//! `CryptoDigitalAsset` pack stub — registered, validated, non-selectable.
// TODO: implement in crypto-pack change.

use std::collections::HashMap;

use crate::{prompts::PromptBundle, state::AssetShape, valuation::ValuatorId};

use super::super::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};

/// Stub digital-asset manifest.
///
/// Uses crypto-flavoured placeholder names / emphasis strings so the
/// manifest passes `validate()` (non-empty name, emphasis, report label,
/// required-inputs list), but `required_inputs` and `valuator_selection`
/// reference crypto `AnalystId` / `AssetShape` variants the runtime does
/// not yet dispatch to — i.e. if this pack is somehow selected the
/// dynamic fan-out returns an empty task vector and the pipeline refuses
/// to do any work. That's the desired safety net until the crypto pack
/// slice wires in real analysts and providers.
pub fn digital_asset_pack() -> AnalysisPackManifest {
    AnalysisPackManifest {
        id: PackId::CryptoDigitalAsset,
        name: "Digital Asset (stub)".to_owned(),
        description: "Crypto digital-asset pack placeholder. Non-selectable in \
                       this slice — the crypto pack implementation populates \
                       analysts, providers, and valuation strategies in a \
                       follow-up change."
            .to_owned(),
        required_inputs: vec![
            "tokenomics".to_owned(),
            "onchain".to_owned(),
            "social".to_owned(),
            "derivatives".to_owned(),
        ],
        enrichment_intent: EnrichmentIntent {
            transcripts: false,
            consensus_estimates: false,
            event_news: false,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Crypto-native analysis (placeholder). Real strategy \
                            content lands with the crypto pack implementation."
            .to_owned(),
        report_strategy_label: "Digital Asset (stub)".to_owned(),
        default_valuation: ValuationAssessment::NotAssessed,
        prompt_bundle: PromptBundle::empty(),
        valuator_selection: {
            // Crypto shapes map to placeholder valuator ids that `equity_baseline`
            // doesn't register. When `ValuatorRegistry::get` returns None the
            // pipeline falls through to `ValuationReport::NotAssessed`, which is
            // the correct outcome for an un-implemented asset class.
            let mut m = HashMap::new();
            m.insert(AssetShape::NativeChainAsset, ValuatorId::CryptoNetworkValue);
            m.insert(AssetShape::Erc20Token, ValuatorId::CryptoTokenomics);
            m.insert(AssetShape::Stablecoin, ValuatorId::CryptoTokenomics);
            m
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digital_asset_pack_validates() {
        let pack = digital_asset_pack();
        assert!(
            pack.validate().is_ok(),
            "crypto digital-asset pack must validate: {:?}",
            pack.validate()
        );
    }

    #[test]
    fn digital_asset_pack_is_not_user_selectable_via_from_str() {
        // `PackId::from_str` intentionally does not recognise the stub pack.
        let err = "crypto_digital_asset"
            .parse::<PackId>()
            .expect_err("stub pack must not be selectable via config");
        assert!(
            err.contains("unknown analysis pack"),
            "error must say pack is unknown, got: {err}"
        );
    }

    #[test]
    fn digital_asset_pack_id_display_matches_snake_case() {
        assert_eq!(PackId::CryptoDigitalAsset.as_str(), "crypto_digital_asset");
    }
}
