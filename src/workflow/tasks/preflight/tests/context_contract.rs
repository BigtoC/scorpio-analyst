use crate::{
    config::DataEnrichmentConfig,
    data::{ResolvedInstrument, adapters::ProviderCapabilities},
    workflow::{
        context_bridge::deserialize_state_from_context,
        tasks::common::{
            KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_TRANSCRIPT,
            KEY_PROVIDER_CAPABILITIES, KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT,
        },
    },
};

use super::run_preflight;

#[tokio::test]
async fn preflight_writes_canonical_uppercase_symbol_to_state() {
    let ctx = run_preflight("nvda", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed with lowercase symbol");

    let state = deserialize_state_from_context(&ctx)
        .await
        .expect("state deserialization");
    assert_eq!(state.asset_symbol, "NVDA");
}

#[tokio::test]
async fn preflight_writes_resolved_instrument_to_context() {
    let ctx = run_preflight("AAPL", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    let json: String = ctx
        .get(KEY_RESOLVED_INSTRUMENT)
        .await
        .expect("resolved_instrument key must be present");
    let instrument: ResolvedInstrument =
        serde_json::from_str(&json).expect("ResolvedInstrument deserialization");
    assert_eq!(instrument.canonical_symbol, "AAPL");
}

#[tokio::test]
async fn preflight_writes_provider_capabilities_to_context() {
    let enrichment = DataEnrichmentConfig {
        enable_transcripts: true,
        enable_consensus_estimates: false,
        enable_event_news: false,
        max_evidence_age_hours: 48,
    };
    let ctx = run_preflight("AAPL", enrichment)
        .await
        .expect("preflight should succeed");

    let json: String = ctx
        .get(KEY_PROVIDER_CAPABILITIES)
        .await
        .expect("provider_capabilities key must be present");
    let caps: ProviderCapabilities =
        serde_json::from_str(&json).expect("ProviderCapabilities deserialization");
    assert!(caps.transcripts);
    assert!(!caps.consensus_estimates);
    assert!(!caps.event_news);
}

#[tokio::test]
async fn preflight_writes_required_coverage_inputs_in_fixed_order() {
    let ctx = run_preflight("MSFT", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    let json: String = ctx
        .get(KEY_REQUIRED_COVERAGE_INPUTS)
        .await
        .expect("required_coverage_inputs key must be present");
    let inputs: Vec<String> =
        serde_json::from_str(&json).expect("required_coverage_inputs deserialization");
    assert_eq!(
        inputs,
        vec!["fundamentals", "sentiment", "news", "technical"]
    );
}

#[tokio::test]
async fn preflight_seeds_cached_transcript_as_null_placeholder() {
    let ctx = run_preflight("TSLA", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    let raw: String = ctx
        .get(KEY_CACHED_TRANSCRIPT)
        .await
        .expect("cached_transcript must be present");
    assert_eq!(raw, "null", "Stage 1 value must be the JSON literal 'null'");
}

#[tokio::test]
async fn preflight_seeds_cached_consensus_as_null_placeholder() {
    let ctx = run_preflight("TSLA", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    let raw: String = ctx
        .get(KEY_CACHED_CONSENSUS)
        .await
        .expect("cached_consensus must be present");
    assert_eq!(raw, "null");
}

#[tokio::test]
async fn preflight_seeds_cached_event_feed_as_null_placeholder() {
    let ctx = run_preflight("TSLA", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    let raw: String = ctx
        .get(KEY_CACHED_EVENT_FEED)
        .await
        .expect("cached_event_feed must be present");
    assert_eq!(raw, "null");
}

#[tokio::test]
async fn preflight_all_six_context_keys_present_after_run() {
    let ctx = run_preflight("BRK.B", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    for key in [
        KEY_RESOLVED_INSTRUMENT,
        KEY_PROVIDER_CAPABILITIES,
        KEY_REQUIRED_COVERAGE_INPUTS,
        KEY_CACHED_TRANSCRIPT,
        KEY_CACHED_CONSENSUS,
        KEY_CACHED_EVENT_FEED,
    ] {
        let val: Option<String> = ctx.get(key).await;
        assert!(
            val.is_some(),
            "context key '{key}' must be present after preflight"
        );
    }
}
