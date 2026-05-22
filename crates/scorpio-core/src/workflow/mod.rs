pub mod builder;
mod context_bridge;
pub mod pack_classifier;
mod pipeline;
mod snapshot;
mod tasks;
mod topology;

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_support;

pub use builder::{PipelineDeps, build_graph_from_pack};
pub use pack_classifier::{RuntimePackSelection, classify_runtime_pack};
pub use pipeline::TradingPipeline;
pub use pipeline::runtime::run_analysis_cycle;
pub use snapshot::{
    ExecutionListing, ExecutionSummary, LoadedReport, LoadedReportSnapshot, LoadedSnapshot,
    SnapshotPhase, SnapshotStore, THESIS_MEMORY_SCHEMA_VERSION,
};
pub use tasks::KEY_ROUTING_FLAGS;
pub use topology::{
    PromptSlot, Role, RoutingFlags, RunRoleTopology, analyst_role_for_input, build_run_topology,
    required_prompt_slots,
};
