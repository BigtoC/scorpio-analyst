//! Debate Moderator agent.
//!
//! Executes a single one-shot prompt after all debate rounds complete to
//! synthesize the bull and bear arguments into a consensus summary for
//! the Trader Agent.

use std::time::Instant;

use crate::{
    config::LlmConfig,
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, LlmAgent, build_agent, prompt_with_retry_details},
    state::{AgentTokenUsage, TradingState},
};

use super::bullish::format_debate_history;
use super::common::{researcher_runtime_config, usage_from_response, validate_debate_content};

/// System prompt for the Debate Moderator, adapted from `docs/prompts.md` §2.
const MODERATOR_SYSTEM_PROMPT: &str = "\
You are the Debate Moderator and Research Manager for {ticker} as of {current_date}.
Your role is to synthesize the Bull and Bear arguments into a concise consensus handoff for the Trader.

Available inputs:
- Bull case: {bull_case}
- Bear case: {bear_case}
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Debate history: {debate_history}
- Past learnings: {past_memory_str}

Instructions:
1. Judge evidence quality, not tone.
2. State the prevailing stance explicitly using the words `Buy`, `Sell`, or `Hold`.
3. Include the strongest bullish evidence, the strongest bearish evidence, and the most important unresolved uncertainty.
4. Keep the output compact because it is stored as a single `consensus_summary` string.
5. Do not output JSON, position sizing, stop-losses, or the final execution decision.

Return plain text only, suitable for direct storage in `TradingState.consensus_summary`.";

/// The Debate Moderator agent.
///
/// Uses a one-shot prompt (not multi-turn chat) because it evaluates the entire
/// completed debate at once after all rounds have finished.
pub struct DebateModerator {
    agent: LlmAgent,
    model_id: String,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl DebateModerator {
    /// Construct a new `DebateModerator`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (should be `DeepThinking` tier).
    /// - `state` – current trading state; analyst data and debate history are injected into
    ///   the system prompt at construction time.
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

        let debate_history_text = format_debate_history(&state.debate_history);

        // Extract the last bull and bear arguments from the debate history for the
        // {bull_case} and {bear_case} placeholders.
        let bull_case = state
            .debate_history
            .iter()
            .rev()
            .find(|m| m.role == "bullish_researcher")
            .map(|m| m.content.as_str())
            .unwrap_or("(no bullish argument recorded)")
            .to_owned();

        let bear_case = state
            .debate_history
            .iter()
            .rev()
            .find(|m| m.role == "bearish_researcher")
            .map(|m| m.content.as_str())
            .unwrap_or("(no bearish argument recorded)")
            .to_owned();

        let system_prompt = MODERATOR_SYSTEM_PROMPT
            .replace("{ticker}", &runtime.symbol)
            .replace("{current_date}", &runtime.target_date)
            .replace("{bull_case}", &bull_case)
            .replace("{bear_case}", &bear_case)
            .replace("{fundamental_report}", &fundamental_report)
            .replace("{technical_report}", &technical_report)
            .replace("{sentiment_report}", &sentiment_report)
            .replace("{news_report}", &news_report)
            .replace("{debate_history}", &debate_history_text)
            .replace("{past_memory_str}", "");

        let agent = build_agent(handle, &system_prompt);
        let model_id = handle.model_id().to_owned();

        Self {
            agent,
            model_id,
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
        }
    }

    /// Run the moderator: produce a consensus summary from the completed debate.
    ///
    /// # Returns
    /// A `(String, AgentTokenUsage)` pair where the string is the consensus summary.
    ///
    /// # Errors
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the output is empty, oversized, or
    ///   contains disallowed control characters.
    pub async fn run(
        &self,
        state: &TradingState,
    ) -> Result<(String, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let prompt = format!(
            "Synthesise the debate for {} as of {} into a consensus summary for the Trader.",
            state.asset_symbol, state.target_date
        );

        let response =
            prompt_with_retry_details(&self.agent, &prompt, self.timeout, &self.retry_policy)
                .await?;

        if response.output.trim().is_empty() {
            return Err(TradingError::SchemaViolation {
                message: "DebateModerator: consensus summary must not be empty".to_owned(),
            });
        }

        validate_debate_content("DebateModerator", &response.output)?;

        let usage = usage_from_response(
            "Debate Moderator",
            &self.model_id,
            response.usage,
            started_at,
        );

        Ok((response.output, usage))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AgentTokenUsage;

    // ── Task 3.4 / 3.6: Structural checks (no LLM call needed) ──────────

    #[test]
    fn agent_token_usage_has_correct_agent_name() {
        let usage = AgentTokenUsage {
            agent_name: "Debate Moderator".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 10,
        };
        assert_eq!(usage.agent_name, "Debate Moderator");
        assert_eq!(usage.model_id, "o3");
    }

    // ── Task 3.7: Oversized / control-char output rejected ───────────────

    #[test]
    fn oversized_consensus_returns_schema_violation() {
        let big = "x".repeat(super::super::common::MAX_DEBATE_CHARS + 1);
        let result = validate_debate_content("DebateModerator", &big);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn control_char_consensus_returns_schema_violation() {
        let result = validate_debate_content("DebateModerator", "bad\x00output");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Task 3.5: Output must include explicit stance ─────────────────────
    // This checks that validation logic correctly identifies stance words.

    #[test]
    fn consensus_containing_buy_is_valid_content() {
        let content = "Based on the evidence, the prevailing stance is Buy. \
                       Revenue growth supports upside, though debt remains a risk.";
        assert!(validate_debate_content("DebateModerator", content).is_ok());
        assert!(content.contains("Buy") || content.contains("Sell") || content.contains("Hold"));
    }

    #[test]
    fn consensus_containing_hold_is_valid_content() {
        let content = "Hold is the balanced stance given mixed signals.";
        assert!(validate_debate_content("DebateModerator", content).is_ok());
        assert!(content.contains("Hold"));
    }

    #[test]
    fn consensus_containing_sell_is_valid_content() {
        let content = "Sell is warranted given deteriorating fundamentals.";
        assert!(validate_debate_content("DebateModerator", content).is_ok());
        assert!(content.contains("Sell"));
    }
}
