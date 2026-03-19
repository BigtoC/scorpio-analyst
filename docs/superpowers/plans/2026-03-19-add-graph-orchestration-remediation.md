# add-graph-orchestration remediation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring `add-graph-orchestration` into full OpenSpec compliance by fixing per-node workflow behavior, audit/snapshot integrity, token accounting, and end-to-end verification.

**Architecture:** First update the OpenSpec change and cross-owner approval trail so the remediation is allowed by repo policy. Then refactor the orchestration boundary so researcher and risk graph nodes perform real single-step work, restore shared analyst-news prefetch, make execution IDs and snapshots audit-grade, and add end-to-end pipeline coverage that verifies the approved behavior.

**Tech Stack:** Rust 2024, `graph-flow`, `tokio`, `sqlx` SQLite, `tracing`, OpenSpec

---

## File Map

- `openspec/changes/add-graph-orchestration/proposal.md` — declare the remediation scope, impact, and `## Cross-Owner Changes` approvals needed before code edits.
- `openspec/changes/add-graph-orchestration/design.md` — describe the corrected orchestration design that implementation must follow.
- `openspec/changes/add-graph-orchestration/tasks.md` — reflect the true remaining remediation work and final verified status.
- `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md` — update the normative requirement deltas and scenarios to match the repaired contract.
- `openspec/changes/add-researcher-debate/tasks.md` — record approved cross-owner touch-points for researcher-file edits.
- `openspec/changes/add-risk-management/tasks.md` — record approved cross-owner touch-points for risk-file edits.
- `openspec/changes/add-analyst-team/tasks.md` — record approved cross-owner touch-points for analyst-file edits.
- `openspec/changes/add-project-foundation/tasks.md` — record approved cross-owner touch-points only if remediation re-edits `Cargo.toml` or `src/error.rs`.
- `openspec/changes/add-llm-providers/tasks.md` — record approved cross-owner touch-points only if remediation re-edits `src/providers/factory.rs`.
- `src/agents/researcher/mod.rs` — expose single-step researcher helpers so graph nodes do real work.
- `src/agents/risk/mod.rs` — expose single-step risk helpers so graph nodes do real work.
- `src/agents/analyst/mod.rs` — expose shared cached-news preparation for workflow fan-out reuse.
- `src/workflow/tasks.rs` — refactor all workflow nodes, round accounting, snapshots, and token aggregation.
- `src/workflow/pipeline.rs` — fix public API, execution IDs, graph wiring, tracing, and error mapping.
- `src/workflow/snapshot.rs` — switch schema setup to migration-driven initialization and preserve snapshot integrity.
- `src/workflow/context_bridge.rs` — add malformed payload coverage or helper changes only if workflow refactor requires it.
- `src/state/token_usage.rs` — add safe helpers for phase aggregation and total updates.
- `src/state/trading_state.rs` — support execution-ID refresh and workflow token-accounting writes.
- `src/error.rs` — tighten `TradingError::GraphFlow` semantics if needed for real phase/task attribution.
- `src/providers/factory.rs` — keep retry/error classification aligned with the remediated workflow errors if needed.
- `migrations/0001_create_phase_snapshots.sql` — become the canonical snapshot schema source.
- `Cargo.toml` — adjust SQLx or related dependency configuration only if the migration-driven snapshot setup requires it.
- `tests/workflow_pipeline.rs` — end-to-end orchestration, snapshot, degradation, and token-accounting coverage.
- `tests/workflow_observability.rs` — tracing and log-exposure coverage for workflow execution.

## Chunk 1: Spec And Approval Prerequisites

### Task 1: Update OpenSpec change docs before more code changes

**Files:**
- Modify: `openspec/changes/add-graph-orchestration/proposal.md`
- Modify: `openspec/changes/add-graph-orchestration/design.md`
- Modify: `openspec/changes/add-graph-orchestration/tasks.md`
- Modify: `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md`

- [ ] **Step 1: Read the current change docs and identify the exact mismatches to repair**

Run:

```bash
openspec show add-graph-orchestration
openspec show add-graph-orchestration --json --deltas-only
```

Then read:

```text
docs/superpowers/specs/2026-03-19-add-graph-orchestration-remediation-design.md
openspec/specs/graph-orchestration/spec.md
openspec/changes/add-graph-orchestration/proposal.md
openspec/changes/add-graph-orchestration/design.md
openspec/changes/add-graph-orchestration/tasks.md
openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md
```

Expected: you can map each remediation item to a concrete proposal/design/task/spec mismatch, not just the delta file.

- [ ] **Step 2: Add the required `## Cross-Owner Changes` section to the proposal**

Update `openspec/changes/add-graph-orchestration/proposal.md` to list each foreign-owned file touched by the remediation, its owner, and the technical justification.

- [ ] **Step 3: Update the design doc to describe the revised remediation architecture**

Revise `openspec/changes/add-graph-orchestration/design.md` so it matches the approved remediation design: real per-node researcher/risk execution, zero-round moderator behavior, required snapshots, generated execution IDs, token-accounting granularity, and sanitized task-aware error mapping.

- [ ] **Step 4: Replace inaccurate completed tasks with accurate remediation tasks**

Update `openspec/changes/add-graph-orchestration/tasks.md` so unchecked work reflects the actual remaining implementation. Do not leave already-false completed items marked `- [x]`.

- [ ] **Step 5: Rewrite `Graph-Flow Pipeline Construction` with a full `MODIFIED` block**

Update the requirement so it matches the corrected task IDs, graph construction contract, zero-round branch behavior, and structured tracing expectations.

- [ ] **Step 6: Rewrite `Researcher Debate Task Wrappers` with a full `MODIFIED` block**

Update the requirement so bullish, bearish, and moderator nodes each perform one real unit of work and the round-accounting semantics match the remediation.

- [ ] **Step 7: Rewrite `Risk Discussion Task Wrappers` with a full `MODIFIED` block**

Update the requirement so aggressive, conservative, neutral, and moderator nodes each perform one real unit of work and the round-accounting semantics match the remediation.

- [ ] **Step 8: Rewrite snapshot, token, public API, and error requirements with full `MODIFIED` blocks**

Update `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md` using full replacement blocks for these remaining requirements:

```text
SQLite Phase Snapshot Storage
Pipeline Token Accounting
Pipeline Public API
FlowRunner Error Propagation
```

Ensure each modified requirement keeps one or more `#### Scenario:` sections.

- [ ] **Step 9: Evaluate whether analyst and fund-manager requirements also need `MODIFIED` blocks**

Review these requirement blocks and update them too if the remediation changes their approved behavior or acceptance criteria:

```text
Analyst Fan-Out Task Wrappers
Fund Manager Task Wrapper
```

- [ ] **Step 10: Validate the revised change docs**

Run:

```bash
openspec validate add-graph-orchestration --strict
```

Expected: validation passes with no delta-format or scenario errors.

- [ ] **Step 11: Pause and request human approval for the revised OpenSpec change docs**

Tell the human reviewer exactly which files changed:

```text
openspec/changes/add-graph-orchestration/proposal.md
openspec/changes/add-graph-orchestration/design.md
openspec/changes/add-graph-orchestration/tasks.md
openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md
```

Expected: do not proceed until the human explicitly replies that the revised change docs are approved.

If the human does not approve or asks for changes, stop implementation, update the change docs, and repeat validation plus the approval request before continuing.

### Task 2: Record cross-owner approvals before touching foreign-owned source files

**Files:**
- Modify: `openspec/changes/add-researcher-debate/tasks.md`
- Modify: `openspec/changes/add-risk-management/tasks.md`
- Modify: `openspec/changes/add-analyst-team/tasks.md`
- Modify if needed: `openspec/changes/add-project-foundation/tasks.md`
- Modify if needed: `openspec/changes/add-llm-providers/tasks.md`

- [ ] **Step 1: Verify explicit approval exists for each foreign-owned file family before continuing**

Check a durable approval record for the updated `add-graph-orchestration` proposal (for example, the recorded proposal review artifact or explicit maintainer approval captured with the change review) and confirm it explicitly approved:

- the revised `add-graph-orchestration` change docs
- edits to `src/agents/researcher/mod.rs`
- edits to `src/agents/risk/mod.rs`
- edits to `src/agents/analyst/mod.rs`
- edits to `Cargo.toml` / `src/error.rs` if they will be re-touched
- edits to `src/providers/factory.rs` if it will be re-touched
- updates to the owner `openspec/changes/*/tasks.md` bookkeeping files required by the policy

Expected: you can point to the approval record before proceeding.

If one or more required file-family approvals are missing, stop here and ask the human for the missing approval before editing any foreign-owned source file.

- [ ] **Step 2: Obtain and record any still-missing owner or maintainer approvals before bookkeeping edits**

If any approval is missing after Step 1, request explicit approval from the relevant owner or a maintainer, record that approval in the durable review artifact for `add-graph-orchestration`, and do not touch any foreign-owned file until that approval exists.

- [ ] **Step 3: Add the `### Cross-Owner Touch-points` note to `add-researcher-debate`**

Update `openspec/changes/add-researcher-debate/tasks.md` under a `### Cross-Owner Touch-points` section with the approved touch-point for `src/agents/researcher/mod.rs`, including the approval-backed reason for the edit.

- [ ] **Step 4: Add the `### Cross-Owner Touch-points` note to `add-risk-management`**

Update `openspec/changes/add-risk-management/tasks.md` under a `### Cross-Owner Touch-points` section with the approved touch-point for `src/agents/risk/mod.rs`, including the approval-backed reason for the edit.

- [ ] **Step 5: Add the `### Cross-Owner Touch-points` note to `add-analyst-team`**

Update `openspec/changes/add-analyst-team/tasks.md` under a `### Cross-Owner Touch-points` section with the approved touch-point for `src/agents/analyst/mod.rs`, including the approval-backed reason for the edit.

- [ ] **Step 6: Add `add-project-foundation` `### Cross-Owner Touch-points` only if those files will be re-edited**

If the remediation will re-edit `Cargo.toml` or `src/error.rs`, update `openspec/changes/add-project-foundation/tasks.md` under `### Cross-Owner Touch-points` with the approved reason. Otherwise skip this step and record that it was not needed.

- [ ] **Step 7: Add `add-llm-providers` `### Cross-Owner Touch-points` only if that file will be re-edited**

If the remediation will re-edit `src/providers/factory.rs`, update `openspec/changes/add-llm-providers/tasks.md` under `### Cross-Owner Touch-points` with the approved reason. Otherwise skip this step and record that it was not needed.

- [ ] **Step 8: Re-validate the touched OpenSpec changes after bookkeeping updates**

Run:

```bash
openspec validate add-graph-orchestration --strict
openspec validate add-researcher-debate --strict
openspec validate add-risk-management --strict
openspec validate add-analyst-team --strict
```

If Step 6 was needed, also run:

```bash
openspec validate add-project-foundation --strict
```

If Step 7 was needed, also run:

```bash
openspec validate add-llm-providers --strict
```

Expected: every touched change still validates successfully.

## Chunk 2: Upstream Agent Surfaces For Real Per-Node Execution

### Task 3: Add single-step researcher APIs with tests

**Files:**
- Modify: `src/agents/researcher/mod.rs`
- Test: `src/agents/researcher/mod.rs`

- [ ] **Step 1: Write failing tests for one-turn bullish, bearish, and moderator helpers**

Add tests covering:

```rust
#[tokio::test]
async fn researcher_single_bullish_turn_appends_one_message() {}

#[tokio::test]
async fn researcher_single_bearish_turn_appends_one_message() {}

#[tokio::test]
async fn researcher_moderation_sets_consensus_without_extra_turns() {}
```

- [ ] **Step 2: Run the new researcher tests and confirm they fail**

Run:

```bash
cargo test src::agents::researcher -- --nocapture
```

Expected: new tests fail because the single-step APIs do not exist yet.

- [ ] **Step 3: Implement minimal single-step public helpers**

Add public functions that execute one real unit of work each, reusing the existing executor seam:

```rust
pub async fn run_bullish_researcher_turn(...) -> Result<AgentTokenUsage, TradingError>
pub async fn run_bearish_researcher_turn(...) -> Result<AgentTokenUsage, TradingError>
pub async fn run_debate_moderation(...) -> Result<AgentTokenUsage, TradingError>
```

Each helper should mutate `TradingState` exactly once for its role and not run the full loop.

- [ ] **Step 4: Re-run the researcher tests**

Run:

```bash
cargo test src::agents::researcher -- --nocapture
```

Expected: new and existing researcher tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agents/researcher/mod.rs
git commit -m "refactor: expose single-step researcher workflow helpers"
```

### Task 4: Add single-step risk APIs with tests

**Files:**
- Modify: `src/agents/risk/mod.rs`
- Test: `src/agents/risk/mod.rs`

- [ ] **Step 1: Write failing tests for aggressive, conservative, neutral, and moderator helpers**

Add tests covering one-step execution for each role plus zero-round moderator synthesis.

- [ ] **Step 2: Run the risk tests and confirm they fail**

Run:

```bash
cargo test src::agents::risk -- --nocapture
```

Expected: new tests fail because the single-step APIs do not exist yet.

- [ ] **Step 3: Implement minimal single-step public helpers**

Add public functions such as:

```rust
pub async fn run_aggressive_risk_turn(...) -> Result<AgentTokenUsage, TradingError>
pub async fn run_conservative_risk_turn(...) -> Result<AgentTokenUsage, TradingError>
pub async fn run_neutral_risk_turn(...) -> Result<AgentTokenUsage, TradingError>
pub async fn run_risk_moderation(...) -> Result<AgentTokenUsage, TradingError>
```

Each helper should mutate only the fields owned by that role and not run the full loop.

- [ ] **Step 4: Re-run the risk tests**

Run:

```bash
cargo test src::agents::risk -- --nocapture
```

Expected: new and existing risk tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agents/risk/mod.rs
git commit -m "refactor: expose single-step risk workflow helpers"
```

### Task 5: Add shared analyst-news prefetch helper for workflow fan-out

**Files:**
- Modify: `src/agents/analyst/mod.rs`
- Test: `src/agents/analyst/mod.rs`

- [ ] **Step 1: Write a failing test for reusable cached-news preparation**

Add a focused unit test that verifies workflow callers can obtain one shared cached news payload for sentiment and news analysts.

- [ ] **Step 2: Run the analyst tests and confirm failure**

Run:

```bash
cargo test src::agents::analyst -- --nocapture
```

Expected: the new helper test fails because the reusable helper does not exist.

- [ ] **Step 3: Implement a minimal shared helper**

Add a small public helper returning `Option<Arc<NewsData>>` from a single Finnhub fetch, reusing the existing prefetch logic from `run_analyst_team`.

- [ ] **Step 4: Re-run the analyst tests**

Run:

```bash
cargo test src::agents::analyst -- --nocapture
```

Expected: all analyst tests pass and the helper is available for workflow tasks.

- [ ] **Step 5: Commit**

```bash
git add src/agents/analyst/mod.rs
git commit -m "refactor: share analyst news prefetch helper"
```

## Chunk 3: Workflow Refactor For Correct Semantics

### Task 6: Add workflow token-accounting helpers first

**Files:**
- Modify: `src/state/token_usage.rs`
- Modify: `src/state/trading_state.rs`
- Modify: `src/workflow/tasks.rs`
- Test: `src/workflow/tasks.rs`

- [ ] **Step 1: Write failing tests for appending phase token usage into `TradingState`**

Cover:

```rust
#[test]
fn phase_usage_updates_tracker_totals() {}

#[test]
fn unavailable_agent_usage_still_materializes_phase_entry() {}
```

- [ ] **Step 2: Run the targeted tests and confirm failure**

Run:

```bash
cargo test workflow::tasks token_usage -- --nocapture
```

Expected: helper methods are missing or totals are not updated yet.

- [ ] **Step 3: Implement minimal token-accounting helpers**

Add small helpers for:

```rust
impl TokenUsageTracker {
    pub fn push_phase_usage(&mut self, phase: PhaseTokenUsage) { ... }
}
```

and any workflow-local aggregation helpers needed to compute phase totals from `Vec<AgentTokenUsage>`.

- [ ] **Step 4: Re-run the targeted tests**

Run:

```bash
cargo test workflow::tasks token_usage -- --nocapture
```

Expected: token helper tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/state/token_usage.rs src/state/trading_state.rs src/workflow/tasks.rs
git commit -m "feat: add workflow phase token accounting helpers"
```

### Task 7: Fix analyst workflow tasks to preserve cache, usage, and degradation behavior

**Files:**
- Modify: `src/workflow/tasks.rs`
- Test: `src/workflow/tasks.rs`

- [ ] **Step 1: Write failing tests for analyst cached-news reuse and phase-1 usage capture**

Add tests that verify:
- sentiment/news tasks can read a shared cache value from context
- phase 1 produces materialized token usage for all analysts
- 1 failure continues and 2 failures abort while still producing the expected phase accounting behavior

- [ ] **Step 2: Run the targeted workflow task tests**

Run:

```bash
cargo test workflow::tasks::tests:: -- --nocapture
```

Expected: new tests fail under current workflow behavior.

- [ ] **Step 3: Implement minimal context/cache/usage changes**

Add context keys for cached analyst news and phase-usage accumulation, update analyst tasks to use shared cache when available, and update `AnalystSyncTask` to write Phase 1 token usage into `TradingState` and snapshots.

- [ ] **Step 4: Re-run the workflow task tests**

Run:

```bash
cargo test workflow::tasks::tests:: -- --nocapture
```

Expected: phase-1 workflow tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/workflow/tasks.rs
git commit -m "fix: restore analyst workflow cache and phase accounting"
```

### Task 8: Refactor researcher workflow tasks to perform real per-node work

**Files:**
- Modify: `src/workflow/tasks.rs`
- Test: `src/workflow/tasks.rs`

- [ ] **Step 1: Write failing tests for real bullish, bearish, and moderator task behavior**

Cover:
- bullish task appends one bullish message only
- bearish task appends one bearish message only
- moderator task produces consensus on zero-round path
- round counter increments at moderator checkpoint, not bullish task entry
- phase usage entries follow the approved round/moderation granularity

- [ ] **Step 2: Run the targeted researcher task tests**

Run:

```bash
cargo test workflow::tasks researcher -- --nocapture
```

Expected: failures under current placeholder/full-loop behavior.

- [ ] **Step 3: Implement minimal researcher task refactor**

Update `BullishResearcherTask`, `BearishResearcherTask`, and `DebateModeratorTask` to use the new single-step researcher helpers and to write proper state, round counters, token usage, and snapshots.

- [ ] **Step 4: Re-run the targeted researcher task tests**

Run:

```bash
cargo test workflow::tasks researcher -- --nocapture
```

Expected: researcher workflow tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/workflow/tasks.rs
git commit -m "fix: make researcher workflow nodes execute real turns"
```

### Task 9: Refactor risk workflow tasks to perform real per-node work

**Files:**
- Modify: `src/workflow/tasks.rs`
- Test: `src/workflow/tasks.rs`

- [ ] **Step 1: Write failing tests for real aggressive, conservative, neutral, and moderator task behavior**

Cover:
- each task mutates only its owned state
- zero-round risk path still runs moderator synthesis
- round counter increments at moderator checkpoint
- per-round/per-moderation token accounting is preserved
- placeholder-node behavior is removed

- [ ] **Step 2: Run the targeted risk task tests**

Run:

```bash
cargo test workflow::tasks risk -- --nocapture
```

Expected: failures under current full-loop/placeholder behavior.

- [ ] **Step 3: Implement minimal risk task refactor**

Update all four risk tasks to use the new single-step risk helpers and real round accounting.

- [ ] **Step 4: Re-run the targeted risk task tests**

Run:

```bash
cargo test workflow::tasks risk -- --nocapture
```

Expected: risk workflow tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/workflow/tasks.rs
git commit -m "fix: make risk workflow nodes execute real turns"
```

### Task 10: Harden snapshots, execution IDs, and migration usage

**Files:**
- Modify: `src/workflow/snapshot.rs`
- Modify: `migrations/0001_create_phase_snapshots.sql`
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/workflow/tasks.rs`
- Test: `src/workflow/snapshot.rs`
- Test: `tests/workflow_pipeline.rs`

- [ ] **Step 1: Write failing tests for fresh execution IDs and mandatory snapshot saves**

Add tests covering:
- new `execution_id` assigned per `run_analysis_cycle`
- repeated runs do not overwrite each other
- snapshot save failure aborts the pipeline/task instead of warning and continuing
- migration-backed schema loads correctly

- [ ] **Step 2: Run the new snapshot/pipeline tests**

Run:

```bash
cargo test workflow_pipeline snapshot -- --nocapture
```

Expected: failures under current caller-controlled ID and best-effort snapshot behavior.

- [ ] **Step 3: Implement minimal snapshot and ID changes**

Make `run_analysis_cycle` assign a fresh `Uuid` to the working state, switch snapshot initialization to SQLx migration-driven setup, and make snapshot persistence failures fatal in workflow tasks.

- [ ] **Step 4: Re-run the snapshot/pipeline tests**

Run:

```bash
cargo test workflow_pipeline snapshot -- --nocapture
```

Expected: execution-ID and snapshot tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/workflow/snapshot.rs migrations/0001_create_phase_snapshots.sql src/workflow/pipeline.rs src/workflow/tasks.rs tests/workflow_pipeline.rs
git commit -m "fix: make workflow execution IDs and snapshots audit-grade"
```

### Task 11: Correct pipeline public API, graph wiring, and error mapping

**Files:**
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/error.rs`
- Modify: `src/providers/factory.rs`
- Modify: `Cargo.toml`
- Test: `tests/workflow_pipeline.rs`

- [ ] **Step 1: Write failing tests for the corrected public API and task-aware GraphFlow errors**

Add tests that verify:
- `TradingPipeline::new(...) -> Result<Self>` if initialization can fail
- graph uses spec-correct task IDs such as `analyst_fanout`
- `TradingError::GraphFlow` carries real phase/task identity on workflow failures
- zero-round branch routing reaches real moderator behavior

- [ ] **Step 2: Run the pipeline tests and confirm failure**

Run:

```bash
cargo test workflow_pipeline -- --nocapture
```

Expected: API and error mapping tests fail under current implementation.

- [ ] **Step 3: Implement minimal pipeline/API corrections**

Refactor `TradingPipeline` to match the revised OpenSpec contract, fix graph task IDs and wiring, and map workflow failures with real task/phase names and sanitized causes.

- [ ] **Step 4: Re-run the pipeline tests**

Run:

```bash
cargo test workflow_pipeline -- --nocapture
```

Expected: pipeline API and error-mapping tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/workflow/pipeline.rs src/error.rs src/providers/factory.rs Cargo.toml tests/workflow_pipeline.rs
git commit -m "fix: align trading pipeline API and graph-flow error mapping"
```

## Chunk 4: End-To-End Verification And Observability

### Task 12: Add end-to-end workflow integration tests

**Files:**
- Create: `tests/workflow_pipeline.rs`

- [ ] **Step 1: Write failing end-to-end tests around `run_analysis_cycle`**

Add tests for:
- full happy path
- one analyst failure continues
- two analyst failures abort
- zero-round debate
- zero-round risk
- multi-round debate ordering
- multi-round risk ordering
- exactly five snapshots on success
- token usage written into state with required granularity

- [ ] **Step 2: Run the new integration tests and confirm failure**

Run:

```bash
cargo test --test workflow_pipeline -- --nocapture
```

Expected: several tests fail until the pipeline behavior is fully compliant.

- [ ] **Step 3: Implement only the missing glue needed to make the tests pass**

Keep changes focused on orchestration/test seams rather than broad refactors.

- [ ] **Step 4: Re-run the integration tests**

Run:

```bash
cargo test --test workflow_pipeline -- --nocapture
```

Expected: all `workflow_pipeline` tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/workflow_pipeline.rs src/workflow/*.rs src/agents/**/*.rs
git commit -m "test: cover end-to-end graph orchestration behavior"
```

### Task 13: Add tracing/observability tests and tighten logging

**Files:**
- Create: `tests/workflow_observability.rs`
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/workflow/tasks.rs`

- [ ] **Step 1: Write failing observability tests**

Add tests asserting emitted structured events for:
- cycle start/end
- phase boundaries
- debate/risk round boundaries
- snapshot persistence
- real task IDs on failure

Also add a test ensuring fund-manager rationale is not logged in the info-level structured event.

- [ ] **Step 2: Run the observability tests and confirm failure**

Run:

```bash
cargo test --test workflow_observability -- --nocapture
```

Expected: tracing expectations fail under the current partial logging behavior.

- [ ] **Step 3: Implement minimal tracing/logging changes**

Add the required structured events and remove or reduce rationale logging exposure.

- [ ] **Step 4: Re-run the observability tests**

Run:

```bash
cargo test --test workflow_observability -- --nocapture
```

Expected: observability tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/workflow_observability.rs src/workflow/pipeline.rs src/workflow/tasks.rs
git commit -m "feat: add workflow phase and round observability"
```

### Task 14: Final verification and checklist truthfulness

**Files:**
- Modify: `openspec/changes/add-graph-orchestration/tasks.md`

- [ ] **Step 1: Run the full project verification suite**

Run:

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt -- --check
openspec validate add-graph-orchestration --strict
```

Expected: all commands succeed.

- [ ] **Step 2: Audit the OpenSpec task checklist against reality**

Only mark items `- [x]` that are now truly implemented and verified. If any items remain intentionally out of scope, leave them unchecked and document why.

- [ ] **Step 3: Update the task checklist**

Edit `openspec/changes/add-graph-orchestration/tasks.md` so it accurately reflects the completed remediation state.

- [ ] **Step 4: Review git diff for unintended changes**

Run:

```bash
git status --short
git diff --stat
```

Expected: only intended files are modified.

- [ ] **Step 5: Commit**

```bash
git add openspec/changes/add-graph-orchestration/tasks.md
git commit -m "docs: finalize graph orchestration remediation checklist"
```
