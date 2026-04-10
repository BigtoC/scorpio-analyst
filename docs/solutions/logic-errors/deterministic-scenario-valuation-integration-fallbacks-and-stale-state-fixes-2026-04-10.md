---
title: Fix deterministic scenario valuation integration fallbacks and stale-state leakage
date: 2026-04-10
category: logic-errors
module: deterministic-scenario-valuation-integration
problem_type: logic_error
component: assistant
symptoms:
  - trader/provider-visible output exposed runtime-owned `scenario_valuation` data
  - trader, fund manager, and final report could mishandle `not computed` or `not assessed` valuation states
  - reused pipeline state could surface stale trader proposal valuation fields on later runs
  - final report could include raw model-authored valuation text when deterministic valuation was unavailable
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - documentation
  - testing_framework
tags:
  - deterministic-valuation
  - scenario-valuation
  - trader
  - fund-manager
  - final-report
  - stale-state
  - prompt-tests
  - rust
---

# Fix runtime-owned `scenario_valuation` boundaries and stale valuation regressions

## Problem
The deterministic scenario valuation integration still left a runtime boundary exposed after the main feature landed. The trader-facing schema allowed the LLM to author `scenario_valuation`, even though the runtime was supposed to stamp that field from `state.derived_valuation`.

This follow-up is about downstream integration and consumption bugs, not the upstream valuation derivation and runtime math covered in the earlier deterministic valuation learning.

## Symptoms
- Trader output schema allowed provider-visible `scenario_valuation`, so the model could author a runtime-owned field.
- Reused-run `trader_proposal` state could retain stale valuation data even when current runtime valuation changed or was absent.
- Prompts handled `not assessed` but not the separate `not computed` state, leaving incomplete guidance for unavailable deterministic valuation.
- Fund-manager prompt order buried deterministic valuation behind lower-priority context.
- Final reports could surface model-authored valuation text that contradicted runtime valuation availability.

## What Didn't Work
- Exposing `scenario_valuation` in the LLM-visible trader response shape. Prompt instructions alone were not enough to enforce runtime ownership.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TradeProposal {
    // ...
    pub scenario_valuation: Option<ScenarioValuation>,
}
```

- Treating only `NotAssessed` as a fallback case. That still left ambiguous behavior when valuation was absent because it had not been computed at all.
- Letting the final report render raw model-authored valuation text even when deterministic valuation was unavailable. That preserved contradictions instead of suppressing them.
- Extending reused-run coverage only around stale `derived_valuation`. That missed the parallel stale-state path through `trader_proposal.scenario_valuation` and `valuation_assessment`.

## Solution
Add a trader-only response type that omits `scenario_valuation` entirely, reject unknown fields, convert that response into the runtime `TradeProposal`, and stamp `scenario_valuation` only after deserialization.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TraderProposalResponse {
    pub action: TradeAction,
    pub target_price: f64,
    pub stop_loss: f64,
    pub confidence: f64,
    pub rationale: String,
    #[serde(default)]
    pub valuation_assessment: Option<String>,
}
```

```rust
let llm_proposal: TradeProposal = outcome.result.output.into();
let mut proposal = llm_proposal;
proposal.scenario_valuation = state
    .derived_valuation
    .as_ref()
    .map(|valuation| valuation.scenario.clone());
```

Support that boundary everywhere deterministic valuation appears:
- Added `src/agents/trader/schema.rs` for the provider-visible trader response contract.
- Updated trader and fund-manager prompts to explicitly handle both `not assessed` and `not computed` valuation states.
- Moved deterministic valuation earlier in the fund-manager user prompt so it is prioritized before trader proposal and analyst/risk context.
- Moved shared valuation prompt rendering into `src/agents/shared/valuation_prompt.rs` and added sanitization coverage for hostile `NotAssessed.reason` values.
- Updated `src/report/final_report.rs` to suppress model-authored valuation text when deterministic valuation is unavailable or `NotAssessed`.
- Extended `tests/workflow_pipeline_e2e.rs` to cover stale reused-run `trader_proposal` valuation fields, not just stale `derived_valuation`.
- Corrected `src/state/proposal.rs` documentation so `scenario_valuation` semantics match runtime behavior: `None` means not computed; `Some(NotAssessed { .. })` means valuation was computed and deemed inapplicable.

Because each run now rebuilds the proposal from fresh LLM output and runtime-stamped valuation state, stale `valuation_assessment` and `scenario_valuation` fields from earlier cycles cannot leak forward unchanged.

Verification passed:
- `cargo fmt -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo nextest run --all-features --locked` (`998` tests)

## Why This Works
The fix turns runtime ownership from a convention into an enforced boundary. Once `scenario_valuation` is removed from the provider-visible schema and `deny_unknown_fields` is enabled, the model can no longer legally supply that field. The only source of truth becomes runtime state stamped from `state.derived_valuation`.

The prompt updates close the semantic gap between two different absence states: `None` means valuation was not computed for this run, while `Some(NotAssessed { .. })` means valuation was computed but is not applicable for the asset shape. The report suppression rules then prevent operator-facing contradictions when deterministic valuation is unavailable.

## Prevention
- Keep runtime-owned fields out of provider-visible schemas. If runtime must own a field, omit it from the LLM response type instead of ignoring it later.
- Use `#[serde(deny_unknown_fields)]` on model response structs that define hard schema boundaries.
- Keep explicit prompt and report coverage for both valuation absence states:
  - not computed (`None`)
  - not assessed (`Some(NotAssessed { .. })`)
- Test both stale-state paths on reused `TradingState` runs:
  - stale `derived_valuation`
  - stale `trader_proposal.scenario_valuation` and `valuation_assessment`
- Treat free-form explanatory fields like `NotAssessed.reason` as untrusted data and sanitize them before prompt or report rendering.
- Keep state documentation aligned with runtime semantics so prompt, report, and test work stays anchored to the same contract.

## Related Issues
- Related learning: `docs/solutions/logic-errors/deterministic-valuation-derivation-fixes-2026-04-10.md`
- Related learning: `docs/solutions/logic-errors/stale-trading-state-evidence-and-unavailable-data-quality-fallbacks-2026-04-07.md`
