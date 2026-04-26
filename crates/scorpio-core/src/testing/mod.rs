//! Test-only facade gated behind `#[cfg(any(test, feature = "test-helpers"))]`.
//!
//! Helpers here let tests bypass `PreflightTask` when exercising downstream
//! components in isolation. **Production code must never depend on this
//! module** — preflight is the sole writer of `state.analysis_runtime_policy`
//! per Unit 4a's structural authority migration.

pub mod prompt_render;
pub mod runtime_policy;

pub use prompt_render::{
    PromptRenderOutput, PromptRenderScenario, canonical_fixture_identity,
    render_baseline_prompt_for_role, render_prompt_output_for_role,
};
pub use runtime_policy::{
    baseline_pack_prompt_for_role, runtime_policy_from_manifest, with_baseline_runtime_policy,
    with_runtime_policy,
};
