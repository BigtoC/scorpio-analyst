//! Researcher Team — Phase 2 of the TradingAgents pipeline.
//!
//! Implements a cyclic adversarial debate between a Bullish Researcher and a
//! Bearish Researcher, moderated by a [`DebateModerator`] that synthesises the
//! arguments into a consensus summary stored in [`TradingState::consensus_summary`].

mod bearish;
mod bullish;
mod common;
mod moderator;

pub use bearish::BearishResearcher;
pub use bullish::BullishResearcher;
#[cfg(any(test, feature = "test-helpers"))]
pub(crate) use common::render_researcher_system_prompt;
pub use moderator::DebateModerator;

use crate::{
    config::Config,
    data::adapters::transcripts::TranscriptFetch,
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

/// Execute a single Bullish Researcher turn and append the result to
/// `state.debate_history`.
///
/// Used by [`BullishResearcherTask`][crate::workflow::tasks::BullishResearcherTask]
/// so that each graph node performs exactly one agent step.
///
/// # Errors
///
/// Returns [`TradingError`] on LLM failure or schema violation.
pub async fn run_bullish_researcher_turn(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    let mut executor = RealDebateExecutor {
        bull: BullishResearcher::new(handle, state, transcript_fetch, &config.llm)?,
        bear: BearishResearcher::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: DebateModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let bear_latest = state
        .debate_history
        .iter()
        .rev()
        .find(|m| m.role == "bearish_researcher")
        .map(|m| m.content.as_str());

    let (msg, usage) = executor
        .bullish_turn(&state.debate_history, bear_latest)
        .await?;
    state.debate_history.push(msg);
    Ok(usage)
}

/// Execute a single Bearish Researcher turn and append the result to
/// `state.debate_history`.
///
/// Used by [`BearishResearcherTask`][crate::workflow::tasks::BearishResearcherTask]
/// so that each graph node performs exactly one agent step.
///
/// # Errors
///
/// Returns [`TradingError`] on LLM failure or schema violation.
pub async fn run_bearish_researcher_turn(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    let mut executor = RealDebateExecutor {
        bull: BullishResearcher::new(handle, state, transcript_fetch, &config.llm)?,
        bear: BearishResearcher::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: DebateModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let bull_latest = state
        .debate_history
        .iter()
        .rev()
        .find(|m| m.role == "bullish_researcher")
        .map(|m| m.content.as_str());

    let (msg, usage) = executor
        .bearish_turn(&state.debate_history, bull_latest)
        .await?;
    state.debate_history.push(msg);
    Ok(usage)
}

/// Run the Debate Moderator once, writing the consensus summary to
/// `state.consensus_summary`.
///
/// Used by [`DebateModeratorTask`][crate::workflow::tasks::DebateModeratorTask]
/// so the moderator runs as its own dedicated graph node.
///
/// # Errors
///
/// Returns [`TradingError`] on LLM failure or schema violation.
pub async fn run_debate_moderation(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    let mut executor = RealDebateExecutor {
        bull: BullishResearcher::new(handle, state, transcript_fetch, &config.llm)?,
        bear: BearishResearcher::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: DebateModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let (consensus, usage) = executor.moderate(state).await?;
    state.consensus_summary = Some(consensus);
    Ok(usage)
}
