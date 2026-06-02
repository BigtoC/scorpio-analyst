---
title: A New Prompt Placeholder Must Be Registered in KNOWN_PLACEHOLDERS or Preflight Fails Closed
date: 2026-06-02
category: docs/solutions/logic-errors
module: prompts/validation + analysis_packs prompts + workflow/tasks/preflight
problem_type: logic_error
component: assistant
symptoms:
  - "Every full-pipeline run fails at preflight: \"active pack Baseline is incomplete under runtime topology: ... is missing 1 required prompt slot(s): fund_manager\"."
  - "Unit tests and pack drift tests that string-match the template (`fm.contains(\"{account_positions}\")`) pass; only the e2e/app_runtime suite (which runs PreflightTask) catches the failure."
  - "The newly added prompt template still renders correctly in isolation — the slot is non-empty — yet completeness validation reports it empty."
root_cause: missing_include
resolution_type: code_fix
severity: high
tags:
  - preflight
  - prompt-bundle
  - known-placeholders
  - completeness-check
  - fail-closed
  - prompt-template
  - test-coverage-gap
  - futu
---

# A New Prompt Placeholder Must Be Registered in KNOWN_PLACEHOLDERS or Preflight Fails Closed

## Problem

Adding a new `{placeholder}` token to a pack-owned prompt template (e.g.
`analysis_packs/equity/prompts/fund_manager.md`) without also registering it in
the `KNOWN_PLACEHOLDERS` allowlist makes **every** full-pipeline run fail at
PreflightTask — even though the template is correct and the placeholder is
substituted properly at render time.

## Symptoms

```
graph-flow error in phase 'preflight' task 'preflight': Task execution failed:
PreflightTask: active pack Baseline is incomplete under runtime topology:
active pack Baseline is missing 1 required prompt slot(s): fund_manager
```

- The colocated unit tests and the per-pack drift tests pass, because they only
  assert `prompt_bundle.fund_manager.contains("{account_positions}")` (a string
  match on the template), which says nothing about completeness validation.
- The golden-fixture `prompt_bundle_regression_gate` also passes — it renders and
  compares bytes, it does not run preflight.
- Only the integration suite that actually executes the pipeline
  (`tests/app_runtime.rs`, `tests/workflow_pipeline_e2e.rs`,
  `workflow_observability_pipeline`, etc.) exercises `PreflightTask` and surfaces
  the failure.

## What Didn't Work

- Inspecting `resolve_pack(PackId::Baseline).prompt_bundle.fund_manager` — it
  contains the new placeholder and looks healthy, which is misleading. The slot
  content is fine; the *validation verdict* about it is what changed.
- Assuming the regenerated golden fixtures or a schema bump were the cause — the
  fixture gate and schema are unrelated to this failure.

## Solution

Register the new token in the closed allowlist and bump its size-lock test.

`crates/scorpio-core/src/prompts/validation.rs`:

```rust
const KNOWN_PLACEHOLDERS: &[&str] = &[
    // ...
    "{current_price}",
    // Fund-manager read-only account-position context (Futu integration);
    // replaced at prompt-render time, empty when disabled/unavailable.
    "{account_positions}",
    // ...
];

// in tests:
assert_eq!(KNOWN_PLACEHOLDERS.len(), 30); // was 29
assert!(KNOWN_PLACEHOLDERS.contains(&"{account_positions}"));
```

## Why This Works

`is_effectively_empty(slot)` (the function `validate_active_pack_completeness`
calls for each required slot) is **fail-closed** on unknown placeholders:

```rust
pub fn is_effectively_empty(slot: &str) -> bool {
    if slot.trim().is_empty() { return true; }
    if contains_unknown_placeholder_token(slot) { return true; } // <-- here
    let mut stripped = slot.to_string();
    for token in KNOWN_PLACEHOLDERS { stripped = stripped.replace(token, ""); }
    stripped.trim().is_empty()
}
```

`contains_unknown_placeholder_token` flags any identifier-style `{...}` token not
in `KNOWN_PLACEHOLDERS` and treats it as a pack-author typo (e.g.
`{ticker_symbol}`), returning `true` so the typo fails completeness validation
rather than rendering verbatim into an LLM prompt. A *legitimate* new placeholder
looks identical to a typo until it is added to the allowlist — so an unregistered
placeholder marks the entire slot "effectively empty," and PreflightTask reports
the slot missing.

## Prevention

Treat `KNOWN_PLACEHOLDERS` as part of the contract for any prompt-template edit:

- **When adding a `{placeholder}` to any pack prompt, add it to
  `KNOWN_PLACEHOLDERS` in the same change** (and update the size-lock test). The
  allowlist is deliberately closed so typos fail loudly; new tokens must opt in.
- **Don't rely on `.contains()` drift tests to prove a placeholder works.** A
  string-match test is necessary but not sufficient — it cannot observe the
  fail-closed completeness check. Keep at least one test that runs the real
  pipeline path (preflight) so placeholder regressions are caught pre-merge. In
  this repo that is the `--all-features` e2e suite; run it, not just `-p
  scorpio-core <unit-filter>`, before declaring prompt work done.
- A fast targeted guard: `cargo nextest run -p scorpio-core --all-features
  run_analysis_cycle_success_path_populates_all_phases` exercises preflight end to
  end.

## Related

- [[prompt-bundle-centralization-runtime-contract-2026-04-25]] — the centralization
  that made the prompt bundle + runtime topology the source of required slots.
- [[etf-runtime-policy-preseed-preflight-contract-2026-05-22]] — PreflightTask as the
  sole authority for runtime surfaces; same preflight-contract area.

## Appendix — other non-obvious facts from the Futu integration

These came up in the same feature and are worth a future searcher's time:

- **Futu OpenD `plRatio` is a wire percentage, not a fraction.** `Trd_Common.proto`
  states "plRatio 等于 8.8 代表涨 8.8%". The domain stores `pl_ratio` as a fraction
  (`0.236`), so `assemble_snapshot` divides the wire value by 100; render code then
  multiplies back by 100. Confirmed against the bundled proto and a live OpenD spike.
- **OpenD JSON mode works for the read-only protos** (`InitConnect 1001`,
  `Trd_GetAccList 2001`, `Trd_GetPositionList 2102`) with `nProtoFmtType=1`,
  `packetEncAlgo=-1` (no encryption), and `GetAccList` accepts `userID=0` (the real
  `loginUserID` is not required). `accID`/`loginUserID` arrive as JSON *strings* but
  OpenD accepts `accID` back as a *number* in `TrdHeader` — handle both with a
  flexible u64 deserializer. Frame = 44-byte LE header (`FT` magic, protoID,
  fmt/ver, serial, bodyLen, 20-byte body SHA-1, 8 reserved) + JSON body.
- **Additive `#[serde(default)]` state fields keep `JsonReport.schema_version` at 2.**
  A new optional `TradingState` field that defaults on legacy snapshots does not
  require a schema bump — mirrors the `json_reporter_keeps_v2_for_additive_etf_profile_fields`
  precedent. Add a legacy-load test (drop the key, assert it deserializes to the
  default) to prove it.
