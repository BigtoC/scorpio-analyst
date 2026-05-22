use graph_flow::Context;

use crate::{
    data::adapters::transcripts::TranscriptFetch,
    error::TradingError,
    state::AgentTokenUsage,
    workflow::context_bridge::{read_prefixed_result, write_prefixed_result},
};

pub(super) const DEBATE_USAGE_PREFIX: &str = "usage.debate";
pub(super) const RISK_USAGE_PREFIX: &str = "usage.risk";

/// Context key for the maximum number of researcher debate rounds.
pub const KEY_MAX_DEBATE_ROUNDS: &str = "max_debate_rounds";
/// Context key for the maximum number of risk discussion rounds.
pub const KEY_MAX_RISK_ROUNDS: &str = "max_risk_rounds";
/// Context key for the current researcher debate round counter.
pub const KEY_DEBATE_ROUND: &str = "debate_round";
/// Context key for the current risk discussion round counter.
pub const KEY_RISK_ROUND: &str = "risk_round";
/// Context key for pre-fetched news data shared between Sentiment and News analysts.
pub const KEY_CACHED_NEWS: &str = "analyst.cached_news";

// ── Stage 1 preflight context keys ──────────────────────────────────────────

/// Context key for the [`ResolvedInstrument`] written by [`PreflightTask`].
///
/// Value: JSON-serialised [`crate::data::ResolvedInstrument`].
pub const KEY_RESOLVED_INSTRUMENT: &str = "resolved_instrument";

/// Context key for [`ProviderCapabilities`] written by [`PreflightTask`].
///
/// Value: JSON-serialised [`crate::data::adapters::ProviderCapabilities`].
pub const KEY_PROVIDER_CAPABILITIES: &str = "provider_capabilities";

/// Context key for the ordered list of required coverage inputs written by
/// [`PreflightTask`].
///
/// Value: JSON array `["fundamentals", "sentiment", "news", "technical"]`.
pub const KEY_REQUIRED_COVERAGE_INPUTS: &str = "required_coverage_inputs";

/// Context key for the serde-serialized
/// [`TranscriptFetch`](crate::data::adapters::transcripts::TranscriptFetch)
/// enum (JSON string).
///
/// Always present after preflight; preflight seeds it to the serialized form
/// of `TranscriptFetch::Unavailable`. Consumers MUST deserialize back to
/// `TranscriptFetch` and pattern-match — never compare raw string contents.
pub const KEY_TRANSCRIPT_FETCH_STATUS: &str = "transcript_fetch_status";

/// Context key for the optional cached consensus-estimates payload.
///
/// Value: JSON-serialised `Option<ConsensusEvidence>` — always present after
/// preflight.  Stage 1 value is the JSON literal `null`.
pub const KEY_CACHED_CONSENSUS: &str = "cached_consensus";

/// Context key for the optional cached event-news payload.
///
/// Value: JSON-serialised `Option<EventNewsEvidence>` — always present after
/// preflight.  Stage 1 value is the JSON literal `null`.
pub const KEY_CACHED_EVENT_FEED: &str = "cached_event_feed";

/// Context key for the pack-derived [`RuntimePolicy`] written by [`PreflightTask`].
///
/// Value: JSON-serialized [`crate::analysis_packs::RuntimePolicy`].
pub const KEY_RUNTIME_POLICY: &str = "runtime_policy";

/// Context key for the per-run routing decisions written by [`PreflightTask`].
///
/// Value: JSON-serialized [`crate::workflow::topology::RoutingFlags`].
/// Replaces the raw `KEY_MAX_DEBATE_ROUNDS` / `KEY_MAX_RISK_ROUNDS` reads in
/// builder closures — *entry* into the debate and risk stages is governed by
/// these flags. Loop-back conditionals (`round < max`) keep using the
/// per-iteration counters.
pub const KEY_ROUTING_FLAGS: &str = "routing_flags";

/// Context key for the runtime pack route chosen by preflight after
/// [`crate::workflow::classify_runtime_pack`] runs.
///
/// Value: lower-snake-case pack id (`"baseline"`, `"etf_baseline"`). Absent
/// before preflight; always present once preflight has emitted runtime policy.
///
/// Consumed by Task 12 (preflight routing wiring); declared now so the
/// classifier wiring can land before that task without churn.
#[allow(dead_code)]
pub const KEY_RUNTIME_PACK_ROUTE: &str = "routing.pack";

/// Context key for the routing fallback reason when classification fell back
/// from an ETF-oriented route to baseline.
///
/// Value: the `&'static str` reason carried by
/// [`crate::workflow::RuntimePackSelection::BaselineFallback`] (currently one
/// of `"profile_lookup_unavailable"`, `"unsupported_fund_shape"`). Absent
/// when the run did not fall back (matched-baseline or ETF-baseline routes).
///
/// Consumed by Task 12 (preflight routing wiring); declared now so the
/// classifier wiring can land before that task without churn.
#[allow(dead_code)]
pub const KEY_ROUTING_FALLBACK_REASON: &str = "routing.fallback_reason";

pub(super) const ANALYST_PREFIX: &str = "analyst";
pub(super) const OK_SUFFIX: &str = "ok";
pub(super) const ERR_SUFFIX: &str = "err";

pub(super) const ANALYST_FUNDAMENTAL: &str = "fundamental";
pub(super) const ANALYST_SENTIMENT: &str = "sentiment";
pub(super) const ANALYST_NEWS: &str = "news";
pub(super) const ANALYST_TECHNICAL: &str = "technical";

pub(super) async fn write_flag(context: &Context, analyst_key: &str, ok: bool) {
    context
        .set(format!("{ANALYST_PREFIX}.{analyst_key}.{OK_SUFFIX}"), ok)
        .await;
}

pub(super) async fn write_err(context: &Context, analyst_key: &str, message: &str) {
    context
        .set(
            format!("{ANALYST_PREFIX}.{analyst_key}.{ERR_SUFFIX}"),
            message.to_owned(),
        )
        .await;
}

pub(super) async fn write_analyst_usage(
    context: &Context,
    analyst_key: &str,
    usage: &AgentTokenUsage,
) -> Result<(), TradingError> {
    write_prefixed_result(context, "usage.analyst", analyst_key, usage).await
}

pub(super) async fn read_analyst_usage(
    context: &Context,
    analyst_key: &str,
    agent_name: &str,
) -> AgentTokenUsage {
    match read_prefixed_result::<AgentTokenUsage>(context, "usage.analyst", analyst_key).await {
        Ok(usage) => usage,
        Err(_) => AgentTokenUsage::unavailable(agent_name, "unknown", 0),
    }
}

pub(super) async fn write_round_usage(
    context: &Context,
    prefix: &str,
    round: u32,
    role: &str,
    usage: &AgentTokenUsage,
) -> Result<(), TradingError> {
    let round_prefix = format!("{prefix}.{round}");
    write_prefixed_result(context, &round_prefix, role, usage).await
}

pub(super) async fn read_round_usage(
    context: &Context,
    prefix: &str,
    round: u32,
    role: &str,
    agent_name: &str,
) -> AgentTokenUsage {
    let round_prefix = format!("{prefix}.{round}");
    match read_prefixed_result::<AgentTokenUsage>(context, &round_prefix, role).await {
        Ok(usage) => usage,
        Err(_) => AgentTokenUsage::unavailable(agent_name, "unknown", 0),
    }
}

pub(super) async fn load_transcript_fetch(
    context: &Context,
) -> Result<TranscriptFetch, TradingError> {
    let raw: String = context
        .get(KEY_TRANSCRIPT_FETCH_STATUS)
        .await
        .ok_or_else(|| TradingError::SchemaViolation {
            message: format!("context missing required key '{KEY_TRANSCRIPT_FETCH_STATUS}'"),
        })?;

    serde_json::from_str(&raw).map_err(|error| TradingError::SchemaViolation {
        message: format!(
            "failed to deserialize {KEY_TRANSCRIPT_FETCH_STATUS} as TranscriptFetch: {error}"
        ),
    })
}

#[cfg(test)]
mod tests {
    use graph_flow::Context;

    use super::{KEY_TRANSCRIPT_FETCH_STATUS, load_transcript_fetch};
    use crate::data::adapters::transcripts::TranscriptFetch;

    #[tokio::test]
    async fn load_transcript_fetch_reads_serialized_status_from_context() {
        let context = Context::new();
        let status = TranscriptFetch::Unavailable;
        let raw = serde_json::to_string(&status).expect("status serialization");
        context.set(KEY_TRANSCRIPT_FETCH_STATUS, raw).await;

        let loaded = load_transcript_fetch(&context)
            .await
            .expect("transcript status should deserialize");

        assert_eq!(loaded, status);
    }

    #[tokio::test]
    async fn load_transcript_fetch_fails_when_context_key_is_missing() {
        let context = Context::new();

        let error = load_transcript_fetch(&context)
            .await
            .expect_err("missing transcript status should fail");

        assert_eq!(
            error.to_string(),
            format!(
                "schema violation: context missing required key '{KEY_TRANSCRIPT_FETCH_STATUS}'"
            )
        );
    }

    #[tokio::test]
    async fn load_transcript_fetch_fails_when_context_value_is_invalid_json() {
        let context = Context::new();
        context
            .set(KEY_TRANSCRIPT_FETCH_STATUS, "not-json".to_owned())
            .await;

        let error = load_transcript_fetch(&context)
            .await
            .expect_err("invalid transcript status should fail");

        assert!(
            error
                .to_string()
                .contains("failed to deserialize transcript_fetch_status as TranscriptFetch"),
            "unexpected error: {error}"
        );
    }
}
