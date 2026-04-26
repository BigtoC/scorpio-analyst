//! Sentiment Analyst agent.
//!
//! Binds a Finnhub news tool (`get_news`) to a quick-thinking LLM agent so
//! the model can fetch recent company news during inference and return a
//! structured [`SentimentData`] JSON object by inferring sentiment from news
//! content.
//!
//! **MVP constraint:** no direct social-platform access (Reddit, X/Twitter,
//! StockTwits). Sentiment is derived solely from news articles.

use std::sync::Arc;
use std::time::Instant;

use rig::tool::ToolDyn;

use crate::{
    agents::shared::{
        agent_token_usage_from_completion, build_authoritative_source_prompt_rule,
        build_data_quality_prompt_rule, build_missing_data_prompt_rule, sanitize_prompt_context,
    },
    analysis_packs::RuntimePolicy,
    config::LlmConfig,
    constants::SENTIMENT_ANALYST_MAX_TURNS,
    data::{FinnhubClient, GetCachedNews, GetNews},
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent_with_tools},
    state::{AgentTokenUsage, NewsData, SentimentData, TradingState},
};

use super::common::{analyst_runtime_config, run_analyst_inference, validate_summary_content};

/// Build the rendered system prompt for the Sentiment Analyst.
///
/// Reads the role's prompt template directly from the active pack's
/// `RuntimePolicy.prompt_bundle.sentiment_analyst` slot and substitutes
/// runtime placeholders, then appends the three shared evidence-discipline
/// rule helpers plus analyst-specific unsupported-inference guards.
/// Preflight's completeness gate ensures the slot is non-empty.
pub(crate) fn build_sentiment_system_prompt(
    symbol: &str,
    target_date: &str,
    policy: &RuntimePolicy,
) -> String {
    let analysis_emphasis = sanitize_prompt_context(&policy.analysis_emphasis);
    let base = policy.prompt_bundle.sentiment_analyst.as_ref();

    format!(
        "{base}\n\n{auth_rule}\n{missing_rule}\n{quality_rule}\n\
Do not infer estimates, transcript commentary, or quarter labels unless the runtime provides them.\n\
If evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims.\n\
Separate observed facts from interpretation.",
        base = base
            .replace("{ticker}", symbol)
            .replace("{current_date}", target_date)
            .replace("{analysis_emphasis}", &analysis_emphasis),
        auth_rule = build_authoritative_source_prompt_rule(),
        missing_rule = build_missing_data_prompt_rule(),
        quality_rule = build_data_quality_prompt_rule(),
    )
}

/// The Sentiment Analyst agent.
///
/// Binds a Finnhub news tool to the LLM so it can fetch news during inference
/// and return a structured [`SentimentData`] output.
pub struct SentimentAnalyst {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    symbol: String,
    target_date: String,
    system_prompt: String,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
    /// Pre-fetched news from `run_analyst_team`.  When `Some`, a
    /// [`GetCachedNews`] tool is bound instead of the live [`GetNews`] tool,
    /// saving one Finnhub API call per cycle.
    cached_news: Option<Arc<NewsData>>,
}

impl SentimentAnalyst {
    /// Construct a new `SentimentAnalyst`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (`QuickThinking` tier).
    /// - `finnhub` – Finnhub client used to fetch news articles.
    /// - `state` – current trading state, including any active runtime policy.
    /// - `llm_config` – LLM configuration, used for timeout.
    /// - `cached_news` – optional pre-fetched news; when `Some`, the live
    ///   [`GetNews`] tool is replaced with a zero-cost [`GetCachedNews`] tool.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        state: &TradingState,
        policy: &RuntimePolicy,
        llm_config: &LlmConfig,
        cached_news: Option<Arc<NewsData>>,
    ) -> Self {
        let runtime = analyst_runtime_config(&state.asset_symbol, &state.target_date, llm_config);
        let system_prompt =
            build_sentiment_system_prompt(&runtime.symbol, &runtime.target_date, policy);

        Self {
            handle,
            finnhub,
            symbol: runtime.symbol,
            target_date: runtime.target_date,
            system_prompt,
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
            cached_news,
        }
    }

    /// Run the analyst: bind Finnhub news tool to the LLM, prompt it, parse and return output.
    ///
    /// When no news articles are available the agent still succeeds, producing
    /// a neutral [`SentimentData`] with `overall_score: 0.0` and empty arrays.
    ///
    /// # Errors
    ///
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the LLM returns malformed JSON.
    pub async fn run(&self) -> Result<(SentimentData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let tools: Vec<Box<dyn ToolDyn>> = match &self.cached_news {
            Some(arc) => vec![Box::new(GetCachedNews::new(
                arc.clone(),
                self.symbol.clone(),
            ))],
            None => vec![Box::new(GetNews::scoped(
                self.finnhub.clone(),
                self.symbol.clone(),
            ))],
        };

        // ── 2. Build agent with tools and invoke LLM ──────────────────────
        let agent = build_agent_with_tools(&self.handle, &self.system_prompt, tools);

        let prompt = format!(
            "Fetch and analyse recent news for {} as of {} using the available tools, \
             then produce a SentimentData JSON object.",
            self.symbol, self.target_date
        );

        let outcome = run_analyst_inference(
            &agent,
            &prompt,
            self.timeout,
            &self.retry_policy,
            SENTIMENT_ANALYST_MAX_TURNS,
            parse_sentiment,
            validate_sentiment,
        )
        .await?;

        let usage = agent_token_usage_from_completion(
            "Sentiment Analyst",
            self.handle.model_id(),
            outcome.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );

        Ok((outcome.output, usage))
    }
}

fn validate_sentiment(data: &SentimentData) -> Result<(), TradingError> {
    if !(-1.0..=1.0).contains(&data.overall_score) {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "SentimentAnalyst: overall_score {} must be within [-1.0, 1.0]",
                data.overall_score
            ),
        });
    }
    if data.summary.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "SentimentAnalyst: summary must not be empty".to_owned(),
        });
    }
    validate_summary_content("SentimentAnalyst", &data.summary)?;
    for source in &data.source_breakdown {
        if !(-1.0..=1.0).contains(&source.score) {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "SentimentAnalyst: source score {} must be within [-1.0, 1.0]",
                    source.score
                ),
            });
        }
    }
    Ok(())
}

/// Deserialize a JSON string into [`SentimentData`], mapping errors to
/// [`TradingError::SchemaViolation`].
///
/// Exposed for use as the `parse` hook in `run_analyst_inference`.
pub(crate) fn parse_sentiment(json_str: &str) -> Result<SentimentData, TradingError> {
    serde_json::from_str(json_str).map_err(|e| TradingError::SchemaViolation {
        message: format!("SentimentAnalyst: failed to parse LLM output: {e}"),
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{EngagementPeak, SentimentData, SentimentSource};

    /// Parse and validate a JSON string — combines `parse_sentiment` + `validate_sentiment`
    /// for test convenience. Tests that need only structural parsing can call `parse_sentiment`
    /// directly; tests that also exercise the semantic validation layer call this helper.
    fn parse_and_validate(json: &str) -> Result<SentimentData, TradingError> {
        parse_sentiment(json).and_then(|data| validate_sentiment(&data).map(|()| data))
    }

    // ── Task 2.4: Correct SentimentData extraction from news inputs ───────

    #[test]
    fn parses_valid_sentiment_json() {
        let json = r#"{
            "overall_score": 0.6,
            "source_breakdown": [
                {
                    "source_name": "Finnhub News",
                    "score": 0.6,
                    "sample_size": 12
                }
            ],
            "engagement_peaks": [],
            "summary": "Recent news skews bullish with positive earnings coverage."
        }"#;

        let data = parse_and_validate(json).expect("should parse valid JSON");
        assert!((data.overall_score - 0.6).abs() < 1e-9);
        assert_eq!(data.source_breakdown.len(), 1);
        assert_eq!(data.source_breakdown[0].source_name, "Finnhub News");
        assert_eq!(data.source_breakdown[0].sample_size, 12);
        assert!(data.engagement_peaks.is_empty());
        assert!(!data.summary.is_empty());
    }

    #[test]
    fn parses_neutral_sentiment_with_empty_arrays() {
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [],
            "engagement_peaks": [],
            "summary": "Sentiment signal is weak or unavailable."
        }"#;

        let data = parse_and_validate(json).expect("should parse neutral");
        assert_eq!(data.overall_score, 0.0);
        assert!(data.source_breakdown.is_empty());
        assert!(data.engagement_peaks.is_empty());
    }

    // ── Task 2.5: Agent does not attempt social-platform access ──────────

    fn baseline_sentiment_prompt() -> &'static str {
        crate::testing::baseline_pack_prompt_for_role(crate::workflow::Role::SentimentAnalyst)
    }

    #[test]
    fn system_prompt_forbids_social_platforms() {
        // Drift-detection guard against the canonical runtime source — the
        // baseline pack's `PromptBundle.sentiment_analyst` slot.
        let prompt = baseline_sentiment_prompt();
        assert!(
            prompt.contains("Reddit"),
            "prompt should mention Reddit constraint"
        );
        assert!(
            prompt.contains("X/Twitter"),
            "prompt should mention X/Twitter constraint"
        );
        assert!(
            prompt.contains("StockTwits"),
            "prompt should mention StockTwits constraint"
        );
        assert!(
            prompt.contains("Do not assume"),
            "prompt should say 'Do not assume'"
        );
    }

    // ── Task 2.6: Empty news input produces valid neutral SentimentData ───

    #[test]
    fn empty_news_produces_valid_neutral_sentiment() {
        // When no news is available, the LLM should return this shape.
        // We verify that such a response parses correctly.
        let neutral_json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [],
            "engagement_peaks": [],
            "summary": "No news articles available for sentiment analysis."
        }"#;

        let data = parse_and_validate(neutral_json).expect("neutral output should parse");
        assert_eq!(data.overall_score, 0.0);
        assert!(data.source_breakdown.is_empty());
        assert!(data.engagement_peaks.is_empty());
        assert!(data.summary.contains("No news"));
    }

    // ── Task 2.5 (continued): AgentTokenUsage recording ──────────────────

    #[test]
    fn agent_token_usage_fields() {
        let usage = AgentTokenUsage {
            agent_name: "Sentiment Analyst".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 180,
            rate_limit_wait_ms: 0,
        };
        assert_eq!(usage.agent_name, "Sentiment Analyst");
        assert_eq!(usage.model_id, "gpt-4o-mini");
    }

    // ── SchemaViolation on malformed JSON ─────────────────────────────────

    #[test]
    fn malformed_json_returns_schema_violation() {
        let result = parse_sentiment("{ not valid json");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn json_missing_required_field_returns_schema_violation() {
        // `summary` is required — omitting it should fail
        let result = parse_sentiment(r#"{"overall_score": 0.3}"#);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn extra_fields_in_json_are_rejected() {
        // deny_unknown_fields must reject any unknown key
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [],
            "engagement_peaks": [],
            "summary": "ok",
            "unexpected_field": "should fail"
        }"#;
        let result = parse_sentiment(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn overall_score_out_of_range_returns_schema_violation() {
        let json = r#"{"overall_score": 2.5, "source_breakdown": [], "engagement_peaks": [], "summary": "x"}"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn source_score_out_of_range_returns_schema_violation() {
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [{"source_name": "x", "score": 1.5, "sample_size": 1}],
            "engagement_peaks": [],
            "summary": "x"
        }"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn whitespace_only_summary_returns_schema_violation() {
        let json = r#"{"overall_score": 0.0, "source_breakdown": [], "engagement_peaks": [], "summary": "   "}"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Struct round-trip ─────────────────────────────────────────────────

    #[test]
    fn sentiment_data_round_trips_through_json() {
        let original = SentimentData {
            overall_score: 0.3,
            source_breakdown: vec![SentimentSource {
                source_name: "Finnhub News".to_owned(),
                score: 0.3,
                sample_size: 5,
            }],
            engagement_peaks: vec![EngagementPeak {
                timestamp: "2026-03-10T12:00:00Z".to_owned(),
                platform: "news".to_owned(),
                intensity: 0.7,
            }],
            summary: "Mildly bullish.".to_owned(),
        };

        let serialized = serde_json::to_string(&original).expect("serialise");
        let roundtripped: SentimentData = serde_json::from_str(&serialized).expect("deserialise");
        assert_eq!(original, roundtripped);
    }

    // TC-9: overall_score at positive boundary 1.0 is valid
    #[test]
    fn overall_score_at_positive_boundary_is_valid() {
        let json = r#"{"overall_score": 1.0, "source_breakdown": [], "engagement_peaks": [], "summary": "boundary"}"#;
        assert!(
            parse_and_validate(json).is_ok(),
            "overall_score = 1.0 must be accepted (inclusive upper bound)"
        );
    }

    // TC-9: overall_score at negative boundary -1.0 is valid
    #[test]
    fn overall_score_at_negative_boundary_is_valid() {
        let json = r#"{"overall_score": -1.0, "source_breakdown": [], "engagement_peaks": [], "summary": "boundary"}"#;
        assert!(
            parse_and_validate(json).is_ok(),
            "overall_score = -1.0 must be accepted (inclusive lower bound)"
        );
    }

    // TC-10: source score at positive boundary 1.0 is valid
    #[test]
    fn source_score_at_positive_boundary_is_valid() {
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [{"source_name": "x", "score": 1.0, "sample_size": 1}],
            "engagement_peaks": [],
            "summary": "boundary"
        }"#;
        assert!(
            parse_and_validate(json).is_ok(),
            "source score = 1.0 must be accepted (inclusive upper bound)"
        );
    }

    // TC-10: source score at negative boundary -1.0 is valid
    #[test]
    fn source_score_at_negative_boundary_is_valid() {
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [{"source_name": "x", "score": -1.0, "sample_size": 1}],
            "engagement_peaks": [],
            "summary": "boundary"
        }"#;
        assert!(
            parse_and_validate(json).is_ok(),
            "source score = -1.0 must be accepted (inclusive lower bound)"
        );
    }

    // TC-18: SentimentSource rejects extra fields
    #[test]
    fn sentiment_source_extra_fields_rejected() {
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [{"source_name": "x", "score": 0.0, "sample_size": 1, "extra": "bad"}],
            "engagement_peaks": [],
            "summary": "should fail"
        }"#;
        let result = parse_sentiment(json);
        assert!(
            result.is_err(),
            "extra field inside SentimentSource should be rejected"
        );
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // TC-18: EngagementPeak rejects extra fields
    #[test]
    fn engagement_peak_extra_fields_rejected() {
        let json = r#"{
            "overall_score": 0.0,
            "source_breakdown": [],
            "engagement_peaks": [{"timestamp": "2026-01-01T00:00:00Z", "platform": "news", "intensity": 0.5, "extra": "bad"}],
            "summary": "should fail"
        }"#;
        let result = parse_sentiment(json);
        assert!(
            result.is_err(),
            "extra field inside EngagementPeak should be rejected"
        );
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Task 5: Migrate to shared inference helper ────────────────────────

    #[test]
    fn sentiment_prompt_states_get_news_argument_shape() {
        let prompt = baseline_sentiment_prompt();
        assert!(
            prompt.contains(r#"get_news requires {"symbol":"<ticker>"}"#),
            "baseline sentiment prompt must contain 'get_news requires {{\"symbol\":\"<ticker>\"}}'"
        );
    }

    #[test]
    fn sentiment_prompt_requires_exactly_one_json_object_response() {
        let prompt = baseline_sentiment_prompt();
        assert!(
            prompt.contains("exactly one JSON object"),
            "baseline sentiment prompt must contain 'exactly one JSON object'"
        );
        assert!(
            prompt.contains("no prose"),
            "baseline sentiment prompt must contain 'no prose'"
        );
        assert!(
            prompt.contains("no markdown fences"),
            "baseline sentiment prompt must contain 'no markdown fences'"
        );
    }

    #[test]
    fn source_alias_accepted_in_sentiment_source() {
        // LLMs sometimes return "source" instead of "source_name".
        // The alias should allow either field name.
        let json = r#"{
            "overall_score": 0.3,
            "source_breakdown": [
                {"source": "Finnhub News", "score": 0.3, "sample_size": 5}
            ],
            "engagement_peaks": [],
            "summary": "Mildly bullish."
        }"#;

        let data = parse_and_validate(json).expect("'source' alias should be accepted");
        assert_eq!(data.source_breakdown[0].source_name, "Finnhub News");
    }

    #[test]
    fn parse_sentiment_rejects_unknown_fields() {
        let result = parse_sentiment(r#"{"unknown_field": 1}"#);
        assert!(
            matches!(result, Err(TradingError::SchemaViolation { .. })),
            "parse_sentiment should return SchemaViolation for unknown fields"
        );
    }

    #[tokio::test]
    async fn sentiment_run_uses_shared_inference_helper_for_openrouter() {
        use super::super::common::run_analyst_inference;
        use crate::providers::ProviderId;
        use crate::providers::factory::agent_test_support;
        use rig::agent::PromptResponse;
        use rig::completion::Usage;

        let valid_json = r#"{
            "overall_score": 0.5,
            "source_breakdown": [
                {"source_name": "Finnhub News", "score": 0.5, "sample_size": 5}
            ],
            "engagement_peaks": [],
            "summary": "Moderately bullish based on recent news."
        }"#;

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenRouter,
            "openrouter-model",
            vec![],
            vec![],
        );
        agent.push_text_turn_ok(PromptResponse::new(
            valid_json,
            Usage {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                cached_input_tokens: 0,
            },
        ));

        let outcome = run_analyst_inference(
            &agent,
            "analyse AAPL sentiment",
            std::time::Duration::from_millis(100),
            &crate::error::RetryPolicy {
                max_retries: 0,
                base_delay: std::time::Duration::from_millis(1),
            },
            1,
            parse_sentiment,
            validate_sentiment,
        )
        .await
        .expect("inference should succeed");

        let _ = outcome.output;

        assert_eq!(agent_test_support::typed_attempts(&agent), 0);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
        assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
    }

    // ── Task chunk1: Rendered-prompt evidence-discipline hardening ─────────

    #[test]
    fn sentiment_rendered_prompt_includes_evidence_discipline_rules() {
        use crate::agents::shared::{
            build_authoritative_source_prompt_rule, build_data_quality_prompt_rule,
            build_missing_data_prompt_rule,
        };
        use crate::analysis_packs::resolve_runtime_policy;

        let policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        let prompt = build_sentiment_system_prompt("AAPL", "2026-01-01", &policy);

        assert!(
            prompt.contains(build_authoritative_source_prompt_rule()),
            "rendered prompt must contain authoritative source rule"
        );
        assert!(
            prompt.contains(build_missing_data_prompt_rule()),
            "rendered prompt must contain missing data rule"
        );
        assert!(
            prompt.contains(build_data_quality_prompt_rule()),
            "rendered prompt must contain data quality rule"
        );
        assert!(
            prompt.contains("Do not infer estimates"),
            "rendered prompt must contain 'Do not infer estimates'"
        );
        assert!(
            prompt.contains("sparse or missing"),
            "rendered prompt must contain 'sparse or missing'"
        );
        assert!(
            prompt.contains("Separate observed facts"),
            "rendered prompt must contain 'Separate observed facts'"
        );
    }

    #[test]
    fn sentiment_rendered_prompt_uses_runtime_policy_prompt_bundle() {
        use crate::{
            agents::shared::{
                build_authoritative_source_prompt_rule, build_data_quality_prompt_rule,
                build_missing_data_prompt_rule,
            },
            analysis_packs::resolve_runtime_policy,
        };

        let mut policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        policy.analysis_emphasis = "weight narrative skew".to_owned();
        policy.prompt_bundle.sentiment_analyst =
            "Pack sentiment prompt for {ticker} at {current_date}. Emphasis: {analysis_emphasis}."
                .into();

        let prompt = build_sentiment_system_prompt("AAPL", "2026-01-01", &policy);

        assert!(
            prompt.contains(
                "Pack sentiment prompt for AAPL at 2026-01-01. Emphasis: weight narrative skew."
            ),
            "runtime-policy prompt bundle should drive the sentiment template: {prompt}"
        );
        assert!(
            prompt.contains(build_authoritative_source_prompt_rule())
                && prompt.contains(build_missing_data_prompt_rule())
                && prompt.contains(build_data_quality_prompt_rule()),
            "evidence-discipline rules must still be appended after prompt-bundle rendering: {prompt}"
        );
    }
}
