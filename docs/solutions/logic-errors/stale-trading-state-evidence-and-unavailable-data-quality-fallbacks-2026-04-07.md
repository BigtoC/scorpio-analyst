---
title: Fix stale evidence state reuse and unavailable data-quality fallbacks
date: 2026-04-07
category: logic-errors
module: workflow-pipeline-prompts
problem_type: logic_error
component: assistant
symptoms:
  - reused TradingState runs retained stale evidence and reporting fields from a previous cycle
  - prompt data-quality context rendered synthetic empty arrays when coverage and provenance were absent
  - downstream agent prompts could consume misleading prior-cycle or not-yet-derived context
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - testing_framework
tags:
  - trading-state
  - state-reset
  - prompt-rendering
  - stale-state
  - data-coverage
  - provenance-summary
  - prompt-fallback
  - cycle-reset
---

# Fix stale evidence state reuse and unavailable data-quality fallbacks

## Problem
The `chunk3-evidence-state-sync` change left two final defects after the main implementation had already landed. The pipeline reset logic did not clear the newly added evidence/reporting fields between runs, and the shared prompt builder treated absent coverage/provenance as empty arrays instead of explicitly unavailable data.

That combination could leak stale evidence into a later reused `TradingState` run and hide the difference between "not derived yet" and "present but empty" in downstream agent prompts.

## Symptoms
- A reused pipeline run could keep stale `evidence_technical` from an earlier cycle when the next cycle continued with a missing technical analyst result.
- `build_data_quality_context()` rendered `[]` for absent `data_coverage` and `provenance_summary`, which made missing derived state look like valid empty output.
- The original reused-state integration test passed even though the bug still existed, because the all-success stub fanout overwrote the stale fields before assertions ran.

## What Didn't Work
- Reusing the existing all-success integration test was a false negative. It exercised state reuse, but not the one-missing-input continue path where stale evidence could survive.
- The bug only reproduced after adding a crate-local regression in `src/workflow/pipeline/tests.rs` with a custom analyst fanout that omitted the technical result while still allowing the pipeline to continue.

## Solution
Update `src/workflow/pipeline/runtime.rs::reset_cycle_outputs()` so it clears every per-cycle evidence/reporting field, not just the legacy analyst outputs:

```rust
state.evidence_fundamental = None;
state.evidence_technical = None;
state.evidence_sentiment = None;
state.evidence_news = None;
state.data_coverage = None;
state.provenance_summary = None;
```

Update `src/agents/shared/prompt.rs::build_data_quality_context()` to emit explicit unavailable markers for absent derived state while preserving the required output keys:

```rust
fn unavailable() -> String {
    "unavailable".to_owned()
}

let required_inputs = state.data_coverage.as_ref().map_or_else(unavailable, |c| {
    sanitize_prompt_context(
        &serde_json::to_string(&c.required_inputs).unwrap_or_else(|_| "[]".to_owned()),
    )
});
```

Add regression coverage around the real failure mode: a reused-state pipeline test with a partial analyst fanout, shared prompt tests for `unavailable`, and downstream prompt-boundary assertions in researcher, risk, trader, and fund manager tests.

## Why This Works
`TradingState` is intentionally reused across pipeline runs, so every cycle-scoped output must be reset before a new run begins. Leaving `evidence_*`, `data_coverage`, and `provenance_summary` intact meant a later cycle could inherit prior-cycle values when one analyst result was missing. Clearing them in `reset_cycle_outputs()` restores run isolation.

The prompt fix restores the semantic distinction between absent and empty derived data. `[]` means a derived list is present but empty; `unavailable` means the system does not currently have that report. That matches the OpenSpec contract and gives downstream agent prompts more honest state.

## Prevention
- When adding a new per-cycle `TradingState` field, update `src/workflow/pipeline/runtime.rs::reset_cycle_outputs()` in the same change.
- Prefer partial-failure regressions over all-success stub pipelines when testing reused state.
- Do not serialize missing derived prompt data as empty collections unless missing and empty are intentionally equivalent.
- Keep shared prompt tests and downstream prompt-boundary tests aligned so contract drift fails quickly.
- Re-run the standard verification commands for state/prompt contract changes:
  - `cargo fmt -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo nextest run --all-features --locked`
  - `cargo build`
  - `openspec validate chunk3-evidence-state-sync --strict`

## Related Issues
- OpenSpec change: `chunk3-evidence-state-sync`
- Related follow-up learning: `docs/solutions/logic-errors/thesis-memory-untrusted-context-boundary-2026-04-09.md`
