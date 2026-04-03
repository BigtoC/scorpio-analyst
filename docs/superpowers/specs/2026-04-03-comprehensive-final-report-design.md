# Comprehensive Final Report

## Context

The current CLI output after a trading analysis run is minimal: only the fund manager's decision enum, rationale, and aggregate token counts are printed (`main.rs:128-145`). All the rich data produced during the pipeline — analyst summaries, research debate consensus, trader proposal, risk reports — flows through `TradingState` but never reaches the terminal. Users must inspect SQLite snapshots or add debug logging to see intermediate results.

This spec adds a dedicated report module that renders a comprehensive, color-coded terminal report covering every pipeline phase. It also extends `ExecutionStatus` with an `action: TradeAction` field so the fund manager explicitly communicates Buy/Sell/Hold alongside Approved/Rejected.

## ExecutionStatus Schema Change

### `src/state/execution.rs`

Add `action: TradeAction` to `ExecutionStatus`:

```rust
use super::TradeAction;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionStatus {
    pub decision: Decision,
    pub action: TradeAction,    // NEW — Buy, Sell, or Hold
    pub rationale: String,
    pub decided_at: String,
}
```

The fund manager decides both:
- **Decision** (Approved/Rejected) — whether the proposal passes review
- **Action** (Buy/Sell/Hold) — what the fund manager recommends

This allows the fund manager to override the trader's action. For example, approve a cautious Hold even when the trader proposed Buy, or reject but still communicate the directional lean.

### `src/agents/fund_manager/validation.rs`

- Update `ExecutionStatusResponse` to include `action: TradeAction`
- Map it through in `parse_and_validate_execution_status`
- **Deterministic reject path** (`agent.rs:123-136`): set `action: TradeAction::Hold` when the safety-net triggers, since no trade should execute on auto-reject

### `docs/prompts.md` — Fund Manager prompt update

Add `action` to the required JSON keys:

```
Return ONLY a JSON object matching `ExecutionStatus`:
- `decision`: one of `Approved` or `Rejected`
- `action`: one of `Buy`, `Sell`, `Hold`
- `rationale`: concise audit-ready explanation
- `decided_at`: use `{current_date}` unless the runtime provides a more precise timestamp
```

Add instruction:

```
8. Set `action` to the trade direction you endorse. This may match the trader's proposed
   action or differ if your review warrants a change. If rejecting, `Hold` is the expected
   default unless the rejection is specifically about direction (e.g., the trader said Buy
   but evidence supports Sell).
```

### Test updates

Files that construct `ExecutionStatus` directly and need the new `action` field:

- `src/agents/fund_manager/validation.rs` (unit tests)
- `src/workflow/tasks/test_helpers.rs` (integration test fixtures)

All hardcoded `ExecutionStatus` values get `action: TradeAction::Hold` (or the appropriate variant for the test scenario).

## Module Structure

New module: `src/report/`

```
src/report/
  mod.rs              — public re-export of format_final_report
  final_report.rs     — report formatting logic
```

### Public API

```rust
/// Render a comprehensive terminal report from the completed trading state.
/// Returns a String containing ANSI color codes (respects NO_COLOR env var).
pub fn format_final_report(state: &TradingState) -> String;
```

`src/lib.rs` re-exports the `report` module.

## Report Sections

The report renders these sections top-to-bottom, matching the template in `docs/prompts.md`:

### 1. Header

- Symbol, target date, execution ID (UUID)
- Final decision: **green** "Approved" or **red** "Rejected"
- Fund manager action: **green** Buy / **red** Sell / **yellow** Hold (from `ExecutionStatus.action`)
- `decided_at` timestamp from `ExecutionStatus`

### 2. Executive Summary

- `ExecutionStatus.rationale` — the fund manager's audit-ready explanation
- Displayed as a wrapped paragraph, not a table

### 3. Trader Proposal (comfy-table)

| Field        | Source                       | Formatting                                                     |
|--------------|------------------------------|----------------------------------------------------------------|
| Action       | `TradeProposal.action`       | Green Buy, Red Sell, Yellow Hold                               |
| Confidence   | `TradeProposal.confidence`   | Color-coded: green >0.7, yellow 0.4-0.7, red <0.4              |
| Target Price | `TradeProposal.target_price` | 2 decimal places                                               |
| Stop Loss    | `TradeProposal.stop_loss`    | 2 decimal places                                               |
| Rationale    | `TradeProposal.rationale`    | Truncated to ~200 chars in table cell; full text below if long |

If `trader_proposal` is `None`, display "Trader proposal: unavailable".

### 4. Analyst Evidence Snapshot (comfy-table + detail blocks)

**Table** — one row per analyst, compact overview:

| Column       | Source                          | Notes                                     |
|--------------|---------------------------------|-------------------------------------------|
| Analyst      | Fixed label                     | Fundamentals, Sentiment, News, Technical  |
| Key Evidence | `.summary`, first sentence only | First sentence extracted for table cell   |
| Data Status  | `Option` presence check         | "Complete" if `Some`, "Missing" if `None` |

**Detail blocks** — printed below the table, one per analyst that has data. Each block shows the full `.summary` text with the analyst name as a sub-header. Skipped for analysts with `None` data.

### 5. Research Debate Summary

- **Consensus Summary**: `TradingState.consensus_summary` (plain text, or "No consensus produced" if `None`)
- If `debate_history` is non-empty, extract last bull and last bear messages for "Strongest Bullish/Bearish Evidence" lines

### 6. Risk Review (comfy-table + detail blocks)

**Table** — one row per risk persona, compact overview:

| Column                  | Source                                 | Notes                                   |
|-------------------------|----------------------------------------|-----------------------------------------|
| Risk Persona            | Fixed: Aggressive/Neutral/Conservative | —                                       |
| Flags Violation         | `RiskReport.flags_violation`           | Red "Yes" / Green "No" / Gray "Unknown" |
| Assessment              | `RiskReport.assessment`                | First sentence only in table cell       |
| Recommended Adjustments | `RiskReport.recommended_adjustments`   | Comma-joined or "None"                  |

If a risk report is `None`, display "Unknown" for all cells in that row.

**Detail blocks** — printed below the table, one per risk persona that has data. Each block shows the full `.assessment` text with the persona name as a sub-header. Skipped for personas with `None` reports.

### 7. Deterministic Safety Check

- Neutral flags violation: true/false/unknown
- Conservative flags violation: true/false/unknown
- Auto-reject rule triggered: Yes (red) / No (green)
  - Triggered when both Conservative AND Neutral `flags_violation == true`

### 8. Token Usage Summary (comfy-table)

Per-phase rows from `TokenUsageTracker.phase_usage`:

| Column            | Source                                     |
|-------------------|--------------------------------------------|
| Phase             | `PhaseTokenUsage.phase_name`               |
| Prompt Tokens     | `phase_prompt_tokens`                      |
| Completion Tokens | `phase_completion_tokens`                  |
| Total Tokens      | `phase_total_tokens`                       |
| Duration          | `phase_duration_ms` (formatted as seconds) |

Footer row with aggregated totals from `TokenUsageTracker`.

### 9. Disclaimer

Fixed text:
```
⚠️ Disclaimers
- This is AI-generated analysis for educational and research purposes only.
- Not financial advice. Market data may be incomplete or delayed.
- Past performance does not guarantee future results
```

Displayed in dim/gray color.

## Internal Helpers

All private to `final_report.rs`:

- `confidence_indicator(f64) -> ColoredString` — green/yellow/red with emoji prefix
- `data_status<T>(Option<&T>) -> ColoredString` — "Complete" (green) or "Missing" (dim)
- `violation_label(Option<&RiskReport>) -> ColoredString` — "Yes" (red) / "No" (green) / "Unknown" (dim)
- `first_sentence(s: &str) -> &str` — extracts text up to the first `.` / `!` / `?` followed by whitespace or end-of-string; falls back to the full string if no sentence boundary is found
- `format_duration_ms(u64) -> String` — converts ms to human-readable seconds

## Dependencies

Add to `Cargo.toml` under `[dependencies]`:

```toml
colored = "3"
comfy-table = "7"
```

Both are well-established, minimal-dependency crates already listed in the CLAUDE.md planned dependencies.

## Changes to Existing Files

### `src/main.rs` (lines 126-145)

Replace the current output block:

```rust
// Before (lines 128-145):
match &state.final_execution_status { ... }
println!("Token usage: ...");

// After:
if state.final_execution_status.is_none() {
    eprintln!("pipeline completed without a final execution status");
    std::process::exit(1);
}
println!("{}", scorpio_analyst::report::format_final_report(&state));
```

### `src/lib.rs`

Add `pub mod report;` to the module declarations.

### `src/report/mod.rs`

Re-export:
```rust
mod final_report;
pub use final_report::format_final_report;
```

## Verification

1. `cargo build` — compiles without warnings
2. `cargo clippy` — no new warnings
3. `cargo nextest run --all-features` — existing tests pass
4. Manual test: run `cargo run` with a valid config and verify the full report renders correctly in terminal
5. Verify `NO_COLOR=1 cargo run` produces output without ANSI escape codes
6. Verify report gracefully handles missing data (e.g., `None` analyst fields show "Missing")
