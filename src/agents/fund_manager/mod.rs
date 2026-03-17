//! Fund Manager Agent — Phase 5 of the TradingAgents pipeline.
//!
//! Reviews the [`TradeProposal`], the three [`RiskReport`] objects, the full
//! `risk_discussion_history`, and the supporting analyst context, then renders an
//! auditable approve/reject [`ExecutionStatus`].
//!
//! A **deterministic safety-net** rejects the proposal immediately — without an LLM
//! call — whenever both the Conservative and Neutral risk reports have
//! `flags_violation == true`.

mod agent;
mod prompt;
mod usage;
mod validation;

#[cfg(test)]
mod tests;

use crate::{
    config::Config,
    error::TradingError,
    state::{AgentTokenUsage, TradingState},
};

pub use self::agent::FundManagerAgent;

/// Maximum characters allowed in the `rationale` field.
pub const MAX_RATIONALE_CHARS: usize = 4_096;

/// Construct a [`FundManagerAgent`] and run it against `state`.
///
/// This is the primary entry point for the downstream `add-graph-orchestration` change.
/// It creates a `DeepThinking` completion model handle from `config`, constructs the
/// agent, and invokes it.
///
/// # Returns
/// [`AgentTokenUsage`] so the upstream orchestrator can incorporate it into a
/// "Fund Manager" [`PhaseTokenUsage`][crate::state::PhaseTokenUsage] entry.
///
/// # Errors
/// - [`TradingError::Config`] for provider or model configuration problems.
/// - [`TradingError::SchemaViolation`] when `trader_proposal` is absent or the LLM
///   returns invalid output.
/// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
pub async fn run_fund_manager(
    state: &mut TradingState,
    config: &Config,
) -> Result<AgentTokenUsage, TradingError> {
    agent::run_fund_manager(state, config).await
}
