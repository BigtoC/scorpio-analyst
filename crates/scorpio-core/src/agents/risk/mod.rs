//! Risk Management Team — Phase 4 of the TradingAgents pipeline.
//!
//! Implements a cyclic risk discussion among three persona agents
//! (Aggressive, Conservative, Neutral) moderated by a [`RiskModerator`]
//! that synthesizes the risk perspectives into a plain-text discussion note
//! stored in [`TradingState::risk_discussion_history`].
//!
//! # Sequential turn order within each round
//!
//! Persona turns are executed **sequentially** within each round
//! (Aggressive → Conservative → Neutral) because each persona's prompt
//! references the other agents' latest same-round views. Parallelising the
//! turns would mean a persona sees stale views from the prior round.

mod aggressive;
mod common;
mod conservative;
mod moderator;
mod neutral;

pub(crate) use self::common::DualRiskStatus;
#[cfg(any(test, feature = "test-helpers"))]
pub(crate) use self::common::render_risk_system_prompt;
pub use aggressive::AggressiveRiskAgent;
pub use conservative::ConservativeRiskAgent;
pub use moderator::RiskModerator;
pub use neutral::NeutralRiskAgent;

use common::{redact_secret_like_values, serialize_risk_report_context};

use crate::{
    config::Config,
    data::adapters::transcripts::TranscriptFetch,
    error::TradingError,
    providers::factory::CompletionModelHandle,
    state::{AgentTokenUsage, DebateMessage, RiskReport, TradingState},
};

// ─── Executor seam ────────────────────────────────────────────────────────────

trait RiskExecutor {
    async fn aggressive_turn(
        &mut self,
        state: &TradingState,
        conservative_response: Option<&str>,
        neutral_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError>;

    async fn conservative_turn(
        &mut self,
        state: &TradingState,
        aggressive_response: Option<&str>,
        neutral_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError>;

    async fn neutral_turn(
        &mut self,
        state: &TradingState,
        aggressive_response: Option<&str>,
        conservative_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError>;

    async fn moderate(
        &mut self,
        state: &TradingState,
    ) -> Result<(String, AgentTokenUsage), TradingError>;
}

struct RealRiskExecutor {
    aggressive: AggressiveRiskAgent,
    conservative: ConservativeRiskAgent,
    neutral: NeutralRiskAgent,
    moderator: RiskModerator,
}

impl RiskExecutor for RealRiskExecutor {
    async fn aggressive_turn(
        &mut self,
        state: &TradingState,
        conservative_response: Option<&str>,
        neutral_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
        self.aggressive
            .run(state, conservative_response, neutral_response)
            .await
    }

    async fn conservative_turn(
        &mut self,
        state: &TradingState,
        aggressive_response: Option<&str>,
        neutral_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
        self.conservative
            .run(state, aggressive_response, neutral_response)
            .await
    }

    async fn neutral_turn(
        &mut self,
        state: &TradingState,
        aggressive_response: Option<&str>,
        conservative_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
        self.neutral
            .run(state, aggressive_response, conservative_response)
            .await
    }

    async fn moderate(
        &mut self,
        state: &TradingState,
    ) -> Result<(String, AgentTokenUsage), TradingError> {
        self.moderator.run(state).await
    }
}

/// Execute a single Aggressive Risk agent turn for one round step.
///
/// Reads the latest conservative and neutral reports from `state`, calls the
/// Aggressive agent, and writes the result back to `state.aggressive_risk_report`
/// and appends to `state.risk_discussion_history`.
///
/// Used by [`AggressiveRiskTask`][crate::workflow::tasks::AggressiveRiskTask]
/// so each graph node performs exactly one agent step.
///
/// # Errors
///
/// - [`TradingError::SchemaViolation`] if `state.trader_proposal` is `None`.
/// - Returns [`TradingError`] on LLM failure.
pub async fn run_aggressive_risk_turn(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_aggressive_risk_turn: trader_proposal is required but not set".to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: RiskModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let prior_conservative = serialize_risk_report_context(state.conservative_risk_report.as_ref());
    let prior_neutral = serialize_risk_report_context(state.neutral_risk_report.as_ref());

    let (report, usage) = executor
        .aggressive_turn(
            state,
            prior_conservative.as_deref(),
            prior_neutral.as_deref(),
        )
        .await?;

    let summary = redact_secret_like_values(&report.assessment);
    state.aggressive_risk_report = Some(report);
    state.risk_discussion_history.push(DebateMessage {
        role: "aggressive_risk".to_owned(),
        content: summary,
    });
    Ok(usage)
}

/// Execute a single Conservative Risk agent turn for one round step.
///
/// Reads the latest aggressive and neutral reports from `state`, calls the
/// Conservative agent, and writes the result back.
///
/// Used by [`ConservativeRiskTask`][crate::workflow::tasks::ConservativeRiskTask].
///
/// # Errors
///
/// - [`TradingError::SchemaViolation`] if `state.trader_proposal` is `None`.
/// - Returns [`TradingError`] on LLM failure.
pub async fn run_conservative_risk_turn(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_conservative_risk_turn: trader_proposal is required but not set"
                .to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: RiskModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let agg_context = serialize_risk_report_context(state.aggressive_risk_report.as_ref());
    let neu_context = serialize_risk_report_context(state.neutral_risk_report.as_ref());

    let (report, usage) = executor
        .conservative_turn(state, agg_context.as_deref(), neu_context.as_deref())
        .await?;

    let summary = redact_secret_like_values(&report.assessment);
    state.conservative_risk_report = Some(report);
    state.risk_discussion_history.push(DebateMessage {
        role: "conservative_risk".to_owned(),
        content: summary,
    });
    Ok(usage)
}

/// Execute a single Neutral Risk agent turn for one round step.
///
/// Reads the latest aggressive and conservative reports from `state`, calls the
/// Neutral agent, and writes the result back.
///
/// Used by [`NeutralRiskTask`][crate::workflow::tasks::NeutralRiskTask].
///
/// # Errors
///
/// - [`TradingError::SchemaViolation`] if `state.trader_proposal` is `None`.
/// - Returns [`TradingError`] on LLM failure.
pub async fn run_neutral_risk_turn(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_neutral_risk_turn: trader_proposal is required but not set".to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: RiskModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let agg_context = serialize_risk_report_context(state.aggressive_risk_report.as_ref());
    let con_context = serialize_risk_report_context(state.conservative_risk_report.as_ref());

    let (report, usage) = executor
        .neutral_turn(state, agg_context.as_deref(), con_context.as_deref())
        .await?;

    let summary = redact_secret_like_values(&report.assessment);
    state.neutral_risk_report = Some(report);
    state.risk_discussion_history.push(DebateMessage {
        role: "neutral_risk".to_owned(),
        content: summary,
    });
    Ok(usage)
}

/// Run the Risk Moderator once, appending the synthesis to
/// `state.risk_discussion_history`.
///
/// Used by [`RiskModeratorTask`][crate::workflow::tasks::RiskModeratorTask]
/// so the moderator runs as its own dedicated graph node.
///
/// # Errors
///
/// - [`TradingError::SchemaViolation`] if `state.trader_proposal` is `None`.
/// - Returns [`TradingError`] on LLM failure.
pub async fn run_risk_moderation(
    state: &mut TradingState,
    transcript_fetch: Option<&TranscriptFetch>,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_risk_moderation: trader_proposal is required but not set".to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, transcript_fetch, &config.llm)?,
        moderator: RiskModerator::new(handle, state, transcript_fetch, &config.llm)?,
    };

    let (synthesis, usage) = executor.moderate(state).await?;
    state.risk_discussion_history.push(DebateMessage {
        role: "risk_moderator".to_owned(),
        content: redact_secret_like_values(&synthesis),
    });
    Ok(usage)
}
