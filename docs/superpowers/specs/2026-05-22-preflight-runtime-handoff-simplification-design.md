# Preflight Runtime Handoff Simplification Design

**Date:** 2026-05-22
**Status:** Draft — awaiting user review

## Goal

Simplify the workflow runtime-to-preflight handoff introduced for per-run ETF
routing without changing intended system behavior. The non-negotiable constraint
is that `PreflightTask` must remain the sole production writer of runtime
surfaces on `TradingState`, and that boundary must stay easy to audit in tests.

## Problem

The current fix is correct, but the transport layer is more complex than it
needs to be:

- `run_analysis_cycle` computes one logical handoff: the chosen runtime policy
  plus the optional ETF routing fallback reason.
- That handoff is currently transported through two internal JSON context keys:
  `KEY_RUNTIME_POLICY_OVERRIDE` and
  `KEY_ROUTING_FALLBACK_REASON_OVERRIDE`.
- `PreflightTask` reads and deserializes both keys separately before hydrating
  public runtime surfaces.

This preserves the sole-writer contract, but it spreads one concept across:

- two private keys,
- two serialization steps,
- two deserialization helpers,
- one re-export in `tasks/mod.rs`, and
- one null-path special case for the optional fallback reason.

There are also two smaller clarity issues adjacent to that path:

- `classify_runtime_pack_selection(...)` in
  `crates/scorpio-core/src/workflow/pipeline/runtime.rs` is a one-use wrapper.
- ETF benchmark fallback is normalized once for benchmark fetch assembly and then
  effectively patched again inside `derive_runtime_valuation(...)` by cloning
  and mutating `FundInfo`.

## Non-Goals

- Do not move runtime pack classification into `PreflightTask` in this slice.
- Do not reintroduce pre-seeding `TradingState.analysis_pack_name`,
  `TradingState.analysis_runtime_policy`, or
  `TradingState.etf_routing_fallback_reason` before graph execution.
- Do not change `TradingPipeline::from_pack(...)` fixed-pack semantics.
- Do not change user-visible routing behavior, final report behavior, or test
  expectations beyond mechanical adjustments required by the simplification.

## Recommended Approach

Use one private, typed runtime handoff object for the preflight override, then
apply the smaller readability cleanups around it.

### Runtime handoff

Replace the two-key JSON override transport with one internal typed context
value, for example:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RuntimePreflightOverride {
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
}
```

This object remains internal to workflow runtime/preflight plumbing.

`run_analysis_cycle` will:

- resolve runtime classification exactly as it does today,
- construct one `RuntimePreflightOverride`,
- store it in context before session save,
- never write runtime surfaces to `TradingState` directly.

`PreflightTask` will:

- read the single override object if present,
- otherwise fall back to its constructor-provided `runtime_policy`,
- hydrate:
  - `state.analysis_pack_name`
  - `state.analysis_runtime_policy`
  - `state.etf_routing_fallback_reason`
- continue writing the public runtime context keys from preflight.

This keeps the authority boundary explicit while reducing transport ceremony.

### Runtime readability cleanup

- Inline `classify_runtime_pack_selection(...)` into `run_analysis_cycle`.
- Replace the current combinator-heavy preflight resolution path with direct
  branching (`if let Some(override) = ...`) so the handoff precedence is easier
  to read during audits.

### ETF benchmark normalization cleanup

Normalize ETF benchmark fallback once during valuation input assembly in
`crates/scorpio-core/src/workflow/tasks/analyst.rs`, near the existing
`resolve_benchmark_symbol(...)` usage that already drives benchmark OHLCV fetch.

After that normalization:

- `ValuationInputs.etf_fund_info` should already carry the effective
  `stated_benchmark` when one can be resolved,
- `derive_runtime_valuation(...)` should no longer need to clone and patch
  `FundInfo`.

## Alternatives Considered

### 1. Keep current behavior and only trim local ceremony

This would mean only inlining the one-use helper and simplifying branch syntax.

Pros:
- Lowest-risk cleanup
- Minimal code movement

Cons:
- Leaves the main complexity in place
- Keeps the two-key override shape and the special null-handling path

### 2. Move runtime classification fully into preflight

Pros:
- Cleanest conceptual model
- Preflight would own both classification and hydration

Cons:
- Larger refactor
- Broader dependency changes for `PreflightTask`
- More graph/test churn than needed for this cleanup

This is out of scope for the current simplification.

## Architecture Impact

### Invariants to preserve

These are the design gates for the simplification:

1. At graph entry, runtime surfaces must still be absent from `TradingState`.
2. After preflight, runtime surfaces must still be present for analyst fan-out.
3. `PreflightTask` must remain the sole production writer of runtime surfaces.
4. Baseline-configured runtime must still reroute ETFs per run.
5. `from_pack(...)` must still remain fixed-manifest.
6. No-fallback runs must still leave fallback reason absent.

### Files expected to change

- `crates/scorpio-core/src/workflow/pipeline/runtime.rs`
- `crates/scorpio-core/src/workflow/tasks/preflight.rs`
- `crates/scorpio-core/src/workflow/tasks/common.rs`
- `crates/scorpio-core/src/workflow/tasks/mod.rs`
- `crates/scorpio-core/src/workflow/tasks/analyst.rs`

Tests may need mechanical updates in:

- `crates/scorpio-core/tests/workflow_pipeline_structure.rs`
- `crates/scorpio-core/src/workflow/pipeline/tests.rs`
- `crates/scorpio-core/src/workflow/tasks/preflight.rs` test module

## Behavior Expectations

### Intended behavior

No intended runtime behavior should change.

The same pack should be selected, the same fallback reason should propagate,
and the same post-preflight runtime surfaces should appear on state/context.

### Internal behavior changes allowed

These are acceptable internal changes:

- one typed internal handoff object instead of two JSON override values,
- fewer serialization/deserialization seams,
- simpler preflight override precedence logic,
- benchmark normalization performed once earlier in the valuation-input path.

## Testing Strategy

Keep and rerun the current authority-boundary coverage:

- preflight-entry activation audits:
  - `activation_path_audit_new_enters_graph_without_runtime_surfaces_pre_hydrated`
  - `activation_path_audit_from_pack_enters_graph_without_runtime_surfaces_pre_hydrated`
- post-preflight runtime-surface audits:
  - `activation_path_audit_new_reaches_analyst_boundary_with_preflight_runtime_surfaces`
  - `activation_path_audit_from_pack_reaches_analyst_boundary_with_preflight_runtime_surfaces`
- routing regressions:
  - `run_analysis_cycle_routes_baseline_pipeline_to_etf_pack_per_run`
  - `run_analysis_cycle_preserves_from_pack_fixed_manifest_over_runtime_etf_route`
- preflight override regression:
  - `preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed`

Also keep full repo verification as the merge gate:

- `cargo fmt -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace --all-features --locked --no-fail-fast`
- `cargo build -p scorpio-core --examples`

## Risks

- If the simplification leaks runtime surfaces back into pre-graph state, it
  reintroduces the exact regression we just fixed.
- If the typed handoff is made public or reused too broadly, it weakens the
  clarity of the workflow boundary instead of improving it.
- If benchmark normalization is moved to the wrong stage, benchmark fetch and
  valuation could diverge again.

## Recommendation

Implement the simplification as a narrow internal refactor:

- one private typed preflight override object,
- preflight remains the sole production writer,
- inline/remove single-use ceremony,
- normalize ETF benchmark fallback once in valuation input assembly.

This is the best balance between clarity, maintainability, and contract safety.
