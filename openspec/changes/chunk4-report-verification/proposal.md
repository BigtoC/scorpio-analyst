## Why

The evidence discipline and provenance tracking introduced in Chunks 1–3 collects rich metadata throughout the pipeline — data coverage assessments in `DataCoverageReport` and provenance summaries in `ProvenanceSummary` — but that metadata is never surfaced to the human reader. The final report currently shows analyst evidence snapshots and debate summaries but gives no indication of which data sources were consulted, what was missing, or how confident the pipeline is in its evidence base. This makes it impossible for operators to audit the analysis or identify when a trading decision was made with incomplete market data.

Chunk 4 is the final delivery slice for Stage 1. It adds two new terminal-rendered sections — `Data Quality and Coverage` and `Evidence Provenance` — to `format_final_report`, and performs full system verification (fmt, clippy, tests, smoke run). These sections make the evidence discipline visible: an operator glancing at the report can immediately see which data inputs were present, what was unavailable, which providers contributed evidence, and any associated caveats. The `Unavailable` fallback ensures the report never panics even if upstream phases did not produce coverage or provenance data.

## What Changes

- **`src/report/coverage.rs`** (new file): `write_data_quality_and_coverage(out, state)` — renders a `Data Quality and Coverage` section from `state.data_coverage: Option<DataCoverageReport>`. Lists required inputs and missing inputs when data is present; prints `Unavailable` when `None`.
- **`src/report/provenance.rs`** (new file): `write_evidence_provenance(out, state)` — renders an `Evidence Provenance` section from `state.provenance_summary: Option<ProvenanceSummary>`. Lists providers used and any caveats when data is present; prints `Unavailable` when `None`.
- **`src/report/final_report.rs`** (modify): Call both new helper functions from `format_final_report`, inserted after the `Analyst Evidence Snapshot` section and before the `Research Debate Summary` and risk sections.
- **`src/report/mod.rs`** (modify): Export the two new sub-modules cleanly (`mod coverage; mod provenance;` using `pub(crate)` visibility on the functions).

## Capabilities

### New Capabilities
- `report-coverage-section`: Human-readable `Data Quality and Coverage` section in the final report, sourced from `DataCoverageReport`. Includes required inputs, missing inputs, and an `Unavailable` fallback when the field is absent.
- `report-provenance-section`: Human-readable `Evidence Provenance` section in the final report, sourced from `ProvenanceSummary`. Includes providers used, associated caveats, and an `Unavailable` fallback when the field is absent.

### Modified Capabilities
- `final-report`: The `format_final_report` function is extended with two additional section calls, placed between `Analyst Evidence Snapshot` and `Research Debate Summary`.

## Impact

- **Code**: Two new `src/report/*.rs` files; `src/report/final_report.rs` gains two function calls and two imports; `src/report/mod.rs` gains two `mod` declarations. No state schema changes, no new crate dependencies, no config changes.
- **Tests**: Unit tests added asserting exact heading strings (`Data Quality and Coverage`, `Evidence Provenance`) are present when data exists, and exact string `Unavailable` appears for each section when the corresponding state field is `None`.
- **Rollback**: Revert the two new files, the two function calls in `format_final_report`, and the two `mod` declarations in `mod.rs`. No state migration, no DB changes, no config changes required. The `DataCoverageReport` and `ProvenanceSummary` types introduced in Chunk 3 remain in `TradingState` and are not affected.

## Alternatives Considered

### Option: Inline coverage and provenance into the existing `write_analyst_snapshot` function
Add the coverage and provenance content directly inside `write_analyst_snapshot` rather than creating dedicated helper functions and files.

Pros: Zero new files. The evidence snapshot section already deals with analyst data completeness, so adding coverage/provenance inline feels thematically cohesive. Fewer imports.

Cons: `write_analyst_snapshot` already spans ~60 lines and handles a separate concern (LLM-produced summaries per analyst). Inlining would make it responsible for three distinct data shapes: analyst summaries, coverage metadata, and provenance metadata. Testing becomes harder because a single large function can only be tested end-to-end rather than in isolation. Section headings would need to be embedded inside an existing function, making reordering difficult.

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
