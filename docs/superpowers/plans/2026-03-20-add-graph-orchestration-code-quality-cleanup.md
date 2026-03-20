# add-graph-orchestration code quality cleanup Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve maintainability, diagnostics, and API clarity of the `add-graph-orchestration` workflow code without changing workflow behavior or expanding the approved product scope.

**Architecture:** First fix the highest-value quality issue: snapshot/runtime persistence errors are currently typed too broadly, and round-usage context writes still rely on ad-hoc JSON string storage. Then split `src/workflow/tasks.rs` into a package-style module with a narrow facade so production tasks, accounting, test helpers, and tests stop cohabiting one large file. Finish by tightening the public workflow surface and helper contracts so future refactors remain internal details instead of API breakage.

**Tech Stack:** Rust 2024, `tokio`, `graph-flow`, `serde`, `serde_json`, `sqlx`, `tracing`

---

## File Map

- `src/error.rs` - add a persistence/storage-oriented `TradingError` variant for runtime snapshot failures
- `src/workflow/snapshot.rs` - remap runtime persistence errors, strengthen snapshot API typing, add `Debug`
- `src/workflow/context_bridge.rs` - validate composite-key leaf parts and document the contract
- `src/workflow/mod.rs` - narrow workflow facade and expose only intentional surface area
- `src/workflow/test_support.rs` - feature-gated/public test support facade for integration tests
- `src/workflow/tasks/mod.rs` - facade that re-exports the stable workflow task API
- `src/workflow/tasks/analyst.rs` - analyst child tasks plus `AnalystSyncTask`
- `src/workflow/tasks/research.rs` - bullish, bearish, and debate moderator tasks
- `src/workflow/tasks/risk.rs` - aggressive, conservative, neutral, and risk moderator tasks
- `src/workflow/tasks/trading.rs` - trader + fund manager tasks
- `src/workflow/tasks/accounting.rs` - shared debate/risk accounting helpers
- `src/workflow/tasks/common.rs` - task-local constants and small shared helpers
- `src/workflow/tasks/test_helpers.rs` - `test-helpers` stubs and accounting helpers
- `src/workflow/tasks/tests.rs` - unit tests that currently live inside `tasks.rs`
- `tests/workflow_pipeline.rs` - integration coverage for snapshot typing, accounting, and public test support
- `tests/workflow_observability.rs` - update imports if test-support surface moves

## Constraints

- Preserve workflow behavior and existing OpenSpec semantics.
- Do not edit `src/agents/trader/mod.rs`, `src/agents/fund_manager/agent.rs`, or broaden cross-owner scope.
- Treat this as a code-quality cleanup, not a feature project: prefer facade/re-export refactors over behavioral changes.
- Keep integration-test support available under `--features test-helpers`.

## Chunk 1: Runtime Error Semantics And Snapshot API Clarity

### Task 1: Introduce a runtime persistence error variant

**Files:**
- Modify: `src/error.rs`
- Modify: `src/workflow/snapshot.rs`
- Test: `src/workflow/snapshot.rs`

- [x] Add a failing unit test that closes the snapshot pool via `close_for_test()` and asserts `save_snapshot()` returns a runtime persistence/storage error variant rather than `TradingError::Config`.
- [x] Add a failing unit test that closes the pool and asserts `load_snapshot()` returns the same runtime persistence/storage error variant rather than `TradingError::Config`.
- [x] Keep `SnapshotStore::new()` configuration/path/open failures mapped to `TradingError::Config`; only runtime save/load/deserialize failures should change classification.
- [x] Add a `TradingError` variant for runtime persistence failures (implemented as `Storage(anyhow::Error)` with source preservation).
- [x] Update `src/workflow/snapshot.rs` so:
  - `new()` continues to return `TradingError::Config` for path/home-dir/open/migration failures
  - `save_snapshot()` returns the new persistence variant for serialization and database write failures
  - `load_snapshot()` returns the new persistence variant for database read and snapshot JSON decode failures
- [x] Update rustdoc in `src/workflow/snapshot.rs` to describe the new error split precisely.
- [x] Re-run the targeted snapshot tests and confirm they pass.

Run:

```bash
cargo test snapshot::tests::save_and_load_round_trip -- --nocapture
cargo test workflow::tasks::tests::analyst_sync_snapshot_failure_propagates_as_err -- --nocapture
```

### Task 2: Replace weakly typed snapshot phase metadata and tuple return values

**Files:**
- Modify: `src/workflow/snapshot.rs`
- Modify: `src/workflow/tasks/analyst.rs`
- Modify: `src/workflow/tasks/research.rs`
- Modify: `src/workflow/tasks/risk.rs`
- Modify: `src/workflow/tasks/trading.rs`
- Modify: `tests/workflow_pipeline.rs`
- Test: `src/workflow/snapshot.rs`

- [x] Add a failing unit test that prevents mismatched snapshot metadata (for example, phase 1 with a non-phase-1 name) from being constructed through the public snapshot API.
- [x] Add a small typed phase model in `src/workflow/snapshot.rs`, such as:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowPhase {
    AnalystTeam,
    ResearcherDebate,
    Trader,
    RiskDiscussion,
    FundManager,
}
```

- [x] Add methods like `number()` and `name()` on that type so callers stop passing independent `u8` and `&str` values.
- [x] Replace `load_snapshot()`'s tuple return with a small named struct, e.g.:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSnapshot {
    pub state: TradingState,
    pub token_usage: Option<Vec<AgentTokenUsage>>,
}
```

- [x] Update all workflow tasks and tests to use the typed phase enum / named snapshot result.
- [x] Add `#[derive(Debug)]` to `SnapshotStore`.
- [x] Re-run snapshot + workflow integration tests and confirm they pass.

Run:

```bash
cargo test --test workflow_pipeline integration_phase_snapshot_written_and_readable -- --nocapture
cargo test --test workflow_pipeline e2e_snapshots_contain_boundary_appropriate_state -- --nocapture
```

### Task 3: Remove ad-hoc JSON string storage for per-round usage

**Files:**
- Modify: `src/workflow/tasks/research.rs`
- Modify: `src/workflow/tasks/risk.rs`
- Modify: `src/workflow/tasks/accounting.rs`
- Modify: `src/workflow/tasks/test_helpers.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add a failing test that proves debate/risk round accounting still works when usage is stored and retrieved as typed `AgentTokenUsage` values instead of JSON strings.
- [x] Replace all per-round `context.set(usage_key, serde_json::to_string(&usage).unwrap_or_default())` writes with typed `AgentTokenUsage` storage helpers.
- [x] Replace the moderator-accounting reads from `Option<String> + serde_json::from_str` with typed round-usage reads.
- [x] Keep the existing unavailable fallback only for truly missing round usage, not malformed intermediary JSON strings.
- [x] Update stub tasks in `test_helpers.rs` to use the same typed storage contract as production tasks.
- [x] Re-run the round-accounting tests and confirm they pass unchanged in behavior.

Run:

```bash
cargo test --features test-helpers --test workflow_pipeline accounting_ -- --nocapture
cargo test --features test-helpers --test workflow_pipeline e2e_multi_round_debate_and_risk_routing_and_accounting -- --nocapture
```

## Chunk 2: Split `tasks.rs` Into Focused Modules

### Task 4: Convert `src/workflow/tasks.rs` into a package-style module with a facade

**Files:**
- Create: `src/workflow/tasks/mod.rs`
- Create: `src/workflow/tasks/common.rs`
- Create: `src/workflow/tasks/accounting.rs`
- Create: `src/workflow/tasks/analyst.rs`
- Create: `src/workflow/tasks/research.rs`
- Create: `src/workflow/tasks/risk.rs`
- Create: `src/workflow/tasks/trading.rs`
- Create: `src/workflow/tasks/test_helpers.rs`
- Create: `src/workflow/tasks/tests.rs`
- Delete after move: `src/workflow/tasks.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Move constants (`KEY_MAX_DEBATE_ROUNDS`, `KEY_RISK_ROUND`, analyst key constants, etc.) and tiny shared helpers into `common.rs`.
- [x] Move shared debate/risk moderator accounting into `accounting.rs`.
- [x] Move Phase 1 analyst tasks + `AnalystSyncTask` into `analyst.rs`.
- [x] Move debate tasks into `research.rs`.
- [x] Move risk tasks into `risk.rs`.
- [x] Move `TraderTask` and `FundManagerTask` into `trading.rs`.
- [x] Move feature-gated stubs and helper functions into `test_helpers.rs`.
- [x] Move unit tests into `tests.rs`.
- [x] In `src/workflow/tasks/mod.rs`, re-export only the intended public surface (`AnalystSyncTask`, task structs, key constants, `test_helpers` behind feature gate) so the file split is internal.
- [x] Re-run the full workflow test suite after the move before doing any further cleanup.

Run:

```bash
cargo test --features test-helpers --test workflow_pipeline -- --nocapture
cargo test --features test-helpers --test workflow_observability -- --nocapture
```

### Task 5: Extract repeated analyst merge logic from `AnalystSyncTask`

**Files:**
- Modify: `src/workflow/tasks/analyst.rs`
- Test: `src/workflow/tasks/tests.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add a failing unit test that exercises a helper-based analyst merge path for both success and read-failure cases.
- [x] Introduce a small helper to eliminate the four repeated merge blocks. Recommended shape:

```rust
async fn merge_analyst_result<T>(
    context: &Context,
    ok: bool,
    analyst_key: &str,
    failures: &mut Vec<&'static str>,
) -> Option<T>
where
    T: DeserializeOwned {}
```

- [x] Use that helper from `AnalystSyncTask::run()` so each analyst branch becomes one line assigning `state.fundamental_metrics`, `state.market_sentiment`, etc.
- [x] Keep the existing degradation policy and logging behavior unchanged.
- [x] Re-run targeted `AnalystSyncTask` unit/integration tests and confirm no behavioral drift.

Run:

```bash
cargo test workflow::tasks::tests::analyst_sync_all_succeed_returns_continue -- --nocapture
cargo test --test workflow_pipeline integration_two_analyst_failures_abort_pipeline -- --nocapture
```

### Task 6: Update stale docs/comments during the module split

**Files:**
- Modify: `src/workflow/tasks/analyst.rs`
- Modify: `src/workflow/tasks/research.rs`
- Modify: `src/workflow/tasks/risk.rs`
- Modify: `src/workflow/tasks/trading.rs`

- [x] Correct rustdoc that no longer matches reality, especially the `BullishResearcherTask` comment that claims the task increments `debate_round` directly.
- [x] Ensure each public task type documents what it does, what it persists, and which phase/next-action it participates in.
- [x] Keep comments focused on why/contract, not line-by-line narration.
- [x] Run `cargo fmt` and `cargo clippy` after doc cleanup.

## Chunk 3: Narrow The Public Workflow Surface

### Task 7: Add a dedicated workflow test-support facade

**Files:**
- Create: `src/workflow/test_support.rs`
- Modify: `src/workflow/mod.rs`
- Modify: `tests/workflow_pipeline.rs`
- Modify: `tests/workflow_observability.rs`

- [x] Create `src/workflow/test_support.rs` with the small external-facing test helpers currently pulled from internal modules, for example:

```rust
pub use super::context_bridge::{
    deserialize_state_from_context,
    serialize_state_to_context,
    TRADING_STATE_KEY,
};
#[cfg(feature = "test-helpers")]
pub use super::tasks::test_helpers::*;
```

- [x] Update integration tests to import from `scorpio_analyst::workflow::test_support` instead of reaching into `workflow::tasks` and `workflow::context_bridge` directly.
- [x] Keep the old public modules temporarily only if needed to avoid a too-large diff; otherwise make them `pub(crate)` once tests are migrated.
- [x] Re-export the intended workflow facade, with an intentional compatibility exception: `SnapshotStore`, `SnapshotPhase`, and `LoadedSnapshot` remain public because they are required by the typed snapshot API.
- [x] Re-run workflow integration tests to confirm the new test-support surface is sufficient.

Run:

```bash
cargo test --features test-helpers --test workflow_pipeline -- --nocapture
cargo test --features test-helpers --test workflow_observability -- --nocapture
```

### Task 8: Harden `context_bridge` composite-key contracts

**Files:**
- Modify: `src/workflow/context_bridge.rs`
- Modify: `src/workflow/test_support.rs`
- Test: `src/workflow/context_bridge.rs`

- [x] Add failing tests for invalid key leaf parts such as `"fundamental.err"` or empty key parts that would alias composite keys.
- [x] Keep namespace-style prefixes allowed (for example `"usage.analyst"`) but validate leaf `key` values before concatenation.
- [x] Add a small validator helper, implemented as `validate_prefixed_leaf_key(...)`.
- [x] Update `write_prefixed_result()` and `read_prefixed_result()` to reject invalid leaf keys with `TradingError::SchemaViolation`.
- [x] Expand rustdoc to make the prefix-vs-leaf-key contract explicit.
- [x] Re-run context-bridge unit tests and the workflow tests that use prefixed results.

Run:

```bash
cargo test workflow::context_bridge::tests -- --nocapture
cargo test --test workflow_pipeline analyst_child_deserialization_failure_returns_err -- --nocapture
```

### Task 9: Final verification and review handoff

**Files:**
- Verify only

- [x] Run the full verification suite.

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test --features test-helpers
openspec validate add-graph-orchestration --strict
```

- [x] Re-read the code-quality review findings and confirm each item is either closed or intentionally deferred with rationale.
- [x] Prepare a short handoff note listing:
  - error-semantics changes
  - module/facade changes
  - any intentionally preserved public compatibility shims
  - verification evidence
