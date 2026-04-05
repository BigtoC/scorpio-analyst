## Context

The `src/report/` module currently contains two files: `mod.rs` (which exports `format_final_report`) and `final_report.rs` (which implements all section writers as private free functions). The section layout of `format_final_report` is:

1. `write_header`
2. `write_executive_summary`
3. `write_trader_proposal`
4. `write_analyst_snapshot` — renders the "Analyst Evidence Snapshot" section
5. `write_research_debate`
6. `write_risk_review`
7. `write_safety_check`
8. `write_token_usage`
9. `write_disclaimer`

`TradingState` (in `src/state/trading_state.rs`) holds all inter-agent data. Chunk 3 added two new optional fields to `TradingState`:

- `data_coverage: Option<DataCoverageReport>` — produced by the analyst phase, records which required data inputs were present and which were missing.
- `provenance_summary: Option<ProvenanceSummary>` — produced by the analyst phase, records which external providers contributed data and any associated caveats.

Both types live in `src/state/reporting.rs` (introduced in Chunk 3). Both are `Option<T>` because upstream phases may not produce them (e.g., during a partial pipeline run or a test harness that only populates a subset of state fields).

### Pre-conditions (Chunk 3 must be complete)

- `src/state/reporting.rs` exists and defines `DataCoverageReport` and `ProvenanceSummary`.
- `TradingState` has `pub data_coverage: Option<DataCoverageReport>` and `pub provenance_summary: Option<ProvenanceSummary>`.
- `TradingState::new()` initializes both fields to `None`.
- The `state` module re-exports `DataCoverageReport` and `ProvenanceSummary` from `src/state/mod.rs`.

### Constraints

- Report rendering must never panic — all `Option` accesses go through pattern matching or `.as_ref()`.
- Heading strings are exact and case-sensitive: `"Data Quality and Coverage"` and `"Evidence Provenance"`.
- Fallback string is exact: `"Unavailable"` (capital U, no additional text on the same line).
- The two new sections are inserted after `write_analyst_snapshot` and before `write_research_debate`.
- No new crate dependencies. The existing `colored` and `comfy_table` crates may be used for formatting.

## Goals / Non-Goals

**Goals:**
- Surface `DataCoverageReport` in a `Data Quality and Coverage` section with required inputs and missing inputs.
- Surface `ProvenanceSummary` in an `Evidence Provenance` section with providers used and caveats.
- Render an exact `Unavailable` string for each section when the corresponding `TradingState` field is `None`.
- Keep each new section writer under 60 lines and independently testable.
- Export the two new modules cleanly from `src/report/mod.rs`.

**Non-Goals:**
- `Scenario Valuation` or `Thesis Status` sections — follow-on work, not part of this chunk.
- Changes to `DataCoverageReport` or `ProvenanceSummary` type definitions — those are owned by Chunk 3.
- TUI or GUI rendering — the `src/report/` module is terminal-only (Phase 1).
- Snapshot persistence of report text — the report is rendered in memory and printed; `SnapshotStore` handles phase-level state, not formatted strings.

## Decisions

### 1. Two dedicated sub-modules: `coverage.rs` and `provenance.rs`

**Decision**: Each new section writer lives in its own file (`src/report/coverage.rs`, `src/report/provenance.rs`) with a single `pub(crate)` function matching the pattern used by `write_analyst_snapshot` in `final_report.rs`.

**Rationale**: The existing report module uses one-function-per-section inside a single file. As sections accumulate, splitting into dedicated files keeps each unit under 60 lines and independently testable without requiring `pub(super)` visibility gymnastics. The `pub(crate)` visibility on the helper functions allows direct import in tests.

**Alternatives considered**:
- *Inline in `final_report.rs`*: Simpler at first, but the file is already ~660 lines. Adding two more 40–60 line functions would make it difficult to navigate. Rejected in favor of the established module-per-section pattern.
- *Single `sections.rs` file for both*: Avoids two new files, but mixes two distinct concerns (coverage vs. provenance) in one file with no clear split point. Rejected — the two-file approach mirrors how the existing module would evolve naturally.

### 2. Section placement: after `Analyst Evidence Snapshot`, before `Research Debate Summary`

**Decision**: The call order in `format_final_report` becomes:
1. `write_analyst_snapshot`
2. `write_data_quality_and_coverage` (new)
3. `write_evidence_provenance` (new)
4. `write_research_debate`
5. ...remainder unchanged

**Rationale**: Coverage and provenance are downstream metadata about the analyst phase: they describe the quality and origin of the evidence that fed into the analyst summaries. Placing them immediately after the analyst snapshot preserves the narrative flow — reader sees what the analysts found, then sees the quality/provenance context for that evidence, then sees the debate that built on it.

**Alternative considered**: Place after `write_research_debate` or at the end before `write_token_usage`. Rejected because coverage/provenance logically annotates the analyst evidence, not the debate conclusion. Burying them after the debate weakens their interpretive value.

### 3. `Unavailable` fallback with no extra text

**Decision**: When `state.data_coverage` or `state.provenance_summary` is `None`, each section writer emits the section header followed by a single line: `Unavailable`.

**Rationale**: A one-word fallback is readable, scannable, and unambiguous. Adding explanatory text like `"Coverage data not produced by this run"` adds noise and is harder to test exactly. Tests assert `report.contains("Unavailable")` — a bare string match that is stable across wording changes in explanatory text.

**Alternative considered**: Skip the section entirely when the field is `None`. Rejected — the section header must always appear so operators know the section exists and can identify when it was not populated. A missing section is harder to notice than an explicit `Unavailable` label.

### 4. `DataCoverageReport` display: table of required vs. missing inputs

**Decision**: The `Data Quality and Coverage` section renders a two-part display: a summary line showing count of required inputs vs. missing inputs, followed by a bulleted list of the missing input names (if any). Uses `comfy_table` for the summary row and plain `writeln!` for the missing-inputs list.

**Rationale**: `DataCoverageReport` (from Chunk 3) contains at minimum a `required_inputs: Vec<String>` and `missing_inputs: Vec<String>`. A table summary (`Required: N | Missing: M`) followed by specific missing input names gives operators the most actionable view: they see the overall completeness ratio and can identify exactly which data sources failed.

**Alternative considered**: Render `DataCoverageReport` as a formatted JSON block via `serde_json::to_string_pretty`. Rejected — JSON output is not aligned with the terminal report's visual style (colored headings, comfy_table tables), and it forces operators to parse field names manually.

### 5. `ProvenanceSummary` display: provider list and caveats

**Decision**: The `Evidence Provenance` section renders a list of provider names (from `provenance_summary.providers_used: Vec<String>`) followed by caveats (from `provenance_summary.caveats: Vec<String>`). Providers are rendered as a comma-separated inline list; caveats are rendered as a bulleted list, matching the style used for `recommended_adjustments` in `write_risk_review`.

**Rationale**: `ProvenanceSummary` (from Chunk 3) contains at minimum `providers_used: Vec<String>` and `caveats: Vec<String>`. Providers are typically a short list (Finnhub, Yahoo Finance, FRED) and read naturally inline. Caveats are variable-length strings that benefit from bullet formatting for scannability.

**Alternative considered**: Render providers in a table with one column per provider and a second column for data type. Rejected — overkill for a typically-short provider list (2–4 entries). Inline is sufficient and consistent with how the researcher debate section lists model names.

## Risks / Trade-offs

- **[Chunk 3 dependency]** → This chunk assumes `DataCoverageReport` and `ProvenanceSummary` are defined in `src/state/reporting.rs` and that `TradingState` has `data_coverage` and `provenance_summary` fields. If Chunk 3 is not merged before this chunk is implemented, the code will not compile. Mitigation: implementation of Chunk 4 must be gated on Chunk 3 landing.
- **[Field name drift]** → If Chunk 3 renames fields on `DataCoverageReport` or `ProvenanceSummary`, the section writers will fail to compile. Mitigation: the task descriptions reference field names at the minimum required level; implementers should read `src/state/reporting.rs` before coding.
- **[Smoke test requires API keys]** → The `cargo run` verification step in Task 13 requires valid API keys in `.env`. CI cannot run this step. Mitigation: Task 13 is explicitly documented as a manual step performed by the implementer with a local `.env` file.
- **[Report rendering regression]** → Adding calls to new functions in `format_final_report` could break the report if the new functions panic on unexpected input. Mitigation: both new functions must handle `None` gracefully via pattern matching, and the `format_final_report_handles_missing_analysts_gracefully` test in `final_report.rs` must continue to pass.

## Migration Plan

No state migration is required. No config changes. The two new source files are additive; the changes to `final_report.rs` and `mod.rs` are limited to adding two function calls and two module declarations respectively. Rollback is trivial: revert the two new files and the two call sites.
