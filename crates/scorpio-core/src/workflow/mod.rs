pub mod builder;
mod context_bridge;
mod pipeline;
mod snapshot;
mod tasks;
pub mod topology;

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_support;

pub use builder::{PipelineDeps, build_graph_from_pack};
pub use pipeline::TradingPipeline;
pub use snapshot::{LoadedSnapshot, SnapshotPhase, SnapshotStore};
pub use topology::{
    PromptSlot, Role, RoutingFlags, RunRoleTopology, analyst_role_for_input, build_run_topology,
    required_prompt_slots,
};
