use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::info;

use crate::{
    agents::researcher::{
        run_bearish_researcher_turn, run_bullish_researcher_turn, run_debate_moderation,
    },
    config::Config,
    providers::factory::CompletionModelHandle,
    workflow::{
        snapshot::{SnapshotPhase, SnapshotStore},
        tasks::{
            accounting::debate_moderator_accounting,
            common::{DEBATE_USAGE_PREFIX, KEY_DEBATE_ROUND, write_round_usage},
            runtime::{load_state, save_state, task_error},
        },
    },
};

/// Runs one bullish researcher turn in phase 2.
///
/// The task reads the current workflow state, appends the bullish argument via
/// the researcher agent, persists typed round token usage into the graph-flow
/// context for the moderator, re-serializes the updated state, and returns
/// [`NextAction::Continue`] so the graph can advance to the bearish turn.
pub struct BullishResearcherTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl BullishResearcherTask {
    const TASK_ID: &str = "bullish_researcher";
    const TASK_NAME: &str = "BullishResearcherTask";

    /// Create a new `BullishResearcherTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for BullishResearcherTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(task = Self::TASK_ID, round = this_round, "task started");

        let usage = run_bullish_researcher_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to run bullish turn", error))?;

        write_round_usage(&context, DEBATE_USAGE_PREFIX, this_round, "bull", &usage)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to persist round usage", error))?;

        save_state(Self::TASK_NAME, &state, &context).await?;

        info!(task = Self::TASK_ID, round = this_round, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs one bearish researcher turn in phase 2.
///
/// The task reads the current workflow state, appends the bearish argument via
/// the researcher agent, persists typed round token usage into the graph-flow
/// context for the moderator, re-serializes the updated state, and returns
/// [`NextAction::Continue`] so the graph can advance to moderation.
pub struct BearishResearcherTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl BearishResearcherTask {
    const TASK_ID: &str = "bearish_researcher";
    const TASK_NAME: &str = "BearishResearcherTask";

    /// Create a new `BearishResearcherTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for BearishResearcherTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(task = Self::TASK_ID, round = this_round, "task started");

        let usage = run_bearish_researcher_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to run bearish turn", error))?;

        write_round_usage(&context, DEBATE_USAGE_PREFIX, this_round, "bear", &usage)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to persist round usage", error))?;

        save_state(Self::TASK_NAME, &state, &context).await?;

        info!(task = Self::TASK_ID, round = this_round, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the debate moderator for phase 2.
///
/// The task synthesizes the current debate state, materializes round and
/// moderation accounting entries, saves the phase-2 snapshot on the final round,
/// and returns [`NextAction::Continue`] so graph-flow can either loop for another
/// round or advance to the trader.
pub struct DebateModeratorTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
    snapshot_store: Arc<SnapshotStore>,
}

impl DebateModeratorTask {
    const TASK_ID: &str = "debate_moderator";
    const TASK_NAME: &str = "DebateModeratorTask";

    /// Create a new `DebateModeratorTask`.
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
impl Task for DebateModeratorTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = Self::TASK_ID, phase = 2, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = load_state(Self::TASK_NAME, &context).await?;

        let mod_usage = run_debate_moderation(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| task_error(Self::TASK_NAME, "failed to run moderation", error))?;

        let is_final =
            debate_moderator_accounting(&context, &mut state, &mod_usage, &phase_start).await;

        save_state(Self::TASK_NAME, &state, &context).await?;

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
                    task_error(Self::TASK_NAME, "failed to save phase 2 snapshot", error)
                })?;
            info!(task = Self::TASK_ID, phase = 2, "snapshot saved");
            info!(
                phase = 2,
                phase_name = "researcher_debate",
                "phase complete"
            );
        }

        info!(task = Self::TASK_ID, phase = 2, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

#[cfg(test)]
mod tests {
    use crate::workflow::tasks::runtime::task_error;

    use super::BullishResearcherTask;

    #[test]
    fn bullish_task_identity_constants_drive_error_identity() {
        assert_eq!(BullishResearcherTask::TASK_ID, "bullish_researcher");
        assert_eq!(BullishResearcherTask::TASK_NAME, "BullishResearcherTask");

        match task_error(
            BullishResearcherTask::TASK_NAME,
            "failed to run bullish turn",
            "boom",
        ) {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert_eq!(
                    message,
                    "BullishResearcherTask: failed to run bullish turn: boom"
                );
            }
            other => panic!("expected TaskExecutionFailed, got: {other:?}"),
        }
    }
}
