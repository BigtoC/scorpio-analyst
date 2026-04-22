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
//!
//! # Usage
//!
//! ```rust,ignore
//! use scorpio_core::agents::risk::run_risk_discussion;
//!
//! let usages = run_risk_discussion(&mut state, &config, &handle).await?;
//! // state.risk_discussion_history and state.*_risk_report are now populated.
//! ```

mod aggressive;
mod common;
mod conservative;
mod moderator;
mod neutral;
mod prompt;

pub(crate) use self::common::DualRiskStatus;
pub use aggressive::AggressiveRiskAgent;
pub use conservative::ConservativeRiskAgent;
pub use moderator::RiskModerator;
pub use neutral::NeutralRiskAgent;

use common::{redact_text_for_storage, serialize_risk_report_context};

use crate::{
    config::Config,
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

// ─── Inner orchestration loop ─────────────────────────────────────────────────

async fn run_risk_discussion_with_executor<E>(
    state: &mut TradingState,
    max_rounds: u32,
    executor: &mut E,
) -> Result<Vec<AgentTokenUsage>, TradingError>
where
    E: RiskExecutor,
{
    // Validate that a trade proposal exists before making any LLM calls.
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_risk_discussion: trader_proposal is required but not set".to_owned(),
        });
    }

    let capacity = (max_rounds as usize).saturating_mul(3).saturating_add(1);
    let mut all_usages: Vec<AgentTokenUsage> = Vec::with_capacity(capacity);

    for _round in 0..max_rounds {
        let prior_conservative =
            serialize_risk_report_context(state.conservative_risk_report.as_ref());
        let prior_neutral = serialize_risk_report_context(state.neutral_risk_report.as_ref());

        // Aggressive sees the latest available conservative and neutral views.
        let (agg_report, agg_usage) = executor
            .aggressive_turn(
                state,
                prior_conservative.as_deref(),
                prior_neutral.as_deref(),
            )
            .await?;

        let agg_summary = redact_text_for_storage(&agg_report.assessment);
        let agg_context = serialize_risk_report_context(Some(&agg_report));
        state.aggressive_risk_report = Some(agg_report);
        state.risk_discussion_history.push(DebateMessage {
            role: "aggressive_risk".to_owned(),
            content: agg_summary,
        });
        all_usages.push(agg_usage);

        let latest_neutral = serialize_risk_report_context(state.neutral_risk_report.as_ref());

        // Conservative sees the aggressive view from this round and the latest neutral view.
        let (con_report, con_usage) = executor
            .conservative_turn(state, agg_context.as_deref(), latest_neutral.as_deref())
            .await?;

        let con_summary = redact_text_for_storage(&con_report.assessment);
        let con_context = serialize_risk_report_context(Some(&con_report));
        state.conservative_risk_report = Some(con_report);
        state.risk_discussion_history.push(DebateMessage {
            role: "conservative_risk".to_owned(),
            content: con_summary,
        });
        all_usages.push(con_usage);

        // Neutral sees both aggressive and conservative views from this round.
        let (neu_report, neu_usage) = executor
            .neutral_turn(state, agg_context.as_deref(), con_context.as_deref())
            .await?;

        let neu_summary = redact_text_for_storage(&neu_report.assessment);
        state.neutral_risk_report = Some(neu_report);
        state.risk_discussion_history.push(DebateMessage {
            role: "neutral_risk".to_owned(),
            content: neu_summary,
        });
        all_usages.push(neu_usage);
    }

    // Moderator synthesises the full discussion once, regardless of round count.
    let (synthesis, mod_usage) = executor.moderate(state).await?;
    state.risk_discussion_history.push(DebateMessage {
        role: "risk_moderator".to_owned(),
        content: redact_text_for_storage(&synthesis),
    });
    all_usages.push(mod_usage);

    Ok(all_usages)
}

/// Run the full risk discussion loop for Phase 4.
///
/// Executes `config.llm.max_risk_rounds` rounds of Aggressive → Conservative →
/// Neutral risk analysis sequentially within each round, then invokes the Risk
/// Moderator to produce a plain-text synthesis.
///
/// # Sequential turn order
///
/// Persona turns are **sequential** within each round because each persona's
/// prompt references the other agents' latest same-round views.
///
/// # Returns
///
/// A `Vec<AgentTokenUsage>` with `3 * max_risk_rounds + 1` entries
/// (Aggressive + Conservative + Neutral per round, plus the Moderator).
///
/// # Errors
///
/// - [`TradingError::SchemaViolation`] if `state.trader_proposal` is `None`.
/// - Returns the first [`TradingError`] encountered — any LLM failure aborts
///   the discussion immediately. Schema violations are propagated unchanged.
pub async fn run_risk_discussion(
    state: &mut TradingState,
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<Vec<AgentTokenUsage>, TradingError> {
    let max_rounds = config.llm.max_risk_rounds;
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, &config.llm)?,
        moderator: RiskModerator::new(handle, state, &config.llm)?,
    };

    run_risk_discussion_with_executor(state, max_rounds, &mut executor).await
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
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_aggressive_risk_turn: trader_proposal is required but not set".to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, &config.llm)?,
        moderator: RiskModerator::new(handle, state, &config.llm)?,
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

    let summary = redact_text_for_storage(&report.assessment);
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
        aggressive: AggressiveRiskAgent::new(handle, state, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, &config.llm)?,
        moderator: RiskModerator::new(handle, state, &config.llm)?,
    };

    let agg_context = serialize_risk_report_context(state.aggressive_risk_report.as_ref());
    let neu_context = serialize_risk_report_context(state.neutral_risk_report.as_ref());

    let (report, usage) = executor
        .conservative_turn(state, agg_context.as_deref(), neu_context.as_deref())
        .await?;

    let summary = redact_text_for_storage(&report.assessment);
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
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_neutral_risk_turn: trader_proposal is required but not set".to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, &config.llm)?,
        moderator: RiskModerator::new(handle, state, &config.llm)?,
    };

    let agg_context = serialize_risk_report_context(state.aggressive_risk_report.as_ref());
    let con_context = serialize_risk_report_context(state.conservative_risk_report.as_ref());

    let (report, usage) = executor
        .neutral_turn(state, agg_context.as_deref(), con_context.as_deref())
        .await?;

    let summary = redact_text_for_storage(&report.assessment);
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
    config: &Config,
    handle: &CompletionModelHandle,
) -> Result<AgentTokenUsage, TradingError> {
    if state.trader_proposal.is_none() {
        return Err(TradingError::SchemaViolation {
            message: "run_risk_moderation: trader_proposal is required but not set".to_owned(),
        });
    }
    let mut executor = RealRiskExecutor {
        aggressive: AggressiveRiskAgent::new(handle, state, &config.llm)?,
        conservative: ConservativeRiskAgent::new(handle, state, &config.llm)?,
        neutral: NeutralRiskAgent::new(handle, state, &config.llm)?,
        moderator: RiskModerator::new(handle, state, &config.llm)?,
    };

    let (synthesis, usage) = executor.moderate(state).await?;
    state.risk_discussion_history.push(DebateMessage {
        role: "risk_moderator".to_owned(),
        content: redact_text_for_storage(&synthesis),
    });
    Ok(usage)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::state::{AgentTokenUsage, RiskLevel, RiskReport, TradeAction, TradeProposal};
    use crate::{error::TradingError, state::TokenUsageTracker};
    use uuid::Uuid;

    use super::*;

    // ── Mock executor ─────────────────────────────────────────────────────

    struct MockRiskExecutor {
        agg_calls: usize,
        con_calls: usize,
        neu_calls: usize,
        mod_calls: usize,
        fail_agg_on_call: Option<usize>,
        fail_con_on_call: Option<usize>,
        fail_mod_on_call: Option<usize>,
        token_counts_available: bool,
        seen_aggressive_inputs: Vec<(Option<String>, Option<String>)>,
        seen_conservative_inputs: Vec<(Option<String>, Option<String>)>,
        seen_neutral_inputs: Vec<(Option<String>, Option<String>)>,
    }

    impl MockRiskExecutor {
        fn new() -> Self {
            Self {
                agg_calls: 0,
                con_calls: 0,
                neu_calls: 0,
                mod_calls: 0,
                fail_agg_on_call: None,
                fail_con_on_call: None,
                fail_mod_on_call: None,
                token_counts_available: false,
                seen_aggressive_inputs: Vec::new(),
                seen_conservative_inputs: Vec::new(),
                seen_neutral_inputs: Vec::new(),
            }
        }

        fn with_agg_failure(call: usize) -> Self {
            Self {
                fail_agg_on_call: Some(call),
                ..Self::new()
            }
        }

        fn with_con_failure(call: usize) -> Self {
            Self {
                fail_con_on_call: Some(call),
                ..Self::new()
            }
        }

        fn with_mod_failure(call: usize) -> Self {
            Self {
                fail_mod_on_call: Some(call),
                ..Self::new()
            }
        }
    }

    impl RiskExecutor for MockRiskExecutor {
        async fn aggressive_turn(
            &mut self,
            _state: &TradingState,
            conservative_response: Option<&str>,
            neutral_response: Option<&str>,
        ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
            self.seen_aggressive_inputs.push((
                conservative_response.map(str::to_owned),
                neutral_response.map(str::to_owned),
            ));
            self.agg_calls += 1;
            if self.fail_agg_on_call == Some(self.agg_calls) {
                return Err(TradingError::Rig(format!(
                    "aggressive failed on call {}",
                    self.agg_calls
                )));
            }
            Ok((
                RiskReport {
                    risk_level: RiskLevel::Aggressive,
                    assessment: format!("Aggressive round {}.", self.agg_calls),
                    recommended_adjustments: vec![],
                    flags_violation: false,
                },
                AgentTokenUsage::unavailable("Aggressive Risk Analyst", "o3", 1),
            ))
        }

        async fn conservative_turn(
            &mut self,
            _state: &TradingState,
            aggressive_response: Option<&str>,
            neutral_response: Option<&str>,
        ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
            self.seen_conservative_inputs.push((
                aggressive_response.map(str::to_owned),
                neutral_response.map(str::to_owned),
            ));
            self.con_calls += 1;
            if self.fail_con_on_call == Some(self.con_calls) {
                return Err(TradingError::Rig(format!(
                    "conservative failed on call {}",
                    self.con_calls
                )));
            }
            Ok((
                RiskReport {
                    risk_level: RiskLevel::Conservative,
                    assessment: format!("Conservative round {}.", self.con_calls),
                    recommended_adjustments: vec![],
                    flags_violation: true,
                },
                if self.token_counts_available {
                    AgentTokenUsage {
                        agent_name: "Conservative Risk Analyst".to_owned(),
                        model_id: "o3".to_owned(),
                        token_counts_available: false,
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        total_tokens: 0,
                        latency_ms: 1,
                        rate_limit_wait_ms: 0,
                    }
                } else {
                    AgentTokenUsage::unavailable("Conservative Risk Analyst", "o3", 1)
                },
            ))
        }

        async fn neutral_turn(
            &mut self,
            _state: &TradingState,
            aggressive_response: Option<&str>,
            conservative_response: Option<&str>,
        ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
            self.seen_neutral_inputs.push((
                aggressive_response.map(str::to_owned),
                conservative_response.map(str::to_owned),
            ));
            self.neu_calls += 1;
            Ok((
                RiskReport {
                    risk_level: RiskLevel::Neutral,
                    assessment: format!("Neutral round {}.", self.neu_calls),
                    recommended_adjustments: vec![],
                    flags_violation: false,
                },
                AgentTokenUsage::unavailable("Neutral Risk Analyst", "o3", 1),
            ))
        }

        async fn moderate(
            &mut self,
            _state: &TradingState,
        ) -> Result<(String, AgentTokenUsage), TradingError> {
            self.mod_calls += 1;
            if self.fail_mod_on_call == Some(self.mod_calls) {
                return Err(TradingError::Rig("moderator failed".to_owned()));
            }
            Ok((
                "Violation status: dual-risk escalation present. Proceed with caution.".to_owned(),
                AgentTokenUsage::unavailable("Risk Moderator", "o3", 1),
            ))
        }
    }

    fn make_state_with_proposal() -> TradingState {
        TradingState {
            execution_id: Uuid::new_v4(),
            asset_symbol: "AAPL".to_owned(),
            target_date: "2026-03-15".to_owned(),
            current_price: None,
            market_volatility: None,
            fundamental_metrics: None,
            technical_indicators: None,
            market_sentiment: None,
            macro_news: None,
            debate_history: Vec::new(),
            consensus_summary: None,
            trader_proposal: Some(TradeProposal {
                action: TradeAction::Buy,
                target_price: 200.0,
                stop_loss: 180.0,
                confidence: 0.75,
                rationale: "Growth outlook".to_owned(),
                valuation_assessment: None,
                scenario_valuation: None,
            }),
            risk_discussion_history: Vec::new(),
            aggressive_risk_report: None,
            neutral_risk_report: None,
            conservative_risk_report: None,
            final_execution_status: None,
            evidence_fundamental: None,
            evidence_technical: None,
            evidence_sentiment: None,
            evidence_news: None,
            enrichment_event_news: Default::default(),
            enrichment_consensus: Default::default(),
            data_coverage: None,
            provenance_summary: None,
            prior_thesis: None,
            current_thesis: None,
            token_usage: TokenUsageTracker::default(),
            derived_valuation: None,
            analysis_pack_name: None,
            analysis_runtime_policy: None,
        }
    }

    fn make_state_no_proposal() -> TradingState {
        let mut s = make_state_with_proposal();
        s.trader_proposal = None;
        s
    }

    // ── Task 5.7: 1 round populates all 3 RiskReport fields ──────────────

    #[test]
    fn one_round_populates_all_three_risk_reports() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        let usages = rt
            .block_on(run_risk_discussion_with_executor(&mut state, 1, &mut exec))
            .unwrap();

        assert!(state.aggressive_risk_report.is_some());
        assert!(state.conservative_risk_report.is_some());
        assert!(state.neutral_risk_report.is_some());
        // 3 persona + 1 moderator = 4 history entries
        assert_eq!(state.risk_discussion_history.len(), 4);
        assert_eq!(usages.len(), 4);
    }

    // ── Task 5.8: 2 rounds → 6 persona + 1 moderator entries ────────────

    #[test]
    fn two_rounds_produce_six_persona_messages_plus_moderator() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        let usages = rt
            .block_on(run_risk_discussion_with_executor(&mut state, 2, &mut exec))
            .unwrap();

        assert_eq!(state.risk_discussion_history.len(), 7);
        assert_eq!(usages.len(), 7);

        // Verify roles alternate correctly per round
        let roles: Vec<&str> = state
            .risk_discussion_history
            .iter()
            .map(|m| m.role.as_str())
            .collect();
        assert_eq!(
            roles,
            vec![
                "aggressive_risk",
                "conservative_risk",
                "neutral_risk",
                "aggressive_risk",
                "conservative_risk",
                "neutral_risk",
                "risk_moderator",
            ]
        );

        // All 3 risk reports populated
        assert!(state.aggressive_risk_report.is_some());
        assert!(state.conservative_risk_report.is_some());
        assert!(state.neutral_risk_report.is_some());

        // Last usage is moderator
        assert_eq!(usages.last().unwrap().agent_name, "Risk Moderator");
    }

    // ── Task 5.9: 0 rounds → no persona messages, moderator still invoked ─

    #[test]
    fn zero_rounds_no_persona_messages_moderator_still_runs() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        let usages = rt
            .block_on(run_risk_discussion_with_executor(&mut state, 0, &mut exec))
            .unwrap();

        assert_eq!(exec.agg_calls, 0);
        assert_eq!(exec.con_calls, 0);
        assert_eq!(exec.neu_calls, 0);
        assert_eq!(exec.mod_calls, 1);
        assert_eq!(state.risk_discussion_history.len(), 1);
        assert_eq!(state.risk_discussion_history[0].role, "risk_moderator");
        assert_eq!(usages.len(), 1);
    }

    // ── Task 5.10: Risk agent failure in round 2 propagates and aborts ───

    #[test]
    fn round_two_failure_aborts_and_propagates_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::with_agg_failure(2);

        let result = rt.block_on(run_risk_discussion_with_executor(&mut state, 3, &mut exec));

        assert!(matches!(result, Err(TradingError::Rig(_))));
        // Round 1 completed: 3 persona messages
        assert_eq!(state.risk_discussion_history.len(), 3);
        // Moderator never ran
        assert_eq!(exec.mod_calls, 0);
    }

    #[test]
    fn conservative_failure_in_round_one_aborts() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::with_con_failure(1);

        let result = rt.block_on(run_risk_discussion_with_executor(&mut state, 2, &mut exec));

        assert!(matches!(result, Err(TradingError::Rig(_))));
        // Aggressive round 1 ran, conservative failed → 1 history entry
        assert_eq!(state.risk_discussion_history.len(), 1);
    }

    // ── Task 5.11: Missing trader_proposal returns error before LLM calls ─

    #[test]
    fn missing_trader_proposal_returns_error_before_llm_calls() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_no_proposal();
        let mut exec = MockRiskExecutor::new();

        let result = rt.block_on(run_risk_discussion_with_executor(&mut state, 1, &mut exec));

        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
        assert_eq!(exec.agg_calls, 0);
        assert_eq!(exec.mod_calls, 0);
    }

    // ── Task 5.12: AgentTokenUsage count = 3 * rounds + 1 ────────────────

    #[test]
    fn token_usage_count_equals_three_rounds_plus_moderator() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        let usages = rt
            .block_on(run_risk_discussion_with_executor(&mut state, 3, &mut exec))
            .unwrap();

        // 3 rounds * 3 personas + 1 moderator = 10
        assert_eq!(usages.len(), 10);
        assert_eq!(usages.last().unwrap().agent_name, "Risk Moderator");
    }

    // ── Task 5.13: token_counts_available = false when provider doesn't expose counts ─

    #[test]
    fn token_counts_unavailable_when_all_zero() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        let usages = rt
            .block_on(run_risk_discussion_with_executor(&mut state, 1, &mut exec))
            .unwrap();

        assert!(usages.iter().all(|u| !u.token_counts_available));
    }

    // ── Task 6.1: roles are correct and risk reports all populated ────────

    #[test]
    fn e2e_roles_are_correct_and_reports_populated() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        rt.block_on(run_risk_discussion_with_executor(&mut state, 2, &mut exec))
            .unwrap();

        for msg in &state.risk_discussion_history {
            assert!(
                matches!(
                    msg.role.as_str(),
                    "aggressive_risk" | "conservative_risk" | "neutral_risk" | "risk_moderator"
                ),
                "unexpected role: {}",
                msg.role
            );
        }

        assert_eq!(
            state.aggressive_risk_report.as_ref().unwrap().risk_level,
            RiskLevel::Aggressive
        );
        assert_eq!(
            state.conservative_risk_report.as_ref().unwrap().risk_level,
            RiskLevel::Conservative
        );
        assert_eq!(
            state.neutral_risk_report.as_ref().unwrap().risk_level,
            RiskLevel::Neutral
        );
    }

    // ── Task 6.3: token usage collection order ────────────────────────────

    #[test]
    fn token_usage_order_is_agg_con_neu_per_round_then_moderator() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        let usages = rt
            .block_on(run_risk_discussion_with_executor(&mut state, 2, &mut exec))
            .unwrap();

        let expected_names = [
            "Aggressive Risk Analyst",
            "Conservative Risk Analyst",
            "Neutral Risk Analyst",
            "Aggressive Risk Analyst",
            "Conservative Risk Analyst",
            "Neutral Risk Analyst",
            "Risk Moderator",
        ];
        let actual_names: Vec<&str> = usages.iter().map(|u| u.agent_name.as_str()).collect();
        assert_eq!(actual_names, expected_names);
    }

    // ── Task 6.4: flags_violation preserved through discussion loop ───────

    #[test]
    fn flags_violation_preserved_through_loop() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::new();

        rt.block_on(run_risk_discussion_with_executor(&mut state, 1, &mut exec))
            .unwrap();

        // Mock always returns flags_violation=false for aggressive/neutral, true for conservative
        assert!(
            !state
                .aggressive_risk_report
                .as_ref()
                .unwrap()
                .flags_violation
        );
        assert!(
            state
                .conservative_risk_report
                .as_ref()
                .unwrap()
                .flags_violation
        );
        assert!(!state.neutral_risk_report.as_ref().unwrap().flags_violation);
    }

    #[test]
    fn same_round_peer_views_are_passed_as_serialized_reports() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        state.conservative_risk_report = Some(RiskReport {
            risk_level: RiskLevel::Conservative,
            assessment: "Prior conservative view".to_owned(),
            recommended_adjustments: vec!["Reduce size".to_owned()],
            flags_violation: true,
        });
        state.neutral_risk_report = Some(RiskReport {
            risk_level: RiskLevel::Neutral,
            assessment: "Prior neutral view".to_owned(),
            recommended_adjustments: vec!["Tighten stop".to_owned()],
            flags_violation: false,
        });
        let mut exec = MockRiskExecutor::new();

        rt.block_on(run_risk_discussion_with_executor(&mut state, 1, &mut exec))
            .unwrap();

        let aggressive_inputs = &exec.seen_aggressive_inputs[0];
        assert!(
            aggressive_inputs
                .0
                .as_ref()
                .unwrap()
                .contains("flags_violation")
        );
        assert!(
            aggressive_inputs
                .1
                .as_ref()
                .unwrap()
                .contains("Tighten stop")
        );

        let conservative_inputs = &exec.seen_conservative_inputs[0];
        assert!(
            conservative_inputs
                .0
                .as_ref()
                .unwrap()
                .contains("Aggressive")
        );
        assert!(
            conservative_inputs
                .0
                .as_ref()
                .unwrap()
                .contains("flags_violation")
        );
        assert!(
            conservative_inputs
                .1
                .as_ref()
                .unwrap()
                .contains("Prior neutral view")
        );

        let neutral_inputs = &exec.seen_neutral_inputs[0];
        assert!(neutral_inputs.0.as_ref().unwrap().contains("Aggressive"));
        assert!(neutral_inputs.1.as_ref().unwrap().contains("Conservative"));
    }

    #[test]
    fn moderator_failure_aborts_discussion() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut state = make_state_with_proposal();
        let mut exec = MockRiskExecutor::with_mod_failure(1);

        let result = rt.block_on(run_risk_discussion_with_executor(&mut state, 1, &mut exec));

        assert!(matches!(result, Err(TradingError::Rig(_))));
        assert_eq!(state.risk_discussion_history.len(), 3);
    }
}
