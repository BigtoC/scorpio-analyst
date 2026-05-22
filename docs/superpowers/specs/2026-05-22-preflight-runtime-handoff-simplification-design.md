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

There is also one smaller clarity issue adjacent to that path:

- `classify_runtime_pack_selection(...)` in
  `crates/scorpio-core/src/workflow/pipeline/runtime.rs` is a one-use wrapper.

ETF benchmark fallback also has a known cleanup opportunity (it is currently
normalized once for benchmark fetch assembly and then effectively patched
again inside `derive_runtime_valuation(...)` by cloning and mutating
`FundInfo`), but that work is deferred to a follow-up plan to keep this
slice's revert blast radius narrow — see
"Follow-up: Deferred ETF benchmark normalization cleanup" below.

## Non-Goals

- Do not move runtime pack classification into `PreflightTask` in this slice.
- Do not reintroduce pre-seeding `TradingState.analysis_pack_name`,
  `TradingState.analysis_runtime_policy`, or
  `TradingState.etf_routing_fallback_reason` before graph execution.
- Do not change `TradingPipeline::from_pack(...)` fixed-pack semantics.
- Do not change user-visible routing behavior, final report behavior, or test
  expectations beyond mechanical adjustments required by the simplification.
- ETF benchmark normalization cleanup is explicitly out of scope for this
  slice; see "Follow-up" below.

## Recommended Approach

Use one private, typed runtime handoff object for the preflight override, then
apply the smaller readability cleanup around it.

### Runtime handoff

Replace the two-key JSON override transport with one internal typed context
value, defined in a new sealed submodule
`crates/scorpio-core/src/workflow/tasks/handoff.rs` with `pub(super)`
visibility and no public field access — readers and writers go through
accessor functions exported from the submodule. The struct shape:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct RuntimePreflightOverride {
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
}
```

Sealing the submodule and refusing field access enforces Risk #2 (the typed
handoff staying narrow) at compile time rather than relying on convention.

Transport uses one private context key constant
`KEY_RUNTIME_PREFLIGHT_OVERRIDE`, defined alongside the struct in the same
sealed submodule.

The constants `KEY_RUNTIME_POLICY_OVERRIDE` and
`KEY_ROUTING_FALLBACK_REASON_OVERRIDE` — and every site that writes them —
are fully removed by this refactor. There is no compat shim, since the
override lives in in-memory context (not a persisted format with external
consumers) and the cleanup is atomic with the rest of the change.

`run_analysis_cycle` will:

- resolve runtime classification exactly as it does today,
- construct one `RuntimePreflightOverride`,
- store it in context under `KEY_RUNTIME_PREFLIGHT_OVERRIDE` before session
  save,
- never write runtime surfaces to `TradingState` directly.

`PreflightTask` will:

- read the single override object if present, via the submodule's accessor
  that deserializes through `serde_json::from_str` (or
  `serde_json::from_value` on a `serde_json::Value` intermediate) with
  explicit error mapping to `TaskExecutionFailed`. A present-but-malformed
  override must surface as `TaskExecutionFailed`, not silently fall back —
  this preserves the fail-loud contract that the existing two-key code
  provides at `preflight.rs:378-399` and that Risk #1 depends on. A typed
  `context.get::<RuntimePreflightOverride>` read is not acceptable on its
  own, because graph-flow's `Context::get` returns `None` on any deserialize
  mismatch and would silently downgrade to the constructor-derived fallback.
- otherwise fall back to deriving runtime policy from the pipeline manifest
  exactly as it does today. No new constructor parameter is introduced —
  `PreflightTask::new(pipeline)` continues to take only `TradingPipeline`,
  and the "constructor-provided runtime policy" phrasing of earlier drafts
  is replaced by this derive-from-pipeline path.
- when an override is present, completely replace the derived runtime policy
  and fallback reason with the override's values — no field-level merging
  occurs,
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

## Follow-up: Deferred ETF benchmark normalization cleanup

The current code normalizes ETF benchmark fallback once for benchmark fetch
assembly and then patches `FundInfo` again inside
`derive_runtime_valuation(...)` by cloning and mutating it. Collapsing those
two sites into one assembly-time normalization is a real cleanup, but it
lives in a different execution path (valuation, not preflight transport) and
has its own divergence risk (originally surfaced as
"benchmark fetch and valuation could diverge again"). To keep this slice's
revert blast radius narrow, the cleanup is deferred to a separate plan.

When that follow-up plan is written, it MUST include a `FundInfo` consumer
inventory before any relocation lands. At minimum the inventory must
enumerate:

- `crates/scorpio-core/src/valuation/etf/premium_discount.rs:55-56`, which
  reads `info.stated_benchmark` directly from a `FundInfo` argument,
- both `resolve_benchmark_symbol(...)` call sites in
  `crates/scorpio-core/src/workflow/tasks/analyst.rs` (`:766` driving
  benchmark OHLCV fetch and `:880` inside `derive_runtime_valuation`),
- any other `stated_benchmark` reader the audit surfaces.

The follow-up plan must assert that valuation-input assembly is upstream of
every listed consumer. If any consumer constructs its own `FundInfo`
independently, normalization must happen at the `FundInfo` construction
boundary instead of at `ValuationInputs` assembly. The proposed assembly-time
site is `build_valuation_inputs()` in `analyst.rs`, immediately after the
existing `resolve_benchmark_symbol(...)` call at `analyst.rs:766` and before
`ValuationInputs` is constructed at `analyst.rs:778` — but this must be
re-validated against the consumer inventory in the follow-up plan, not
assumed here.

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
- `crates/scorpio-core/src/workflow/tasks/handoff.rs` (new file)

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
- simpler preflight override precedence logic.

### Snapshot compatibility

The override lives in `graph_flow::Context`, not on `TradingState`. The new
key `KEY_RUNTIME_PREFLIGHT_OVERRIDE` does not enter
`phase_snapshots.trading_state_json`, so `THESIS_MEMORY_SCHEMA_VERSION` does
not need to be bumped.

In-flight session contexts saved with the old two-key layout
(`KEY_RUNTIME_POLICY_OVERRIDE` + `KEY_ROUTING_FALLBACK_REASON_OVERRIDE`) and
then resumed on a new binary that only reads `KEY_RUNTIME_PREFLIGHT_OVERRIDE`
fall through cleanly to the constructor-derived runtime policy — the old
keys are simply ignored, and preflight's derive-from-pipeline path computes
the same policy `run_analysis_cycle` would have written. Operators should
be aware that runs straddling the upgrade lose any persisted ETF-routing
fallback reason from the prior cycle, but ETF rerouting itself remains
correct because `run_analysis_cycle` reruns classification on each run.

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
- If the typed handoff escapes the sealed `handoff` submodule — visibility
  widened beyond `pub(super)`, or accessor functions extended to bypass the
  intended boundary — it weakens the clarity of the workflow boundary
  instead of improving it.

## Recommendation

Implement the simplification as a narrow internal refactor:

- one private typed preflight override object in a sealed submodule,
- preflight remains the sole production writer,
- inline/remove single-use ceremony.

Ship the two changes (typed handoff + readability cleanup) as separate
commits so a regression in either can be reverted independently. The
deferred ETF benchmark normalization cleanup remains in its own follow-up
plan and is not part of this slice's revert blast radius.

This is the best balance between clarity, maintainability, and contract
safety.
