use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::info;

use crate::{
    agents::{fund_manager::run_fund_manager, trader::run_trader},
    config::Config,
    state::PhaseTokenUsage,
    workflow::{
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        snapshot::{SnapshotPhase, SnapshotStore},
    },
};

/// Runs the phase-3 trader synthesis task.
///
/// The task reads the accumulated workflow state, creates the trade proposal,
/// records phase token accounting, persists the phase-3 snapshot, and returns
/// [`NextAction::Continue`] so graph-flow advances into risk discussion.
pub struct TraderTask {
    config: Arc<Config>,
    snapshot_store: Arc<SnapshotStore>,
}

impl TraderTask {
    /// Create a new `TraderTask`.
    pub fn new(config: Arc<Config>, snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self {
            config,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for TraderTask {
    fn id(&self) -> &str {
        "trader"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = "trader", phase = 3, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: failed to deserialize state: {error}"
                ))
            })?;

        let usage = run_trader(&mut state, &self.config)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: run_trader failed: {error}"
                ))
            })?;

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
                    "TraderTask: failed to serialize state: {error}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(&execution_id, SnapshotPhase::Trader, &state, Some(&[usage]))
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: failed to save phase 3 snapshot: {error}"
                ))
            })?;

        info!(task = "trader", phase = 3, "snapshot saved");
        info!(phase = 3, phase_name = "trader", "phase complete");
        info!(task = "trader", phase = 3, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the phase-5 fund manager decision task.
///
/// The task produces the final execution decision, records phase token
/// accounting, persists the phase-5 snapshot, and returns [`NextAction::End`]
/// to terminate the workflow.
pub struct FundManagerTask {
    config: Arc<Config>,
    snapshot_store: Arc<SnapshotStore>,
}

impl FundManagerTask {
    /// Create a new `FundManagerTask`.
    pub fn new(config: Arc<Config>, snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self {
            config,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for FundManagerTask {
    fn id(&self) -> &str {
        "fund_manager"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = "fund_manager", phase = 5, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: failed to deserialize state: {error}"
                ))
            })?;

        let usage = run_fund_manager(&mut state, &self.config)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: run_fund_manager failed: {error}"
                ))
            })?;

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
                    "FundManagerTask: failed to serialize state: {error}"
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
                    "FundManagerTask: failed to save phase 5 snapshot: {error}"
                ))
            })?;

        info!(task = "fund_manager", phase = 5, "snapshot saved");

        let decision_label = state
            .final_execution_status
            .as_ref()
            .map(|status| format!("{:?}", status.decision))
            .unwrap_or_else(|| "none".to_owned());
        info!(task = "fund_manager", decision = %decision_label, phase = 5, "task completed");
        info!(phase = 5, phase_name = "fund_manager", "phase complete");

        Ok(TaskResult::new(None, NextAction::End))
    }
}
