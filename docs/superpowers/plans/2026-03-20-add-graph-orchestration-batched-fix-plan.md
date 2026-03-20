# add-graph-orchestration batched fixes Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring `add-graph-orchestration` from "tests pass but spec review is no-go" to a reviewable, spec-aligned implementation by fixing blockers in ordered batches.

**Architecture:** Fix the workflow contract first inside `src/workflow/` so the orchestration layer reports correct errors, handles zero-round flows correctly, and fails safely on orchestration corruption. Then add a real end-to-end execution seam and richer workflow-local validation, harden persistence and sanitization within the currently approved file boundary, and finish by improving observability and accounting fidelity so downstream CLI/TUI work can trust the workflow surface.

**Tech Stack:** Rust 2024, `graph-flow`, `tokio`, `sqlx` SQLite, `tracing`, OpenSpec

---

## File Map

- `src/workflow/pipeline.rs` - graph wiring, session setup, execution loop, error mapping, cycle-level tracing
- `src/workflow/tasks.rs` - per-node task wrappers, zero-round handling, phase token accounting, snapshot calls
- `src/workflow/snapshot.rs` - SQLite path handling, migration init, snapshot persistence
- `src/workflow/context_bridge.rs` - `TradingState` <-> `Context` serialization helpers
- `src/error.rs` - `TradingError::GraphFlow` shape already exists; keep behavior aligned with spec
- `tests/workflow_pipeline.rs` - integration tests for graph routing, `run_analysis_cycle`, snapshots, execution IDs, token accounting
- `tests/workflow_observability.rs` - structured tracing assertions for phases, rounds, tasks, snapshots
- `src/agents/trader/mod.rs` - not in the currently approved cross-owner list; do not edit unless the OpenSpec proposal/spec are updated first
- `src/agents/fund_manager/agent.rs` - not in the currently approved cross-owner list; do not edit unless the OpenSpec proposal/spec are updated first
- `src/agents/researcher/mod.rs` - currently approved only for narrow helper additions already captured by the OpenSpec remediation; do not broaden changes here without updating the approval scope first
- `src/agents/risk/mod.rs` - cross-owner file only if lighter single-role execution helpers are needed for performance cleanup

## Findings To Task Mapping

- Real `GraphFlow` phase/task identity lost -> Task 1
- Zero-round debate/risk semantics create phantom round entries -> Task 2
- Fan-out orchestration corruption treated like analyst degradation -> Task 3
- No successful `run_analysis_cycle()` coverage -> Task 5
- Snapshot and execution-id behavior not proven end-to-end -> Task 7
- Workflow-level error sanitization gap -> Task 9
- Persisted workflow output and snapshot-hardening concerns -> Tasks 10-11
- Missing structured observability surface -> Task 13
- Cyclic phase timing/accounting fidelity gaps -> Task 14

## Batch Order

1. Batch 1 - spec blockers in `src/workflow/`
2. Batch 2 - real end-to-end execution coverage
3. Batch 3 - security / persistence hardening
4. Batch 4 - observability and accounting fidelity cleanup

Do not start a later batch until the current batch is green and reviewed.

## Chunk 1: Batch 1 - Workflow Spec Blockers

### Task 1: Preserve real phase/task identity in workflow errors

**Files:**
- Modify: `src/workflow/pipeline.rs`
- Modify if needed: `src/workflow/tasks.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add a failing test that forces a known node failure and asserts `TradingError::GraphFlow` contains the real task id and real phase name instead of generic labels.
- [x] Run the targeted test and confirm it fails.

Run:

```bash
cargo test --test workflow_pipeline graphflow_errors_preserve_real_task_identity -- --nocapture
```

Expected: FAIL before the fix because the workflow currently returns generic phase/task labels.

- [x] Refactor workflow error mapping in `src/workflow/pipeline.rs` so runner/task failures preserve real node identity when known.
- [x] Ensure workflow-surfaced graph errors do not collapse into generic `pipeline_execution`, `flow_runner`, or `task_failure` placeholders.
- [x] Re-run the targeted test and confirm it passes.

### Task 2: Fix zero-round debate and risk semantics

**Files:**
- Modify: `src/workflow/tasks.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add failing tests for `max_debate_rounds = 0` and `max_risk_rounds = 0` that assert:
  - counters stay at `0`
  - no fake `Researcher Debate Round 1` / `Risk Discussion Round 1` phase entries are created
  - only the moderation entry is appended for the skipped cyclic phase
- [x] Run the targeted tests and confirm they fail.

Run:

```bash
cargo test --test workflow_pipeline zero_round_debate_does_not_create_phantom_round_entry -- --nocapture
cargo test --test workflow_pipeline zero_round_risk_does_not_create_phantom_round_entry -- --nocapture
```

Expected: FAIL before the fix because the moderators currently increment counters and create fake round entries.

- [x] Update `DebateModeratorTask` so zero-round routing does not increment `KEY_DEBATE_ROUND` or materialize a fake round entry.
- [x] Update `RiskModeratorTask` so zero-round routing does not increment `KEY_RISK_ROUND` or materialize a fake round entry.
- [x] Re-run the targeted tests and confirm they pass.

### Task 3: Fail hard on orchestration corruption in analyst fan-out wrappers

**Files:**
- Modify: `src/workflow/tasks.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add failing tests for analyst child tasks that prove context/state deserialization or orchestration write failures surface as workflow errors rather than being silently downgraded into analyst degradation.
- [x] Run the targeted tests and confirm they fail.
- [x] Use exact test names when adding them, for example one deserialization-failure case and one context-write-failure case, and run those exact tests while iterating.
- [x] Refactor analyst child wrappers so true orchestration corruption fails closed while normal upstream analyst failures still follow the degradation policy.
- [x] Keep the intended degradation behavior for real analyst runtime failures: `1` failure continues, `2+` failures abort.
- [x] Re-run the targeted tests and confirm they pass.

### Task 4: Verify Batch 1

**Files:**
- Verify only

- [x] Run focused workflow tests.

```bash
cargo test --test workflow_pipeline -- --nocapture
cargo test --test workflow_observability -- --nocapture
```

- [x] Run full repo verification.

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
openspec validate add-graph-orchestration --strict
```

- [x] Record any follow-up findings before moving to Batch 2.

## Chunk 2: Batch 2 - End-to-End Execution Coverage

### Task 5: Add a true success-path `run_analysis_cycle()` test seam

**Files:**
- Modify: `src/workflow/pipeline.rs`
- Modify: `tests/workflow_pipeline.rs`

- [x] Add a workflow-local deterministic test seam inside `src/workflow/` rather than changing agent-file ownership. Prefer dependency injection or test-only constructors in the workflow layer over cross-owner edits.
- [x] Add a failing integration test that successfully executes `TradingPipeline::run_analysis_cycle()` and asserts:
  - all 5 phases execute
  - returned `TradingState` is populated through final execution status
  - 5 snapshots exist
  - phase-usage entries exist in expected order
  - caller-supplied `execution_id` is overwritten
- [x] Run the targeted integration test and confirm it fails.

Run:

```bash
cargo test --test workflow_pipeline run_analysis_cycle_success_path_populates_all_phases -- --nocapture
```

Expected: FAIL before the seam is added because the suite currently has no successful end-to-end `run_analysis_cycle()` execution.

- [x] Implement the smallest workflow-local seam needed to make the success-path test deterministic without live provider calls.
- [x] Re-run the targeted integration test and confirm it passes.

### Task 6: Expand end-to-end assertions for routing and accounting

**Files:**
- Modify: `tests/workflow_pipeline.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add explicit assertions for real graph routing on `max_debate_rounds = 0/1/N` and `max_risk_rounds = 0/1/N` using the workflow-local seam from Task 5.
- [x] Add explicit assertions for phase-usage ordering across analyst, researcher round(s), moderation, trader, risk round(s), moderation, and fund manager.
- [x] Re-run the targeted tests and confirm they pass.

### Task 7: Verify snapshot and execution-id behavior end to end

**Files:**
- Modify: `tests/workflow_pipeline.rs`

- [x] Add explicit assertions that two invocations with the same caller input state produce distinct saved execution IDs.
- [x] Add explicit assertions that snapshots for phases 1-5 are loadable and contain boundary-appropriate state.
- [x] Re-run the workflow integration tests.

### Task 8: Verify Batch 2

- [x] Run focused workflow tests.

```bash
cargo test workflow_pipeline -- --nocapture
```

- [x] Run full repo verification.

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
openspec validate add-graph-orchestration --strict
```

- [x] Stop here for review before touching security-hardening files in Batch 3.

## Chunk 3: Batch 3 - Security And Persistence Hardening

### Task 9: Sanitize workflow-surfaced graph errors

**Files:**
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/workflow/tasks.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add failing tests that prove raw provider/model text is not surfaced through workflow-level `TradingError::GraphFlow` messages.
- [x] Implement a workflow-local sanitizer with the same behavior goals as the provider layer unless the approved cross-owner list is expanded.
- [x] Re-run the targeted tests and confirm they pass.

### Task 10: Defer agent-level persistence hardening unless approval scope is expanded

**Files:**
- Verify only unless scope is expanded

- [x] Do not edit `src/agents/researcher/mod.rs` or any fund-manager file in this batch unless the OpenSpec proposal/spec and cross-owner approvals are expanded first.
- [x] Record agent-level persistence redaction as deferred follow-up if workflow-local sanitization and snapshot hardening do not fully close the security concern.

> **Deferred follow-up (Task 10):** Agent-level persistence redaction was NOT needed in this batch. Workflow-level sanitization (Task 9) reuses the provider layer's `sanitize_error_summary` via `pub(crate)` visibility, covering all `TradingError::GraphFlow` causes. Agent files were not edited. If future work surfaces credential data through agent-level persistence paths (e.g. researcher debate transcripts or fund-manager rationale logs), agent-specific redaction should be added at that time.

### Task 11: Add fail-closed execution ceilings and tighten snapshot path handling

**Files:**
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/workflow/snapshot.rs`
- Test: `tests/workflow_pipeline.rs`

- [x] Add a failing test for an unexpected runaway workflow condition or corrupted round/step state.
- [x] Add a hard workflow step ceiling so orchestration fails closed instead of looping indefinitely on bad session state.
- [x] Add failing tests for hostile or malformed snapshot path edge cases; the snapshot path is required to remain configurable by spec.
- [x] Tighten snapshot path handling enough to reject clearly unsafe or malformed cases while keeping supported paths working.
- [x] Re-run targeted tests and confirm they pass.

### Task 12: Verify Batch 3

- [x] Run focused workflow and persistence tests.

```bash
cargo test --test workflow_pipeline -- --nocapture
cargo test --test workflow_observability -- --nocapture
```

- [x] Run full repo verification.

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
openspec validate add-graph-orchestration --strict
```

## Chunk 4: Batch 4 - Observability And Accounting Fidelity

### Task 13: Emit the full required workflow event surface

**Files:**
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/workflow/tasks.rs`
- Modify: `tests/workflow_observability.rs`

- [ ] Add failing observability tests for cycle start/end, phase start/end, round boundaries, task start/success/failure, and snapshot persistence events.
- [ ] Add explicit structured tracing events for each required boundary.
- [ ] Confirm fund-manager rationale is not emitted at info level while decision metadata still is.
- [ ] Re-run observability tests and confirm they pass.

### Task 14: Make cyclic phase timing and accounting trustworthy

**Files:**
- Modify: `src/workflow/tasks.rs`
- Modify: `tests/workflow_pipeline.rs`

- [ ] Add failing tests for researcher/risk per-round `PhaseTokenUsage` entries covering:
  - correct phase names
  - correct agent attribution
  - no phantom zero-round round entries
  - credible nonzero timing when work actually ran
  - correct total reconciliation
- [ ] Replace `phase_duration_ms = 0` placeholders with real timings for cyclic round entries.
- [ ] Ensure moderation and round entries are both materialized in the expected order.
- [ ] Re-run the targeted accounting tests and confirm they pass.

### Task 15: Final verification and review handoff

**Files:**
- Verify only

- [ ] Run the full verification suite.

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
openspec validate add-graph-orchestration --strict
```

- [ ] Re-read the original review findings and confirm each high-severity issue is closed or intentionally deferred with justification.
- [ ] Prepare a short review-ready summary listing:
  - closed issues
  - any remaining medium/low debt
  - cross-owner files touched
  - verification evidence

## Exit Criteria

- [x] Batch 1 closes the main no-go blockers inside `src/workflow/`
- [x] Batch 2 proves successful `run_analysis_cycle()` execution and real routing/accounting coverage within the currently approved file boundary
- [x] Batch 3 closes sanitization / persistence / runaway-execution concerns
- [ ] Batch 4 closes observability and timing/accounting fidelity gaps
- [ ] Full verification is green at the end of each batch and at final handoff
