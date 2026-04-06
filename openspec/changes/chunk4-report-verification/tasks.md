## 12. Add Report Coverage and Provenance Sections

- [x] 12.0 Confirm the Chunk 3 prerequisite is present.

  Before starting implementation, confirm all three prerequisites from `chunk3-evidence-state-sync` exist:

  - `src/state/reporting.rs` defines `DataCoverageReport` and `ProvenanceSummary`
  - `src/state/trading_state.rs` exposes `data_coverage` and `provenance_summary`
  - `src/state/mod.rs` re-exports the reporting types

  If any prerequisite is missing, stop and land `chunk3-evidence-state-sync` first.

- [x] 12.1 Create `src/report/coverage.rs`.

Create `src/report/coverage.rs` with a single `pub(crate)` function:

```rust
pub(crate) fn write_data_quality_and_coverage(out: &mut String, state: &TradingState)
```

- Call `section_header(out, "Data Quality and Coverage")` via `super::final_report::section_header` after Task 12.4 widens that helper to `pub(super)`. Do not duplicate heading-formatting logic.
- If `state.data_coverage` is `None`: write a single line `Unavailable` with no additional text.
- If `state.data_coverage` is `Some(coverage)`:
  - Write a line listing the required analyst inputs explicitly (for example: `Required inputs: fundamentals, sentiment, news, technical`). The names must come from `coverage.required_inputs`, not from a hard-coded string.
  - If `coverage.missing_inputs` is non-empty, write a `Missing inputs:` label followed by a bulleted list, one per line, prefixed with `  - `.
  - If `coverage.missing_inputs` is empty, write `Missing inputs: none`.
  - If `coverage.stale_inputs` is non-empty, write a `Stale inputs:` label followed by a bulleted list.
  - If `coverage.partial_inputs` is non-empty, write a `Partial inputs:` label followed by a bulleted list.
  - If all three issue lists are empty, write a line indicating all required inputs are present.
- Never call `.unwrap()` or `.expect()` — all access is via pattern matching or `.as_ref()`.

- [x] 12.2 Create `src/report/provenance.rs`.

Create `src/report/provenance.rs` with a single `pub(crate)` function:

```rust
pub(crate) fn write_evidence_provenance(out: &mut String, state: &TradingState)
```

- Call `section_header(out, "Evidence Provenance")`.
- If `state.provenance_summary` is `None`: write a single line `Unavailable` with no additional text.
- If `state.provenance_summary` is `Some(provenance)`:
  - Write providers used as a labeled inline list (e.g. `Providers: Finnhub, Yahoo Finance, FRED`). If `provenance.providers_used` is empty, write `Providers: none`.
  - Write caveats as a bulleted list under a `Caveats:` label, one per line, prefixed with `  - `. If `provenance.caveats` is empty, write `Caveats: none`.
  - Do not render `generated_at` separately in Stage 1; the report header already carries the run timestamp.
- Never call `.unwrap()` or `.expect()` — all access is via pattern matching or `.as_ref()`.

- [x] 12.3 Update `src/report/mod.rs`.

Add the two new sub-module declarations and make their functions accessible:

```rust
mod coverage;
mod provenance;
```

The helper functions are `pub(crate)` within their modules; they will be called from `final_report.rs` via `coverage::write_data_quality_and_coverage` and `provenance::write_evidence_provenance`. No additional `pub use` re-exports are needed unless the public `format_final_report` API changes (it does not).

- [x] 12.4 Update `src/report/final_report.rs`.

In `format_final_report`, insert the two new section calls between `write_analyst_snapshot` and `write_research_debate`:

```rust
write_analyst_snapshot(&mut out, state);
coverage::write_data_quality_and_coverage(&mut out, state);   // NEW
provenance::write_evidence_provenance(&mut out, state);       // NEW
write_research_debate(&mut out, state);
```

Add the necessary `use` imports at the top of `final_report.rs` (or use the `super::` path if the module structure requires it). Widen `section_header` from private to `pub(super)` so the two new sibling modules can reuse it. Confirm that the existing `format_final_report_handles_missing_analysts_gracefully` test still compiles and passes after this change.

- [x] 12.5 Add tests for the new sections.

Add a `#[cfg(test)]` block in each new file (or extend the existing test module in `final_report.rs` if re-using `minimal_state`):

- In `coverage.rs` or `final_report.rs` tests:

```rust
fn write_data_quality_and_coverage_lists_required_and_missing_inputs() { /* ... */ }
fn write_data_quality_and_coverage_shows_unavailable_when_none() { /* ... */ }
```

- In `provenance.rs` or `final_report.rs` tests:

```rust
fn write_evidence_provenance_lists_providers_and_caveats() { /* ... */ }
fn write_evidence_provenance_shows_unavailable_when_none() { /* ... */ }
```

Also add one integration assertion on `format_final_report` verifying section order: `Analyst Evidence Snapshot` → `Data Quality and Coverage` → `Evidence Provenance` → `Research Debate Summary`.

Use the existing `minimal_state()` helper from `final_report.rs` as a base if available after Chunk 3 lands, or add focused local helpers in the new test modules.

- [x] 12.6 Record cross-owner approval and owner awareness.

Before implementation begins, obtain maintainer or owner approval for the CLI-owned edits listed in `proposal.md`:

- `src/report/mod.rs`
- `src/report/final_report.rs`

When `openspec/changes/add-cli/tasks.md` exists, add a `### Cross-Owner Touch-points` note there for awareness as required by `docs/architect-plan.md`.

- [ ] 12.7 Commit.

```
git commit -m "feat: add report coverage and provenance sections"
```

---

## 13. Full Verification

This is a process task. No code changes are expected. All commands must pass without modification.

- [x] 13.1 Formatting check.

```bash
cargo fmt -- --check
```

Expected: exits 0, no diff output. If any formatting issues are reported, run `cargo fmt` and re-commit before proceeding.

- [x] 13.2 Lint check.

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: exits 0, no warnings. Address any clippy diagnostics before proceeding to the test step. Common issues to watch for: unused imports in the new `coverage.rs` / `provenance.rs` files, redundant `.as_ref()` calls, or `writeln!` result unused warnings (use `let _ = writeln!(...)` to silence).

- [x] 13.3 Test suite.

```bash
cargo nextest run --all-features --locked
```

Expected: all tests pass. If any tests fail, diagnose and fix before claiming the chunk is complete. The new report-rendering tests from Task 12.5 must pass, including the section-order assertion.

- [ ] 13.4 Smoke test (requires API keys in `.env`).

```bash
cargo run
```

Expected: the full pipeline completes successfully and the final report printed to stdout contains both:

- The exact string `Data Quality and Coverage`
- The exact string `Evidence Provenance`

This step requires valid API keys in `.env` for the configured providers and data sources (for example `SCORPIO_OPENAI_API_KEY`, `SCORPIO_FINNHUB_API_KEY`, and `SCORPIO_FRED_API_KEY`). If API keys are unavailable, verify section presence by running the existing `format_final_report` tests which exercise the report rendering path without live API calls.

Note: if a partial run or test harness leaves `state.data_coverage` and `state.provenance_summary` as `None`, both sections must render `Unavailable`. This is the correct graceful-degradation behavior — confirm `Unavailable` appears rather than any panic or missing section.

- [ ] 13.5 OpenSpec validation.

```bash
openspec validate chunk4-report-verification --strict
```

Expected: `Change 'chunk4-report-verification' is valid`.
