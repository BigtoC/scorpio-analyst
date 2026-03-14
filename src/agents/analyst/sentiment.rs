//! Sentiment Analyst agent.
//!
//! Binds a Finnhub news tool (`get_news`) to a quick-thinking LLM agent so
//! the model can fetch recent company news during inference and return a
//! structured [`SentimentData`] JSON object by inferring sentiment from news
//! content.
//!
//! **MVP constraint:** no direct social-platform access (Reddit, X/Twitter,
//! StockTwits). Sentiment is derived solely from news articles.

use std::time::Instant;

use rig::tool::ToolDyn;

use crate::{
    config::LlmConfig,
    data::{FinnhubClient, GetNews},
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent_with_tools, prompt_typed_with_retry},
    state::{AgentTokenUsage, SentimentData},
};

const MAX_TOOL_TURNS: usize = 6;

/// System prompt for the Sentiment Analyst, adapted from `docs/prompts.md`.
const SENTIMENT_SYSTEM_PROMPT: &str = "\
You are the Sentiment Analyst for {ticker} as of {current_date}.
Your job is to infer the current market narrative from the sources actually available in the MVP and return a \
`SentimentData` JSON object.

Important MVP constraint:
- Do not assume direct Reddit, X/Twitter, StockTwits, or other social-platform access unless those tools are explicitly \
  bound.
- In the current system, sentiment is usually inferred from company news and any runtime-provided sentiment proxies.

Populate only these schema fields:
- `overall_score`
- `source_breakdown`
- `engagement_peaks`
- `summary`

Instructions:
1. Derive sentiment from the available sources only.
2. Use a consistent numeric convention for `overall_score` and `source_breakdown[].score`: `-1.0` means clearly bearish, \
   `0.0` neutral or inconclusive, and `1.0` clearly bullish.
3. Use `source_breakdown[].sample_size` for the count of items actually analyzed for that source grouping.
4. In the MVP, `engagement_peaks` will often be `[]`. Do not fabricate peaks unless the runtime gives you explicit \
   engagement timing data.
5. If no meaningful sentiment signal is available, return `overall_score: 0.0`, empty arrays where appropriate, and a \
   `summary` explaining that the signal is weak or unavailable.
6. Distinguish sentiment from facts: explain how the market appears to be interpreting events, not only what happened.
7. Return ONLY the single JSON object required by `SentimentData`.

Do not include any trade recommendation, target price, or final transaction proposal.";

/// The Sentiment Analyst agent.
///
/// Binds a Finnhub news tool to the LLM so it can fetch news during inference
/// and return a structured [`SentimentData`] output.
pub struct SentimentAnalyst {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    symbol: String,
    target_date: String,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl SentimentAnalyst {
    /// Construct a new `SentimentAnalyst`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (`QuickThinking` tier).
    /// - `finnhub` – Finnhub client used to fetch news articles.
    /// - `symbol` – asset ticker symbol.
    /// - `target_date` – analysis date string.
    /// - `llm_config` – LLM configuration, used for timeout.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        symbol: impl Into<String>,
        target_date: impl Into<String>,
        llm_config: &LlmConfig,
    ) -> Self {
        Self {
            handle,
            finnhub,
            symbol: symbol.into(),
            target_date: target_date.into(),
            timeout: std::time::Duration::from_secs(llm_config.analyst_timeout_secs),
            retry_policy: RetryPolicy::default(),
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

        let tools: Vec<Box<dyn ToolDyn>> = vec![Box::new(GetNews::scoped(
            self.finnhub.clone(),
            self.symbol.clone(),
        ))];

        // ── 2. Build agent with tools and invoke LLM ──────────────────────
        let system_prompt = SENTIMENT_SYSTEM_PROMPT
            .replace("{ticker}", &self.symbol)
            .replace("{current_date}", &self.target_date);

        let agent = build_agent_with_tools(&self.handle, &system_prompt, tools);

        let prompt = format!(
            "Fetch and analyse recent news for {} as of {} using the available tools, \
             then produce a SentimentData JSON object.",
            self.symbol, self.target_date
        );

        let response = prompt_typed_with_retry::<SentimentData>(
            &agent,
            &prompt,
            self.timeout,
            &self.retry_policy,
            MAX_TOOL_TURNS,
        )
        .await?;

        validate_sentiment(&response.output)?;

        Ok((
            response.output,
            AgentTokenUsage {
                agent_name: "Sentiment Analyst".to_owned(),
                model_id: self.handle.model_id().to_owned(),
                token_counts_available: response.usage.total_tokens > 0
                    || response.usage.input_tokens > 0
                    || response.usage.output_tokens > 0,
                prompt_tokens: response.usage.input_tokens,
                completion_tokens: response.usage.output_tokens,
                total_tokens: response.usage.total_tokens,
                latency_ms: started_at.elapsed().as_millis() as u64,
            },
        ))
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{EngagementPeak, SentimentData, SentimentSource};

    fn parse_sentiment(json: &str) -> Result<SentimentData, TradingError> {
        serde_json::from_str(json)
            .map_err(|e| TradingError::SchemaViolation {
                message: format!("SentimentAnalyst: failed to parse LLM output: {e}"),
            })
            .and_then(|data| validate_sentiment(&data).map(|()| data))
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

        let data = parse_sentiment(json).expect("should parse valid JSON");
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

        let data = parse_sentiment(json).expect("should parse neutral");
        assert_eq!(data.overall_score, 0.0);
        assert!(data.source_breakdown.is_empty());
        assert!(data.engagement_peaks.is_empty());
    }

    // ── Task 2.5: Agent does not attempt social-platform access ──────────

    #[test]
    fn system_prompt_forbids_social_platforms() {
        // The system prompt should explicitly warn against social-platform access
        assert!(
            SENTIMENT_SYSTEM_PROMPT.contains("Reddit"),
            "prompt should mention Reddit constraint"
        );
        assert!(
            SENTIMENT_SYSTEM_PROMPT.contains("X/Twitter"),
            "prompt should mention X/Twitter constraint"
        );
        assert!(
            SENTIMENT_SYSTEM_PROMPT.contains("StockTwits"),
            "prompt should mention StockTwits constraint"
        );
        // It should say NOT to assume those are available
        assert!(
            SENTIMENT_SYSTEM_PROMPT.contains("Do not assume"),
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

        let data = parse_sentiment(neutral_json).expect("neutral output should parse");
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
        let result = parse_sentiment(json);
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
        let result = parse_sentiment(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn whitespace_only_summary_returns_schema_violation() {
        let json = r#"{"overall_score": 0.0, "source_breakdown": [], "engagement_peaks": [], "summary": "   "}"#;
        let result = parse_sentiment(json);
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
}
