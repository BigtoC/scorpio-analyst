---
title: ETF Runtime Policy Preseed Broke Preflight Authority Contract
date: 2026-05-22
category: docs/solutions/logic-errors
module: workflow/pipeline/runtime + workflow/tasks/preflight
problem_type: logic_error
component: assistant
symptoms:
  - "Per-run ETF routing was injected into `TradingState` before graph execution, so preflight no longer owned first-write of runtime surfaces."
  - "Activation-path audits observed `analysis_pack_name` and `analysis_runtime_policy` at graph entry instead of after `PreflightTask`."
  - "The optional routing fallback reason failed when the override path serialized `null` for no-fallback runs."
root_cause: logic_error
resolution_type: code_fix
severity: high
tags:
  - preflight
  - runtime-policy
  - etf-routing
  - trading-state
  - activation-path-audit
  - context-override
---

# ETF Runtime Policy Preseed Broke Preflight Authority Contract

## Problem

`run_analysis_cycle` was resolving the per-run ETF runtime route and writing
runtime surfaces directly onto `TradingState` before the workflow graph
started. That broke the contract established by the workflow runtime: in
production, `PreflightTask` is supposed to be the sole writer of
`analysis_pack_name`, `analysis_runtime_policy`, and routing-derived runtime
metadata.

## Symptoms

- Activation-path audits at graph entry failed because `analysis_pack_name` was
  already populated before `PreflightTask` ran.
- The workflow still mostly behaved correctly after preflight, which made the
  bug easy to miss outside the structural audit tests.
- The new override path exposed a second bug: when the routing fallback reason
  was absent, JSON `null` failed deserialization in preflight.

## What Didn't Work

- Resetting runtime fields in `reset_cycle_outputs` was not enough. The fields
  were immediately re-populated by `run_analysis_cycle` before the session was
  saved.
- The earlier ETF routing implementation treated runtime classification as a
  `run_analysis_cycle` concern instead of a preflight hydration concern. Session
  history from the ETF rollout shows Task 11 focused on classifier + builder
  wiring, but not on defending the preflight sole-writer boundary (session
  history).
- The no-fallback path was misleading because `Some(...)` override cases worked.
  The bug only surfaced when the fallback reason was serialized as JSON `null`.

## Solution

Move the per-run ETF routing transport into private context keys and let
`PreflightTask` hydrate public runtime surfaces from that override.

1. Added internal-only context keys in
   `crates/scorpio-core/src/workflow/tasks/common.rs`:
   - `KEY_RUNTIME_POLICY_OVERRIDE`
   - `KEY_ROUTING_FALLBACK_REASON_OVERRIDE`
2. Updated `crates/scorpio-core/src/workflow/pipeline/runtime.rs` so
   `run_analysis_cycle` still resolves the per-run runtime policy and optional
   fallback reason, but serializes them into the session context before saving
   the session instead of pre-seeding `TradingState`.
3. Updated `crates/scorpio-core/src/workflow/tasks/preflight.rs` so preflight:
   - reads the private override keys first,
   - falls back to its constructor-provided runtime policy when no override is
     present,
   - hydrates `state.analysis_pack_name`,
     `state.analysis_runtime_policy`, and
     `state.etf_routing_fallback_reason`,
   - writes the public runtime context keys from that same preflight-owned
     state.
4. Fixed the optional fallback-reason override reader to deserialize
   `Option<String>` and flatten the nested option so serialized JSON `null`
   becomes `None` instead of a preflight failure.
5. During full verification, fixed two neighboring regressions exposed by the
   broader test suite:
   - updated the ETF tracking-error identical-series test data in
     `crates/scorpio-core/src/valuation/etf/tracking_error.rs` so date-based
     alignment still has enough overlap samples,
   - collapsed a clippy-flagged nested `if` in
     `crates/scorpio-core/src/workflow/tasks/analyst.rs`.

## Why This Works

This restores the intended authority boundary:

- `run_analysis_cycle` can still perform async runtime classification before the
  graph starts,
- but those results stay internal until `PreflightTask` validates and hydrates
  the runtime surfaces,
- so graph-entry audits once again prove that runtime surfaces are absent before
  preflight and present after it.

That keeps downstream invariants coherent: tasks that treat missing runtime
policy as orchestration corruption continue to rely on preflight as the single
production writer.

## Prevention

- Keep production writes to `TradingState.analysis_pack_name` and
  `TradingState.analysis_runtime_policy` confined to `PreflightTask`.
- If per-run data must be available before graph execution, pass it through
  internal context-only keys instead of pre-hydrating serialized state.
- Maintain both sides of the authority-boundary tests:
  - graph entry must not expose runtime surfaces before preflight,
  - the analyst boundary must expose them after preflight.
- Cover optional override shapes explicitly. A serialized `null` path needs its
  own regression test, not just the `Some(...)` path.
- When changing date alignment logic, update fixtures that accidentally relied
  on duplicate date keys or positional zip behavior.

## Related Issues

- `docs/solutions/logic-errors/prompt-bundle-centralization-runtime-contract-2026-04-25.md`
  — broader runtime-contract migration that established preflight as the
  runtime-policy authority.
- Session history: the ETF baseline rollout on `feature/enhance-etf-analysis`
  introduced runtime classifier wiring first, and later design work continued to
  describe ETF-specific runtime behavior as preflight-scoped (session history).

## Verification

- Targeted tests:
  - `cargo test -p scorpio-core --features test-helpers --test workflow_pipeline_structure activation_path_audit_new_enters_graph_without_runtime_surfaces_pre_hydrated`
  - `cargo test -p scorpio-core --features test-helpers --test workflow_pipeline_structure activation_path_audit_from_pack_enters_graph_without_runtime_surfaces_pre_hydrated`
  - `cargo test -p scorpio-core --features test-helpers --test workflow_pipeline_structure activation_path_audit_new_reaches_analyst_boundary_with_preflight_runtime_surfaces`
  - `cargo test -p scorpio-core --features test-helpers --test workflow_pipeline_structure activation_path_audit_from_pack_reaches_analyst_boundary_with_preflight_runtime_surfaces`
  - `cargo test -p scorpio-core --lib run_analysis_cycle_routes_baseline_pipeline_to_etf_pack_per_run`
  - `cargo test -p scorpio-core --lib run_analysis_cycle_preserves_from_pack_fixed_manifest_over_runtime_etf_route`
  - `cargo test -p scorpio-core preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed`
- Full repo verification passed after the fix:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast`
  - `cargo build -p scorpio-core --examples`
