---
title: Fix Fund Manager dual-risk prefix contract drift and missing-input fixture ambiguity
date: 2026-04-21
category: logic-errors
module: fund-manager-dual-risk-contract
problem_type: logic_error
component: assistant
symptoms:
  - Fund Manager prompt wording could drift from the strict validator's byte-for-byte dual-risk first-line contract
  - `DualRiskStatus::Absent` responses could still carry a fabricated `Dual-risk escalation:` first-line prefix without an explicit regression guard
  - shared missing-input fixtures blurred the difference between missing-risk `Unknown` behavior and missing-analyst `Absent` behavior
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - documentation
  - testing_framework
tags:
  - fund-manager
  - dual-risk
  - prompt-contract
  - validator
  - test-fixtures
  - docs-sync
  - rationale-prefix
---

# Fix Fund Manager dual-risk prefix contract drift and missing-input fixture ambiguity

## Problem
The initial Fund Manager dual-risk escalation rollout left a follow-up correctness gap between the prompt contract, the runtime validator, and the test fixtures that exercised missing-input behavior.

The validator in `src/agents/fund_manager/validation.rs` enforced an exact first-line prefix contract, but the surrounding prompt/tests/docs still needed to be tightened so the LLM instructions, `Absent` behavior, and fixture semantics all described the same runtime truth.

## Symptoms
- The system prompt could describe the dual-risk first-line requirement less strictly than the validator actually enforced.
- `DualRiskStatus::Absent` needed an explicit regression test proving a fabricated `Dual-risk escalation:` prefix is invalid, even when other missing-data acknowledgment text is present.
- Missing-risk and missing-analyst paths were sharing similar fixtures, which risked collapsing two distinct behaviors: `Unknown` requires the `indeterminate` prefix, while `Absent` must not emit any dual-risk escalation prefix.

## What Didn't Work
- Treating the prompt examples as "close enough" to the validator contract. The runtime validator is case-sensitive and prefix-sensitive, so loose wording invites prompt drift.
- Reusing a generic missing-data fixture across both missing-risk and missing-analyst cases. That obscured whether a passing test was validating `DualRiskStatus::Unknown` or `DualRiskStatus::Absent` semantics.

## Solution
Align the Fund Manager system prompt with the validator contract byte-for-byte and keep the exact wording mirrored in `docs/prompts.md`:

```rust
2. Check the `Dual-risk escalation:` indicator at the top of the user context. \
When it is `present` ... your first rationale line MUST begin with one of: \
`Dual-risk escalation: upheld because ` ... \
`Dual-risk escalation: deferred because ` ... \
`Dual-risk escalation: overridden because ` ... \
When it is `unknown` ... start the first line with: \
`Dual-risk escalation: indeterminate because `. \
When it is `absent`, no first-line prefix is required.
Emit the prefix byte-for-byte. Do not use markdown fences, lowercase variants, \
mixed-case variants, or em-dashes.
```

Keep the validator strict about both `Absent` and `Unknown` behavior:

```rust
if dual_risk_status == DualRiskStatus::Absent && first_line.starts_with(ESCALATION_PREFIX) {
    return Err(TradingError::SchemaViolation {
        message: "FundManager: dual-risk escalation absent — rationale must not use a dual-risk escalation first-line prefix"
            .to_owned(),
    });
}

const REQUIRED_PREFIX: &str = "Dual-risk escalation: indeterminate because ";
```

Split the fixtures and tests so each missing-input path asserts the right contract:
- Missing risk reports continue through the `Unknown` path and require the `indeterminate` prefix.
- Missing analyst inputs continue through the `Absent` path and must acknowledge missing data without fabricating a dual-risk escalation first line.
- Add a focused regression test for `DualRiskStatus::Absent` rejecting a first-line `Dual-risk escalation:` prefix.

Sync the narrative docs with runtime behavior:
- `src/agents/fund_manager/prompt.rs`
- `docs/prompts.md`
- `PRD.md`

Verification recorded with the fix:
- `cargo fmt -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo nextest run --all-features --locked --no-fail-fast`

## Why This Works
This change removes ambiguity from a contract that is intentionally stricter than normal free-form rationale validation. The LLM sees the exact prefixes the validator expects, the validator rejects fabricated escalation prefixes when the status is actually `Absent`, and the fixtures now preserve the semantic boundary between "risk reports missing" and "analyst inputs missing."

That keeps three surfaces aligned: prompt instructions, runtime enforcement, and regression coverage. Once those surfaces agree, a future prompt tweak or fixture reuse mistake is much more likely to fail immediately in tests instead of silently redefining the contract.

## Prevention
- When a validator enforces byte-for-byte prompt output, copy the exact accepted strings into the system prompt and prompt docs rather than paraphrasing them.
- Keep separate fixtures for superficially similar missing-input states when they map to different enums or validation branches.
- Add explicit negative tests for forbidden-but-plausible model output, especially fabricated prefixes that look valid to a human reviewer.
- Update runtime prompt text, prompt documentation, and product docs in the same change whenever a model-output contract changes.

## Related Issues
- Related learning: `docs/solutions/logic-errors/stale-trading-state-evidence-and-unavailable-data-quality-fallbacks-2026-04-07.md`
- Related learning: `docs/solutions/logic-errors/deterministic-scenario-valuation-integration-fallbacks-and-stale-state-fixes-2026-04-10.md`
- Related plan: `docs/superpowers/plans/2026-04-20-fund-manager-dual-risk-escalation.md`
- GitHub issue search skipped: `gh` is not installed in this environment.
