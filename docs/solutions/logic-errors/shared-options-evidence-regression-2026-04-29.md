---
title: Fix shared options evidence contract drift for live vs non-live options outcomes
date: 2026-04-29
category: logic-errors
module: equity-options-evidence
problem_type: logic_error
component: assistant
symptoms:
  - Technical analyst output could copy raw `get_options_snapshot` JSON into `options_summary` instead of a short interpretation.
  - Fund Manager guidance could treat non-`snapshot` options outcomes as live options evidence.
  - The prompt-bundle regression gate failed after the prompt contract changed until `technical_analyst.txt` was regenerated with the exact rendered bytes.
root_cause: logic_error
resolution_type: code_fix
severity: medium
related_components:
  - documentation
  - testing_framework
tags:
  - options-evidence
  - prompt-contract
  - technical-analyst
  - fund-manager
  - prompt-bundle
  - regression-fixture
  - golden-bytes
---

# Fix shared options evidence contract drift for live vs non-live options outcomes

## Problem

Shared options evidence had drifted across the technical analyst prompt,
runtime validation, and Fund Manager prompt guidance.

`TechnicalData.options_summary` was intended to be supplemental analyst prose,
but the contract had drifted close to treating it as a transport for raw
`get_options_snapshot` payloads. At the same time, downstream prompt guidance
needed to distinguish live `snapshot` evidence from non-live options outcomes
such as `historical_run` and `sparse_chain`.

## Symptoms

- `crates/scorpio-core/src/agents/analyst/equity/technical.rs` could accept
  `options_summary` values that looked like raw JSON instead of analyst-written
  prose.
- The technical analyst system prompt did not clearly forbid copying raw
  `get_options_snapshot` JSON into `options_summary`.
- `validate_technical` sanitized `summary` but did not apply the same guard to
  `options_summary`.
- `crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md`
  needed to treat only `technical_report.options_context.outcome.kind ==
  snapshot` as live structured options evidence.
- `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` compares golden
  fixtures byte-for-byte, so the prompt fixture had to be regenerated through
  the harness after the contract changed.

## What Didn't Work

- Manually editing
  `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt`
  was not enough once the prompt contract changed. The regression gate asserts
  exact rendered bytes, so the fixture had to be regenerated through the test
  harness instead of hand-edited.

```bash
UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
```

## Solution

- In `crates/scorpio-core/src/agents/analyst/equity/technical.rs`,
  `assemble_technical_data` now preserves `options_summary` only when
  `options_context` is
  `Some(TechnicalOptionsContext::Available { outcome: OptionsOutcome::Snapshot(_) })`.
  Non-snapshot outcomes clear it before persistence.
- The same file's technical prompt now explicitly says:
  - `options_summary` is a brief interpretation of a live snapshot
  - omit it entirely for `historical_run`, `sparse_chain`,
    `no_listed_instrument`, `missing_spot`, or unavailable-tool paths
  - do not copy raw tool JSON into the field
- `validate_technical` now calls
  `validate_summary_content("TechnicalAnalyst options_summary", ...)`,
  matching the sanitizer already used for `summary`.
- `crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md` now
  instructs the model to inspect `technical_report.options_context.outcome.kind`
  and treat only `snapshot` as live evidence. `historical_run`, `sparse_chain`,
  `no_listed_instrument`, and `missing_spot` are named explicitly as
  unavailable or low-confidence outcomes.
- `OptionsToolContext` re-exports were removed from
  `crates/scorpio-core/src/data/mod.rs` and
  `crates/scorpio-core/src/data/yfinance/mod.rs`. The technical analyst path now
  imports the type directly from `crate::data::yfinance::options`, keeping the
  seam local to the options flow. This was adjacent cleanup that reinforced the
  intended boundary; the main behavioral fix was the prompt/validation/outcome
  contract above.
- The technical prompt golden fixture at
  `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt` was
  regenerated through the regression harness so the stored bytes match the
  intentional prompt contract exactly.

Prompt-contract shift:

Before:

```json
{
  "options_summary": "{\"kind\":\"snapshot\",\"spot_price\":150.0,\"atm_iv\":0.25}"
}
```

After:

```json
{
  "options_summary": "Near-term IV remains elevated into earnings."
}
```

For non-snapshot outcomes:

```json
{
  "options_summary": null
}
```

Regression coverage added or tightened around:

- `build_technical_system_prompt_includes_options_guidance_when_tool_available`
- `validate_technical_rejects_control_chars_in_options_summary`
- `fund_manager_prompt_names_non_snapshot_options_outcomes_as_unavailable`
- `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`

Verification recorded with the fix:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

Final result: `1588 passed, 3 skipped`.

## Why This Works

The underlying issue was contract drift. The Rust-owned `options_context`
already knew whether the options data represented a live snapshot, but prompt
text, validation, and downstream consumers were not all enforcing that same
boundary.

This fix makes one source of truth explicit across phases:

- `options_context.outcome.kind` decides whether live options evidence exists
- `options_summary` is supplemental prose, not transport for tool payloads
- the same summary sanitizer now covers both `summary` and `options_summary`
- downstream prompts no longer infer validity from a free-form string

Keeping `OptionsToolContext` local also reduces accidental coupling. The
prefetch/tool seam stays inside the technical analyst flow instead of becoming a
broader `data` module contract.

## Prevention

- Keep `options_context` authoritative and `options_summary` supplemental. Any
  new consumer should branch on
  `technical_report.options_context.outcome.kind`, not on whether
  `options_summary` happens to be non-empty.
- Reuse `validate_summary_content` for any new human-readable summary fields
  that cross workflow phases.
- Update shared evidence-field behavior in one change across prompt text,
  runtime validation, and downstream consumer logic. If any one of those lags,
  the system will drift back into conflicting interpretations.
- When changing prompt text covered by golden fixtures, regenerate through the
  harness instead of hand-editing fixture files:

  ```bash
  UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
  ```

- Review regenerated files under
  `crates/scorpio-core/tests/fixtures/prompt_bundle/` and confirm only the
  intended prompt drift changed.
- Preserve regression tests that lock the contract:
  - snapshot preserves `options_summary`
  - non-snapshot clears it
  - control characters are rejected
  - Fund Manager guidance names non-snapshot outcomes and checks `outcome.kind`

## Related Issues

- Related learning:
  `docs/solutions/logic-errors/prompt-bundle-centralization-runtime-contract-2026-04-25.md`
- Related learning:
  `docs/solutions/logic-errors/fund-manager-dual-risk-prefix-contract-follow-up-fixes-2026-04-21.md`
- Related learning:
  `docs/solutions/logic-errors/stale-trading-state-evidence-and-unavailable-data-quality-fallbacks-2026-04-07.md`
- GitHub issue search skipped: `gh` is not installed in this environment.
