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
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        snapshot::{SnapshotPhase, SnapshotStore},
        tasks::{
            accounting::debate_moderator_accounting,
            common::{DEBATE_USAGE_PREFIX, KEY_DEBATE_ROUND, write_round_usage},
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
    /// Create a new `BullishResearcherTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for BullishResearcherTask {
    fn id(&self) -> &str {
        "bullish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to deserialize state: {error}"
                ))
            })?;

        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(
            task = "bullish_researcher",
            round = this_round,
            "task started"
        );

        let usage = run_bullish_researcher_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to run bullish turn: {error}"
                ))
            })?;

        write_round_usage(&context, DEBATE_USAGE_PREFIX, this_round, "bull", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to persist round usage: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to serialize state: {error}"
                ))
            })?;

        info!(
            task = "bullish_researcher",
            round = this_round,
            "task completed"
        );
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
    /// Create a new `BearishResearcherTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for BearishResearcherTask {
    fn id(&self) -> &str {
        "bearish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to deserialize state: {error}"
                ))
            })?;

        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        info!(
            task = "bearish_researcher",
            round = this_round,
            "task started"
        );

        let usage = run_bearish_researcher_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to run bearish turn: {error}"
                ))
            })?;

        write_round_usage(&context, DEBATE_USAGE_PREFIX, this_round, "bear", &usage)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to persist round usage: {error}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to serialize state: {error}"
                ))
            })?;

        info!(
            task = "bearish_researcher",
            round = this_round,
            "task completed"
        );
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
        "debate_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = "debate_moderator", phase = 2, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to deserialize state: {error}"
                ))
            })?;

        let mod_usage = run_debate_moderation(&mut state, &self.config, &self.handle)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to run moderation: {error}"
                ))
            })?;

        let is_final =
            debate_moderator_accounting(&context, &mut state, &mod_usage, &phase_start).await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to serialize state: {error}"
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
                        "DebateModeratorTask: failed to save phase 2 snapshot: {error}"
                    ))
                })?;
            info!(task = "debate_moderator", phase = 2, "snapshot saved");
            info!(
                phase = 2,
                phase_name = "researcher_debate",
                "phase complete"
            );
        }

        info!(task = "debate_moderator", phase = 2, "task completed");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}
