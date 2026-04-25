//! News Analyst agent.
//!
//! Binds a Finnhub news tool (`get_news`) to a quick-thinking LLM agent so
//! the model can fetch recent company news during inference and return a
//! structured [`NewsData`] JSON object capturing the most relevant articles
//! and macro events.

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
    constants::NEWS_ANALYST_MAX_TURNS,
    data::{
        FinnhubClient, FredClient, GetCachedNews, GetEconomicIndicators, GetMarketNews, GetNews,
    },
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent_with_tools},
    state::{AgentTokenUsage, NewsData, TradingState},
};

use super::common::{analyst_runtime_config, run_analyst_inference, validate_summary_content};
use super::prompt::NEWS_SYSTEM_PROMPT;

/// Build the rendered system prompt for the News Analyst.
///
/// Applies `{ticker}` / `{current_date}` substitution and appends the three shared
/// evidence-discipline rule helpers plus analyst-specific unsupported-inference guards.
fn news_system_prompt_template(policy: Option<&RuntimePolicy>) -> &str {
    policy
        .map(|policy| policy.prompt_bundle.news_analyst.as_ref())
        .filter(|template| !template.is_empty())
        .unwrap_or(NEWS_SYSTEM_PROMPT)
}

pub(crate) fn build_news_system_prompt(
    symbol: &str,
    target_date: &str,
    policy: Option<&RuntimePolicy>,
) -> String {
    let analysis_emphasis = policy
        .map(|policy| sanitize_prompt_context(&policy.analysis_emphasis))
        .unwrap_or_default();

    format!(
        "{base}\n\n{auth_rule}\n{missing_rule}\n{quality_rule}\n\
Do not infer estimates, transcript commentary, or quarter labels unless the runtime provides them.\n\
If evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims.\n\
Separate observed facts from interpretation.",
        base = news_system_prompt_template(policy)
            .replace("{ticker}", symbol)
            .replace("{current_date}", target_date)
            .replace("{analysis_emphasis}", &analysis_emphasis),
        auth_rule = build_authoritative_source_prompt_rule(),
        missing_rule = build_missing_data_prompt_rule(),
        quality_rule = build_data_quality_prompt_rule(),
    )
}

/// The News Analyst agent.
///
/// Binds a Finnhub news tool to the LLM so it can fetch news during inference
/// and return a structured [`NewsData`] output highlighting causal
/// relationships and macro events.
pub struct NewsAnalyst {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    fred: FredClient,
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

impl NewsAnalyst {
    /// Construct a new `NewsAnalyst`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (`QuickThinking` tier).
    /// - `finnhub` – Finnhub client used to fetch news articles.
    /// - `symbol` – asset ticker symbol.
    /// - `state` – current trading state, including any active runtime policy.
    /// - `llm_config` – LLM configuration, used for timeout.
    /// - `cached_news` – optional pre-fetched news; when `Some`, the live
    ///   [`GetNews`] tool is replaced with a zero-cost [`GetCachedNews`] tool.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        fred: FredClient,
        state: &TradingState,
        llm_config: &LlmConfig,
        cached_news: Option<Arc<NewsData>>,
    ) -> Self {
        let runtime = analyst_runtime_config(&state.asset_symbol, &state.target_date, llm_config);
        let system_prompt = build_news_system_prompt(
            &runtime.symbol,
            &runtime.target_date,
            state.analysis_runtime_policy.as_ref(),
        );

        Self {
            handle,
            finnhub,
            fred,
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
    /// # Errors
    ///
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the LLM returns malformed JSON.
    pub async fn run(&self) -> Result<(NewsData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let news_tool: Box<dyn ToolDyn> = match &self.cached_news {
            Some(arc) => Box::new(GetCachedNews::new(arc.clone(), self.symbol.clone())),
            None => Box::new(GetNews::scoped(self.finnhub.clone(), self.symbol.clone())),
        };
        let tools: Vec<Box<dyn ToolDyn>> = vec![
            news_tool,
            Box::new(GetMarketNews::new(self.finnhub.clone())),
            Box::new(GetEconomicIndicators::new(self.fred.clone())),
        ];

        // ── 2. Build agent with tools and invoke LLM ──────────────────────
        let agent = build_agent_with_tools(&self.handle, &self.system_prompt, tools);

        let prompt = format!(
            "Analyse {} as of {}. Use get_news for company-specific developments, get_market_news for broader market context, and get_economic_indicators for macro data, then produce a NewsData JSON object.",
            self.symbol, self.target_date
        );

        let outcome = run_analyst_inference(
            &agent,
            &prompt,
            self.timeout,
            &self.retry_policy,
            NEWS_ANALYST_MAX_TURNS,
            parse_news,
            validate_news,
        )
        .await?;

        let usage = agent_token_usage_from_completion(
            "News Analyst",
            self.handle.model_id(),
            outcome.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );

        Ok((outcome.output, usage))
    }
}

fn validate_news(data: &NewsData) -> Result<(), TradingError> {
    if data.summary.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "NewsAnalyst: summary must not be empty".to_owned(),
        });
    }
    validate_summary_content("NewsAnalyst", &data.summary)?;
    for event in &data.macro_events {
        if !(0.0..=1.0).contains(&event.confidence) {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "NewsAnalyst: macro event confidence {} must be within [0.0, 1.0]",
                    event.confidence
                ),
            });
        }
    }
    Ok(())
}

/// Deserialize a JSON string into [`NewsData`], mapping errors to
/// [`TradingError::SchemaViolation`].
///
/// Exposed for use as the `parse` hook in `run_analyst_inference`.
pub(crate) fn parse_news(json_str: &str) -> Result<NewsData, TradingError> {
    serde_json::from_str(json_str).map_err(|e| TradingError::SchemaViolation {
        message: format!("NewsAnalyst: failed to parse LLM output: {e}"),
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ImpactDirection, MacroEvent, NewsArticle, NewsData};

    /// Parse and validate a JSON string — combines `parse_news` + `validate_news`
    /// for test convenience.  Tests that need only structural parsing can call `parse_news`
    /// directly; tests that also exercise the semantic validation layer call this helper.
    fn parse_and_validate(json: &str) -> Result<NewsData, TradingError> {
        parse_news(json).and_then(|data| validate_news(&data).map(|()| data))
    }

    // ── Task 3.4: Correct NewsData extraction with causal relationships ───

    #[test]
    fn parses_valid_news_json() {
        let json = r#"{
            "articles": [
                {
                    "title": "Apple Reports Record Revenue",
                    "source": "Reuters",
                    "published_at": "2026-03-10T10:00:00Z",
                    "relevance_score": 0.95,
                    "snippet": "Apple Inc. reported record quarterly revenue beating analyst expectations."
                }
            ],
            "macro_events": [
                {
                    "event": "Interest-rate policy shift",
                    "impact_direction": "positive",
                    "confidence": 0.8
                }
            ],
            "summary": "Strong earnings beat with favorable rate environment."
        }"#;

        let data = parse_news(json).expect("should parse valid JSON");
        assert_eq!(data.articles.len(), 1);
        assert_eq!(data.articles[0].title, "Apple Reports Record Revenue");
        assert_eq!(data.articles[0].source, "Reuters");
        assert_eq!(data.macro_events.len(), 1);
        assert_eq!(
            data.macro_events[0].impact_direction,
            ImpactDirection::Positive
        );
        assert!(!data.summary.is_empty());
    }

    #[test]
    fn parses_news_with_empty_macro_events() {
        let json = r#"{
            "articles": [
                {
                    "title": "Routine Maintenance Update",
                    "source": "PR Newswire",
                    "published_at": "2026-03-12T09:00:00Z",
                    "relevance_score": null,
                    "snippet": "Scheduled maintenance window announced."
                }
            ],
            "macro_events": [],
            "summary": "No macro-level causal links identified."
        }"#;

        let data = parse_news(json).expect("should parse");
        assert_eq!(data.articles.len(), 1);
        assert!(data.articles[0].relevance_score.is_none());
        assert!(data.macro_events.is_empty());
    }

    #[test]
    fn parses_news_with_multiple_macro_events_and_causal_links() {
        let json = r#"{
            "articles": [],
            "macro_events": [
                {
                    "event": "Inflation signal",
                    "impact_direction": "negative",
                    "confidence": 0.7
                },
                {
                    "event": "Geopolitical trade pressure",
                    "impact_direction": "negative",
                    "confidence": 0.75
                }
            ],
            "summary": "Two macro headwinds identified from news coverage."
        }"#;

        let data = parse_news(json).expect("should parse");
        assert!(data.articles.is_empty());
        assert_eq!(data.macro_events.len(), 2);
        assert_eq!(data.macro_events[0].event, "Inflation signal");
        assert!((data.macro_events[1].confidence - 0.75).abs() < 1e-9);
    }

    // ── Task 3.5: AgentTokenUsage recording ──────────────────────────────

    #[test]
    fn agent_token_usage_fields() {
        let usage = AgentTokenUsage {
            agent_name: "News Analyst".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 220,
            rate_limit_wait_ms: 0,
        };
        assert_eq!(usage.agent_name, "News Analyst");
        assert_eq!(usage.model_id, "gpt-4o-mini");
        assert_eq!(usage.latency_ms, 220);
    }

    // ── SchemaViolation on malformed JSON ─────────────────────────────────

    #[test]
    fn malformed_json_returns_schema_violation() {
        let result = parse_news("this is not json");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn json_missing_required_field_returns_schema_violation() {
        // `summary` is required — omitting it should fail
        let result = parse_news(r#"{"articles": []}"#);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn extra_fields_in_json_are_rejected() {
        let json = r#"{
            "articles": [],
            "macro_events": [],
            "summary": "ok",
            "unexpected_field": "should fail"
        }"#;
        let result = parse_news(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn confidence_out_of_range_returns_schema_violation() {
        let json = r#"{
            "articles": [],
            "macro_events": [{"event": "test", "impact_direction": "positive", "confidence": 1.5}],
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
    fn negative_confidence_returns_schema_violation() {
        let json = r#"{
            "articles": [],
            "macro_events": [{"event": "test", "impact_direction": "negative", "confidence": -0.1}],
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
        let json = r#"{"articles": [], "macro_events": [], "summary": "  "}"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn json_with_wrong_types_returns_schema_violation() {
        // `confidence` should be f64, not a string
        let result = parse_news(
            r#"{
            "articles": [],
            "macro_events": [{"event": "test", "impact_direction": "positive", "confidence": "high"}],
            "summary": "test"
        }"#,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Struct round-trip ─────────────────────────────────────────────────

    #[test]
    fn news_data_round_trips_through_json() {
        let original = NewsData {
            articles: vec![NewsArticle {
                title: "Big News".to_owned(),
                source: "Bloomberg".to_owned(),
                published_at: "2026-03-10T12:00:00Z".to_owned(),
                relevance_score: Some(0.9),
                snippet: "A significant development occurred.".to_owned(),
            }],
            macro_events: vec![MacroEvent {
                event: "Fed rate decision".to_owned(),
                impact_direction: ImpactDirection::Mixed,
                confidence: 0.65,
            }],
            summary: "Notable earnings and rate news.".to_owned(),
        };

        let serialized = serde_json::to_string(&original).expect("serialise");
        let roundtripped: NewsData = serde_json::from_str(&serialized).expect("deserialise");
        assert_eq!(original, roundtripped);
    }

    // TC-4: ImpactDirection::Neutral parses from "neutral"
    #[test]
    fn impact_direction_neutral_parses() {
        let json = r#"{
            "articles": [],
            "macro_events": [{"event": "Sideways market", "impact_direction": "neutral", "confidence": 0.5}],
            "summary": "No directional signal."
        }"#;
        let data = parse_news(json).expect("should parse");
        assert_eq!(
            data.macro_events[0].impact_direction,
            ImpactDirection::Neutral
        );
    }

    // TC-4: ImpactDirection::Uncertain parses from "uncertain"
    #[test]
    fn impact_direction_uncertain_parses() {
        let json = r#"{
            "articles": [],
            "macro_events": [{"event": "Ambiguous policy", "impact_direction": "uncertain", "confidence": 0.4}],
            "summary": "Outcome is unclear."
        }"#;
        let data = parse_news(json).expect("should parse");
        assert_eq!(
            data.macro_events[0].impact_direction,
            ImpactDirection::Uncertain
        );
    }

    // TC-11: confidence at exact lower boundary 0.0 is valid
    #[test]
    fn confidence_at_zero_boundary_is_valid() {
        let json = r#"{
            "articles": [],
            "macro_events": [{"event": "test", "impact_direction": "neutral", "confidence": 0.0}],
            "summary": "boundary test"
        }"#;
        assert!(
            parse_news(json).is_ok(),
            "confidence = 0.0 must be accepted (inclusive lower bound)"
        );
    }

    // TC-11: confidence at exact upper boundary 1.0 is valid
    #[test]
    fn confidence_at_one_boundary_is_valid() {
        let json = r#"{
            "articles": [],
            "macro_events": [{"event": "test", "impact_direction": "positive", "confidence": 1.0}],
            "summary": "boundary test"
        }"#;
        assert!(
            parse_news(json).is_ok(),
            "confidence = 1.0 must be accepted (inclusive upper bound)"
        );
    }

    // TC-17: NewsArticle rejects extra fields (deny_unknown_fields)
    #[test]
    fn news_article_extra_fields_rejected() {
        let json = r#"{
            "articles": [
                {
                    "title": "Test", "source": "Reuters",
                    "published_at": "2026-03-14T00:00:00Z",
                    "relevance_score": null, "snippet": "ok",
                    "unexpected_field": "should fail"
                }
            ],
            "macro_events": [],
            "summary": "Should fail."
        }"#;
        let result = parse_news(json);
        assert!(
            result.is_err(),
            "extra field inside NewsArticle should be rejected"
        );
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Task 4: Migrate to shared inference helper ────────────────────────

    #[test]
    fn news_prompt_states_exact_tool_argument_shapes() {
        assert!(
            NEWS_SYSTEM_PROMPT.contains(r#"get_news requires {"symbol":"<ticker>"}"#),
            r#"NEWS_SYSTEM_PROMPT must contain 'get_news requires {{"symbol":"<ticker>"}}"#
        );
        assert!(
            NEWS_SYSTEM_PROMPT.contains("get_market_news takes {}"),
            "NEWS_SYSTEM_PROMPT must contain 'get_market_news takes {{}}'"
        );
        assert!(
            NEWS_SYSTEM_PROMPT.contains("get_economic_indicators takes {}"),
            "NEWS_SYSTEM_PROMPT must contain 'get_economic_indicators takes {{}}'"
        );
    }

    #[test]
    fn news_prompt_requires_exactly_one_json_object_response() {
        assert!(
            NEWS_SYSTEM_PROMPT.contains("exactly one JSON object"),
            "NEWS_SYSTEM_PROMPT must contain 'exactly one JSON object'"
        );
        assert!(
            NEWS_SYSTEM_PROMPT.contains("no prose"),
            "NEWS_SYSTEM_PROMPT must contain 'no prose'"
        );
        assert!(
            NEWS_SYSTEM_PROMPT.contains("no markdown fences"),
            "NEWS_SYSTEM_PROMPT must contain 'no markdown fences'"
        );
    }

    #[test]
    fn parse_news_rejects_unknown_fields() {
        let result = parse_news(r#"{"unknown_field": 1}"#);
        assert!(
            matches!(result, Err(TradingError::SchemaViolation { .. })),
            "parse_news should return SchemaViolation for unknown fields"
        );
    }

    #[tokio::test]
    async fn news_run_uses_shared_inference_helper_for_openrouter() {
        use super::super::common::run_analyst_inference;
        use crate::providers::ProviderId;
        use crate::providers::factory::agent_test_support;
        use rig::agent::PromptResponse;
        use rig::completion::Usage;

        let valid_json = r#"{
            "articles": [
                {
                    "title": "Breaking News",
                    "source": "Reuters",
                    "published_at": "2026-03-10T10:00:00Z",
                    "relevance_score": 0.9,
                    "snippet": "Significant development for the asset."
                }
            ],
            "macro_events": [],
            "summary": "Key development with direct relevance to price action."
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
            "analyse AAPL news",
            std::time::Duration::from_millis(100),
            &crate::error::RetryPolicy {
                max_retries: 0,
                base_delay: std::time::Duration::from_millis(1),
            },
            1,
            parse_news,
            validate_news,
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
    fn news_rendered_prompt_includes_evidence_discipline_rules() {
        use crate::agents::shared::{
            build_authoritative_source_prompt_rule, build_data_quality_prompt_rule,
            build_missing_data_prompt_rule,
        };

        let prompt = build_news_system_prompt("AAPL", "2026-01-01", None);

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
    fn news_rendered_prompt_prefers_runtime_policy_prompt_bundle() {
        use crate::{
            agents::shared::{
                build_authoritative_source_prompt_rule, build_data_quality_prompt_rule,
                build_missing_data_prompt_rule,
            },
            analysis_packs::resolve_runtime_policy,
        };

        let mut policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        policy.analysis_emphasis = "prioritise decision-relevant catalysts".to_owned();
        policy.prompt_bundle.news_analyst =
            "Pack news prompt for {ticker} at {current_date}. Emphasis: {analysis_emphasis}."
                .into();

        let prompt = build_news_system_prompt("AAPL", "2026-01-01", Some(&policy));

        assert!(
            prompt.contains(
                "Pack news prompt for AAPL at 2026-01-01. Emphasis: prioritise decision-relevant catalysts."
            ),
            "runtime-policy prompt bundle should override the legacy news template: {prompt}"
        );
        assert!(
            !prompt.contains(
                "Your job is to identify the most relevant recent company and macro developments"
            ),
            "legacy news template should not leak through when a pack override is present: {prompt}"
        );
        assert!(
            prompt.contains(build_authoritative_source_prompt_rule())
                && prompt.contains(build_missing_data_prompt_rule())
                && prompt.contains(build_data_quality_prompt_rule()),
            "evidence-discipline rules must still be appended after prompt-bundle rendering: {prompt}"
        );
    }
}
