# `evidence-provenance` Capability

## ADDED Requirements

### Requirement: TradingState Carries Typed Evidence And Run-Level Provenance Metadata

The system MUST define `EvidenceKind`, `EvidenceRecord<T>`, `EvidenceSource`, `DataQualityFlag`,
`DataCoverageReport`, and `ProvenanceSummary` in focused modules under `src/state/`.

`TradingState` MUST add the following optional fields during Stage 1 dual-write:

- `evidence_fundamental`
- `evidence_technical`
- `evidence_sentiment`
- `evidence_news`
- `data_coverage`
- `provenance_summary`

These types and fields MUST serialize, deserialize, and round-trip cleanly through graph-flow context storage and
phase snapshot persistence so the evidence/provenance state is preserved end to end.

Legacy analyst fields remain in place during Stage 1. The typed evidence fields are additive and become the
authoritative source for newly introduced evidence-aware readers.

#### Scenario: TradingState Round-Trips With Evidence Fields Through Context

- **WHEN** a `TradingState` with non-`None` typed evidence, coverage, and provenance fields is serialized into
  graph-flow `Context` and then deserialized
- **THEN** the recovered `TradingState` preserves those new fields without loss

#### Scenario: Snapshot Persistence Preserves Evidence Fields

- **WHEN** a phase snapshot is saved for a `TradingState` that contains non-`None` typed evidence, coverage, and
  provenance fields and is then loaded back from SQLite
- **THEN** the loaded snapshot preserves the same values for those fields

### Requirement: AnalystSyncTask Dual-Writes Evidence And Derives Coverage And Provenance From Present Evidence

`AnalystSyncTask` MUST continue to merge successful analyst fan-out results into the legacy analyst fields.

On the continue path (`0-1` analyst failures), it MUST also:

- populate the corresponding `evidence_*` field for each successful analyst output
- compute `DataCoverageReport` from the presence or absence of the typed `evidence_*` fields using the fixed required
  input order `["fundamentals", "sentiment", "news", "technical"]`
- compute `ProvenanceSummary.providers_used` from the providers attached to evidence records that are actually present
  in the current run

Stage 1 source mappings are fixed:

- fundamentals → `finnhub` / `fundamentals`
- sentiment → `finnhub` / `company_news_sentiment_inputs`
- news → `finnhub` + `fred` / `company_news` + `macro_indicators`
- technical → `yfinance` / `ohlcv`

`providers_used` MUST be sorted ascending and deduplicated. It MUST NOT pre-populate providers for evidence that is
absent.

If `AnalystSyncTask` aborts because `2+` analyst tasks fail, this requirement does not force it to fabricate a partial
coverage or provenance summary before returning the error.

#### Scenario: All Four Analysts Produce Typed Evidence

- **WHEN** all four analyst fan-out results are present and `AnalystSyncTask` completes successfully
- **THEN** all four `evidence_*` fields are populated, `data_coverage.missing_inputs` is empty, and
  `provenance_summary.providers_used` equals `["finnhub", "fred", "yfinance"]`

#### Scenario: One Missing Technical Input Preserves Continue Path Semantics

- **WHEN** the technical analyst result is missing but the other three analyst results are present and
  `AnalystSyncTask` still returns `NextAction::Continue`
- **THEN** `evidence_technical` remains `None`, `data_coverage.missing_inputs` equals `["technical"]`, and
  `provenance_summary.providers_used` equals `["finnhub", "fred"]`

### Requirement: Shared Prompt Context Builders Expose Typed Evidence And Data Quality To Downstream Agents

`src/agents/shared/prompt.rs` MUST provide two state-dependent helper functions:

- `build_evidence_context(state: &TradingState) -> String`
- `build_data_quality_context(state: &TradingState) -> String`

These helpers MUST render prompt-safe summaries of the typed evidence and coverage/provenance state, and they MUST
never panic when some or all of the underlying fields are absent.

The downstream consumer modules MUST include those helpers at their existing dynamic prompt-construction boundaries:

- researcher prompt construction in `src/agents/researcher/common.rs`
- risk prompt construction in `src/agents/risk/common.rs`
- trader prompt construction in `src/agents/trader/mod.rs::build_prompt_context`
- fund-manager prompt construction in `src/agents/fund_manager/prompt.rs::build_prompt_context`

The implementation MUST adapt to the current code shape rather than forcing every agent through a single
system-prompt-only injection pattern.

#### Scenario: Shared Builders Fall Back Safely On Empty State

- **WHEN** `build_evidence_context(...)` and `build_data_quality_context(...)` are called on a `TradingState` where
  the typed evidence, coverage, and provenance fields are all `None`
- **THEN** both helpers return non-empty fallback strings and do not panic

#### Scenario: Researcher And Risk Shared Context Includes Typed Evidence And Data Quality

- **WHEN** the researcher or risk modules build their shared analyst context for downstream prompts
- **THEN** that context includes both the legacy analyst snapshot and the new typed evidence/data-quality sections

#### Scenario: Trader And Fund Manager Prompt Builders Include Typed Evidence And Data Quality

- **WHEN** the trader or fund-manager modules build runtime prompt context from a `TradingState` with typed evidence,
  coverage, and provenance present
- **THEN** their prompt context includes the rendered typed evidence and data-quality sections in addition to the
  existing legacy analyst context
