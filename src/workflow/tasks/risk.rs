use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::info;

use crate::{
    agents::risk::{
        run_aggressive_risk_turn, run_conservative_risk_turn, run_neutral_risk_turn,
        run_risk_moderation,
    },
    config::Config,
    providers::factory::CompletionModelHandle,
    workflow::{
        snapshot::{SnapshotPhase, SnapshotStore},
        tasks::{
            accounting::risk_moderator_accounting,
            common::{KEY_RISK_ROUND, RISK_USAGE_PREFIX, write_round_usage},
            runtime::{load_state, save_state, task_error},
        },
    },
};

/// Runs one aggressive risk turn in phase 4.
///
/// The task updates the shared trading state with the aggressive assessment,
/// writes typed round token usage for moderator accounting, re-serializes the
/// state into context, and returns [`NextAction::Continue`] so the sequential
/// risk loop can proceed.
pub struct AggressiveRiskTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl AggressiveRiskTask {
    const TASK_ID: &str = "aggressive_risk";
    const TASK_NAME: &str = "AggressiveRiskTask";

    /// Create a new `AggressiveRiskTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for AggressiveRiskTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(task = Self::TASK_ID, round = this_round, "task started");

        let usage = run_aggressive_risk_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to run aggressive turn", error))?;

        write_round_usage(&context, RISK_USAGE_PREFIX, this_round, "agg", &usage)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to persist round usage", error))?;

        save_state(Self::TASK_NAME, &state, &context).await?;

        info!(task = Self::TASK_ID, round = this_round, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs one conservative risk turn in phase 4.
///
/// The task updates the shared trading state with the conservative assessment,
/// writes typed round token usage for moderator accounting, re-serializes the
/// state into context, and returns [`NextAction::Continue`] so the sequential
/// risk loop can proceed.
pub struct ConservativeRiskTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl ConservativeRiskTask {
    const TASK_ID: &str = "conservative_risk";
    const TASK_NAME: &str = "ConservativeRiskTask";

    /// Create a new `ConservativeRiskTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for ConservativeRiskTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(task = Self::TASK_ID, round = this_round, "task started");

        let usage = run_conservative_risk_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| {
                task_error(Self::TASK_NAME, "failed to run conservative turn", error)
            })?;

        write_round_usage(&context, RISK_USAGE_PREFIX, this_round, "con", &usage)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to persist round usage", error))?;

        save_state(Self::TASK_NAME, &state, &context).await?;

        info!(task = Self::TASK_ID, round = this_round, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs one neutral risk turn in phase 4.
///
/// The task updates the shared trading state with the neutral assessment,
/// writes typed round token usage for moderator accounting, re-serializes the
/// state into context, and returns [`NextAction::Continue`] so moderation can
/// run after the sequential risk turns complete.
pub struct NeutralRiskTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl NeutralRiskTask {
    const TASK_ID: &str = "neutral_risk";
    const TASK_NAME: &str = "NeutralRiskTask";

    /// Create a new `NeutralRiskTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for NeutralRiskTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(task = Self::TASK_ID, round = this_round, "task started");

        let usage = run_neutral_risk_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to run neutral turn", error))?;

        write_round_usage(&context, RISK_USAGE_PREFIX, this_round, "neu", &usage)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to persist round usage", error))?;

        save_state(Self::TASK_NAME, &state, &context).await?;

        info!(task = Self::TASK_ID, round = this_round, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the risk moderator for phase 4.
///
/// The task synthesizes the current risk discussion, materializes round and
/// moderation accounting entries, saves the phase-4 snapshot on the final round,
/// and returns [`NextAction::Continue`] so graph-flow can either loop for another
/// round or advance to the fund manager.
pub struct RiskModeratorTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
    snapshot_store: Arc<SnapshotStore>,
}

impl RiskModeratorTask {
    const TASK_ID: &str = "risk_moderator";
    const TASK_NAME: &str = "RiskModeratorTask";

    /// Create a new `RiskModeratorTask`.
    pub fn new(
        config: Arc<Config>,
        handle: CompletionModelHandle,
        snapshot_store: Arc<SnapshotStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            handle,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for RiskModeratorTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = Self::TASK_ID, phase = 4, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let mod_usage = run_risk_moderation(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to run moderation", error))?;

        let is_final =
            risk_moderator_accounting(&context, &mut state, &mod_usage, &phase_start).await;

        save_state(Self::TASK_NAME, &state, &context).await?;

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
                    task_error(Self::TASK_NAME, "failed to save phase 4 snapshot", error)
                })?;
            info!(task = Self::TASK_ID, phase = 4, "snapshot saved");
            info!(phase = 4, phase_name = "risk_discussion", "phase complete");
        }

        info!(task = Self::TASK_ID, phase = 4, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

#[cfg(test)]
mod tests {
    use crate::workflow::tasks::runtime::task_error;

    use super::AggressiveRiskTask;

    #[test]
    fn aggressive_task_identity_constants_drive_error_identity() {
        assert_eq!(AggressiveRiskTask::TASK_ID, "aggressive_risk");
        assert_eq!(AggressiveRiskTask::TASK_NAME, "AggressiveRiskTask");

        match task_error(
            AggressiveRiskTask::TASK_NAME,
            "failed to run aggressive turn",
            "boom",
        ) {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert_eq!(
                    message,
                    "AggressiveRiskTask: failed to run aggressive turn: boom"
                );
            }
            other => panic!("expected TaskExecutionFailed, got: {other:?}"),
        }
    }
}
