//! News Analyst agent.
//!
//! Fetches recent company news from Finnhub, serialises it as context, and
//! passes it to a quick-thinking LLM that returns a structured [`NewsData`]
//! JSON object capturing the most relevant articles and macro events.

use std::time::Instant;

use crate::{
    config::LlmConfig,
    data::FinnhubClient,
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent, prompt_with_retry},
    state::{AgentTokenUsage, NewsData},
};

/// System prompt for the News Analyst, adapted from `docs/prompts.md`.
const NEWS_SYSTEM_PROMPT: &str = "\
You are the News Analyst for {ticker} as of {current_date}.
Your job is to identify the most relevant recent company and macro developments and convert them into a `NewsData` JSON \
object.

Use only the bound news tools available at runtime. In the current system, `get_news` is the primary concrete tool.
There may not be a dedicated macro data tool in the run, so do not assume one exists.

Populate only these schema fields:
- `articles`
- `macro_events`
- `summary`

Instructions:
1. Prefer recent, clearly relevant developments over generic market commentary.
2. Fill `articles` with the most decision-relevant items only. Use the provided article facts; do not rewrite entire \
   articles into the output.
3. Add `macro_events` only when the article set actually supports a macro or sector-level causal link. If not, return \
   `[]`.
4. Keep `impact_direction` simple and explicit, such as `positive`, `negative`, `mixed`, or `uncertain`.
5. Use `summary` to explain why the news matters for the asset right now.
6. If coverage is sparse, say so in `summary` and keep the arrays short or empty rather than padding weak items.
7. Return ONLY the single JSON object required by `NewsData`.

Do not include any trade recommendation, target price, or final transaction proposal.";

/// The News Analyst agent.
///
/// Uses Finnhub company news and returns a structured [`NewsData`] output
/// highlighting causal relationships and macro events.
pub struct NewsAnalyst {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    symbol: String,
    target_date: String,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl NewsAnalyst {
    /// Construct a new `NewsAnalyst`.
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
            timeout: std::time::Duration::from_secs(llm_config.agent_timeout_secs),
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Run the analyst: fetch news, prompt the LLM, parse and return output.
    ///
    /// # Errors
    ///
    /// - [`TradingError::AnalystError`] when Finnhub news fetching fails.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the LLM returns malformed JSON.
    pub async fn run(&self) -> Result<(NewsData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        // ── 1. Fetch news from Finnhub ────────────────────────────────────
        let news_data =
            self.finnhub
                .get_news(&self.symbol)
                .await
                .map_err(|e| TradingError::AnalystError {
                    agent: "news".to_owned(),
                    message: e.to_string(),
                })?;

        // ── 2. Serialize context ──────────────────────────────────────────
        let context = serde_json::to_string_pretty(&news_data).unwrap_or_default();

        // ── 3. Build agent and invoke LLM ─────────────────────────────────
        let system_prompt = NEWS_SYSTEM_PROMPT
            .replace("{ticker}", &self.symbol)
            .replace("{current_date}", &self.target_date);

        let agent = build_agent(&self.handle, &system_prompt);

        let prompt = format!(
            "Using the news data below, identify the most relevant developments for {} as of {} \
             and produce a `NewsData` JSON object.\n\n{}",
            self.symbol, self.target_date, context
        );

        let raw = prompt_with_retry(&agent, &prompt, self.timeout, &self.retry_policy).await?;

        // ── 4. Parse structured output ────────────────────────────────────
        let data: NewsData =
            serde_json::from_str(raw.trim()).map_err(|e| TradingError::SchemaViolation {
                message: format!("NewsAnalyst: failed to parse LLM output: {e}"),
            })?;

        // ── 5. Record token usage (counts unavailable from provider) ───────
        let latency_ms = started_at.elapsed().as_millis() as u64;
        let usage = AgentTokenUsage {
            agent_name: "news".to_owned(),
            model_id: self.handle.model_id().to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms,
        };

        Ok((data, usage))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MacroEvent, NewsArticle, NewsData};

    fn parse_news(json: &str) -> Result<NewsData, TradingError> {
        serde_json::from_str(json).map_err(|e| TradingError::SchemaViolation {
            message: format!("NewsAnalyst: failed to parse LLM output: {e}"),
        })
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
        assert_eq!(data.macro_events[0].impact_direction, "positive");
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
            agent_name: "news".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 220,
        };
        assert_eq!(usage.agent_name, "news");
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
                impact_direction: "mixed".to_owned(),
                confidence: 0.65,
            }],
            summary: "Notable earnings and rate news.".to_owned(),
        };

        let serialized = serde_json::to_string(&original).expect("serialise");
        let roundtripped: NewsData = serde_json::from_str(&serialized).expect("deserialise");
        assert_eq!(original, roundtripped);
    }
}
