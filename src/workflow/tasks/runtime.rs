use std::fmt::Display;

use graph_flow::{Context, GraphError};

use crate::{
    state::TradingState,
    workflow::context_bridge::{deserialize_state_from_context, serialize_state_to_context},
};

pub(crate) async fn load_state(
    task_name: &str,
    context: &Context,
) -> graph_flow::Result<TradingState> {
    deserialize_state_from_context(context)
        .await
        .map_err(|error| task_error(task_name, "failed to deserialize state", error))
}

pub(crate) async fn save_state(
    task_name: &str,
    state: &TradingState,
    context: &Context,
) -> graph_flow::Result<()> {
    serialize_state_to_context(state, context)
        .await
        .map_err(|error| task_error(task_name, "failed to serialize state", error))
}

pub(crate) fn task_error(task_name: &str, action: &str, error: impl Display) -> GraphError {
    GraphError::TaskExecutionFailed(format!("{task_name}: {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use graph_flow::Context;

    use super::{load_state, save_state, task_error};
    use crate::{state::TradingState, workflow::context_bridge::deserialize_state_from_context};

    fn sample_state() -> TradingState {
        TradingState::new("AAPL", "2026-03-19")
    }

    #[test]
    fn task_error_preserves_task_identity_and_cause() {
        let error = task_error(
            "BullishResearcherTask",
            "failed to deserialize state",
            "context missing required key 'trading_state'",
        );

        match error {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert_eq!(
                    message,
                    "BullishResearcherTask: failed to deserialize state: context missing required key 'trading_state'"
                );
            }
            other => panic!("expected TaskExecutionFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_state_wraps_deserialize_errors_with_task_identity() {
        let context = Context::new();

        let error = load_state("BullishResearcherTask", &context)
            .await
            .expect_err("missing state should fail");

        match error {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert_eq!(
                    message,
                    "BullishResearcherTask: failed to deserialize state: schema violation: context missing required key 'trading_state'"
                );
            }
            other => panic!("expected TaskExecutionFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn save_state_persists_trading_state_in_context() {
        let context = Context::new();
        let state = sample_state();

        save_state("TraderTask", &state, &context)
            .await
            .expect("saving state should succeed");

        let recovered = deserialize_state_from_context(&context)
            .await
            .expect("saved state should deserialize");

        assert_eq!(recovered.asset_symbol, state.asset_symbol);
        assert_eq!(recovered.target_date, state.target_date);
    }
}
