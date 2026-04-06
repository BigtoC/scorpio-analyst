## Why

The evidence discipline and provenance tracking introduced in Chunks 1–3 collects rich metadata throughout the pipeline — data coverage assessments in `DataCoverageReport` and provenance summaries in `ProvenanceSummary` — but that metadata is never surfaced to the human reader. The PRD's CLI output contract explicitly calls for `Data Quality and Coverage` and `Evidence Provenance` sections after the analyst snapshot. Without them, operators cannot audit which analyst inputs were missing or degraded, which providers contributed evidence, or what caveats applied to a recommendation.

Chunk 4 is the terminal delivery slice for the Stage 1 `evidence-provenance` capability. It adds those two terminal-rendered sections to `format_final_report` and closes the Stage 1 verification loop (`cargo fmt`, `cargo clippy`, `cargo nextest`, `cargo run`, and strict `openspec validate`). The report must degrade gracefully: if upstream phases leave `data_coverage` or `provenance_summary` unset, the sections still appear and render `Unavailable` rather than disappearing or panicking.

The architect plan models this work as one `evidence-provenance` capability owned by a single broader change. The repository is currently executing that capability in reviewable chunked changes (`chunk1` through `chunk4`), so this chunk intentionally modifies the existing `evidence-provenance` capability rather than introducing a new report-only capability.

## What Changes

- **`src/report/coverage.rs`** (new file): `write_data_quality_and_coverage(out, state)` — renders a `Data Quality and Coverage` section from `state.data_coverage: Option<DataCoverageReport>`. When present it lists required inputs explicitly and surfaces any missing, stale, or partial inputs; when absent it prints `Unavailable`.
- **`src/report/provenance.rs`** (new file): `write_evidence_provenance(out, state)` — renders an `Evidence Provenance` section from `state.provenance_summary: Option<ProvenanceSummary>`. When present it lists providers used and caveats; when absent it prints `Unavailable`.
- **`src/report/final_report.rs`** (modify): Call both new helper functions from `format_final_report`, inserted after the `Analyst Evidence Snapshot` section and before `Research Debate Summary`. Widen `section_header` to `pub(super)` so sibling section modules can reuse the existing heading style without duplicating formatting logic.
- **`src/report/mod.rs`** (modify): Add `mod coverage; mod provenance;`. The public API remains `format_final_report`.
- **`openspec/changes/chunk4-report-verification/specs/evidence-provenance/spec.md`** (new file): Add the missing OpenSpec delta for the Stage 1 report-rendering slice of the `evidence-provenance` capability.

## Capabilities

### Modified Capabilities
- `evidence-provenance`: Completes the Stage 1 terminal-report slice by surfacing `DataCoverageReport` and `ProvenanceSummary` in the human-readable final report.

## Cross-Owner Changes

This change requires approved cross-owner edits before implementation begins.

- **`src/report/mod.rs`** (owner: `add-cli`): add `mod coverage; mod provenance;` so the new evidence-provenance section writers are compiled into the report module.
- **`src/report/final_report.rs`** (owner: `add-cli`): insert the two new section calls in `format_final_report` and widen `section_header` to `pub(super)` so sibling report modules can reuse the existing heading formatting.
- **Dependency only, no cross-owner edit in this chunk**: `src/state/reporting.rs`, `src/state/trading_state.rs`, and `src/state/mod.rs` must already expose `DataCoverageReport`, `ProvenanceSummary`, `TradingState.data_coverage`, and `TradingState.provenance_summary` from `chunk3-evidence-state-sync`.

## Impact

- **Code**: Two new `src/report/*.rs` files; additive edits to CLI-owned `src/report/final_report.rs` and `src/report/mod.rs`. No new state schema, config, or provider API changes in this chunk.
- **Upstream dependency**: `chunk3-evidence-state-sync` must land first because this chunk consumes `DataCoverageReport` and `ProvenanceSummary` without redefining them.
- **Tests**: Unit tests for exact heading rendering, required-input rendering, provider/caveat rendering, and the exact `Unavailable` fallback; one integration assertion that `format_final_report` places the new sections between analyst snapshot and research debate.
- **Rollback**: Revert the two new files and the additive report-module edits. No migration or config rollback is required. The `DataCoverageReport` and `ProvenanceSummary` types introduced in Chunk 3 remain in `TradingState` and are not affected.

## Alternatives Considered

### Option: Inline coverage and provenance into the existing `write_analyst_snapshot` function
Add the coverage and provenance content directly inside `write_analyst_snapshot` rather than creating dedicated helper functions and files.

Pros: Zero new files. The evidence snapshot section already deals with analyst data completeness, so adding coverage/provenance inline feels thematically cohesive. Fewer imports.

Cons: `write_analyst_snapshot` already spans a separate concern (LLM-produced summaries per analyst). Inlining would make it responsible for three distinct data shapes: analyst summaries, coverage metadata, and provenance metadata. Testing becomes harder because a single large function can only be tested end-to-end rather than in isolation. Section headings would need to be embedded inside an existing function, making reordering difficult.

Why rejected: The existing section-per-function pattern in `final_report.rs` (one `write_*` function per logical section) is consistent and scales well. Creating dedicated `coverage.rs` and `provenance.rs` modules follows the same convention, keeps each function under 50 lines, and allows the tests to target each section independently. The small overhead of two extra files is well justified.

### Option: Add the sections to the report using a trait-based extension mechanism
Define a `ReportSection` trait with a `write(out: &mut String, state: &TradingState)` method, implement it for `DataCoverageReport` and `ProvenanceSummary`, and call the trait from `format_final_report`.

Pros: Sections are self-contained and independently extensible. Adding future sections requires no changes to `format_final_report` — only a new trait impl. Aligns with open/closed principle.

Cons: Significant indirection for two simple string-rendering functions. The trait would require `dyn` dispatch or generic bounds propagating into `format_final_report`. Coverage and provenance sections both need `&TradingState` (not just `&DataCoverageReport`), so the trait would need a different signature or an extra state parameter — defeating the abstraction benefit. Tests become more complex.

Why rejected: The project currently uses plain free-function modules for all report helpers. Introducing a trait for two functions adds complexity without clear benefit at this stage. If the report module grows to 10+ sections, a trait or registry pattern would be worth revisiting as a dedicated refactoring change.

### Option: Defer the verification task (Task 13) and ship only the report sections
Land Task 12 (the two new report sections) without gating on a formal verification task. Trust the CI pipeline to validate.

Pros: Smaller change. Verification is already enforced by GitHub Actions on every push to `main`.

Cons: The evidence-provenance-foundation implementation spans four chunks. Without an explicit verification task in the spec, there is no documented checkpoint confirming the full Stage 1 implementation — from Chunk 1 prompt rules through Chunk 4 report sections — compiles, lints, and passes tests end-to-end as a cohesive unit. The smoke-test step (`cargo run`) is particularly valuable because it exercises the full pipeline path including the new report sections, which CI does not run due to requiring live API keys.

Why rejected: The verification task is lightweight (no code changes) and provides explicit traceability that Stage 1 is complete and validated. The smoke-test step catches integration issues that unit tests cannot, and the explicit task makes the delivery milestone unambiguous for reviewers.
