## 12. Add Report Coverage and Provenance Sections

### 12.1 Create `src/report/coverage.rs`

Create `src/report/coverage.rs` with a single `pub(crate)` function:

```rust
pub(crate) fn write_data_quality_and_coverage(out: &mut String, state: &TradingState)
```

- Call `section_header(out, "Data Quality and Coverage")` (re-use the existing helper via `super::final_report::section_header` or move `section_header` to `mod.rs` if needed).
- If `state.data_coverage` is `None`: write a single line `Unavailable` with no additional text.
- If `state.data_coverage` is `Some(coverage)`:
  - Write a summary line showing the count of required inputs and the count of missing inputs (e.g. `Required inputs: N | Missing: M`).
  - If `coverage.missing_inputs` is non-empty, write a bulleted list of the missing input names, one per line, prefixed with `  - `.
  - If `coverage.missing_inputs` is empty, write a line indicating all required inputs are present.
- Never call `.unwrap()` or `.expect()` — all access is via pattern matching or `.as_ref()`.

### 12.2 Create `src/report/provenance.rs`

Create `src/report/provenance.rs` with a single `pub(crate)` function:

```rust
pub(crate) fn write_evidence_provenance(out: &mut String, state: &TradingState)
```

- Call `section_header(out, "Evidence Provenance")`.
- If `state.provenance_summary` is `None`: write a single line `Unavailable` with no additional text.
- If `state.provenance_summary` is `Some(provenance)`:
  - Write providers used as a labeled inline list (e.g. `Providers: Finnhub, Yahoo Finance, FRED`). If `provenance.providers_used` is empty, write `Providers: none`.
  - Write caveats as a bulleted list under a `Caveats:` label, one per line, prefixed with `  - `. If `provenance.caveats` is empty, write `Caveats: none`.
- Never call `.unwrap()` or `.expect()` — all access is via pattern matching or `.as_ref()`.

### 12.3 Update `src/report/mod.rs`

Add the two new sub-module declarations and make their functions accessible:

```rust
mod coverage;
mod provenance;
```

The helper functions are `pub(crate)` within their modules; they will be called from `final_report.rs` via `coverage::write_data_quality_and_coverage` and `provenance::write_evidence_provenance`. No additional `pub use` re-exports are needed unless the public `format_final_report` API changes (it does not).

### 12.4 Update `src/report/final_report.rs`

In `format_final_report`, insert the two new section calls between `write_analyst_snapshot` and `write_research_debate`:

```rust
write_analyst_snapshot(&mut out, state);
coverage::write_data_quality_and_coverage(&mut out, state);   // NEW
provenance::write_evidence_provenance(&mut out, state);       // NEW
write_research_debate(&mut out, state);
```

Add the necessary `use` imports at the top of `final_report.rs` (or use the `super::` path if the module structure requires it). Confirm that the existing `format_final_report_handles_missing_analysts_gracefully` test still compiles and passes after this change.

### 12.5 Add tests for the new sections

Add a `#[cfg(test)]` block in each new file (or extend the existing test module in `final_report.rs` if re-using `minimal_state`):

**In `coverage.rs` or `final_report.rs` tests:**

```rust
#[test]
fn write_data_quality_and_coverage_heading_present_when_data_exists() {
    // Construct a TradingState with state.data_coverage = Some(DataCoverageReport { ... })
    // Call format_final_report or write_data_quality_and_coverage directly
    // Assert report.contains("Data Quality and Coverage")
}

#[test]
fn write_data_quality_and_coverage_shows_unavailable_when_none() {
    // Construct a TradingState with state.data_coverage = None
    // Assert report.contains("Unavailable")
}
```

**In `provenance.rs` or `final_report.rs` tests:**

```rust
#[test]
fn write_evidence_provenance_heading_present_when_data_exists() {
    // Construct a TradingState with state.provenance_summary = Some(ProvenanceSummary { ... })
    // Assert report.contains("Evidence Provenance")
}

#[test]
fn write_evidence_provenance_shows_unavailable_when_none() {
    // Construct a TradingState with state.provenance_summary = None
    // Assert report.contains("Unavailable")
}
```

Use the existing `minimal_state()` helper from `final_report.rs` as a base and extend it with the coverage/provenance fields as needed. If `DataCoverageReport` and `ProvenanceSummary` require non-trivial construction, add a `coverage_state()` and `provenance_state()` helper in the respective test modules.

### 12.6 Commit

```
git commit -m "feat: add report coverage and provenance sections"
```

---

## 13. Full Verification

This is a process task. No code changes are expected. All commands must pass without modification.

### 13.1 Formatting check

```bash
cargo fmt -- --check
```

Expected: exits 0, no diff output. If any formatting issues are reported, run `cargo fmt` and re-commit before proceeding.

### 13.2 Lint check

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: exits 0, no warnings. Address any clippy diagnostics before proceeding to the test step. Common issues to watch for: unused imports in the new `coverage.rs` / `provenance.rs` files, redundant `.as_ref()` calls, or `writeln!` result unused warnings (use `let _ = writeln!(...)` to silence).

### 13.3 Test suite

```bash
cargo nextest run --all-features --locked
```

Expected: all tests pass. If any tests fail, diagnose and fix before claiming the chunk is complete. The four new tests from Task 12.5 must appear in the output:

- `write_data_quality_and_coverage_heading_present_when_data_exists`
- `write_data_quality_and_coverage_shows_unavailable_when_none`
- `write_evidence_provenance_heading_present_when_data_exists`
- `write_evidence_provenance_shows_unavailable_when_none`

### 13.4 Smoke test (requires API keys in `.env`)

```bash
cargo run
```

Expected: the full pipeline completes successfully and the final report printed to stdout contains both:

- The exact string `Data Quality and Coverage`
- The exact string `Evidence Provenance`

This step requires valid API keys in `.env` (`SCORPIO_OPENAI_API_KEY` or equivalent, `FINNHUB_API_KEY`, etc.). If API keys are unavailable, verify section presence by running the existing `format_final_report` tests which exercise the report rendering path without live API calls.

Note: if `state.data_coverage` and `state.provenance_summary` are not yet populated by the upstream analyst phase (because Chunk 3 pipeline wiring is not yet complete), both sections will render as `Unavailable`. This is the correct graceful-degradation behavior — confirm `Unavailable` appears rather than any panic or missing section.
