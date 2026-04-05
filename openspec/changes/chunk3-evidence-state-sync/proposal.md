## Why

Chunks 1 and 2 established evidence discipline at the prompt layer and created the runtime infrastructure
(enrichment config, entity resolution, `PreflightTask`). Chunk 3 introduces the typed state model that makes
evidence discipline concrete and machine-verifiable.

Currently, each analyst writes raw data into a single legacy field on `TradingState`
(`fundamental_metrics`, `technical_indicators`, `market_sentiment`, `macro_news`). There is no record of _where_
the data came from, _when_ it was fetched, _which provider_ produced it, or _what quality flags_ apply. As a
result:

- Downstream agents (researcher, trader, risk, fund manager) receive analyst output with no provenance metadata.
  They cannot distinguish high-confidence complete data from partial or stale data.
- The final report cannot surface data coverage gaps or provider attribution — there is no authoritative record
  of missing inputs.
- Adding new evidence categories (transcripts, consensus estimates, event feeds) in later milestones would require
  ad-hoc state fields with no consistent schema.

Chunk 3 closes these gaps by:

1. Adding three new state modules (`evidence.rs`, `provenance.rs`, `reporting.rs`) with typed structs for
   evidence records, provenance sources, data quality flags, coverage reports, and provenance summaries.
2. Extending `TradingState` with six new `Option<>` fields that carry the typed evidence and reporting data
   alongside the existing legacy fields — preserving backward compatibility via a dual-write strategy.
3. Updating `AnalystSyncTask` to populate both legacy fields and the new `evidence_*` fields, compute
   `DataCoverageReport` and `ProvenanceSummary` from fixed Stage 1 source mappings, and write them to state.
4. Injecting `build_evidence_context(state)` and `build_data_quality_context(state)` into the system prompts of
   all four downstream agents (researcher, risk, trader, fund manager) so they reason with awareness of data
   quality and provenance.

Chunk 4 (report sections) then reads `data_coverage` and `provenance_summary` from state to render the two new
report sections.

## What Changes

- **`src/state/provenance.rs`** (new file): `EvidenceSource` and `DataQualityFlag`.
- **`src/state/evidence.rs`** (new file): `EvidenceKind` enum and `EvidenceRecord<T>` generic wrapper struct.
- **`src/state/reporting.rs`** (new file): `DataCoverageReport` and `ProvenanceSummary`.
- **`src/state/mod.rs`**: Export the three new modules.
- **`src/state/trading_state.rs`**: Add six new `Option<>` fields — `evidence_fundamental`,
  `evidence_technical`, `evidence_sentiment`, `evidence_news`, `data_coverage`, `provenance_summary` — all
  initialized to `None` in `TradingState::new`. Legacy fields remain untouched.
- **`src/workflow/context_bridge.rs`**: Add round-trip test covering the new `TradingState` fields.
- **`src/workflow/snapshot.rs`**: Add round-trip test saving and loading `TradingState` with the new fields.
- **`src/workflow/tasks/analyst.rs`**: Add source-mapping helpers; update `AnalystSyncTask` to dual-write
  legacy and `evidence_*` fields and to compute `DataCoverageReport` / `ProvenanceSummary`.
- **`src/workflow/tasks/tests.rs`**: Extend `analyst_sync_all_succeed_returns_continue` and add
  `analyst_sync_marks_missing_inputs_in_coverage_report`.
- **`src/agents/shared/prompt.rs`**: Add `build_evidence_context(state)` and `build_data_quality_context(state)`
  with no-panic fallback paths.
- **`src/agents/researcher/common.rs`**, **`src/agents/risk/common.rs`**, **`src/agents/trader/mod.rs`**,
  **`src/agents/fund_manager/prompt.rs`**: Inject both context builders after existing analyst-context blocks.

## Capabilities

### New Capabilities

- `evidence-state-model`: Typed `EvidenceRecord<T>`, `EvidenceKind`, `EvidenceSource`, `DataQualityFlag` structs
  in `src/state/`. Every analyst output can now be wrapped with provenance and quality metadata.
- `data-coverage-report`: `DataCoverageReport` computed by `AnalystSyncTask` — tracks required, missing, stale,
  and partial inputs for each run.
- `provenance-summary`: `ProvenanceSummary` computed by `AnalystSyncTask` — tracks providers used, timestamp,
  and caveats for the current run.
- `evidence-prompt-context`: Two shared context builders (`build_evidence_context`, `build_data_quality_context`)
  that render typed evidence and coverage snapshots for injection into downstream agent prompts.

### Modified Capabilities

- `analyst-sync`: `AnalystSyncTask` now dual-writes both legacy analyst fields and typed `evidence_*` fields,
  and computes coverage/provenance reports after each run.
- `downstream-prompts`: Researcher, risk, trader, and fund-manager system prompts now include evidence and
  data-quality context sections, making quality gaps visible to reasoning agents.

## Impact

- **State schema**: Six new `Option<>` fields added to `TradingState`. All default to `None`. No existing field
  is renamed or removed. Code paths that construct `TradingState { ... }` with all fields must be updated to
  include the new `None` initializers — `cargo build` surfaces these sites.
- **Serialization**: `TradingState` serialization (context bridge, snapshot store) is backward-compatible
  because the new fields are `Option<>` and serialize as `null`; existing snapshots without these fields
  deserialize with `None` values.
- **Tests**: Unit tests for each new state type (serde round-trips); context bridge round-trip test; snapshot
  round-trip test; `AnalystSyncTask` dual-write and coverage tests; prompt-rendering string-contains tests.
- **Rollback**: Remove the three new state files; revert `src/state/mod.rs`, `src/state/trading_state.rs`,
  `src/workflow/tasks/analyst.rs`, the four agent prompt files, and `src/agents/shared/prompt.rs`. No database
  migration is required — new fields are nullable and ignored by existing snapshot readers.

## Alternatives Considered

### Option: Add provenance as free-text fields on existing analyst structs instead of a generic wrapper

Extend `FundamentalData`, `TechnicalData`, `SentimentData`, and `NewsData` directly with `provider: String`
and `fetched_at: String` fields rather than introducing a generic `EvidenceRecord<T>` wrapper.

Pros: No generic type in the state model. Simpler serialization. Each analyst struct is self-describing with
no indirection.

Cons: Duplicates provenance fields across four structs. Adding a new evidence category (e.g., `TranscriptData`
from Milestone 7) requires adding the same fields again. The `DataQualityFlag` mechanism would need to be
replicated per type. The unified `EvidenceKind` discriminant used by `AnalystSyncTask` and the coverage report
would have no natural home.

Why rejected: The architecture spec explicitly defines `EvidenceRecord<T>` as the generic evidence wrapper. The
generic approach means provenance and quality-flag logic is written once and works for all current and future
evidence categories. The cost — one generic type — is low.

### Option: Compute coverage and provenance in each analyst task rather than in `AnalystSyncTask`

Have each analyst task (`FundamentalAnalystTask`, etc.) populate its own `EvidenceSource` and write a partial
`DataCoverageReport` to state, then aggregate in `AnalystSyncTask`.

Pros: Provenance is written closest to where data is fetched, so `fetched_at` timestamps are more accurate.
Each analyst task is self-contained.

Cons: The spec explicitly assigns cross-source normalization and coverage/provenance aggregation to
`AnalystSyncTask`, not to individual analyst tasks. Individual analyst tasks run concurrently and would need
synchronization to aggregate into a shared coverage report. Partial coverage writes during fan-out would leave
`TradingState` in an inconsistent intermediate state visible to any concurrent reader.

Why rejected: The spec's analyst-vs-sync ownership boundary is clear and correct. `AnalystSyncTask` runs after
all analyst tasks complete — it is the natural aggregation point. The slightly less precise `fetched_at`
(recorded at sync time rather than fetch time) is an acceptable Stage 1 trade-off.

### Option: Defer downstream prompt injection to Chunk 4

Add `build_evidence_context` and `build_data_quality_context` to `src/agents/shared/prompt.rs` in Chunk 3 but
wire them into agent prompts in Chunk 4 alongside the report sections.

Pros: Smaller per-chunk scope. Chunk 4 becomes a unified "consumer" chunk for all new state.

Cons: The report sections (Chunk 4) read from `TradingState.data_coverage` and `TradingState.provenance_summary`
— the same fields populated in Chunk 3. Agents reasoning about evidence quality is independent of how the
report renders it. Deferring prompt injection would mean running Chunks 3 and 4 sequentially with no
intermediate verification that the prompt builders work correctly on real state.

Why rejected: Prompt injection belongs with the state model change that makes the data available. Testing both
together in Chunk 3 means the evidence context helpers are verified before Chunk 4 depends on them.
