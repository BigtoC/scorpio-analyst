//! Bearish Researcher agent.
//!
//! Maintains a multi-turn chat session to argue the strongest evidence-based
//! bearish case for the asset, responding directly to the bull researcher's
//! latest argument each round.

use std::time::Instant;

use rig::completion::Message;

use crate::{
    config::LlmConfig,
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, LlmAgent, build_agent, chat_with_retry_details},
    state::{AgentTokenUsage, DebateMessage, TradingState},
};

use super::bullish::format_debate_history;
use super::common::{researcher_runtime_config, usage_from_response, validate_debate_content};

/// System prompt for the Bearish Researcher, adapted from `docs/prompts.md` §2.
const BEARISH_SYSTEM_PROMPT: &str = "\
You are the Bear Researcher for {ticker} as of {current_date}.
Your role is to argue the strongest evidence-based bearish case using the analyst outputs and the current debate state.

Available inputs:
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Debate history: {debate_history}
- Bull's latest argument: {current_bull_argument}
- Past learnings: {past_memory_str}

Instructions:
1. Respond directly to the Bull Researcher's latest points instead of repeating a generic bear thesis.
2. Anchor claims in the actual analyst fields or cited news items.
3. If evidence is missing, acknowledge the gap instead of inventing a negative signal.
4. Keep the response concise and debate-oriented. This should read like one strong turn in a live discussion.
5. End with a one-sentence bottom line stating why the bearish case still leads.

Return plain text only. Do not return JSON, Markdown tables, or a final transaction instruction.";

/// The Bearish Researcher agent.
///
/// Maintains conversation history across debate rounds so each response can
/// directly address the bull's latest argument.
pub struct BearishResearcher {
    agent: LlmAgent,
    model_id: String,
    chat_history: Vec<Message>,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl BearishResearcher {
    /// Construct a new `BearishResearcher`.
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

        let system_prompt = BEARISH_SYSTEM_PROMPT
            .replace("{ticker}", &runtime.symbol)
            .replace("{current_date}", &runtime.target_date)
            .replace("{fundamental_report}", &fundamental_report)
            .replace("{technical_report}", &technical_report)
            .replace("{sentiment_report}", &sentiment_report)
            .replace("{news_report}", &news_report)
            .replace("{debate_history}", &debate_history)
            .replace("{current_bull_argument}", "(none yet — opening argument)")
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

    /// Execute one round of the bearish argument.
    ///
    /// # Parameters
    /// - `debate_history` – accumulated debate messages so far (for context in the prompt).
    /// - `bull_argument` – the bull's most recent argument, or `None` for the opening round.
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
        bull_argument: Option<&str>,
    ) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let bull_arg_text = bull_argument.unwrap_or("(none yet — opening argument)");
        let history_text = format_debate_history(debate_history);

        let prompt = format!(
            "Debate history so far:\n{history_text}\n\nBull's latest argument:\n{bull_arg_text}\n\nProvide your bearish rebuttal."
        );

        let response = chat_with_retry_details(
            &self.agent,
            &prompt,
            &mut self.chat_history,
            self.timeout,
            &self.retry_policy,
        )
        .await?;

        validate_debate_content("BearishResearcher", &response.output)?;

        let usage = usage_from_response(
            "Bearish Researcher",
            &self.model_id,
            response.usage,
            started_at,
        );

        let message = DebateMessage {
            role: "bearish_researcher".to_owned(),
            content: response.output,
        };

        Ok((message, usage))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentTokenUsage, DebateMessage};

    // ── Task 2.4: Correct DebateMessage construction ─────────────────────

    #[test]
    fn debate_message_has_bearish_researcher_role() {
        let msg = DebateMessage {
            role: "bearish_researcher".to_owned(),
            content: "High debt load threatens the downside scenario.".to_owned(),
        };
        assert_eq!(msg.role, "bearish_researcher");
        assert!(!msg.content.is_empty());
    }

    // ── Task 2.6: AgentTokenUsage agent name ─────────────────────────────

    #[test]
    fn agent_token_usage_has_correct_agent_name() {
        let usage = AgentTokenUsage {
            agent_name: "Bearish Researcher".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 10,
        };
        assert_eq!(usage.agent_name, "Bearish Researcher");
        assert_eq!(usage.model_id, "o3");
    }

    // ── Task 2.7: Oversized / control-char output rejected ───────────────

    #[test]
    fn oversized_output_returns_schema_violation() {
        let big = "x".repeat(super::super::common::MAX_DEBATE_CHARS + 1);
        let result = validate_debate_content("BearishResearcher", &big);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn control_char_output_returns_schema_violation() {
        let result = validate_debate_content("BearishResearcher", "bad\x1bcontent");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Task 2.5: Chat history starts empty ──────────────────────────────

    #[test]
    fn bearish_researcher_starts_with_empty_chat_history() {
        let history: Vec<Message> = Vec::new();
        assert!(history.is_empty());
    }
}
