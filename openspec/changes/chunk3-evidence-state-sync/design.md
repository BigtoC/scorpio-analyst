## Context

Chunk 2 delivered `ResolvedInstrument` in workflow context and a `PreflightTask` that runs before the analyst
fan-out. The five-phase pipeline is otherwise unchanged. `TradingState` still carries only legacy analyst
fields; there is no typed evidence wrapper, no provenance record, and no coverage report in state.

Existing code that must not break:

- `src/state/trading_state.rs` — `TradingState` legacy fields (`fundamental_metrics`, `technical_indicators`,
  `market_sentiment`, `macro_news`) must not be renamed or removed. All new fields are additive.
- `src/workflow/tasks/analyst.rs` — `AnalystSyncTask` existing logic for populating legacy fields and
  returning `NextAction::Continue` is preserved. Dual-write is additive.
- `src/agents/shared/prompt.rs` — existing helper functions (`build_analyst_context`, etc.) are not changed.
  Two new functions are added.
- Agent prompt files (`researcher/common.rs`, `risk/common.rs`, `trader/mod.rs`, `fund_manager/prompt.rs`) —
  the injection is append-only; no existing prompt text is removed.

Constraints:

- No new crate dependencies. `schemars` is already a dependency (used by existing `#[tool]` structs).
- All new state types derive `Serialize`, `Deserialize`, and `JsonSchema` — required for context-bridge
  round-trips and schema-enforcement downstream.
- `EvidenceRecord<T>.quality_flags` is always initialized to `[]` in Stage 1. Quality flags within evidence
  records are reserved for later milestones.
- `DataQualityFlag::Conflicted` must not be emitted in Stage 1.
- Coverage authority rule: `data_coverage.missing_inputs` is derived from the _new_ `evidence_*` fields,
  not from the legacy fields. If `evidence_fundamental` is `None`, `"fundamentals"` is in `missing_inputs`.
- `providers_used` in `ProvenanceSummary` must be sorted ascending and deduplicated for stable test assertions.
- `required_inputs` and `missing_inputs` in `DataCoverageReport` keep the fixed order:
  `["fundamentals", "sentiment", "news", "technical"]`.
- Report rendering must never panic — both new prompt context builders must return fallback strings when the
  relevant state fields are `None`.

## Goals / Non-Goals

**Goals:**

- Define `EvidenceKind`, `EvidenceRecord<T>`, `EvidenceSource`, `DataQualityFlag`, `DataCoverageReport`,
  and `ProvenanceSummary` with serde and `JsonSchema` derives in three focused state modules.
- Extend `TradingState` with six `Option<>` fields for typed evidence and reporting; initialize all to `None`.
- Update `AnalystSyncTask` to dual-write legacy and `evidence_*` fields; compute `DataCoverageReport` and
  `ProvenanceSummary` with exact Stage 1 source mappings.
- Add context bridge and snapshot round-trip tests for the new state fields.
- Add `build_evidence_context(state)` and `build_data_quality_context(state)` to `src/agents/shared/prompt.rs`
  with no-panic fallback paths.
- Inject both context builders into researcher, risk, trader, and fund-manager prompts.

**Non-Goals:**

- Emitting `DataQualityFlag` variants inside `EvidenceRecord<T>.quality_flags` — deferred to later milestones.
- Fetching live provenance metadata (`effective_at`, `url`, `citation`) from data adapters — `None` in Stage 1
  unless the adapter already provides the value without extra work.
- Adding `thesis.rs` or `derived.rs` state files — follow-on milestones.
- Changing legacy analyst fields or removing the dual-write compatibility layer.
- Adding `build_thesis_memory_context` — deferred to the thesis-memory milestone.

## Decisions

### 1. Three focused state modules rather than one large `evidence.rs`

**Decision**: Split the new state types across three files:

- `src/state/provenance.rs` — `EvidenceSource`, `DataQualityFlag` (source attribution + quality primitives)
- `src/state/evidence.rs` — `EvidenceKind`, `EvidenceRecord<T>` (the generic evidence wrapper)
- `src/state/reporting.rs` — `DataCoverageReport`, `ProvenanceSummary` (aggregated run-level reporting)

**Rationale**: The three files represent three distinct concern layers: what a single piece of evidence came
from (`provenance`), how evidence is typed and wrapped (`evidence`), and what the aggregated run-level picture
looks like (`reporting`). Separating them makes each file small and independently testable. The dependency
direction is natural: `evidence.rs` imports from `provenance.rs`; `reporting.rs` imports from neither (it is
purely reporting-level).

**Alternatives considered**:

- *Single `evidence.rs` with all six types*: Simpler module tree but creates a large file mixing provenance
  primitives, generic wrappers, and run-level reports. Harder to navigate and test independently. Rejected.

### 2. `EvidenceRecord<T>` is a generic wrapper; `EvidenceKind` is a discriminant enum

**Decision**:

```rust
pub enum EvidenceKind {
    Fundamental, Technical, Sentiment, News, Macro,
    Transcript, Estimates, Peers, Volatility,
}

pub struct EvidenceRecord<T> {
    pub kind: EvidenceKind,
    pub payload: T,
    pub sources: Vec<EvidenceSource>,
    pub quality_flags: Vec<DataQualityFlag>,
}
```

`EvidenceRecord<T>` is the single envelope for all evidence categories. `EvidenceKind` allows
`AnalystSyncTask` and report code to identify the category without downcasting.

**Rationale**: The generic `T` payload allows each analyst type (`FundamentalData`, `TechnicalData`, etc.) to
be wrapped without losing its type. `sources` and `quality_flags` are shared across all categories.
`EvidenceKind` as a separate enum makes pattern-matching and coverage-id mapping straightforward. The
`JsonSchema` derive is required because `EvidenceRecord<T>` may be used in `#[tool]` contexts downstream.

### 3. Dual-write strategy: legacy fields remain authoritative mirrors during Stage 1

**Decision**: `AnalystSyncTask` writes both:

1. The existing legacy field (e.g., `state.fundamental_metrics = Some(data.clone())`).
2. The new typed field (e.g., `state.evidence_fundamental = Some(EvidenceRecord { ... })`).

Legacy fields remain the compatibility source for any code paths not yet updated to read `evidence_*` fields.
New typed evidence fields are authoritative for newly added readers (Chunk 4 report, prompt context builders).
If the two disagree, that is a bug — new typed evidence is authoritative for new readers.

**Rationale**: Dual-write avoids a big-bang migration where every reader must be updated in the same PR.
Stage 1 introduces the new fields and proves the data flows through them; later milestones can drop the legacy
fields once all consumers migrate.

### 4. `AnalystSyncTask` owns all provenance construction; source mappings are fixed constants

**Decision**: `AnalystSyncTask` builds `EvidenceSource` values using these exact fixed Stage 1 mappings:

| Coverage ID    | Provider(s)                                        | Dataset(s)                                   |
|----------------|----------------------------------------------------|----------------------------------------------|
| `fundamentals` | `finnhub`                                          | `fundamentals`                               |
| `sentiment`    | `finnhub`                                          | `company_news_sentiment_inputs`              |
| `news`         | `finnhub` + `fred`                                 | `company_news` + `macro_indicators`          |
| `technical`    | `yfinance`                                         | `ohlcv`                                      |

`effective_at`, `url`, and `citation` are `None` in Stage 1. `fetched_at` is the current UTC RFC3339
timestamp at the time `AnalystSyncTask` runs. `quality_flags` on each `EvidenceRecord` is `[]`.

**Rationale**: Hardcoding the mappings in Stage 1 avoids requiring every data adapter to instrument itself
with provenance metadata in this PR. The spec explicitly lists these mappings as the fixed Stage 1 contract.
When concrete enrichment providers are added in Milestone 7, each adapter can pass its own `EvidenceSource`
and the mapping constants become the fallback for the existing four analysts.

### 5. `build_evidence_context` and `build_data_quality_context` have no-panic fallback paths

**Decision**:

```rust
pub(crate) fn build_evidence_context(state: &TradingState) -> String {
    // Returns a compact block; if evidence_* fields are None, renders "null" for each.
    // Never panics.
}

pub(crate) fn build_data_quality_context(state: &TradingState) -> String {
    // Returns a compact block; if data_coverage / provenance_summary are None, renders fallback text.
    // Never panics.
}
```

Both builders render explicit compact fallback text when the relevant state fields are absent, rather than
panicking or returning an empty string.

**Rationale**: These builders are called during prompt construction, which runs inside async tasks. A panic
during prompt construction would crash the entire pipeline run. An empty string would silently omit the context
section, making it harder to diagnose missing data. Explicit fallback text (`"(evidence not yet available)"`,
`"(data quality snapshot not yet available)"`) makes the absence visible to the LLM and to logs.

### 6. Injection is append-only in downstream agent prompts

**Decision**: In each of the four downstream agent prompt files, append the evidence and data-quality context
after the existing analyst-context block. Do not modify or reorganize existing prompt text.

**Rationale**: Append-only injection minimizes diff noise and reduces regression risk. The evidence/quality
context is supplementary — it provides additional signal after the agent has already received the core
analyst output. Placing it at the end avoids disrupting existing prompt token counts or LLM attention patterns
that the current prompts were tuned for.

## Risks / Trade-offs

- **[Exhaustive struct literal sites]** Adding six fields to `TradingState` requires updating every
  `TradingState { ... }` literal in the codebase (tests, context bridge, agent test stubs). `cargo build` will
  surface all of them; there may be more than the five files listed in the plan. Mitigation: follow the
  compilation errors systematically; all new fields are `None` initializers.
- **[`JsonSchema` derive on generic type]** `EvidenceRecord<T>` requires `T: JsonSchema`. The four analyst
  data types (`FundamentalData`, `TechnicalData`, `SentimentData`, `NewsData`) must either already derive
  `JsonSchema` or have it added. Mitigation: check each type before writing `EvidenceRecord<T>` fields to
  `TradingState`; add `#[derive(JsonSchema)]` where missing.
- **[Prompt length increase]** Each downstream agent prompt grows by the size of the evidence and data-quality
  context blocks (typically 5–15 lines). For deep-thinking models this is negligible; for quick-thinking models
  with tight context budgets it may push token counts. Mitigation: keep the rendered blocks compact (no verbose
  JSON formatting); use `serde_json::to_string` (not `to_string_pretty`) for evidence payloads.
- **[`fetched_at` approximation]** `fetched_at` is recorded at sync time, not at the moment each adapter
  returned data. For Stage 1 this is acceptable; the spec notes that `effective_at`, `url`, and `citation` are
  `None` unless an adapter already provides them without extra work.

## Migration Plan

1. Add the three new state files and update `src/state/mod.rs` — no existing code breaks yet.
2. Extend `TradingState` with the six new `None` fields — `cargo build` reveals all literal initializer sites.
   Update each one.
3. Update `AnalystSyncTask` dual-write logic — all new fields are `Option<>`, so incomplete data (fewer than
   four analyst outputs) simply leaves some `evidence_*` fields as `None`, which is correct.
4. Update agent prompt files with append-only injection — no existing prompt text is removed.
5. Run `cargo fmt`, `cargo clippy`, `cargo test` to verify.

No database migration required. New `TradingState` fields are nullable — existing SQLite snapshots
deserialize correctly with `None` for the new fields.

## Open Questions

None. The spec defines the exact types, source mappings, quality-flag deferral, and prompt rendering contracts
for Stage 1. Milestone-specific decisions (live `fetched_at` from adapters, quality flag emission, thesis
memory context) are explicitly deferred.
