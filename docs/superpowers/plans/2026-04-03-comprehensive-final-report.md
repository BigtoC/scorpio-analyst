# Comprehensive Final Report Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the minimal 3-line CLI output with a comprehensive, color-coded terminal report covering all pipeline phases — analyst summaries, research consensus, trader proposal, risk review, fund manager decision, and token usage.

**Architecture:** Add `action: TradeAction` to `ExecutionStatus` so the fund manager explicitly outputs Buy/Sell/Hold. Create a new `src/report/` module with `format_final_report(&TradingState) -> String` that builds the report using `colored` and `comfy-table`. Replace the output block in `main.rs` with a single `println!` call.

**Tech Stack:** `colored` 3.x (ANSI terminal colors), `comfy-table` 7.x (aligned table rendering), existing `serde`/`serde_json` for serialization.

---

## File Map

| File                                    | Action | Responsibility                                                 |
|-----------------------------------------|--------|----------------------------------------------------------------|
| `Cargo.toml`                            | Modify | Add `colored` and `comfy-table` dependencies                   |
| `src/state/execution.rs`                | Modify | Add `action: TradeAction` field to `ExecutionStatus`           |
| `src/agents/fund_manager/validation.rs` | Modify | Parse and validate `action` field in `ExecutionStatusResponse` |
| `src/agents/fund_manager/agent.rs`      | Modify | Set `action: TradeAction::Hold` on deterministic reject path   |
| `src/agents/fund_manager/prompt.rs`     | Modify | Update system prompt to require `action` field                 |
| `src/agents/fund_manager/tests.rs`      | Modify | Add `action` field to all JSON fixtures and assertions         |
| `src/workflow/tasks/test_helpers.rs`    | Modify | Add `action` field to `StubFundManagerTask`                    |
| `src/report/mod.rs`                     | Create | Re-export `format_final_report`                                |
| `src/report/final_report.rs`            | Create | Report formatting logic                                        |
| `src/lib.rs`                            | Modify | Add `pub mod report;`                                          |
| `src/main.rs`                           | Modify | Replace output block with `format_final_report` call           |
| `docs/prompts.md`                       | Modify | Update Fund Manager prompt section with `action` field         |

---

### Task 1: Add `action: TradeAction` to `ExecutionStatus`

**Files:**
- Modify: `src/state/execution.rs`

- [ ] **Step 1: Add `TradeAction` import and `action` field**

In `src/state/execution.rs`, add the import and new field:

```rust
use serde::{Deserialize, Serialize};

use super::TradeAction;

/// Final decision issued by the Fund Manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Approved,
    Rejected,
}

/// Terminal execution status for a trading cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionStatus {
    pub decision: Decision,
    pub action: TradeAction,
    pub rationale: String,
    pub decided_at: String,
}
```

- [ ] **Step 2: Run build to see all compilation errors**

Run: `cargo build 2>&1 | head -80`

Expected: Multiple compilation errors in files that construct `ExecutionStatus` without the new `action` field. This is expected — we fix them in subsequent tasks.

- [ ] **Step 3: Commit the struct change**

```bash
git add src/state/execution.rs
git commit -m "feat: add action field to ExecutionStatus struct"
```

---

### Task 2: Update fund manager validation to parse `action`

**Files:**
- Modify: `src/agents/fund_manager/validation.rs`

- [ ] **Step 1: Add `TradeAction` import and `action` field to `ExecutionStatusResponse`**

In `src/agents/fund_manager/validation.rs`, update the imports and internal response struct:

```rust
use crate::{
    agents::shared::extract_json_object,
    constants::{MAX_RATIONALE_CHARS, MAX_RAW_RESPONSE_CHARS},
    error::TradingError,
    state::{Decision, ExecutionStatus, TradeAction, TradingState},
};
```

Update `ExecutionStatusResponse`:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionStatusResponse {
    decision: Decision,
    action: TradeAction,
    rationale: String,
    decided_at: Option<String>,
}
```

- [ ] **Step 2: Map `action` through in `parse_and_validate_execution_status`**

Update the `ExecutionStatus` construction inside `parse_and_validate_execution_status`:

```rust
    let mut status = ExecutionStatus {
        decision: parsed.decision,
        action: parsed.action,
        rationale: parsed.rationale,
        decided_at: parsed.decided_at.unwrap_or_else(|| target_date.to_owned()),
    };
```

- [ ] **Step 3: Add `action` to all test `ExecutionStatus` literals in validation.rs**

Update every `ExecutionStatus` in the `#[cfg(test)] mod tests` block to include `action: TradeAction::Hold` (or appropriate variant). There are 5 test functions that construct `ExecutionStatus` directly. Add the import `use crate::state::TradeAction;` at the top of the test module, then update each:

```rust
    // In validate_rejects_empty_rationale:
    let status = ExecutionStatus {
        decision: Decision::Approved,
        action: TradeAction::Buy,
        rationale: String::new(),
        decided_at: "2026-03-15".to_owned(),
    };

    // In validate_rejects_whitespace_only_rationale:
    let status = ExecutionStatus {
        decision: Decision::Approved,
        action: TradeAction::Buy,
        rationale: "   ".to_owned(),
        decided_at: "2026-03-15".to_owned(),
    };

    // In validate_rejects_control_char_in_rationale:
    let status = ExecutionStatus {
        decision: Decision::Approved,
        action: TradeAction::Buy,
        rationale: "bad\x00content".to_owned(),
        decided_at: "2026-03-15".to_owned(),
    };

    // In validate_rejects_escape_char_in_rationale:
    let status = ExecutionStatus {
        decision: Decision::Approved,
        action: TradeAction::Buy,
        rationale: "bad\x1bcontent".to_owned(),
        decided_at: "2026-03-15".to_owned(),
    };

    // In validate_allows_newline_and_tab_in_rationale:
    let status = ExecutionStatus {
        decision: Decision::Approved,
        action: TradeAction::Buy,
        rationale: "Approved.\nRisk:\tWithin bounds.".to_owned(),
        decided_at: "2026-03-15".to_owned(),
    };

    // In valid_approved_status_passes_validation:
    let status = ExecutionStatus {
        decision: Decision::Approved,
        action: TradeAction::Buy,
        rationale: "The proposal is well-supported by all available evidence.".to_owned(),
        decided_at: "2026-03-15T00:00:00Z".to_owned(),
    };

    // In valid_rejected_status_passes_validation:
    let status = ExecutionStatus {
        decision: Decision::Rejected,
        action: TradeAction::Hold,
        rationale: "The stop-loss is too wide relative to the evidence quality.".to_owned(),
        decided_at: "2026-03-15T00:00:00Z".to_owned(),
    };
```

- [ ] **Step 4: Run validation tests**

Run: `cargo test --lib agents::fund_manager::validation -- -v`

Expected: All 7 validation tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agents/fund_manager/validation.rs
git commit -m "feat: parse and validate action field in ExecutionStatusResponse"
```

---

### Task 3: Update fund manager agent deterministic reject path

**Files:**
- Modify: `src/agents/fund_manager/agent.rs`

- [ ] **Step 1: Add `TradeAction` import**

Add `TradeAction` to the existing state imports in `agent.rs`:

```rust
    state::{AgentTokenUsage, Decision, ExecutionStatus, TradeAction, TradingState},
```

- [ ] **Step 2: Set `action: TradeAction::Hold` on deterministic reject**

Update the `ExecutionStatus` construction in the deterministic reject path (around line 125):

```rust
            let status = ExecutionStatus {
                decision: Decision::Rejected,
                action: TradeAction::Hold,
                rationale: DETERMINISTIC_REJECT_RATIONALE.to_owned(),
                decided_at,
            };
```

- [ ] **Step 3: Run agent tests**

Run: `cargo test --lib agents::fund_manager -- -v`

Expected: Compilation succeeds. Some tests may still fail because the JSON fixtures in `tests.rs` don't include `action` yet — that's fixed in Task 4.

- [ ] **Step 4: Commit**

```bash
git add src/agents/fund_manager/agent.rs
git commit -m "feat: set action Hold on deterministic reject path"
```

---

### Task 4: Update fund manager test fixtures with `action` field

**Files:**
- Modify: `src/agents/fund_manager/tests.rs`

- [ ] **Step 1: Update all JSON fixture functions**

Update the JSON strings to include the `action` field:

```rust
fn approved_json() -> String {
    r#"{"decision":"Approved","action":"Buy","rationale":"All risk checks passed. Proposal is well-supported by analyst data.","decided_at":"2026-03-15"}"#.to_owned()
}

fn approved_json_without_decided_at() -> String {
    r#"{"decision":"Approved","action":"Buy","rationale":"All risk checks passed. Proposal is well-supported by analyst data."}"#.to_owned()
}

fn approved_json_with_missing_data_ack() -> String {
    r#"{"decision":"Approved","action":"Hold","rationale":"Approved with reduced confidence because one or more upstream inputs are missing.","decided_at":"2026-03-15"}"#.to_owned()
}

fn rejected_json() -> String {
    r#"{"decision":"Rejected","action":"Hold","rationale":"Insufficient supporting evidence for the proposed position size.","decided_at":"2026-03-15"}"#.to_owned()
}
```

- [ ] **Step 2: Update the schema violation test JSON to include `action`**

Update `schema_violation_on_invalid_decision_value_from_llm` test:

```rust
    let bad_json = r#"{"decision":"Maybe","action":"Buy","rationale":"Seems fine.","decided_at":"2026-03-15"}"#;
```

Update `decided_at_is_overwritten_with_runtime_timestamp` test:

```rust
    let stale_json =
        r#"{"decision":"Approved","action":"Buy","rationale":"Looks good.","decided_at":"1900-01-01T00:00:00Z"}"#;
```

- [ ] **Step 3: Add assertion for `action` field in deterministic reject test**

In `deterministic_rejection_when_both_conservative_and_neutral_flag_violation`, add:

```rust
    assert_eq!(status.action, TradeAction::Hold);
```

Add `TradeAction` to the existing state imports at the top of the test file if not already present.

- [ ] **Step 4: Run all fund manager tests**

Run: `cargo test --lib agents::fund_manager -- -v`

Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agents/fund_manager/tests.rs
git commit -m "test: update fund manager test fixtures with action field"
```

---

### Task 5: Update fund manager prompt

**Files:**
- Modify: `src/agents/fund_manager/prompt.rs`
- Modify: `docs/prompts.md`

- [ ] **Step 1: Update `FUND_MANAGER_SYSTEM_PROMPT` in prompt.rs**

Replace the schema output section in the system prompt constant:

```rust
Return ONLY a JSON object matching `ExecutionStatus`:
- `decision`: `Approved` or `Rejected`
- `action`: one of `Buy`, `Sell`, `Hold`
- `rationale`: concise audit-ready explanation
- `decided_at`: use `{current_date}` unless the runtime provides a more precise timestamp
```

Add instruction 8 before the final "Do not restate" line:

```rust
8. Set `action` to the trade direction you endorse. This may match the trader's proposed \
action or differ if your review warrants a change. If rejecting, `Hold` is the expected \
default unless the rejection is specifically about direction.
```

- [ ] **Step 2: Update the prompt assertion test**

In the `system_prompt_contains_safety_net_instructions` test, add an assertion for the new field:

```rust
    assert!(
        FUND_MANAGER_SYSTEM_PROMPT.contains("action"),
        "system prompt must mention action field"
    );
```

- [ ] **Step 3: Update `docs/prompts.md` Fund Manager section**

In the Fund Manager section (around line 600), update the required keys list to add `action`:

Under `**Required keys:**` add `action` to the list.

Update the return schema in the system prompt code block to match what was done in Step 1. Add instruction 8 about the `action` field.

- [ ] **Step 4: Run prompt tests**

Run: `cargo test --lib agents::fund_manager::prompt -- -v`

Expected: All prompt tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/agents/fund_manager/prompt.rs docs/prompts.md
git commit -m "feat: update fund manager prompt to require action field"
```

---

### Task 6: Update `StubFundManagerTask` in test helpers

**Files:**
- Modify: `src/workflow/tasks/test_helpers.rs`

- [ ] **Step 1: Add `action` to `StubFundManagerTask` `ExecutionStatus` construction**

In `StubFundManagerTask::run` (around line 722), add the `action` field:

```rust
        state.final_execution_status = Some(ExecutionStatus {
            decision: Decision::Approved,
            action: TradeAction::Buy,
            rationale: "stub: approved — risk within tolerances".to_owned(),
            decided_at: "2026-03-20T00:00:00Z".to_owned(),
        });
```

`TradeAction` is already imported at the top of the file.

- [ ] **Step 2: Run full test suite to verify no remaining compile errors**

Run: `cargo test 2>&1 | tail -20`

Expected: All tests compile and pass. No remaining references to `ExecutionStatus` without the `action` field.

- [ ] **Step 3: Commit**

```bash
git add src/workflow/tasks/test_helpers.rs
git commit -m "test: add action field to StubFundManagerTask"
```

---

### Task 7: Add `colored` and `comfy-table` dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add dependencies**

Add under `[dependencies]` in `Cargo.toml`, in the `# Utilities` section:

```toml
colored = "3"
comfy-table = "7"
```

- [ ] **Step 2: Verify dependencies resolve**

Run: `cargo check`

Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add colored and comfy-table for terminal report"
```

---

### Task 8: Create `src/report/` module with `format_final_report`

**Files:**
- Create: `src/report/mod.rs`
- Create: `src/report/final_report.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/report/mod.rs`**

```rust
mod final_report;

pub use final_report::format_final_report;
```

- [ ] **Step 2: Create `src/report/final_report.rs` with helper functions**

```rust
use std::fmt::Write;

use colored::Colorize;
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table};

use crate::state::{
    Decision, RiskReport, TokenUsageTracker, TradeAction, TradeProposal, TradingState,
};

/// Render a comprehensive terminal report from the completed trading state.
pub fn format_final_report(state: &TradingState) -> String {
    let mut out = String::new();

    write_header(&mut out, state);
    write_executive_summary(&mut out, state);
    write_trader_proposal(&mut out, state);
    write_analyst_snapshot(&mut out, state);
    write_research_debate(&mut out, state);
    write_risk_review(&mut out, state);
    write_safety_check(&mut out, state);
    write_token_usage(&mut out, &state.token_usage);
    write_disclaimer(&mut out);

    out
}

// ── helpers ──────────────────────────────────────────────────────────────

fn first_sentence(s: &str) -> &str {
    for (i, c) in s.char_indices() {
        if matches!(c, '.' | '!' | '?') {
            let after = i + c.len_utf8();
            // End-of-string or followed by whitespace => sentence boundary
            if after >= s.len() || s[after..].starts_with(char::is_whitespace) {
                return &s[..after];
            }
        }
    }
    s
}

fn action_colored(action: &TradeAction) -> String {
    match action {
        TradeAction::Buy => "Buy".green().bold().to_string(),
        TradeAction::Sell => "Sell".red().bold().to_string(),
        TradeAction::Hold => "Hold".yellow().bold().to_string(),
    }
}

fn decision_colored(decision: &Decision) -> String {
    match decision {
        Decision::Approved => "Approved".green().bold().to_string(),
        Decision::Rejected => "Rejected".red().bold().to_string(),
    }
}

fn confidence_colored(confidence: f64) -> String {
    let label = format!("{confidence:.2}");
    if confidence > 0.7 {
        label.green().to_string()
    } else if confidence >= 0.4 {
        label.yellow().to_string()
    } else {
        label.red().to_string()
    }
}

fn data_status_label(present: bool) -> String {
    if present {
        "Complete".green().to_string()
    } else {
        "Missing".dimmed().to_string()
    }
}

fn violation_label(report: Option<&RiskReport>) -> String {
    match report {
        Some(r) if r.flags_violation => "Yes".red().to_string(),
        Some(_) => "No".green().to_string(),
        None => "Unknown".dimmed().to_string(),
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

fn section_header(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n{}", title.bold().underline());
}

// ── section writers ──────────────────────────────────────────────────────

fn write_header(out: &mut String, state: &TradingState) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}",
        format!("Trading Decision: {}", state.asset_symbol)
            .bold()
            .on_bright_black()
    );
    let _ = writeln!(out, "As of: {}  |  Execution ID: {}", state.target_date, state.execution_id);

    if let Some(exec) = &state.final_execution_status {
        let _ = writeln!(
            out,
            "Decision: {}  |  Action: {}  |  Timestamp: {}",
            decision_colored(&exec.decision),
            action_colored(&exec.action),
            exec.decided_at,
        );
    }
}

fn write_executive_summary(out: &mut String, state: &TradingState) {
    section_header(out, "Executive Summary");
    match &state.final_execution_status {
        Some(exec) => {
            let _ = writeln!(out, "{}", exec.rationale);
        }
        None => {
            let _ = writeln!(out, "{}", "No execution status available.".dimmed());
        }
    }
}

fn write_trader_proposal(out: &mut String, state: &TradingState) {
    section_header(out, "Trader Proposal");
    match &state.trader_proposal {
        Some(proposal) => {
            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![
                Cell::new("Field").add_attribute(Attribute::Bold),
                Cell::new("Value").add_attribute(Attribute::Bold),
            ]);
            table.add_row(vec!["Action", &action_colored(&proposal.action)]);
            table.add_row(vec![
                "Confidence".to_owned(),
                confidence_colored(proposal.confidence),
            ]);
            table.add_row(vec![
                "Target Price".to_owned(),
                format!("{:.2}", proposal.target_price),
            ]);
            table.add_row(vec![
                "Stop Loss".to_owned(),
                format!("{:.2}", proposal.stop_loss),
            ]);
            let _ = writeln!(out, "{table}");
            let _ = writeln!(out, "\n{} {}", "Rationale:".bold(), proposal.rationale);
        }
        None => {
            let _ = writeln!(out, "{}", "Trader proposal: unavailable.".dimmed());
        }
    }
}

fn write_analyst_snapshot(out: &mut String, state: &TradingState) {
    section_header(out, "Analyst Evidence Snapshot");

    let analysts: Vec<(&str, Option<&str>, bool)> = vec![
        (
            "Fundamentals",
            state.fundamental_metrics.as_ref().map(|d| d.summary.as_str()),
            state.fundamental_metrics.is_some(),
        ),
        (
            "Sentiment",
            state.market_sentiment.as_ref().map(|d| d.summary.as_str()),
            state.market_sentiment.is_some(),
        ),
        (
            "News",
            state.macro_news.as_ref().map(|d| d.summary.as_str()),
            state.macro_news.is_some(),
        ),
        (
            "Technical",
            state.technical_indicators.as_ref().map(|d| d.summary.as_str()),
            state.technical_indicators.is_some(),
        ),
    ];

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Analyst").add_attribute(Attribute::Bold),
        Cell::new("Key Evidence").add_attribute(Attribute::Bold),
        Cell::new("Status").add_attribute(Attribute::Bold),
    ]);

    for (name, summary, present) in &analysts {
        let evidence = summary.map(|s| first_sentence(s)).unwrap_or("-");
        table.add_row(vec![
            Cell::new(name),
            Cell::new(evidence),
            Cell::new(data_status_label(*present)),
        ]);
    }
    let _ = writeln!(out, "{table}");

    // Detail blocks: full summaries below the table
    for (name, summary, present) in &analysts {
        if *present {
            if let Some(full) = summary {
                let _ = writeln!(out, "\n  {} {}", format!("[{name}]").bold(), full);
            }
        }
    }
}

fn write_research_debate(out: &mut String, state: &TradingState) {
    section_header(out, "Research Debate Summary");

    match &state.consensus_summary {
        Some(consensus) => {
            let _ = writeln!(out, "{} {}", "Consensus:".bold(), consensus);
        }
        None => {
            let _ = writeln!(out, "{}", "No consensus produced.".dimmed());
        }
    }

    // Extract last bull and bear messages from debate history
    let last_bull = state
        .debate_history
        .iter()
        .rev()
        .find(|m| m.role == "bullish_researcher");
    let last_bear = state
        .debate_history
        .iter()
        .rev()
        .find(|m| m.role == "bearish_researcher");

    if let Some(bull) = last_bull {
        let _ = writeln!(
            out,
            "{} {}",
            "Strongest Bullish Evidence:".bold(),
            first_sentence(&bull.content)
        );
    }
    if let Some(bear) = last_bear {
        let _ = writeln!(
            out,
            "{} {}",
            "Strongest Bearish Evidence:".bold(),
            first_sentence(&bear.content)
        );
    }
}

fn write_risk_review(out: &mut String, state: &TradingState) {
    section_header(out, "Risk Review");

    let personas: Vec<(&str, Option<&RiskReport>)> = vec![
        ("Aggressive", state.aggressive_risk_report.as_ref()),
        ("Neutral", state.neutral_risk_report.as_ref()),
        ("Conservative", state.conservative_risk_report.as_ref()),
    ];

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Persona").add_attribute(Attribute::Bold),
        Cell::new("Violation").add_attribute(Attribute::Bold),
        Cell::new("Assessment").add_attribute(Attribute::Bold),
        Cell::new("Adjustments").add_attribute(Attribute::Bold),
    ]);

    for (name, report) in &personas {
        let (assessment, adjustments) = match report {
            Some(r) => {
                let adj = if r.recommended_adjustments.is_empty() {
                    "None".to_owned()
                } else {
                    r.recommended_adjustments.join(", ")
                };
                (first_sentence(&r.assessment).to_owned(), adj)
            }
            None => ("Unknown".to_owned(), "Unknown".to_owned()),
        };
        table.add_row(vec![
            Cell::new(name),
            Cell::new(violation_label(*report)),
            Cell::new(assessment),
            Cell::new(adjustments),
        ]);
    }
    let _ = writeln!(out, "{table}");

    // Detail blocks: full assessments below the table
    for (name, report) in &personas {
        if let Some(r) = report {
            let _ = writeln!(out, "\n  {} {}", format!("[{name}]").bold(), r.assessment);
        }
    }
}

fn write_safety_check(out: &mut String, state: &TradingState) {
    section_header(out, "Deterministic Safety Check");

    let neutral_flag = state
        .neutral_risk_report
        .as_ref()
        .map(|r| r.flags_violation);
    let conservative_flag = state
        .conservative_risk_report
        .as_ref()
        .map(|r| r.flags_violation);
    let auto_reject = neutral_flag == Some(true) && conservative_flag == Some(true);

    let flag_str = |f: Option<bool>| -> String {
        match f {
            Some(true) => "true".red().to_string(),
            Some(false) => "false".green().to_string(),
            None => "unknown".dimmed().to_string(),
        }
    };

    let _ = writeln!(out, "  Neutral flags violation: {}", flag_str(neutral_flag));
    let _ = writeln!(
        out,
        "  Conservative flags violation: {}",
        flag_str(conservative_flag)
    );
    let auto_reject_label = if auto_reject {
        "Yes".red().bold().to_string()
    } else {
        "No".green().to_string()
    };
    let _ = writeln!(out, "  Auto-reject rule triggered: {auto_reject_label}");
}

fn write_token_usage(out: &mut String, tracker: &TokenUsageTracker) {
    section_header(out, "Token Usage Summary");

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Phase").add_attribute(Attribute::Bold),
        Cell::new("Prompt").add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
        Cell::new("Completion").add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
        Cell::new("Total").add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
        Cell::new("Duration").add_attribute(Attribute::Bold).set_alignment(CellAlignment::Right),
    ]);

    for phase in &tracker.phase_usage {
        table.add_row(vec![
            Cell::new(&phase.phase_name),
            Cell::new(phase.phase_prompt_tokens).set_alignment(CellAlignment::Right),
            Cell::new(phase.phase_completion_tokens).set_alignment(CellAlignment::Right),
            Cell::new(phase.phase_total_tokens).set_alignment(CellAlignment::Right),
            Cell::new(format_duration_ms(phase.phase_duration_ms))
                .set_alignment(CellAlignment::Right),
        ]);
    }

    // Totals row
    table.add_row(vec![
        Cell::new("TOTAL").add_attribute(Attribute::Bold),
        Cell::new(tracker.total_prompt_tokens)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(tracker.total_completion_tokens)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(tracker.total_tokens)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(""),
    ]);

    let _ = writeln!(out, "{table}");
}

fn write_disclaimer(out: &mut String) {
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", "Disclaimers".bold().dimmed());
    let disclaimer = "\
- This is AI-generated analysis for educational and research purposes only.
- Not financial advice. Market data may be incomplete or delayed.
- Past performance does not guarantee future results.";
    let _ = writeln!(out, "{}", disclaimer.dimmed());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        Decision, ExecutionStatus, FundamentalData, NewsData, PhaseTokenUsage, RiskLevel,
        RiskReport, SentimentData, TechnicalData, TradeAction, TradeProposal, TradingState,
    };

    fn minimal_state() -> TradingState {
        let mut state = TradingState::new("AAPL", "2026-04-03");
        state.final_execution_status = Some(ExecutionStatus {
            decision: Decision::Approved,
            action: TradeAction::Buy,
            rationale: "Approved based on strong fundamentals.".to_owned(),
            decided_at: "2026-04-03T12:00:00Z".to_owned(),
        });
        state.trader_proposal = Some(TradeProposal {
            action: TradeAction::Buy,
            target_price: 190.0,
            stop_loss: 175.0,
            confidence: 0.8,
            rationale: "Strong growth and momentum.".to_owned(),
        });
        state
    }

    #[test]
    fn format_final_report_contains_decision() {
        let state = minimal_state();
        let report = format_final_report(&state);
        assert!(report.contains("AAPL"));
        assert!(report.contains("2026-04-03"));
    }

    #[test]
    fn format_final_report_handles_missing_analysts_gracefully() {
        let state = minimal_state();
        let report = format_final_report(&state);
        // Should contain "Missing" for analysts that are None
        assert!(report.contains("Missing") || report.contains("missing"));
    }

    #[test]
    fn format_final_report_shows_token_totals() {
        let mut state = minimal_state();
        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Test Phase".to_owned(),
            agent_usage: vec![],
            phase_prompt_tokens: 100,
            phase_completion_tokens: 50,
            phase_total_tokens: 150,
            phase_duration_ms: 2500,
        });
        let report = format_final_report(&state);
        assert!(report.contains("Test Phase"));
        assert!(report.contains("150"));
    }

    #[test]
    fn format_final_report_contains_disclaimer() {
        let state = minimal_state();
        let report = format_final_report(&state);
        assert!(report.contains("educational"));
    }

    #[test]
    fn first_sentence_extracts_up_to_period() {
        assert_eq!(first_sentence("Hello world. More text here."), "Hello world.");
    }

    #[test]
    fn first_sentence_returns_full_string_without_boundary() {
        assert_eq!(first_sentence("No period here"), "No period here");
    }

    #[test]
    fn first_sentence_handles_abbreviation_like_patterns() {
        // "e.g." has periods not followed by whitespace, so it continues
        assert_eq!(first_sentence("Use e.g. this. Then more."), "Use e.g. this.");
    }

    #[test]
    fn format_duration_ms_formats_seconds() {
        assert_eq!(format_duration_ms(2500), "2.5s");
        assert_eq!(format_duration_ms(500), "500ms");
    }
}
```

- [ ] **Step 3: Add `pub mod report;` to `src/lib.rs`**

Add after the existing `pub mod cli;` line:

```rust
pub mod report;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib report -- -v`

Expected: All 8 report tests pass.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -- -D warnings`

Expected: No warnings.

- [ ] **Step 6: Commit**

```bash
git add src/report/mod.rs src/report/final_report.rs src/lib.rs
git commit -m "feat: add comprehensive final report module with colored terminal output"
```

---

### Task 9: Wire `format_final_report` into `main.rs`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the output block in `main.rs`**

Replace lines 127-145 (the `Ok(state) =>` arm body) with:

```rust
                Ok(state) => {
                    if state.final_execution_status.is_none() {
                        eprintln!("pipeline completed without a final execution status");
                        std::process::exit(1);
                    }
                    println!("{}", scorpio_analyst::report::format_final_report(&state));
                }
```

- [ ] **Step 2: Run build**

Run: `cargo build`

Expected: Compiles without errors.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`

Expected: All tests pass.

- [ ] **Step 4: Run clippy and fmt check**

Run: `cargo clippy -- -D warnings && cargo fmt -- --check`

Expected: Clean.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire format_final_report into CLI output"
```

---

### Task 10: Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo nextest run --all-features `

Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`

Expected: No warnings.

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt -- --check`

Expected: No formatting issues.

- [ ] **Step 4: Verify NO_COLOR behavior**

Run: `NO_COLOR=1 cargo build`

Expected: `colored` respects the `NO_COLOR` env var automatically at runtime. The code compiles identically; at runtime with `NO_COLOR=1`, ANSI escape codes are suppressed.
