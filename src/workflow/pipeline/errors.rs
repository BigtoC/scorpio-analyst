use crate::{error::TradingError, providers::factory::sanitize_error_summary};

use super::constants::TASKS;

/// Map a [`graph_flow::GraphError`] into a [`TradingError::GraphFlow`],
/// preserving task identity and phase when available.
///
/// # Examples
///
/// ```ignore
/// let trading_error = map_graph_error(graph_error);
/// ```
pub fn map_graph_error(err: graph_flow::GraphError) -> TradingError {
    match err {
        graph_flow::GraphError::TaskExecutionFailed(ref msg) => {
            let (task_id, cause) = extract_task_identity(msg);
            let phase = phase_for_task(&task_id);
            TradingError::GraphFlow {
                phase,
                task: task_id,
                cause: sanitize_error_summary(&cause),
            }
        }
        graph_flow::GraphError::TaskNotFound(ref id) => TradingError::GraphFlow {
            phase: phase_for_task(id),
            task: id.clone(),
            cause: format!("task not found: {id}"),
        },
        other => {
            let variant = match &other {
                graph_flow::GraphError::GraphNotFound(_) => "graph_not_found",
                graph_flow::GraphError::InvalidEdge(_) => "invalid_edge",
                graph_flow::GraphError::ContextError(_) => "context_error",
                graph_flow::GraphError::StorageError(_) => "storage_error",
                graph_flow::GraphError::SessionNotFound(_) => "session_not_found",
                _ => "graph_flow",
            };
            TradingError::GraphFlow {
                phase: "orchestration".into(),
                task: variant.into(),
                cause: sanitize_error_summary(&other.to_string()),
            }
        }
    }
}

pub(super) fn extract_task_identity(msg: &str) -> (String, String) {
    if let Some(rest) = msg.strip_prefix("Task '")
        && let Some(quote_end) = rest.find('\'')
    {
        let task_id = &rest[..quote_end];
        let cause = rest[quote_end..]
            .strip_prefix("' failed: ")
            .unwrap_or(&rest[quote_end..])
            .to_owned();
        return (task_id.to_owned(), cause);
    }

    if let Some(rest) = msg.strip_prefix("FanOut child '")
        && let Some(quote_end) = rest.find('\'')
    {
        let task_id = &rest[..quote_end];
        let cause = rest[quote_end..]
            .strip_prefix("' failed: ")
            .unwrap_or(&rest[quote_end..])
            .to_owned();
        return (task_id.to_owned(), cause);
    }

    ("unknown".to_owned(), msg.to_owned())
}

pub(super) fn phase_for_task(task_id: &str) -> String {
    match task_id {
        id if id == TASKS.preflight => "preflight".into(),
        id if [
            TASKS.analyst_fan_out,
            "fundamental_analyst",
            "sentiment_analyst",
            "news_analyst",
            "technical_analyst",
            TASKS.analyst_sync,
        ]
        .contains(&id) =>
        {
            "analyst_team".into()
        }
        id if [
            TASKS.bullish_researcher,
            TASKS.bearish_researcher,
            TASKS.debate_moderator,
        ]
        .contains(&id) =>
        {
            "researcher_debate".into()
        }
        id if id == TASKS.trader => "trader".into(),
        id if [
            TASKS.aggressive_risk,
            TASKS.conservative_risk,
            TASKS.neutral_risk,
            TASKS.risk_moderator,
        ]
        .contains(&id) =>
        {
            "risk_discussion".into()
        }
        id if id == TASKS.fund_manager => "fund_manager".into(),
        _ => "unknown_phase".into(),
    }
}
