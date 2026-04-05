## Context

The `src/report/` module currently contains two files: `mod.rs` (which only re-exports `format_final_report`) and `final_report.rs` (which implements all section writers as free functions). `final_report.rs` already owns a shared `section_header` helper, but it is currently private to that file. The section layout of `format_final_report` is:

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

These pre-conditions are not yet present in the checked-out codebase at review time. Chunk 4 therefore remains gated on `chunk3-evidence-state-sync` landing first; Chunk 4 must consume those state types and fields, not redefine them.

### Constraints

- Report rendering must never panic — all `Option` accesses go through pattern matching or `.as_ref()`.
- Heading strings are exact and case-sensitive: `"Data Quality and Coverage"` and `"Evidence Provenance"`.
- Fallback string is exact: `"Unavailable"` (capital U, no additional text on the same line).
- The two new sections are inserted after `write_analyst_snapshot` and before `write_research_debate`.
- The new section modules must reuse the existing report heading style rather than duplicating their own heading formatter.
- No new crate dependencies. The existing `colored` and `comfy_table` crates may be used for formatting.

## Goals / Non-Goals

**Goals:**
- Surface `DataCoverageReport` in a `Data Quality and Coverage` section with required inputs and any missing, stale, or partial inputs.
- Surface `ProvenanceSummary` in an `Evidence Provenance` section with providers used and caveats.
- Render an exact `Unavailable` string for each section when the corresponding `TradingState` field is `None`.
- Keep each new section writer under 60 lines and independently testable.
- Keep CLI-owned report-file edits additive and minimal.

**Non-Goals:**
- `Scenario Valuation` or `Thesis Status` sections — follow-on work, not part of this chunk.
- Changes to `DataCoverageReport` or `ProvenanceSummary` type definitions — those are owned by Chunk 3.
- TUI or GUI rendering — the `src/report/` module is terminal-only (Phase 1).
- Snapshot persistence of report text — the report is rendered in memory and printed; `SnapshotStore` handles phase-level state, not formatted strings.
- Rendering `ProvenanceSummary.generated_at` separately — the final report header already carries the run timestamp, so Stage 1 avoids duplicating that metadata in a second timestamp line.

## Decisions

### 1. Two dedicated sub-modules with shared heading formatting

**Decision**: Each new section writer lives in its own file (`src/report/coverage.rs`, `src/report/provenance.rs`) with a single `pub(crate)` function. The existing `section_header` helper remains in `final_report.rs`, but its visibility is widened to `pub(super)` so the two sibling modules can reuse the same heading rendering without copy-pasting it.

**Rationale**: The existing report module already has a single source of truth for section heading styling. Reusing that helper avoids subtle formatting drift while still keeping the new section renderers small and isolated. The new section writers themselves remain `pub(crate)` so they can be imported directly in tests.

**Alternatives considered**:
- *Inline in `final_report.rs`*: Simpler at first, but the file is already large and already owns many unrelated section writers. Adding two more 40–60 line functions would make it harder to navigate and harder to test in isolation. Rejected in favor of dedicated files.
- *Copy `section_header` into each new file*: Keeps `final_report.rs` untouched, but duplicates formatting logic that should stay identical across sections. Rejected — minimal visibility widening is the smaller long-term surface area.

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

**Rationale**: A one-word fallback is readable, scannable, and unambiguous. Adding explanatory text like `"Coverage data not produced by this run"` adds noise and is harder to test exactly.

**Alternative considered**: Skip the section entirely when the field is `None`. Rejected — the section header must always appear so operators know the section exists and can identify when it was not populated. A missing section is harder to notice than an explicit `Unavailable` label.

### 4. `DataCoverageReport` display: explicit required-input list plus optional issue lists

**Decision**: The `Data Quality and Coverage` section renders the required analyst inputs explicitly (for example `Required inputs: fundamentals, sentiment, news, technical`) followed by labeled bullet lists for any non-empty issue categories. When all issue lists are empty, it renders a single line indicating that all required inputs are present.

**Rationale**: `DataCoverageReport` (from Chunk 3) includes `required_inputs`, `missing_inputs`, `stale_inputs`, and `partial_inputs`. The PRD explicitly requires the report to list required and missing analyst inputs, not just counts. Plain-text labeled lines are the smallest correct implementation and fit the existing report style without introducing extra layout code for a short section. Optional issue lists keep the section future-proof even when Stage 1 usually leaves `stale_inputs` and `partial_inputs` empty.

**Alternative considered**: Render `DataCoverageReport` as a formatted JSON block via `serde_json::to_string_pretty`. Rejected — JSON output is not aligned with the terminal report's visual style (colored headings, comfy_table tables), and it forces operators to parse field names manually.

**Alternative considered**: Render only counts (`Required: N | Missing: M | Stale: S | Partial: P`) without listing the required input names. Rejected because it does not satisfy the PRD's explicit `listing required and missing analyst inputs` requirement.

### 5. `ProvenanceSummary` display: provider list and caveats

**Decision**: The `Evidence Provenance` section renders a list of provider names (from `provenance_summary.providers_used: Vec<String>`) followed by caveats (from `provenance_summary.caveats: Vec<String>`). Providers are rendered as a labeled comma-separated inline list; caveats are rendered as a labeled bulleted list. If either list is empty, the section renders `Providers: none` and/or `Caveats: none` explicitly.

**Rationale**: `ProvenanceSummary` (from Chunk 3) contains `providers_used`, `generated_at`, and `caveats`. Providers are typically a short list and read naturally inline. Caveats are variable-length strings that benefit from bullet formatting for scannability. `generated_at` is intentionally omitted from Stage 1 terminal rendering because the report header already provides a run timestamp and a second timestamp line adds little operator value.

**Alternative considered**: Render providers in a table with one column per provider and a second column for data type. Rejected — overkill for a typically-short provider list (2–4 entries). Inline is sufficient and consistent with how the researcher debate section lists model names.

## Risks / Trade-offs

- **[Chunk 3 dependency]** → This chunk assumes `DataCoverageReport` and `ProvenanceSummary` are defined in `src/state/reporting.rs` and that `TradingState` has `data_coverage` and `provenance_summary` fields. If Chunk 3 is not merged before this chunk is implemented, the code will not compile. Mitigation: implementation of Chunk 4 must be gated on Chunk 3 landing.
- **[Field name drift]** → If Chunk 3 renames fields on `DataCoverageReport` or `ProvenanceSummary`, the section writers will fail to compile. Mitigation: the task descriptions reference field names at the minimum required level; implementers should read `src/state/reporting.rs` before coding.
- **[Smoke test requires API keys]** → The `cargo run` verification step in Task 13 requires valid API keys in `.env` (for example `SCORPIO_OPENAI_API_KEY`, `SCORPIO_FINNHUB_API_KEY`, and `SCORPIO_FRED_API_KEY`, or equivalent configured providers). CI cannot run this step. Mitigation: Task 13 is explicitly documented as a manual step performed by the implementer with a local `.env` file.
- **[Report rendering regression]** → Adding calls to new functions in `format_final_report` could break the report if the new functions panic on unexpected input. Mitigation: both new functions must handle `None` gracefully via pattern matching, and the `format_final_report_handles_missing_analysts_gracefully` test in `final_report.rs` must continue to pass.
- **[Cross-owner approval]** → `src/report/mod.rs` and `src/report/final_report.rs` are CLI-owned files per `docs/architect-plan.md`. Mitigation: the proposal records both files in `## Cross-Owner Changes`, and implementation must wait for approval before touching them.

## Migration Plan

No state migration is required. No config changes. The two new source files are additive; the changes to `final_report.rs` and `mod.rs` are limited to adding two function calls, widening `section_header` visibility to `pub(super)`, and adding two module declarations. Rollback is trivial: revert the two new files and the small report-module edits.
