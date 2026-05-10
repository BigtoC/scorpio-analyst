use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::{info, warn};

use crate::{
    agents::auditor::{build_fail_open_report, run_auditor, run_deterministic_checks},
    config::Config,
    state::{PhaseTokenUsage, auditor::AuditStatus},
    workflow::{
        tasks::{
            KEY_ROUTING_FLAGS,
            runtime::{load_state, save_state},
        },
        topology::RoutingFlags,
    },
};

/// Runs the post-decision advisory audit (Phase 6).
///
/// This task is always registered in the graph. When `skip_auditor = true`
/// (the default for packs with `auditor_enabled = false`), the task is a
/// no-op and terminates immediately.
/// When `skip_auditor = false`, it runs deterministic checks followed by a
/// quick-thinker LLM pass and writes the result to `state.audit_report`.
///
/// Fail-open: any LLM or parse failure marks [`AuditStatus::FailedOpen`]
/// and returns [`NextAction::End`] so the completed run is never blocked.
pub struct AuditorTask {
    config: Arc<Config>,
}

impl AuditorTask {
    const TASK_ID: &str = "auditor";
    const TASK_NAME: &str = "AuditorTask";

    pub fn new(config: Arc<Config>) -> Arc<Self> {
        Arc::new(Self { config })
    }
}

#[async_trait]
impl Task for AuditorTask {
    fn id(&self) -> &str {
        Self::TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let skip = context
            .get_sync::<RoutingFlags>(KEY_ROUTING_FLAGS)
            .map(|f| f.skip_auditor)
            .unwrap_or(true);

        if skip {
            info!(
                task = Self::TASK_ID,
                "auditor skipped (auditor_enabled = false)"
            );
            return Ok(TaskResult::new(None, NextAction::End));
        }

        info!(task = Self::TASK_ID, phase = 6, "task started");
        let phase_start = Instant::now();
        let mut state = load_state(Self::TASK_NAME, &context).await?;
        let deterministic = run_deterministic_checks(&state);

        match run_auditor(&state, &self.config).await {
            Ok((report, usage)) => {
                state.audit_status = if report.findings.is_empty() {
                    AuditStatus::Passed
                } else {
                    AuditStatus::Findings
                };
                state.audit_report = Some(report);
                state.token_usage.push_phase_usage(PhaseTokenUsage {
                    phase_name: "Auditor Review".to_owned(),
                    agent_usage: vec![usage.clone()],
                    phase_prompt_tokens: usage.prompt_tokens,
                    phase_completion_tokens: usage.completion_tokens,
                    phase_total_tokens: usage.total_tokens,
                    phase_duration_ms: phase_start.elapsed().as_millis() as u64,
                });
                save_state(Self::TASK_NAME, &state, &context).await?;
                info!(phase = 6, phase_name = "auditor", "phase complete");
                info!(task = Self::TASK_ID, phase = 6, "task completed");
            }
            Err(error) => {
                warn!(
                    task = Self::TASK_ID,
                    error = %error,
                    "auditor LLM call failed — marking fail-open; run is not blocked"
                );
                state.audit_status = AuditStatus::FailedOpen;
                state.audit_report = Some(build_fail_open_report(deterministic));
                save_state(Self::TASK_NAME, &state, &context).await?;
            }
        }

        Ok(TaskResult::new(None, NextAction::End))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auditor_task_id_constants_are_stable() {
        assert_eq!(AuditorTask::TASK_ID, "auditor");
        assert_eq!(AuditorTask::TASK_NAME, "AuditorTask");
    }
}
