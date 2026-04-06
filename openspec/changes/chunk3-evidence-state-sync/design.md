## Context

Chunk 2 delivered `ResolvedInstrument` in workflow context and a `PreflightTask` that runs before the analyst fan-out.
The five-phase pipeline is otherwise unchanged. `TradingState` still carries only legacy analyst fields; there is no
typed evidence wrapper, no provenance record, and no run-level coverage report in state.

The current codebase also has three prompt-construction boundaries that matter for this chunk:

- `src/agents/shared/prompt.rs` currently provides sanitization and serialization helpers only; it has no
  state-dependent evidence/data-quality builders yet.
- Researcher and risk agents already centralize analyst context in `src/agents/researcher/common.rs` and
  `src/agents/risk/common.rs`. Persona agents and moderators reuse those helpers.
- Trader and fund-manager prompts are built through existing `build_prompt_context(...)` helpers. The trader places
  runtime context in the system prompt; the fund manager intentionally keeps serialized runtime context in the user
  prompt while preserving a mostly static system prompt.

Existing code that must not break:

- `src/state/trading_state.rs` — legacy analyst fields (`fundamental_metrics`, `technical_indicators`,
  `market_sentiment`, `macro_news`) stay additive-only; they are not renamed or removed.
- `src/workflow/tasks/analyst.rs` — `AnalystSyncTask` retains its graph-orchestration degradation policy: `0-1`
  failures continue, `2+` failures abort.
- `src/agents/shared/prompt.rs` — current sanitization/redaction helpers remain the common foundation for any new
  context builders.
- `src/agents/researcher/common.rs`, `src/agents/risk/common.rs`, `src/agents/trader/mod.rs`, and
  `src/agents/fund_manager/prompt.rs` — new evidence/data-quality context is additive at each module's existing dynamic
  prompt boundary; no existing legacy analyst snapshot is removed in Stage 1.

## Constraints

- No new crate dependencies.
- All new state types derive `Serialize`, `Deserialize`, and `JsonSchema`.
- `EvidenceRecord<T>.quality_flags` is always initialized to `[]` in Stage 1; quality flags inside evidence records are
  reserved for later milestones.
- `DataQualityFlag::Conflicted` must not be emitted in Stage 1.
- Coverage authority rule: `data_coverage` is derived from the new `evidence_*` fields, not from the legacy mirrors.
- `required_inputs` keeps the fixed order `["fundamentals", "sentiment", "news", "technical"]`, and the issue lists
  preserve that order when they contain a subset of those inputs.
- `ProvenanceSummary.providers_used` is derived from the providers attached to evidence records that are actually
  present on the continue path. It must be sorted ascending and deduplicated; absent evidence must not contribute
  placeholder providers.
- Prompt construction must never panic. New prompt-context builders must use the existing prompt-safe
  serialization/sanitization posture.
- Chunk 3 must not add human-readable report sections; those remain in Chunk 4.

## Goals / Non-Goals

**Goals:**

- Define `EvidenceKind`, `EvidenceRecord<T>`, `EvidenceSource`, `DataQualityFlag`, `DataCoverageReport`, and
  `ProvenanceSummary` with serde/schemars derives in three focused state modules.
- Extend `TradingState` with six additive `Option<>` fields for typed evidence and run-level reporting.
- Update `AnalystSyncTask` to dual-write legacy and `evidence_*` fields and to derive `DataCoverageReport` /
  `ProvenanceSummary` on the continue path.
- Add context bridge and snapshot round-trip coverage for the new fields.
- Add `build_evidence_context(state)` and `build_data_quality_context(state)` to `src/agents/shared/prompt.rs`.
- Inject those builders into researcher, risk, trader, and fund-manager prompt construction at the real code boundaries
  already used by each module.

**Non-Goals:**

- Emitting `DataQualityFlag` variants inside `EvidenceRecord<T>.quality_flags`.
- Fetching live provenance metadata (`effective_at`, `url`, `citation`) from adapters.
- Removing legacy analyst fields or finishing the consumer migration away from them.
- Human-readable report rendering of coverage/provenance data.
- Thesis-memory or scenario-valuation work.

## Decisions

### 1. Three focused state modules rather than one large provenance file

**Decision**: Split the new types across three files:

- `src/state/provenance.rs` — `EvidenceSource`, `DataQualityFlag`
- `src/state/evidence.rs` — `EvidenceKind`, `EvidenceRecord<T>`
- `src/state/reporting.rs` — `DataCoverageReport`, `ProvenanceSummary`

**Rationale**: These represent three distinct concern layers: provenance primitives, generic evidence wrapping, and
run-level reporting. Keeping them separate makes each file smaller and easier to test.

### 2. `EvidenceRecord<T>` remains the generic typed evidence envelope

**Decision**:

```rust
pub enum EvidenceKind {
    Fundamental,
    Technical,
    Sentiment,
    News,
    Macro,
    Transcript,
    Estimates,
    Peers,
    Volatility,
}

pub struct EvidenceRecord<T> {
    pub kind: EvidenceKind,
    pub payload: T,
    pub sources: Vec<EvidenceSource>,
    pub quality_flags: Vec<DataQualityFlag>,
}
```

**Rationale**: The generic wrapper lets the system carry typed analyst payloads without duplicating the provenance and
quality envelope on every data struct.

**Implementation note**: The docs intentionally do not over-prescribe one exact generic-bound syntax for serde/schemars
derive macros. The contract is behavioral: `EvidenceRecord<FundamentalData>`, `EvidenceRecord<TechnicalData>`,
`EvidenceRecord<SentimentData>`, `EvidenceRecord<NewsData>`, and `EvidenceRecord<serde_json::Value>` must all compile,
serialize, deserialize, and derive schema cleanly.

### 3. Stage 1 uses dual-write: legacy mirrors stay, typed evidence becomes authoritative for new readers

**Decision**: `AnalystSyncTask` writes both the existing legacy analyst fields and the new typed `evidence_*` fields.

Legacy fields remain for existing readers. New evidence-aware readers introduced by this chunk and Chunk 4 consume the
typed evidence fields and run-level reports as the authoritative source.

**Rationale**: Dual-write avoids a big-bang migration. It lets the project adopt typed evidence incrementally.

### 4. `AnalystSyncTask` owns coverage and provenance derivation on the continue path

**Decision**: `AnalystSyncTask` builds `EvidenceSource` values using the fixed Stage 1 mappings:

| Coverage ID    | Provider(s)                | Dataset(s)                      |
|----------------|----------------------------|----------------------------------|
| `fundamentals` | `finnhub`                  | `fundamentals`                   |
| `sentiment`    | `finnhub`                  | `company_news_sentiment_inputs`  |
| `news`         | `finnhub` + `fred`         | `company_news` + `macro_indicators` |
| `technical`    | `yfinance`                 | `ohlcv`                          |

`effective_at`, `url`, and `citation` stay `None` in Stage 1. `fetched_at` is recorded at sync time. `quality_flags`
on each `EvidenceRecord` stay empty.

`DataCoverageReport` is derived from the new `evidence_*` fields only.

`ProvenanceSummary.providers_used` is built from the providers attached to evidence records that are actually present
after merge, not from a pre-populated "all configured providers" list.

**Rationale**: This keeps the aggregation deterministic and aligned with the graph-orchestration ownership boundary.

### 5. New shared prompt-context builders must reuse the existing prompt-safety posture

**Decision**:

```rust
pub(crate) fn build_evidence_context(state: &TradingState) -> String { /* never panics */ }
pub(crate) fn build_data_quality_context(state: &TradingState) -> String { /* never panics */ }
```

These helpers render compact prompt-safe summaries of typed evidence and run-level data quality/provenance. They reuse
the existing shared prompt-safe serialization/sanitization posture already present in `src/agents/shared/prompt.rs`.

**Rationale**: The new blocks are untrusted runtime context. They should inherit the same sanitization and redaction
rules as existing prompt context.

### 6. Injection happens at each module's real dynamic prompt boundary

**Decision**:

- `src/agents/researcher/common.rs` extends the shared analyst-context helper already consumed by bullish, bearish, and
  moderator prompt paths.
- `src/agents/risk/common.rs` extends the shared analyst-context helper already consumed by persona agents and the risk
  moderator.
- `src/agents/trader/mod.rs` appends the new blocks inside the existing `build_prompt_context(...)` flow.
- `src/agents/fund_manager/prompt.rs` appends the new blocks inside the existing
  `build_prompt_context(...)` / `build_user_prompt(...)` flow while preserving the current separation between static
  system instructions and serialized runtime context.

**Rationale**: The downstream modules do not all build prompts the same way. The smallest correct change is to inject at
the boundaries each module already owns.

## Risks / Trade-offs

- **[Exhaustive struct literal sites]** Adding six fields to `TradingState` will require updating many test fixtures.
  `cargo build` will surface them, and the list in the tasks doc is only a starting point.
- **[Serde/schemars generic derive quirks]** `EvidenceRecord<T>` may need explicit derive-bound annotations depending
  on how the macros expand. The docs deliberately focus on the behavioral contract rather than one exact syntax.
- **[Degradation-policy mismatch in tests]** `AnalystSyncTask` aborts on `2+` failures. Coverage/provenance regression
  tests must target the `0-1` failure continue path instead of expecting reports after an abort.
- **[Prompt length increase]** Downstream prompts grow modestly. The new blocks should stay compact.
- **[Cross-owner approval]** This chunk touches foundation-, orchestration-, and agent-owned files. Implementation must
  wait for approval.

## Migration Plan

1. Add the three new state files and update `src/state/mod.rs`.
2. Extend `TradingState` with the six additive fields and update all struct literal sites flagged by `cargo build`.
3. Update `AnalystSyncTask` dual-write logic and derive coverage/provenance on the `0-1` failure continue path.
4. Add the shared prompt-context builders.
5. Update the four downstream consumer boundaries (`researcher/common.rs`, `risk/common.rs`, `trader/mod.rs`,
   `fund_manager/prompt.rs`) without removing the legacy analyst snapshot.
6. Verify with `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`,
   `cargo nextest run --all-features --locked`, `cargo build`, and
   `openspec validate chunk3-evidence-state-sync --strict`.

## Open Questions

None for this chunk. Future work such as richer quality flag emission, adapter-sourced provenance metadata, and human
report rendering is explicitly deferred.
