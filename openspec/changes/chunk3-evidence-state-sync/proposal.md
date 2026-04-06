## Why

Chunks 1 and 2 established the Stage 1 evidence-discipline posture at the docs/prompt layer and created the runtime
scaffolding (`DataEnrichmentConfig`, entity resolution, `PreflightTask`). Chunk 3 introduces the typed state and
downstream prompt-consumer slice that makes evidence discipline concrete and machine-verifiable.

Per `docs/architect-plan.md`, this work belongs to the single architected `evidence-provenance` capability. The
repository is currently executing that capability in reviewable chunked changes (`chunk1` through `chunk4`), so Chunk 3
must stay constrained to the typed-state and prompt-consumer slice rather than inventing new capability IDs or drifting
into adjacent chunks.

Today, each analyst writes raw data into a single legacy field on `TradingState`
(`fundamental_metrics`, `technical_indicators`, `market_sentiment`, `macro_news`). There is no typed record of _where_
the data came from, _when_ it was fetched, _which provider_ contributed it, or _what_ quality caveats apply. As a
result:

- downstream agents (researcher, trader, risk, fund manager) receive analyst output with no typed provenance or
  coverage metadata, so they cannot reliably distinguish complete evidence from partial evidence
- the final report chunk has no authoritative run-level coverage or provenance state to render
- adding future evidence categories (transcripts, estimates, event feeds) would require ad-hoc state growth instead of
  using a consistent typed envelope

The previous draft was incomplete and partially inconsistent: it had no OpenSpec delta, no `## Cross-Owner Changes`
section, it invented standalone capability names that do not exist in the architect plan, it prescribed
`ProvenanceSummary.providers_used` semantics that conflicted with "evidence used in the current run", and it described
prompt injection patterns that do not match the current code shape. This review corrects those gaps.

## What Changes

- **`src/state/provenance.rs`** (new file): `EvidenceSource` and `DataQualityFlag`.
- **`src/state/evidence.rs`** (new file): `EvidenceKind` and generic `EvidenceRecord<T>`.
- **`src/state/reporting.rs`** (new file): `DataCoverageReport` and `ProvenanceSummary`.
- **`src/state/mod.rs`** (modify): export the three new state modules and re-export their public types.
- **`src/state/trading_state.rs`** (modify): add six additive `Option<>` fields — `evidence_fundamental`,
  `evidence_technical`, `evidence_sentiment`, `evidence_news`, `data_coverage`, and `provenance_summary` — all
  initialized to `None` in `TradingState::new`.
- **`src/workflow/context_bridge.rs`** (modify): add round-trip coverage proving the new state fields survive
  graph-flow context serialization.
- **`src/workflow/snapshot.rs`** (modify): add round-trip coverage proving the new state fields survive SQLite
  snapshot save/load.
- **`src/workflow/tasks/analyst.rs`** (modify): update `AnalystSyncTask` to dual-write legacy and `evidence_*`
  fields and to derive `DataCoverageReport` / `ProvenanceSummary` from the typed evidence that is actually present on
  the continue path.
- **`src/workflow/tasks/tests.rs`** (modify): extend the all-success regression and add a one-missing-input regression
  that exercises the `0-1` failure continue path.
- **`src/agents/shared/prompt.rs`** (modify): add `build_evidence_context(state)` and `build_data_quality_context(state)`
  with no-panic fallback paths using the shared prompt-safe serialization/sanitization posture already present in this
  module.
- **`src/agents/researcher/common.rs`**, **`src/agents/risk/common.rs`**, **`src/agents/trader/mod.rs`**, and
  **`src/agents/fund_manager/prompt.rs`** (modify): inject the new typed evidence/data-quality builders at each
  module's existing dynamic prompt-construction boundary.
- **`openspec/changes/chunk3-evidence-state-sync/specs/evidence-provenance/spec.md`** (new file): add the missing
  OpenSpec delta for this typed-state, analyst-sync, and prompt-consumer slice of the `evidence-provenance`
  capability.

## Capabilities

### Architected Capability Slice

- `evidence-provenance`: This change delivers the typed evidence/provenance/coverage state, the `AnalystSyncTask`
  derivation rules, and the downstream prompt-consumer slice of the architected cross-cutting capability described in
  `docs/architect-plan.md`. It does **not** create separate top-level capability IDs such as `evidence-state-model`,
  `data-coverage-report`, `provenance-summary`, or `evidence-prompt-context`.

### Explicitly Deferred To Later Chunks

- Documentation tightening and shared static evidence-discipline rule helpers remain in `chunk1-docs-prompt-rules`.
- `DataEnrichmentConfig`, entity resolution, enrichment adapter contracts, and `PreflightTask` remain in
  `chunk2-config-entity-preflight`.
- Human-readable final report sections remain in `chunk4-report-verification`.

## Cross-Owner Changes

This slice requires explicit cross-owner acknowledgement under
[`docs/architect-plan.md#conflict-analysis`](../../../docs/architect-plan.md#conflict-analysis) and
[`docs/architect-plan.md#module-ownership-map`](../../../docs/architect-plan.md#module-ownership-map).

- [`src/state/trading_state.rs`](../../../src/state/trading_state.rs) — owner: `add-project-foundation`. Chunk 3 adds
  the new `evidence_*`, `data_coverage`, and `provenance_summary` fields to the shared core state.
- [`src/state/mod.rs`](../../../src/state/mod.rs) — owner: `add-project-foundation`. Chunk 3 re-exports the new
  evidence/provenance/reporting modules for downstream consumers.
- [`src/agents/shared/prompt.rs`](../../../src/agents/shared/prompt.rs) — owner: `add-project-foundation`. This is
  the architected home for shared state-dependent prompt-context builders.
- [`src/workflow/context_bridge.rs`](../../../src/workflow/context_bridge.rs) — owner: `add-graph-orchestration`.
  Context round-trip coverage must expand to the new state fields.
- [`src/workflow/snapshot.rs`](../../../src/workflow/snapshot.rs) — owner: `add-graph-orchestration`. Snapshot
  persistence must prove the new fields survive save/load round-trips.
- [`src/workflow/tasks/analyst.rs`](../../../src/workflow/tasks/analyst.rs) — owner: `add-graph-orchestration`.
  `AnalystSyncTask` is the architected aggregation point for dual-write plus coverage/provenance derivation.
- [`src/workflow/tasks/tests.rs`](../../../src/workflow/tasks/tests.rs) — owner: `add-graph-orchestration`. Existing
  `AnalystSyncTask` regressions must expand to assert the new evidence/provenance behavior.
- [`src/agents/researcher/common.rs`](../../../src/agents/researcher/common.rs) — owner: `add-researcher-debate`.
  Researcher prompt construction already centralizes analyst context here, so this is the smallest correct injection
  point.
- [`src/agents/risk/common.rs`](../../../src/agents/risk/common.rs) — owner: `add-risk-management`. Risk prompt
  construction already centralizes analyst context here, so this is the smallest correct injection point.
- [`src/agents/trader/mod.rs`](../../../src/agents/trader/mod.rs) and
  [`src/agents/trader/tests.rs`](../../../src/agents/trader/tests.rs) — owner: `add-trader-agent`. Trader prompt
  construction and its regression coverage must surface the new typed evidence/data-quality context alongside the legacy
  analyst snapshot.
- [`src/agents/fund_manager/prompt.rs`](../../../src/agents/fund_manager/prompt.rs) and
  [`src/agents/fund_manager/tests.rs`](../../../src/agents/fund_manager/tests.rs) — owner: `add-fund-manager`.
  Fund-manager prompt assembly and its regression coverage must surface the new typed evidence/data-quality context at
  the existing user-prompt boundary.

No cross-owner modifications to `src/config.rs`, `config.toml`, `src/data/*`, `src/report/*`, or `src/providers/*`
are required in this chunk. If implementation needs those files, the work has drifted back into Chunks 1, 2, or 4 and
the proposal must be re-scoped.

## Impact

- **State schema**: Six new `Option<>` fields are added to `TradingState`. All default to `None`. No existing field is
  renamed or removed.
- **Serialization**: `TradingState` remains backward-compatible for context/snapshot serialization because the new
  fields are optional and deserialize to `None` when absent.
- **Prompt consumers**: Downstream agents continue to receive the legacy analyst snapshot during the Stage 1 dual-write
  window, but now also receive typed evidence/data-quality context sourced from the new fields.
- **Tests**: Unit tests for the new state types; context bridge and snapshot round-trip tests; `AnalystSyncTask`
  dual-write/coverage/provenance tests; prompt-context string-contains tests at the real downstream injection points.
- **Rollback**: Remove the three new state files and revert the additive state/workflow/prompt-consumer edits. No
  database migration is required because the new fields are nullable and ignored by older snapshot readers.

## Alternatives Considered

### Option: Add provenance fields directly onto each analyst payload type

Extend `FundamentalData`, `TechnicalData`, `SentimentData`, and `NewsData` directly with provenance fields instead of
introducing a generic `EvidenceRecord<T>` wrapper.

Pros: No generic wrapper. Each analyst payload is self-describing.

Cons: Duplicates provenance fields across multiple structs, gives future evidence categories no common envelope, and
scatters shared provenance logic across many types.

Why rejected: The PRD and architect plan explicitly model `EvidenceRecord<T>` as the generic evidence wrapper. The
shared envelope is the smaller long-term surface area.

### Option: Compute coverage and provenance inside each analyst task instead of `AnalystSyncTask`

Have each analyst task populate provenance metadata and write partial coverage state before the sync task runs.

Pros: Provenance is recorded closer to the fetch point.

Cons: Analyst tasks run concurrently, so they would need additional synchronization to aggregate shared coverage state.
It also blurs the graph-orchestration ownership boundary that already assigns fan-out aggregation to `AnalystSyncTask`.

Why rejected: `AnalystSyncTask` is the natural aggregation point after all child results have been read from context.
That keeps the state transition deterministic and centralized.

### Option: Pre-populate `ProvenanceSummary.providers_used` with all Stage 1 providers

Always write `["finnhub", "fred", "yfinance"]` to `providers_used` regardless of which evidence records are present.

Pros: Very simple implementation and stable test output.

Cons: Misstates which providers actually contributed evidence in the current run, conflicts with the PRD's
"provenance of all evidence used in the current run" wording, and weakens Chunk 4's human-readable provenance section.

Why rejected: `providers_used` must summarize the providers that actually contributed evidence on the continue path.

### Option: Defer state-dependent prompt context builders to Chunk 4

Add the typed state in Chunk 3 but postpone `build_evidence_context` / `build_data_quality_context` and downstream
prompt consumption until the report-rendering chunk.

Pros: Smaller Chunk 3 scope.

Cons: Leaves downstream agents unaware of evidence quality even though the typed metadata already exists, and makes the
prompt-builder behavior harder to verify before Chunk 4 starts depending on the same state.

Why rejected: The prompt-consumer slice belongs with the typed state that powers it.
