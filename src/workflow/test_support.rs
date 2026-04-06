//! Workflow-scoped helpers for integration tests.
//!
//! These re-export deterministic workflow seams and context helpers so tests can
//! depend on a workflow-level surface instead of reaching through internal
//! modules.

#[cfg(any(test, feature = "test-helpers"))]
pub use crate::workflow::context_bridge::{
    TRADING_STATE_KEY, deserialize_state_from_context, serialize_state_to_context,
    write_prefixed_result,
};

#[cfg(any(test, feature = "test-helpers"))]
pub use crate::workflow::tasks::test_helpers::{
    run_debate_moderator_accounting, run_risk_moderator_accounting, write_round_debate_usage,
    write_round_risk_usage,
};

#[cfg(any(test, feature = "test-helpers"))]
pub use crate::workflow::{
    pipeline::map_graph_error,
    tasks::{
        AnalystSyncTask, FundamentalAnalystTask, KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED,
        KEY_CACHED_TRANSCRIPT, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS,
        KEY_PROVIDER_CAPABILITIES, KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT,
        KEY_RISK_ROUND,
    },
};
