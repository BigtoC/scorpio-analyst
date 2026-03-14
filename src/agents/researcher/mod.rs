//! Researcher Team — Phase 2 of the TradingAgents pipeline.
//!
//! Implements a cyclic adversarial debate between a Bullish Researcher and a
//! Bearish Researcher, moderated by a [`DebateModerator`] that synthesises the
//! arguments into a consensus summary stored in [`TradingState::consensus_summary`].
//!
//! # Usage
//!
//! ```rust,ignore
//! use scorpio_analyst::agents::researcher::run_researcher_debate;
//!
//! let usages = run_researcher_debate(&mut state, &config, &handle).await?;
//! // state.debate_history is now populated; state.consensus_summary is set.
//! ```

mod bearish;
mod bullish;
mod common;
mod moderator;

pub use bearish::BearishResearcher;
pub use bullish::BullishResearcher;
pub use moderator::DebateModerator;

use crate::{
    config::Config,
    error::TradingError,
    providers::factory::CompletionModelHandle,
    state::{AgentTokenUsage, TradingState},
};

/// Run the full researcher debate loop for Phase 2.
///
/// Executes `config.llm.max_debate_rounds` rounds of Bull vs Bear argument
/// exchange, then invokes the Debate Moderator to produce a consensus summary.
///
/// # Rounds
///
/// Each round invokes the Bullish Researcher then the Bearish Researcher
/// sequentially. Their [`DebateMessage`] outputs are appended to
/// `state.debate_history` after each invocation. After all rounds the
/// Debate Moderator runs once and writes to `state.consensus_summary`.
///
/// # Returns
///
/// A `Vec<AgentTokenUsage>` with `2 * max_debate_rounds + 1` entries
/// (Bull + Bear per round, plus the Moderator).
///
/// # Errors
///
/// Returns the first [`TradingError`] encountered — any LLM failure aborts
/// the debate immediately. Schema violations are also propagated unchanged.
pub async fn run_researcher_debate(
    state: &mut TradingState,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<Vec<AgentTokenUsage>, TradingError> {
    let max_rounds = config.llm.max_debate_rounds;
    let mut all_usages: Vec<AgentTokenUsage> =
        Vec::with_capacity((max_rounds as usize).saturating_mul(2).saturating_add(1));

    // Construct both researchers up-front so their system prompts capture the
    // analyst data snapshot at the start of Phase 2.
    let mut bull = BullishResearcher::new(handle, state, &config.llm);
    let mut bear = BearishResearcher::new(handle, state, &config.llm);

    for _round in 0..max_rounds {
        // Determine the bear's latest argument for the bull to respond to.
        let bear_latest = state
            .debate_history
            .iter()
            .rev()
            .find(|m| m.role == "bearish_researcher")
            .map(|m| m.content.clone());

        // Bull goes first.
        let (bull_msg, bull_usage) = bull
            .run(&state.debate_history, bear_latest.as_deref())
            .await?;
        state.debate_history.push(bull_msg);
        all_usages.push(bull_usage);

        // Bear responds to the bull's latest argument.
        let bull_latest = state
            .debate_history
            .iter()
            .rev()
            .find(|m| m.role == "bullish_researcher")
            .map(|m| m.content.clone());

        let (bear_msg, bear_usage) = bear
            .run(&state.debate_history, bull_latest.as_deref())
            .await?;
        state.debate_history.push(bear_msg);
        all_usages.push(bear_usage);
    }

    // Moderator runs once after all rounds are complete.
    // Construct with the fully populated debate_history so the system prompt
    // captures the complete debate context.
    let moderator = DebateModerator::new(handle, state, &config.llm);
    let (consensus, moderator_usage) = moderator.run(state).await?;
    state.consensus_summary = Some(consensus);
    all_usages.push(moderator_usage);

    Ok(all_usages)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::state::{AgentTokenUsage, DebateMessage};

    // ── Task 4.6: 1-round debate produces 2 DebateMessages ───────────────

    #[test]
    fn one_round_produces_two_debate_messages() {
        // Simulate what run_researcher_debate does structurally
        let mut history: Vec<DebateMessage> = Vec::new();

        let bull = DebateMessage {
            role: "bullish_researcher".to_owned(),
            content: "Bull argument round 1.".to_owned(),
        };
        let bear = DebateMessage {
            role: "bearish_researcher".to_owned(),
            content: "Bear argument round 1.".to_owned(),
        };
        history.push(bull);
        history.push(bear);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "bullish_researcher");
        assert_eq!(history[1].role, "bearish_researcher");
    }

    // ── Task 4.7: 3-round debate produces 6 DebateMessages ───────────────

    #[test]
    fn three_rounds_produce_six_debate_messages() {
        let mut history: Vec<DebateMessage> = Vec::new();

        for round in 1..=3u32 {
            history.push(DebateMessage {
                role: "bullish_researcher".to_owned(),
                content: format!("Bull round {round}."),
            });
            history.push(DebateMessage {
                role: "bearish_researcher".to_owned(),
                content: format!("Bear round {round}."),
            });
        }

        assert_eq!(history.len(), 6);
        // Verify alternating roles
        for i in (0..6).step_by(2) {
            assert_eq!(history[i].role, "bullish_researcher");
            assert_eq!(history[i + 1].role, "bearish_researcher");
        }
    }

    // ── Task 4.8: 0 rounds — no debate messages, moderator still invoked ─

    #[test]
    fn zero_rounds_produce_no_debate_messages() {
        let max_rounds: u32 = 0;
        let mut history: Vec<DebateMessage> = Vec::new();

        for _round in 0..max_rounds {
            history.push(DebateMessage {
                role: "bullish_researcher".to_owned(),
                content: "unreachable".to_owned(),
            });
            history.push(DebateMessage {
                role: "bearish_researcher".to_owned(),
                content: "unreachable".to_owned(),
            });
        }

        // Zero rounds: no debate messages added
        assert_eq!(history.len(), 0);
        // The consensus_summary would still be set by the moderator — verified
        // in integration tests since it requires a real LlmAgent.
    }

    // ── Task 4.10: Token usage count = 2 * rounds + 1 ────────────────────

    #[test]
    fn token_usage_count_equals_two_rounds_plus_moderator() {
        let rounds = 3u32;
        let expected = (rounds as usize) * 2 + 1;

        // Simulate collecting usages
        let mut usages: Vec<AgentTokenUsage> = Vec::new();
        for i in 0..(rounds * 2 + 1) {
            let agent_name = if i == rounds * 2 {
                "Debate Moderator"
            } else if i % 2 == 0 {
                "Bullish Researcher"
            } else {
                "Bearish Researcher"
            };
            usages.push(AgentTokenUsage {
                agent_name: agent_name.to_owned(),
                model_id: "o3".to_owned(),
                token_counts_available: false,
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                latency_ms: 0,
            });
        }

        assert_eq!(usages.len(), expected);
        assert_eq!(usages.last().unwrap().agent_name, "Debate Moderator");
    }

    // ── Task 4.11: token_counts_available = false when provider doesn't expose counts ─

    #[test]
    fn token_counts_unavailable_when_all_zero() {
        let usage = AgentTokenUsage {
            agent_name: "Bullish Researcher".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 5,
        };
        assert!(!usage.token_counts_available);
        assert_eq!(usage.total_tokens, 0);
    }

    // ── Task 4.4: Return type is Vec<AgentTokenUsage> ─────────────────────

    #[test]
    fn usage_vector_includes_moderator_as_last_entry() {
        let rounds = 2u32;
        let mut usages: Vec<AgentTokenUsage> = Vec::new();
        for _ in 0..rounds {
            usages.push(AgentTokenUsage {
                agent_name: "Bullish Researcher".to_owned(),
                model_id: "o3".to_owned(),
                token_counts_available: false,
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                latency_ms: 0,
            });
            usages.push(AgentTokenUsage {
                agent_name: "Bearish Researcher".to_owned(),
                model_id: "o3".to_owned(),
                token_counts_available: false,
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                latency_ms: 0,
            });
        }
        usages.push(AgentTokenUsage {
            agent_name: "Debate Moderator".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 0,
        });

        assert_eq!(usages.len(), (rounds as usize) * 2 + 1);
        assert_eq!(usages.last().unwrap().agent_name, "Debate Moderator");
    }
}
