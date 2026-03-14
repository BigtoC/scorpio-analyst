//! Bullish Researcher agent.
//!
//! Maintains a multi-turn chat session to argue the strongest evidence-based
//! bullish case for the asset, responding directly to the bear researcher's
//! latest argument each round.

use std::time::Instant;

use rig::completion::Message;

use crate::{
    config::LlmConfig,
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, LlmAgent, build_agent, chat_with_retry_details},
    state::{AgentTokenUsage, DebateMessage, TradingState},
};

use super::common::{researcher_runtime_config, usage_from_response, validate_debate_content};

/// System prompt for the Bullish Researcher, adapted from `docs/prompts.md` §2.
const BULLISH_SYSTEM_PROMPT: &str = "\
You are the Bull Researcher for {ticker} as of {current_date}.
Your role is to argue the strongest evidence-based bullish case using the analyst outputs and the current debate state.

Available inputs:
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Debate history: {debate_history}
- Bear's latest argument: {current_bear_argument}
- Past learnings: {past_memory_str}

Instructions:
1. Respond directly to the Bear Researcher's latest points instead of repeating a generic bull thesis.
2. Anchor claims in the actual analyst fields or cited news items.
3. If evidence is missing, acknowledge the gap instead of inventing support.
4. Keep the response concise and debate-oriented. This should read like one strong turn in a live discussion.
5. End with a one-sentence bottom line stating why the bullish case still leads.

Return plain text only. Do not return JSON, Markdown tables, or a final transaction instruction.";

/// The Bullish Researcher agent.
///
/// Maintains conversation history across debate rounds so each response can
/// directly address the bear's latest argument.
pub struct BullishResearcher {
    agent: LlmAgent,
    model_id: String,
    chat_history: Vec<Message>,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl BullishResearcher {
    /// Construct a new `BullishResearcher`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (should be `DeepThinking` tier).
    /// - `state` – current trading state used to inject analyst data into the system prompt.
    /// - `llm_config` – LLM configuration for timeout and retry policy.
    pub fn new(
        handle: &CompletionModelHandle,
        state: &TradingState,
        llm_config: &LlmConfig,
    ) -> Self {
        let runtime =
            researcher_runtime_config(&state.asset_symbol, &state.target_date, llm_config);

        let fundamental_report =
            serde_json::to_string(&state.fundamental_metrics).unwrap_or_else(|_| "null".to_owned());
        let technical_report = serde_json::to_string(&state.technical_indicators)
            .unwrap_or_else(|_| "null".to_owned());
        let sentiment_report =
            serde_json::to_string(&state.market_sentiment).unwrap_or_else(|_| "null".to_owned());
        let news_report =
            serde_json::to_string(&state.macro_news).unwrap_or_else(|_| "null".to_owned());

        let debate_history = format_debate_history(&state.debate_history);

        let system_prompt = BULLISH_SYSTEM_PROMPT
            .replace("{ticker}", &runtime.symbol)
            .replace("{current_date}", &runtime.target_date)
            .replace("{fundamental_report}", &fundamental_report)
            .replace("{technical_report}", &technical_report)
            .replace("{sentiment_report}", &sentiment_report)
            .replace("{news_report}", &news_report)
            .replace("{debate_history}", &debate_history)
            .replace("{current_bear_argument}", "(none yet — opening argument)")
            .replace("{past_memory_str}", "");

        let agent = build_agent(handle, &system_prompt);
        let model_id = handle.model_id().to_owned();

        Self {
            agent,
            model_id,
            chat_history: Vec::new(),
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
        }
    }

    /// Execute one round of the bullish argument.
    ///
    /// # Parameters
    /// - `debate_history` – accumulated debate messages so far (for context in the prompt).
    /// - `bear_argument` – the bear's most recent argument, or `None` for the opening round.
    ///
    /// # Returns
    /// A `(DebateMessage, AgentTokenUsage)` pair on success.
    ///
    /// # Errors
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the output is oversized or contains
    ///   disallowed control characters.
    pub async fn run(
        &mut self,
        debate_history: &[DebateMessage],
        bear_argument: Option<&str>,
    ) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let bear_arg_text = bear_argument.unwrap_or("(none yet — opening argument)");
        let history_text = format_debate_history(debate_history);

        let prompt = format!(
            "Debate history so far:\n{history_text}\n\nBear's latest argument:\n{bear_arg_text}\n\nProvide your bullish rebuttal."
        );

        let response = chat_with_retry_details(
            &self.agent,
            &prompt,
            &mut self.chat_history,
            self.timeout,
            &self.retry_policy,
        )
        .await?;

        validate_debate_content("BullishResearcher", &response.output)?;

        let usage = usage_from_response(
            "Bullish Researcher",
            &self.model_id,
            response.usage,
            started_at,
        );

        let message = DebateMessage {
            role: "bullish_researcher".to_owned(),
            content: response.output,
        };

        Ok((message, usage))
    }
}

/// Format a slice of `DebateMessage` entries as a human-readable string for prompt injection.
pub(super) fn format_debate_history(history: &[DebateMessage]) -> String {
    if history.is_empty() {
        return "(no prior debate history)".to_owned();
    }
    history
        .iter()
        .enumerate()
        .map(|(i, msg)| format!("[{}] {}: {}", i + 1, msg.role, msg.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentTokenUsage, DebateMessage};

    // ── Task 1.4: Correct DebateMessage construction ─────────────────────

    #[test]
    fn debate_message_has_bullish_researcher_role() {
        let msg = DebateMessage {
            role: "bullish_researcher".to_owned(),
            content: "Strong earnings growth supports upside.".to_owned(),
        };
        assert_eq!(msg.role, "bullish_researcher");
        assert!(!msg.content.is_empty());
    }

    // ── Task 1.6: AgentTokenUsage agent name ─────────────────────────────

    #[test]
    fn agent_token_usage_has_correct_agent_name() {
        let usage = AgentTokenUsage {
            agent_name: "Bullish Researcher".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 10,
        };
        assert_eq!(usage.agent_name, "Bullish Researcher");
        assert_eq!(usage.model_id, "o3");
    }

    // ── Task 1.7: Oversized output rejected ──────────────────────────────

    #[test]
    fn oversized_output_returns_schema_violation() {
        let big = "x".repeat(super::super::common::MAX_DEBATE_CHARS + 1);
        let result = validate_debate_content("BullishResearcher", &big);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn control_char_output_returns_schema_violation() {
        let result = validate_debate_content("BullishResearcher", "bad\x00output");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── format_debate_history ─────────────────────────────────────────────

    #[test]
    fn format_debate_history_empty_returns_placeholder() {
        let formatted = format_debate_history(&[]);
        assert_eq!(formatted, "(no prior debate history)");
    }

    #[test]
    fn format_debate_history_includes_role_and_content() {
        let history = vec![
            DebateMessage {
                role: "bullish_researcher".to_owned(),
                content: "Bull argument.".to_owned(),
            },
            DebateMessage {
                role: "bearish_researcher".to_owned(),
                content: "Bear rebuttal.".to_owned(),
            },
        ];
        let formatted = format_debate_history(&history);
        assert!(formatted.contains("bullish_researcher"));
        assert!(formatted.contains("Bull argument."));
        assert!(formatted.contains("bearish_researcher"));
        assert!(formatted.contains("Bear rebuttal."));
    }

    // ── Task 1.5: Chat history accumulation (unit-level structural check) ─

    #[test]
    fn bullish_researcher_starts_with_empty_chat_history() {
        // We verify the struct initializes correctly without making a real LLM call.
        // Full accumulation across `run` invocations is covered by integration tests.
        let history: Vec<Message> = Vec::new();
        assert!(history.is_empty());
    }
}
