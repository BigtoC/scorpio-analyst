# add-graph-orchestration remediation design

## Goal

Bring the `add-graph-orchestration` implementation into full compliance with the approved OpenSpec change by fixing architectural drift, audit integrity gaps, token-accounting omissions, and missing end-to-end verification.

## Why this remediation is needed

The current implementation compiles, passes tests, and validates under OpenSpec, but it does not faithfully implement several approved behaviors:

- Phase 2 and Phase 4 graph nodes do not map to real per-role execution.
- Zero-round debate and zero-round risk paths skip real moderator synthesis.
- Pipeline execution does not generate a fresh `execution_id` per run.
- Snapshot persistence is treated as best-effort instead of required audit evidence.
- Per-phase token accounting is not written back into `TradingState.token_usage`.
- Pipeline tests do not exercise `run_analysis_cycle` end to end.

This remediation restores the approved behavior instead of layering more workflow-only indirection on top of the drift.

## Scope

This is a full spec-compliance repair.

It includes:

- `src/workflow/` orchestration fixes
- narrow upstream agent-surface changes so each graph node can execute one real role step
- token-accounting completion
- snapshot and execution identity hardening
- observability and error-mapping corrections
- end-to-end pipeline tests
- OpenSpec cross-owner bookkeeping updates

It does not include unrelated refactoring outside the orchestration, analyst, researcher, risk, provider, and error boundaries needed for compliance.

## Architecture changes

### 1. Real per-node execution for researcher phase

The workflow must stop treating `BullishResearcherTask` as a wrapper around the entire debate loop.

Instead, the researcher module will expose narrowly-scoped operations for:

- one bullish turn
- one bearish turn
- one moderator synthesis

`BullishResearcherTask` will execute exactly one bullish turn.
`BearishResearcherTask` will execute exactly one bearish turn.
`DebateModeratorTask` will execute the moderator synthesis and own the round-completion checkpoint logic.

Round accounting will happen at the workflow boundary so graph transitions match real work. The debate round counter will increment only when a full round has completed at the moderator checkpoint.

If `max_debate_rounds = 0`, the graph will still route to `DebateModeratorTask`, and that task will perform real consensus synthesis from the analyst outputs already present in `TradingState`.

### 2. Real per-node execution for risk phase

The workflow must stop treating `AggressiveRiskTask` as a wrapper around the entire risk loop.

The risk module will expose narrowly-scoped operations for:

- one aggressive risk turn
- one conservative risk turn
- one neutral risk turn
- one moderator synthesis

Each workflow task will perform one real role step. `RiskModeratorTask` will perform moderator synthesis and own the round completion checkpoint.

If `max_risk_rounds = 0`, `RiskModeratorTask` will still perform real synthesis from the trader proposal and whatever prior state is available.

### 3. Token accounting becomes part of the workflow contract

The workflow layer will accumulate per-agent usage during each phase and write the approved data model into `TradingState.token_usage`.

At each phase boundary, the workflow will:

- build and append the required `PhaseTokenUsage` entries for that boundary
- preserve the spec's granularity where phases contain multiple reportable units
- update total prompt/completion/total token counters
- store the same phase usage in the corresponding snapshot payload

For avoidance of doubt, this must follow the OpenSpec accounting granularity rather than flattening everything to one record per top-level phase. In particular:

- Phase 1 records one analyst-team phase entry containing the four analyst usages
- Phase 2 records separate entries for each debate round plus a separate moderation entry if that is what the approved spec requires
- Phase 3 records one trader entry
- Phase 4 records separate entries for each risk-discussion round plus a separate moderation entry if that is what the approved spec requires
- Phase 5 records one fund-manager entry

If a provider does not return authoritative counts, the workflow may still use `AgentTokenUsage::unavailable`, but phase entries must still be materialized and totals must remain internally consistent.

### 4. Execution identity and snapshots become audit-grade

`run_analysis_cycle` will generate a fresh execution ID for every invocation and write it into the in-flight `TradingState` before the graph starts.

All snapshots for a run will use that generated ID. Caller-provided `TradingState.execution_id` must not be reused for the new run.

Snapshot persistence becomes required for successful phase completion. If a snapshot save fails, the task fails and the pipeline returns an error. The system should not log-and-continue through audit failure.

### 5. Snapshot schema uses one source of truth

`SnapshotStore` will use SQLx migrations as the canonical schema source for `phase_snapshots`.

The remediation will remove or minimize duplicated inline schema definitions so future schema changes do not drift between Rust and SQL.

## Error handling and observability

### Error mapping

`TradingError::GraphFlow` must preserve real workflow identity:

- phase name
- task name
- sanitized cause

Errors should no longer collapse to generic `step_N` labels when the real task/phase is known.

Workflow-surfaced error messages should apply the same sanitization posture already used for provider-layer errors, avoiding accidental leakage of verbose provider or model output text.

### Logging and tracing

The workflow layer will emit explicit structured events for:

- cycle start and end
- phase start and end
- round start and end for debate and risk
- snapshot success/failure
- task execution and task failure with real task IDs

The fund-manager path should stop logging the full `ExecutionStatus` payload at info level. Logs should retain structured decision metadata without unnecessarily exposing generated rationale text.

## Performance adjustments

The remediation should also remove avoidable runtime waste introduced by the current orchestration shape:

- restore shared analyst-news prefetch so Sentiment and News tasks do not duplicate Finnhub calls
- remove extra moderator invocations caused by wrapping full debate/risk loops inside one task
- avoid placeholder graph steps that do no real work
- cache the built graph on `TradingPipeline` if the final design leaves graph construction immutable and reusable

These are secondary to correctness, but should be fixed while the node semantics are being repaired.

## Testing strategy

### Required new tests

Add integration-style workflow tests around `TradingPipeline::run_analysis_cycle` that verify:

- full happy-path phase order
- analyst degradation with one failed analyst still continuing
- analyst degradation with two or more failed analysts aborting after Phase 1
- zero-round debate behavior
- zero-round risk behavior
- multi-round debate looping
- multi-round risk looping and ordering
- exactly five snapshots for a successful run
- fresh execution IDs per invocation
- token aggregation written into `TradingState.token_usage`
- sanitized graph-flow error propagation with real phase/task identity
- required tracing events for phase and round transitions

### Required task-wrapper tests

Replace or supplement the current placeholder-task tests with tests that verify real behavior for:

- bullish, bearish, and moderator researcher tasks
- trader task snapshot + state mutation behavior
- aggressive, conservative, neutral, and moderator risk tasks
- fund-manager terminal behavior without rationale over-logging

### Required boundary tests

Add tests for:

- malformed `trading_state` JSON in context
- malformed prefixed task payloads
- snapshot failure causing task failure
- exact default snapshot path semantics

## OpenSpec and ownership updates

This remediation requires additional cross-owner changes beyond the currently declared scope.

Because the approved OpenSpec currently encodes workflow-only wrapper behavior and does not declare the required `src/agents/**` touch-points, this remediation must first update all four change documents for `add-graph-orchestration`:

- `proposal.md`
- `design.md`
- `tasks.md`
- `specs/graph-orchestration/spec.md`

Those updates must explicitly describe the revised per-node execution model, zero-round moderator behavior, execution-ID rules, snapshot/token-accounting contract, and the newly required cross-owner edits.

`openspec/changes/add-graph-orchestration/proposal.md` must also gain or update a `## Cross-Owner Changes` section listing each foreign-owned file touched by this remediation, its owner, and the technical justification, before implementation work begins.

The delta update to `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md` must follow OpenSpec rules exactly:

- use `## MODIFIED Requirements` with full replacement requirement blocks where existing requirements change
- keep at least one `#### Scenario:` under every modified requirement
- run `openspec validate add-graph-orchestration --strict` after the spec updates and again after implementation

The minimum requirement blocks that must be updated in the change delta are:

- `### Requirement: Graph-Flow Pipeline Construction`
- `### Requirement: Researcher Debate Task Wrappers`
- `### Requirement: Risk Discussion Task Wrappers`
- `### Requirement: SQLite Phase Snapshot Storage`
- `### Requirement: Pipeline Token Accounting`
- `### Requirement: Pipeline Public API`
- `### Requirement: FlowRunner Error Propagation`

Depending on the final implementation shape, `### Requirement: Analyst Fan-Out Task Wrappers` and `### Requirement: Fund Manager Task Wrapper` may also need full `MODIFIED` replacement blocks rather than partial edits.

### Files needing cross-owner approval

- `src/agents/researcher/mod.rs` — owner: `add-researcher-debate`
- `src/agents/risk/mod.rs` — owner: `add-risk-management`
- `src/agents/analyst/mod.rs` — owner: `add-analyst-team`

Per repository policy, these files must not be edited until the relevant owner or a maintainer approves the cross-owner change. After approval, the owner's `tasks.md` must be updated with the required `Cross-Owner Touch-points` note.

The approval record should be preserved in the proposal review/approval trail for this change so the cross-owner edits remain auditable.

### Files with existing cross-owner scope that need bookkeeping updates

- `Cargo.toml` — owner: `add-project-foundation`
- `src/error.rs` — owner: `add-project-foundation`
- `src/providers/factory.rs` — owner: `add-llm-providers`

If remediation edits touch any of those already-foreign-owned files again, owner or maintainer approval must be verified before making the new edit; bookkeeping alone is not sufficient.

The `add-graph-orchestration` change docs should be updated so the declared cross-owner scope and contract details match the actual remediation, and the owner task files should receive the required `Cross-Owner Touch-points` notes per repository policy.

The concrete owner bookkeeping files for this remediation are:

- `openspec/changes/add-researcher-debate/tasks.md`
- `openspec/changes/add-risk-management/tasks.md`
- `openspec/changes/add-analyst-team/tasks.md`
- `openspec/changes/add-project-foundation/tasks.md`
- `openspec/changes/add-llm-providers/tasks.md`

## Recommended implementation order

1. Update the `add-graph-orchestration` OpenSpec change docs (`proposal.md`, `design.md`, `tasks.md`, `spec.md`) to reflect the remediation contract and required cross-owner scope.
2. Re-submit the revised `add-graph-orchestration` change docs for approval and do not resume implementation until the updated proposal is approved.
3. Obtain owner or maintainer approval for the required cross-owner edits in `src/agents/researcher/mod.rs`, `src/agents/risk/mod.rs`, `src/agents/analyst/mod.rs`, and any additional foreign-owned files touched during remediation; preserve that approval in the proposal review trail.
4. Add the required `Cross-Owner Touch-points` notes to the approved owner task files.
5. Expose single-step APIs in researcher and risk modules.
6. Restore analyst shared-news prefetch support for workflow execution.
7. Refactor workflow tasks to perform real per-node work.
8. Add execution-ID generation and required snapshot failure semantics.
9. Complete token accounting in `TradingState` and snapshot payloads.
10. Make snapshot schema management migration-driven and remove duplicated contract drift.
11. Correct graph-flow error mapping and structured tracing.
12. Add full pipeline and edge-case tests.
13. Re-run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt -- --check`, and `openspec validate add-graph-orchestration --strict`.

## Success criteria

This remediation is complete when:

- every graph node performs the real approved unit of work
- zero-round paths still produce real moderator outputs
- every run gets a fresh execution ID
- snapshot persistence is mandatory and reliable
- `TradingState.token_usage` contains phase-level records and totals after a full run
- graph-flow errors preserve real phase/task identity
- full workflow tests cover the approved orchestration behavior
- OpenSpec docs, tasks, and cross-owner records accurately reflect the final implementation
