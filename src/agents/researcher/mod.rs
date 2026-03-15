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
    state::{AgentTokenUsage, DebateMessage, TradingState},
};

trait DebateExecutor {
    async fn bullish_turn(
        &mut self,
        debate_history: &[DebateMessage],
        bear_argument: Option<&str>,
    ) -> Result<(DebateMessage, AgentTokenUsage), TradingError>;

    async fn bearish_turn(
        &mut self,
        debate_history: &[DebateMessage],
        bull_argument: Option<&str>,
    ) -> Result<(DebateMessage, AgentTokenUsage), TradingError>;

    async fn moderate(
        &mut self,
        state: &TradingState,
    ) -> Result<(String, AgentTokenUsage), TradingError>;
}

struct RealDebateExecutor {
    bull: BullishResearcher,
    bear: BearishResearcher,
    moderator: DebateModerator,
}

impl DebateExecutor for RealDebateExecutor {
    async fn bullish_turn(
        &mut self,
        debate_history: &[DebateMessage],
        bear_argument: Option<&str>,
    ) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
        self.bull.run(debate_history, bear_argument).await
    }

    async fn bearish_turn(
        &mut self,
        debate_history: &[DebateMessage],
        bull_argument: Option<&str>,
    ) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
        self.bear.run(debate_history, bull_argument).await
    }

    async fn moderate(
        &mut self,
        state: &TradingState,
    ) -> Result<(String, AgentTokenUsage), TradingError> {
        self.moderator.run(state).await
    }
}

async fn run_researcher_debate_with_executor<E>(
    state: &mut TradingState,
    max_rounds: u32,
    executor: &mut E,
) -> Result<Vec<AgentTokenUsage>, TradingError>
where
    E: DebateExecutor,
{
    let mut all_usages: Vec<AgentTokenUsage> =
        Vec::with_capacity((max_rounds as usize).saturating_mul(2).saturating_add(1));

    for _round in 0..max_rounds {
        let bear_latest = state
            .debate_history
            .iter()
            .rev()
            .find(|m| m.role == "bearish_researcher")
            .map(|m| m.content.as_str());

        let (bull_msg, bull_usage) = executor
            .bullish_turn(&state.debate_history, bear_latest)
            .await?;
        state.debate_history.push(bull_msg);
        all_usages.push(bull_usage);

        let bull_latest = state
            .debate_history
            .iter()
            .rev()
            .find(|m| m.role == "bullish_researcher")
            .map(|m| m.content.as_str());

        let (bear_msg, bear_usage) = executor
            .bearish_turn(&state.debate_history, bull_latest)
            .await?;
        state.debate_history.push(bear_msg);
        all_usages.push(bear_usage);
    }

    let (consensus, moderator_usage) = executor.moderate(state).await?;
    state.consensus_summary = Some(consensus);
    all_usages.push(moderator_usage);

    Ok(all_usages)
}

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
    let mut executor = RealDebateExecutor {
        bull: BullishResearcher::new(handle, state, &config.llm),
        bear: BearishResearcher::new(handle, state, &config.llm),
        moderator: DebateModerator::new(handle, state, &config.llm),
    };

    run_researcher_debate_with_executor(state, max_rounds, &mut executor).await
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::state::{AgentTokenUsage, DebateMessage};
    use crate::{error::TradingError, state::TokenUsageTracker};
    use uuid::Uuid;

    use super::*;

    struct MockDebateExecutor {
        bull_calls: usize,
        bear_calls: usize,
        fail_bear_on_call: Option<usize>,
    }

    impl MockDebateExecutor {
        fn new() -> Self {
            Self {
                bull_calls: 0,
                bear_calls: 0,
                fail_bear_on_call: None,
            }
        }

        fn with_bear_failure(call: usize) -> Self {
            Self {
                bull_calls: 0,
                bear_calls: 0,
                fail_bear_on_call: Some(call),
            }
        }
    }

    impl DebateExecutor for MockDebateExecutor {
        async fn bullish_turn(
            &mut self,
            _debate_history: &[DebateMessage],
            _bear_argument: Option<&str>,
        ) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
            self.bull_calls += 1;
            Ok((
                DebateMessage {
                    role: "bullish_researcher".to_owned(),
                    content: format!("Bull round {}.", self.bull_calls),
                },
                AgentTokenUsage::unavailable("Bullish Researcher", "o3", 1),
            ))
        }

        async fn bearish_turn(
            &mut self,
            _debate_history: &[DebateMessage],
            _bull_argument: Option<&str>,
        ) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
            self.bear_calls += 1;
            if self.fail_bear_on_call == Some(self.bear_calls) {
                return Err(TradingError::Rig(format!(
                    "bear failed on call {}",
                    self.bear_calls
                )));
            }

            Ok((
                DebateMessage {
                    role: "bearish_researcher".to_owned(),
                    content: format!("Bear round {}.", self.bear_calls),
                },
                AgentTokenUsage::unavailable("Bearish Researcher", "o3", 1),
            ))
        }

        async fn moderate(
            &mut self,
            _state: &TradingState,
        ) -> Result<(String, AgentTokenUsage), TradingError> {
            Ok((
                "Hold - bullish growth is balanced by downside risk.".to_owned(),
                AgentTokenUsage::unavailable("Debate Moderator", "o3", 1),
            ))
        }
    }

    fn make_state() -> TradingState {
        TradingState {
            execution_id: Uuid::new_v4(),
            asset_symbol: "AAPL".to_owned(),
            target_date: "2026-03-15".to_owned(),
            fundamental_metrics: None,
            technical_indicators: None,
            market_sentiment: None,
            macro_news: None,
            debate_history: Vec::new(),
            consensus_summary: None,
            trader_proposal: None,
            risk_discussion_history: Vec::new(),
            aggressive_risk_report: None,
            neutral_risk_report: None,
            conservative_risk_report: None,
            final_execution_status: None,
            token_usage: TokenUsageTracker::default(),
        }
    }

    // ── Task 4.6: 1-round debate produces 2 DebateMessages ───────────────

    #[test]
    fn one_round_produces_two_debate_messages() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state();
        let mut exec = MockDebateExecutor::new();

        let usages = rt
            .block_on(run_researcher_debate_with_executor(&mut state, 1, &mut exec))
            .unwrap();

        assert_eq!(state.debate_history.len(), 2);
        assert_eq!(state.debate_history[0].role, "bullish_researcher");
        assert_eq!(state.debate_history[1].role, "bearish_researcher");
        assert!(state.consensus_summary.is_some());
        assert_eq!(usages.len(), 3);
    }

    // ── Task 4.7: 3-round debate produces 6 DebateMessages ───────────────

    #[test]
    fn three_rounds_produce_six_debate_messages() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state();
        let mut exec = MockDebateExecutor::new();

        let usages = rt
            .block_on(run_researcher_debate_with_executor(&mut state, 3, &mut exec))
            .unwrap();

        assert_eq!(state.debate_history.len(), 6);
        for i in (0..6).step_by(2) {
            assert_eq!(state.debate_history[i].role, "bullish_researcher");
            assert_eq!(state.debate_history[i + 1].role, "bearish_researcher");
        }
        assert!(state.consensus_summary.is_some());
        assert_eq!(usages.len(), 7);
    }

    // ── Task 4.8: 0 rounds — no debate messages, moderator still invoked ─

    #[test]
    fn zero_rounds_produce_no_debate_messages() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state();
        let mut exec = MockDebateExecutor::new();

        let usages = rt
            .block_on(run_researcher_debate_with_executor(&mut state, 0, &mut exec))
            .unwrap();

        assert_eq!(state.debate_history.len(), 0);
        assert!(state.consensus_summary.is_some());
        assert_eq!(usages.len(), 1);
    }

    #[test]
    fn round_two_bear_failure_aborts_and_skips_moderator() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state();
        let mut exec = MockDebateExecutor::with_bear_failure(2);

        let result = rt.block_on(run_researcher_debate_with_executor(&mut state, 3, &mut exec));

        assert!(matches!(result, Err(TradingError::Rig(_))));
        assert_eq!(state.debate_history.len(), 3);
        assert!(state.consensus_summary.is_none());
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
