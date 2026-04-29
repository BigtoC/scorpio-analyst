use std::sync::Arc;

use graph_flow::Context;

use crate::{
    state::{
        AgentTokenUsage, DebateMessage, Decision, ExecutionStatus, FundamentalData, NewsData,
        PhaseTokenUsage, RiskLevel, RiskReport, SentimentData, TechnicalData, ThesisMemory,
        TradeAction, TradeProposal,
    },
    workflow::{
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        snapshot::{SnapshotPhase, SnapshotStore},
    },
};

use super::{
    accounting::{debate_moderator_accounting, risk_moderator_accounting},
    common::{
        self, ANALYST_FUNDAMENTAL, ANALYST_NEWS, ANALYST_PREFIX, ANALYST_SENTIMENT,
        ANALYST_TECHNICAL, DEBATE_USAGE_PREFIX, RISK_USAGE_PREFIX, write_round_usage,
    },
};

pub async fn write_round_debate_usage(
    context: &Context,
    round: u32,
    bull_usage: &AgentTokenUsage,
    bear_usage: &AgentTokenUsage,
) {
    write_round_usage(context, DEBATE_USAGE_PREFIX, round, "bull", bull_usage)
        .await
        .expect("test: debate bull usage write");
    write_round_usage(context, DEBATE_USAGE_PREFIX, round, "bear", bear_usage)
        .await
        .expect("test: debate bear usage write");
}

pub async fn write_round_risk_usage(
    context: &Context,
    round: u32,
    agg_usage: &AgentTokenUsage,
    con_usage: &AgentTokenUsage,
    neu_usage: &AgentTokenUsage,
) {
    write_round_usage(context, RISK_USAGE_PREFIX, round, "agg", agg_usage)
        .await
        .expect("test: risk agg usage write");
    write_round_usage(context, RISK_USAGE_PREFIX, round, "con", con_usage)
        .await
        .expect("test: risk con usage write");
    write_round_usage(context, RISK_USAGE_PREFIX, round, "neu", neu_usage)
        .await
        .expect("test: risk neu usage write");
}

/// Run the accounting portion of [`super::DebateModeratorTask`] with a supplied
/// moderation usage value.
pub async fn run_debate_moderator_accounting(
    context: &Context,
    mod_usage: &AgentTokenUsage,
    _snapshot_store: Arc<SnapshotStore>,
) {
    let phase_start = std::time::Instant::now();
    let mut state = deserialize_state_from_context(context)
        .await
        .expect("test: state deserialization");

    debate_moderator_accounting(context, &mut state, mod_usage, &phase_start).await;

    serialize_state_to_context(&state, context)
        .await
        .expect("test: state serialization");
}

/// Run the accounting portion of [`super::RiskModeratorTask`] with a supplied
/// moderation usage value.
pub async fn run_risk_moderator_accounting(
    context: &Context,
    mod_usage: &AgentTokenUsage,
    _snapshot_store: Arc<SnapshotStore>,
) {
    let phase_start = std::time::Instant::now();
    let mut state = deserialize_state_from_context(context)
        .await
        .expect("test: state deserialization");

    risk_moderator_accounting(context, &mut state, mod_usage, &phase_start).await;

    serialize_state_to_context(&state, context)
        .await
        .expect("test: state serialization");
}

/// Synthetic [`AgentTokenUsage`] with deterministic values.
pub fn stub_usage(agent_name: &str) -> AgentTokenUsage {
    AgentTokenUsage {
        agent_name: agent_name.to_owned(),
        model_id: "stub-model".to_owned(),
        token_counts_available: true,
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        latency_ms: 1,
        rate_limit_wait_ms: 0,
    }
}

/// Stub analyst child that writes synthetic data for one analyst type.
pub struct StubAnalystChild {
    analyst_key: &'static str,
    task_id: String,
}

impl StubAnalystChild {
    pub fn fundamental() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_FUNDAMENTAL,
            task_id: "fundamental_analyst".to_owned(),
        })
    }

    pub fn sentiment() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_SENTIMENT,
            task_id: "sentiment_analyst".to_owned(),
        })
    }

    pub fn news() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_NEWS,
            task_id: "news_analyst".to_owned(),
        })
    }

    pub fn technical() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_TECHNICAL,
            task_id: "technical_analyst".to_owned(),
        })
    }
}

#[async_trait::async_trait]
impl graph_flow::Task for StubAnalystChild {
    fn id(&self) -> &str {
        &self.task_id
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        use crate::workflow::context_bridge::write_prefixed_result;

        let _state: crate::state::TradingState = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubAnalystChild({}): orchestration corruption: state deserialization failed: {error}",
                    self.analyst_key
                ))
            })?;

        match self.analyst_key {
            ANALYST_FUNDAMENTAL => {
                let data = FundamentalData {
                    revenue_growth_pct: Some(12.5),
                    pe_ratio: Some(24.5),
                    eps: Some(6.05),
                    current_ratio: None,
                    debt_to_equity: None,
                    gross_margin: None,
                    net_income: None,
                    insider_transactions: vec![],
                    summary: "stub: strong fundamentals".to_owned(),
                };
                write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_FUNDAMENTAL, &data)
                    .await
                    .map_err(|error| {
                        graph_flow::GraphError::TaskExecutionFailed(format!(
                            "StubAnalystChild(fundamental): orchestration corruption: context write failed: {error}"
                        ))
                    })?;
            }
            ANALYST_SENTIMENT => {
                let data = SentimentData {
                    overall_score: 0.72,
                    source_breakdown: vec![],
                    engagement_peaks: vec![],
                    summary: "stub: positive sentiment".to_owned(),
                };
                write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_SENTIMENT, &data)
                    .await
                    .map_err(|error| {
                        graph_flow::GraphError::TaskExecutionFailed(format!(
                            "StubAnalystChild(sentiment): orchestration corruption: context write failed: {error}"
                        ))
                    })?;
            }
            ANALYST_NEWS => {
                let data = NewsData {
                    articles: vec![],
                    macro_events: vec![],
                    summary: "stub: no major news".to_owned(),
                };
                write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_NEWS, &data)
                    .await
                    .map_err(|error| {
                        graph_flow::GraphError::TaskExecutionFailed(format!(
                            "StubAnalystChild(news): orchestration corruption: context write failed: {error}"
                        ))
                    })?;
            }
            ANALYST_TECHNICAL => {
                let data = TechnicalData {
                    rsi: Some(55.0),
                    macd: None,
                    atr: None,
                    sma_20: None,
                    sma_50: None,
                    ema_12: None,
                    ema_26: None,
                    bollinger_upper: None,
                    bollinger_lower: None,
                    support_level: None,
                    resistance_level: None,
                    volume_avg: None,
                    summary: "stub: neutral technical".to_owned(),
                    options_summary: None,
                    options_context: None,
                };
                write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_TECHNICAL, &data)
                    .await
                    .map_err(|error| {
                        graph_flow::GraphError::TaskExecutionFailed(format!(
                            "StubAnalystChild(technical): orchestration corruption: context write failed: {error}"
                        ))
                    })?;
            }
            other => {
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubAnalystChild: unknown analyst key '{other}'"
                )));
            }
        }

        common::write_flag(&context, self.analyst_key, true).await;
        let usage = stub_usage(&format!("Stub {} Analyst", self.analyst_key));
        common::write_analyst_usage(&context, self.analyst_key, &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubAnalystChild({}): usage write failed: {error}",
                    self.analyst_key
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubBullishResearcherTask;

#[async_trait::async_trait]
impl graph_flow::Task for StubBullishResearcherTask {
    fn id(&self) -> &str {
        "bullish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubBullishResearcherTask: state deser failed: {error}"
                ))
            })?;

        state.debate_history.push(DebateMessage {
            role: "bullish_researcher".to_owned(),
            content: "stub: bullish argument — strong growth outlook".to_owned(),
        });

        let current_round: u32 = context.get(super::KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage = stub_usage("Bullish Researcher");
        write_round_usage(&context, DEBATE_USAGE_PREFIX, this_round, "bull", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubBullishResearcherTask: round usage write failed: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubBullishResearcherTask: state ser failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubBearishResearcherTask;

#[async_trait::async_trait]
impl graph_flow::Task for StubBearishResearcherTask {
    fn id(&self) -> &str {
        "bearish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubBearishResearcherTask: state deser failed: {error}"
                ))
            })?;

        state.debate_history.push(DebateMessage {
            role: "bearish_researcher".to_owned(),
            content: "stub: bearish argument — overvaluation risk".to_owned(),
        });

        let current_round: u32 = context.get(super::KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage = stub_usage("Bearish Researcher");
        write_round_usage(&context, DEBATE_USAGE_PREFIX, this_round, "bear", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubBearishResearcherTask: round usage write failed: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubBearishResearcherTask: state ser failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubDebateModeratorTask {
    pub snapshot_store: Arc<SnapshotStore>,
}

#[async_trait::async_trait]
impl graph_flow::Task for StubDebateModeratorTask {
    fn id(&self) -> &str {
        "debate_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubDebateModeratorTask: state deser failed: {error}"
                ))
            })?;

        state.consensus_summary = Some("stub: moderator consensus — cautiously bullish".to_owned());
        let mod_usage = stub_usage("Debate Moderator");

        let is_final =
            debate_moderator_accounting(&context, &mut state, &mod_usage, &phase_start).await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubDebateModeratorTask: state ser failed: {error}"
                ))
            })?;

        if is_final {
            let execution_id = state.execution_id.to_string();
            self.snapshot_store
                .save_snapshot(
                    &execution_id,
                    SnapshotPhase::ResearcherDebate,
                    &state,
                    Some(&[mod_usage]),
                )
                .await
                .map_err(|error| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "StubDebateModeratorTask: snapshot save failed: {error}"
                    ))
                })?;
        }

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubTraderTask {
    pub snapshot_store: Arc<SnapshotStore>,
}

#[async_trait::async_trait]
impl graph_flow::Task for StubTraderTask {
    fn id(&self) -> &str {
        "trader"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubTraderTask: state deser failed: {error}"
                ))
            })?;

        state.trader_proposal = Some(TradeProposal {
            action: TradeAction::Buy,
            target_price: 195.0,
            stop_loss: 180.0,
            confidence: 0.75,
            rationale: "stub: buy on strong fundamentals and positive sentiment".to_owned(),
            valuation_assessment: None,
            scenario_valuation: None,
        });

        let usage = stub_usage("Trader");
        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Trader Synthesis".to_owned(),
            agent_usage: vec![usage.clone()],
            phase_prompt_tokens: usage.prompt_tokens,
            phase_completion_tokens: usage.completion_tokens,
            phase_total_tokens: usage.total_tokens,
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
        });

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubTraderTask: state ser failed: {error}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(&execution_id, SnapshotPhase::Trader, &state, Some(&[usage]))
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubTraderTask: snapshot save failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubAggressiveRiskTask;

#[async_trait::async_trait]
impl graph_flow::Task for StubAggressiveRiskTask {
    fn id(&self) -> &str {
        "aggressive_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubAggressiveRiskTask: state deser failed: {error}"
                ))
            })?;

        state.aggressive_risk_report = Some(RiskReport {
            risk_level: RiskLevel::Aggressive,
            assessment: "stub: acceptable risk/reward ratio".to_owned(),
            recommended_adjustments: vec![],
            flags_violation: false,
        });
        state.risk_discussion_history.push(DebateMessage {
            role: "aggressive_risk".to_owned(),
            content: "stub: risk is manageable, proceed".to_owned(),
        });

        let current_round: u32 = context.get(super::KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage = stub_usage("Aggressive Risk");
        write_round_usage(&context, RISK_USAGE_PREFIX, this_round, "agg", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubAggressiveRiskTask: round usage write failed: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubAggressiveRiskTask: state ser failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubConservativeRiskTask;

#[async_trait::async_trait]
impl graph_flow::Task for StubConservativeRiskTask {
    fn id(&self) -> &str {
        "conservative_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubConservativeRiskTask: state deser failed: {error}"
                ))
            })?;

        state.conservative_risk_report = Some(RiskReport {
            risk_level: RiskLevel::Conservative,
            assessment: "stub: within risk tolerances".to_owned(),
            recommended_adjustments: vec!["tighten stop-loss by 2%".to_owned()],
            flags_violation: false,
        });
        state.risk_discussion_history.push(DebateMessage {
            role: "conservative_risk".to_owned(),
            content: "stub: acceptable with tighter stop-loss".to_owned(),
        });

        let current_round: u32 = context.get(super::KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage = stub_usage("Conservative Risk");
        write_round_usage(&context, RISK_USAGE_PREFIX, this_round, "con", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubConservativeRiskTask: round usage write failed: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubConservativeRiskTask: state ser failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubNeutralRiskTask;

#[async_trait::async_trait]
impl graph_flow::Task for StubNeutralRiskTask {
    fn id(&self) -> &str {
        "neutral_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubNeutralRiskTask: state deser failed: {error}"
                ))
            })?;

        state.neutral_risk_report = Some(RiskReport {
            risk_level: RiskLevel::Neutral,
            assessment: "stub: balanced risk assessment".to_owned(),
            recommended_adjustments: vec![],
            flags_violation: false,
        });
        state.risk_discussion_history.push(DebateMessage {
            role: "neutral_risk".to_owned(),
            content: "stub: balanced view, proceed with caution".to_owned(),
        });

        let current_round: u32 = context.get(super::KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage = stub_usage("Neutral Risk");
        write_round_usage(&context, RISK_USAGE_PREFIX, this_round, "neu", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubNeutralRiskTask: round usage write failed: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubNeutralRiskTask: state ser failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubRiskModeratorTask {
    pub snapshot_store: Arc<SnapshotStore>,
}

#[async_trait::async_trait]
impl graph_flow::Task for StubRiskModeratorTask {
    fn id(&self) -> &str {
        "risk_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubRiskModeratorTask: state deser failed: {error}"
                ))
            })?;

        state.risk_discussion_history.push(DebateMessage {
            role: "risk_moderator".to_owned(),
            content: "stub: moderator synthesis - risk views consolidated".to_owned(),
        });

        let mod_usage = stub_usage("Risk Moderator");
        let is_final =
            risk_moderator_accounting(&context, &mut state, &mod_usage, &phase_start).await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubRiskModeratorTask: state ser failed: {error}"
                ))
            })?;

        if is_final {
            let execution_id = state.execution_id.to_string();
            self.snapshot_store
                .save_snapshot(
                    &execution_id,
                    SnapshotPhase::RiskDiscussion,
                    &state,
                    Some(&[mod_usage]),
                )
                .await
                .map_err(|error| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "StubRiskModeratorTask: snapshot save failed: {error}"
                    ))
                })?;
        }

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

pub struct StubFundManagerTask {
    pub snapshot_store: Arc<SnapshotStore>,
}

#[async_trait::async_trait]
impl graph_flow::Task for StubFundManagerTask {
    fn id(&self) -> &str {
        "fund_manager"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubFundManagerTask: state deser failed: {error}"
                ))
            })?;

        state.final_execution_status = Some(ExecutionStatus {
            decision: Decision::Approved,
            action: TradeAction::Buy,
            rationale: "stub: approved — risk within tolerances".to_owned(),
            decided_at: "2026-03-20T00:00:00Z".to_owned(),
            entry_guidance: None,
            suggested_position: None,
        });
        state.current_thesis = Some(ThesisMemory {
            symbol: state.asset_symbol.clone(),
            action: "Buy".to_owned(),
            decision: "Approved".to_owned(),
            rationale: "stub: approved — risk within tolerances".to_owned(),
            summary: None,
            execution_id: state.execution_id.to_string(),
            target_date: state.target_date.clone(),
            captured_at: chrono::Utc::now(),
        });

        let usage = stub_usage("Fund Manager");
        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Fund Manager Decision".to_owned(),
            agent_usage: vec![usage.clone()],
            phase_prompt_tokens: usage.prompt_tokens,
            phase_completion_tokens: usage.completion_tokens,
            phase_total_tokens: usage.total_tokens,
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
        });

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubFundManagerTask: state ser failed: {error}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(
                &execution_id,
                SnapshotPhase::FundManager,
                &state,
                Some(&[usage]),
            )
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "StubFundManagerTask: snapshot save failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::End,
        ))
    }
}

/// Stub technical analyst child that returns caller-supplied [`TechnicalData`].
///
/// Use when a test needs specific `options_context` values; the default
/// neutral fixture from [`StubAnalystChild::technical`] has `options_context: None`.
pub struct CustomTechnicalAnalystChild {
    data: TechnicalData,
}

impl CustomTechnicalAnalystChild {
    /// Wrap `data` in an `Arc`-boxed task ready for use in a [`FanOutTask`].
    pub fn new(data: TechnicalData) -> Arc<Self> {
        Arc::new(Self { data })
    }
}

#[async_trait::async_trait]
impl graph_flow::Task for CustomTechnicalAnalystChild {
    fn id(&self) -> &str {
        "technical_analyst"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
        use crate::workflow::context_bridge::write_prefixed_result;

        write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_TECHNICAL, &self.data)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "CustomTechnicalAnalystChild: context write failed: {error}"
                ))
            })?;

        common::write_flag(&context, ANALYST_TECHNICAL, true).await;
        let usage = stub_usage("Stub Technical Analyst");
        common::write_analyst_usage(&context, ANALYST_TECHNICAL, &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "CustomTechnicalAnalystChild: usage write failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::Continue,
        ))
    }
}

/// Replace all LLM-calling tasks in the pipeline with deterministic stubs.
pub fn replace_with_stubs(
    pipeline: &crate::workflow::TradingPipeline,
    snapshot_store: Arc<SnapshotStore>,
) -> Result<(), crate::workflow::pipeline::WorkflowTestSeamError> {
    use graph_flow::fanout::FanOutTask;

    let stub_fanout = FanOutTask::new(
        "analyst_fanout",
        vec![
            StubAnalystChild::fundamental(),
            StubAnalystChild::sentiment(),
            StubAnalystChild::news(),
            StubAnalystChild::technical(),
        ],
    );
    pipeline.replace_task_for_test(stub_fanout)?;

    pipeline.replace_task_for_test(Arc::new(StubBullishResearcherTask))?;
    pipeline.replace_task_for_test(Arc::new(StubBearishResearcherTask))?;
    pipeline.replace_task_for_test(Arc::new(StubDebateModeratorTask {
        snapshot_store: Arc::clone(&snapshot_store),
    }))?;

    pipeline.replace_task_for_test(Arc::new(StubTraderTask {
        snapshot_store: Arc::clone(&snapshot_store),
    }))?;

    pipeline.replace_task_for_test(Arc::new(StubAggressiveRiskTask))?;
    pipeline.replace_task_for_test(Arc::new(StubConservativeRiskTask))?;
    pipeline.replace_task_for_test(Arc::new(StubNeutralRiskTask))?;
    pipeline.replace_task_for_test(Arc::new(StubRiskModeratorTask {
        snapshot_store: Arc::clone(&snapshot_store),
    }))?;

    pipeline.replace_task_for_test(Arc::new(StubFundManagerTask { snapshot_store }))?;

    Ok(())
}

/// Replace all stubs and override the technical analyst child with `data`.
///
/// Calls [`replace_with_stubs`] first, then replaces the analyst fan-out with a
/// custom child that returns `data`. Use in tests that need specific
/// `options_context` values — the default neutral fixture has `options_context: None`.
pub fn replace_with_stubs_using_technical(
    pipeline: &crate::workflow::TradingPipeline,
    snapshot_store: Arc<SnapshotStore>,
    data: TechnicalData,
) -> Result<(), crate::workflow::pipeline::WorkflowTestSeamError> {
    use graph_flow::fanout::FanOutTask;

    replace_with_stubs(pipeline, Arc::clone(&snapshot_store))?;

    pipeline.replace_task_for_test(FanOutTask::new(
        "analyst_fanout",
        vec![
            StubAnalystChild::fundamental(),
            StubAnalystChild::sentiment(),
            StubAnalystChild::news(),
            CustomTechnicalAnalystChild::new(data),
        ],
    ))?;

    Ok(())
}
