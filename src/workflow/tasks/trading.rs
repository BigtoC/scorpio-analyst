use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::info;

use crate::{
    agents::{fund_manager::run_fund_manager, trader::run_trader},
    config::Config,
    state::{PhaseTokenUsage, ThesisMemory},
    workflow::{
        snapshot::{SnapshotPhase, SnapshotStore},
        tasks::runtime::{load_state, save_state, task_error},
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
    const TASK_ID: &str = "trader";
    const TASK_NAME: &str = "TraderTask";

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
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = Self::TASK_ID, phase = 3, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let usage = run_trader(&mut state, &self.config)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "run_trader failed", error))?;

        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Trader Synthesis".to_owned(),
            agent_usage: vec![usage.clone()],
            phase_prompt_tokens: usage.prompt_tokens,
            phase_completion_tokens: usage.completion_tokens,
            phase_total_tokens: usage.total_tokens,
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
        });

        save_state(Self::TASK_NAME, &state, &context).await?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(&execution_id, SnapshotPhase::Trader, &state, Some(&[usage]))
            .await
            .map_err(|error| {
                task_error(Self::TASK_NAME, "failed to save phase 3 snapshot", error)
            })?;

        info!(task = Self::TASK_ID, phase = 3, "snapshot saved");
        info!(phase = 3, phase_name = "trader", "phase complete");
        info!(task = Self::TASK_ID, phase = 3, "task completed");
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
    const TASK_ID: &str = "fund_manager";
    const TASK_NAME: &str = "FundManagerTask";

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
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = Self::TASK_ID, phase = 5, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let usage = run_fund_manager(&mut state, &self.config)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "run_fund_manager failed", error))?;

        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Fund Manager Decision".to_owned(),
            agent_usage: vec![usage.clone()],
            phase_prompt_tokens: usage.prompt_tokens,
            phase_completion_tokens: usage.completion_tokens,
            phase_total_tokens: usage.total_tokens,
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
        });

        // Build and persist the current-run thesis before the final snapshot so
        // future runs for the same symbol can load it as prior context.
        if let Some(status) = &state.final_execution_status {
            state.current_thesis = Some(ThesisMemory {
                symbol: state.asset_symbol.clone(),
                action: format!("{:?}", status.action),
                decision: format!("{:?}", status.decision),
                rationale: status.rationale.clone(),
                summary: None,
                execution_id: state.execution_id.to_string(),
                target_date: state.target_date.clone(),
                captured_at: Utc::now(),
            });
        }

        save_state(Self::TASK_NAME, &state, &context).await?;

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
                task_error(Self::TASK_NAME, "failed to save phase 5 snapshot", error)
            })?;

        info!(task = Self::TASK_ID, phase = 5, "snapshot saved");

        let decision_label = state
            .final_execution_status
            .as_ref()
            .map(|status| format!("{:?}", status.decision))
            .unwrap_or_else(|| "none".to_owned());
        info!(task = Self::TASK_ID, decision = %decision_label, phase = 5, "task completed");
        info!(phase = 5, phase_name = "fund_manager", "phase complete");

        Ok(TaskResult::new(None, NextAction::End))
    }
}

#[cfg(test)]
mod tests {
    use crate::workflow::tasks::runtime::task_error;

    use super::TraderTask;

    #[test]
    fn trader_task_identity_constants_drive_error_identity() {
        assert_eq!(TraderTask::TASK_ID, "trader");
        assert_eq!(TraderTask::TASK_NAME, "TraderTask");

        match task_error(TraderTask::TASK_NAME, "run_trader failed", "boom") {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert_eq!(message, "TraderTask: run_trader failed: boom");
            }
            other => panic!("expected TaskExecutionFailed, got: {other:?}"),
        }
    }
}
