# Fund Manager Dual-Risk Escalation Design

**Date:** 2026-04-20
**Status:** Approved

## Goal

Replace the current deterministic "Conservative + Neutral dual violation => immediate rejection" behavior with a judgment-based design where the Fund Manager always uses the deep-thinking LLM and treats the dual-risk condition as a high-severity input rather than an automatic outcome.

## Problem

The current implementation and docs still encode the older safety-net behavior in multiple places:

- `src/agents/fund_manager/agent.rs` short-circuits to a deterministic `Rejected + Hold` result without calling the LLM.
- `src/agents/fund_manager/prompt.rs` instructs the Fund Manager to reject when both Conservative and Neutral set `flags_violation = true`.
- `src/agents/risk/moderator.rs` tells the Risk Moderator to frame the dual-risk condition as a deterministic rejection trigger.
- Current tests and some current-facing docs/specs still assert the bypass path.

This conflicts with the desired product behavior: the Fund Manager should remain the final decision-maker after reviewing the trader proposal, risk reports, analyst outputs, valuation context, and prior learnings.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Dual-risk semantics | High-severity escalation signal | Preserve the seriousness of the signal without converting it into an auto-veto |
| Dual-risk audit contract | Require a dedicated dual-risk first line in `rationale`; require override explanation for non-rejection outcomes | Keep approvals and holds audit-distinguishable from ordinary cases without adding schema |
| Prompt framing | Serious but non-mandatory escalation | Prevent the old deterministic rule from surviving as soft prompt pressure |
| Fund Manager execution path | Always call the deep-thinking LLM | Keep final judgment with the Fund Manager on every fully-formed run |
| `ExecutionStatus` schema | No changes | Existing `decision`, `action`, `rationale`, and timing fields already express the outcome |
| Risk Moderator wording | Explicitly note the dual-risk condition without calling it deterministic rejection | Keep the signal visible downstream while removing policy drift |
| Validation scope | Keep existing schema and missing-data validation, plus a narrow dual-risk rationale contract when both flags are true | Enforce minimum audit semantics without adding new structured fields |
| Workflow topology | No new tasks or branches | This is a policy correction inside the existing Phase 5 boundary |

## Architecture

The Fund Manager remains the terminal decision-maker in Phase 5. The dual `flags_violation` signal from the Conservative and Neutral risk agents is still preserved and emphasized, but it no longer controls execution flow on its own.

Phase 4 continues to produce three `RiskReport` objects plus a moderator synthesis. Phase 5 continues to produce one `ExecutionStatus`. The only architectural change is the meaning of the dual-risk signal:

- before: deterministic bypass of the LLM
- after: high-severity evidence that the Fund Manager must weigh in its final judgment

No new state type, no schema expansion, and no extra workflow phase are needed.

## Dual-Risk Decision Contract

Prompt assembly must expose a stable dual-risk indicator derived from the typed risk reports:

- `Dual-risk escalation: present` when both Conservative and Neutral reports exist and both set `flags_violation = true`
- `Dual-risk escalation: absent` when both reports exist and the dual-violation condition is not met
- `Dual-risk escalation: unknown` when either the Conservative or Neutral report is missing

The dedicated rationale contract applies only when the indicator is `present`.

When `Dual-risk escalation: present`:

- `ExecutionStatus.rationale` must use a dedicated first line beginning exactly with `Dual-risk escalation:`
- that first line must use one of these exact forms:
  - `Dual-risk escalation: upheld because <blocking reason>`
  - `Dual-risk escalation: deferred because <specific unresolved objection or gating condition>`
  - `Dual-risk escalation: overridden because <concrete counter-evidence and risk mitigation>`
- the required form depends on the final outcome:
  - `decision = Rejected` -> `upheld because`
  - `decision = Approved` and `action = Hold` -> `deferred because`
  - `decision = Approved` and `action = Buy` or `Sell` -> `overridden because`
- if `decision = Rejected`, `action` must be either `Hold` or the opposite direction of the trader proposal; rejecting with the same direction as the trader proposal is invalid for this dual-risk contract because it does not communicate a meaningful review result

This yields the minimum outcome matrix for dual-risk cases:

- `Rejected + Hold` or `Rejected + opposite direction` means the escalation was upheld and the current proposal was rejected
- `Approved + Hold` means the escalation prevented immediate execution but the fund manager kept a cautious thesis alive
- `Approved + Buy` or `Approved + Sell` is an override outcome and requires unusually strong justification grounded in concrete counter-evidence and explicit mitigation

The exact first line is a classification header for validation and audit filtering. The remaining rationale body stays free-form and should contain the fuller explanation.

When `Dual-risk escalation: unknown`, the rationale must still satisfy the existing missing-input acknowledgment rule, but no dedicated dual-risk first line is required because the dual condition cannot be evaluated.

The prompt must not frame dual-risk as mandatory rejection or presumptive rejection. It may frame it as an elevated-justification case.

This preserves the seriousness of the dual-risk condition in a way that remains reviewable and testable without adding a new output field.

## Component Changes

### `src/agents/fund_manager/agent.rs`

- Remove the deterministic early-return branch that currently writes `Rejected + Hold` without model inference.
- Always build prompt context and call the deep-thinking Fund Manager model.
- Compute a private per-run decision context from `TradingState` before prompt building and response validation. That context should include:
  - dual-risk indicator (`present` / `absent` / `unknown`)
  - trader proposal action
- Keep the existing runtime timestamp overwrite, state write, and token accounting behavior.

### `src/agents/fund_manager/validation.rs`

- Remove the deterministic-reject helper and the hardcoded deterministic-reject rationale constant.
- Keep JSON parsing, missing-data acknowledgment checks, and runtime timestamp normalization.
- Add a narrow dual-risk validation contract driven by typed inputs:
  - when the dual-risk indicator is `present`, `rationale` must use the dedicated first line in one of the exact forms defined above
  - the validator must enforce the required form from the final `decision` / `action` combination
  - the validator must use trader proposal action from the same per-run decision context to reject `decision = Rejected` plus same-direction `action`
  - when the indicator is `absent` or `unknown`, no dedicated dual-risk first line is required
- Wire this through an explicit private validation input, e.g. `FundManagerDecisionContext`, passed from `agent.rs` into `parse_and_validate_execution_status()`; do not recompute from raw model output alone
- Keep the rest of `ExecutionStatus` validation unchanged.

### `src/agents/fund_manager/prompt.rs`

- Replace the current instruction that says the Fund Manager must reject when both Conservative and Neutral flag a material violation.
- Reword it so the dual-risk condition is treated as a high-severity signal that must be weighed explicitly in the rationale, alongside the trader proposal, analyst outputs, valuation context, and prior learnings.
- Add a canonical prompt line derived from typed reports using the exact `present` / `absent` / `unknown` values, so the Fund Manager always receives an unambiguous signal even if moderator prose changes.
- In dual-risk-present cases, require the rationale to use the exact dedicated first-line `Dual-risk escalation:` form appropriate to the chosen outcome.
- Explicitly forbid wording that presents dual-risk as deterministic rejection or as a presumed default rejection.
- Place the `Dual-risk escalation: <value>` indicator near the top of the user prompt, before long serialized context blocks, so it survives the current prompt-size budget.
- Keep the exact rationale-form instruction in the system prompt so it is not truncated by `MAX_USER_PROMPT_CHARS`.
- Preserve the existing instruction that the Fund Manager may return any valid `ExecutionStatus` it endorses after review.

### `src/agents/risk/moderator.rs` and `src/agents/risk/common.rs`

- Replace the current binary moderator sentence with the same tri-state signal used by the Fund Manager path:
  - `Violation status: dual-risk escalation present.`
  - `Violation status: dual-risk escalation absent.`
  - `Violation status: dual-risk escalation unknown due to missing Conservative or Neutral report.`
- Remove wording that claims the Fund Manager uses the condition as a deterministic rejection rule.
- Reframe the sentence as downstream escalation context for the Fund Manager.

### Current docs and specs

For this change, treat only the following as the canonical current-behavior sources to update:

- `src/agents/fund_manager/mod.rs`
- `src/agents/fund_manager/agent.rs`
- `src/agents/fund_manager/validation.rs`
- `src/agents/fund_manager/prompt.rs`
- `src/agents/risk/mod.rs`
- `src/agents/risk/moderator.rs`
- `src/agents/risk/common.rs`
- `src/agents/fund_manager/tests.rs`
- `docs/prompts.md`
- `PRD.md`
- `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md`

Do not expand beyond this list for implementation or cleanup work. Archived historical documents may remain unchanged unless the user explicitly asks to modernize them too.

## Data Flow

The runtime flow stays the same, but the interpretation of the dual-risk signal changes.

1. Risk agents produce their `RiskReport`s.
2. The Risk Moderator produces a plain-text synthesis that explicitly records whether Conservative and Neutral both flagged a material violation.
3. The Fund Manager receives:
   - the trader proposal
   - all three risk reports
   - the moderator synthesis
   - a stable dual-risk escalation indicator derived directly from the typed risk reports with values `present`, `absent`, or `unknown`
   - analyst outputs
   - valuation context
   - prior thesis memory
4. The Fund Manager deep-thinking LLM returns the final `ExecutionStatus`.
5. Validation checks the response and writes it to `TradingState::final_execution_status`.

The key behavioral difference is that the dual-risk condition remains visible and emphasized end-to-end, but it no longer short-circuits the model path or predetermines the outcome.

This design does not change downstream consumer wiring. Existing consumers continue to surface both `decision` and `action` plus the full rationale. In particular, `Approved + Hold` remains an auditable "no immediate execution" recommendation combination, not a new workflow control state.

## Error Handling

The failure model stays conservative.

- Missing `trader_proposal` remains a hard `TradingError::SchemaViolation`.
- LLM timeout, provider failure, or retry exhaustion still surface through the existing error paths.
- Invalid JSON or invalid `ExecutionStatus` still fails validation and does not write partial state.
- Missing analyst or risk inputs still require the Fund Manager rationale to acknowledge that gap.
- If dual-risk escalation is present and the Fund Manager model is unavailable, the phase still fails; there is no conservative reject/hold fallback.

The dual Conservative+Neutral violation condition is not an error and is not a deterministic branch. It is a required consideration in prompt guidance and audit reasoning, handled through the normal LLM-plus-validation pipeline.

## Testing

Update tests from bypass-oriented assertions to judgment-path assertions.

- Replace tests that currently assert "no LLM call when both flags are true" with tests asserting that the Fund Manager still invokes the deep-thinking LLM in that case.
- Keep coverage showing that one-sided or no-violation cases also use the normal LLM path.
- Add prompt-composition coverage for the exact dual-risk indicator values:
  - `present` when both reports exist and both flags are true
  - `absent` when both reports exist and not both flags are true
  - `unknown` when either report is missing
- Add explicit dual-risk scenario coverage for the outcome matrix:
  - a schema-valid `Rejected` response using `Dual-risk escalation: upheld because ...`
  - a schema-valid `Approved + Hold` response using `Dual-risk escalation: deferred because ...`
  - a schema-valid `Approved + Buy` or `Approved + Sell` response using `Dual-risk escalation: overridden because ...`
- Add validation coverage showing that dual-risk-present outputs are rejected if they omit the required dedicated first-line `Dual-risk escalation:` header or use the wrong disposition for the chosen `decision` / `action` combination.
- Add validation coverage showing that dual-risk-present rejection outputs are rejected when they keep the trader's same-direction `action`.
- Update prompt tests so the Fund Manager prompt and Risk Moderator prompt both describe the dual-risk condition as a serious signal, not a deterministic rejection rule.
- Update only the assertions inside the canonical file list above that still name a deterministic rejection path through `FundManagerTask` or the old safety-net wording.
- Preserve existing tests for malformed JSON, missing rationale, timestamp overwrite, missing-input acknowledgment, and deep-thinking model enforcement.

`FundManagerTask` itself should remain unchanged at the workflow boundary: it still writes `ExecutionStatus`, persists the final snapshot, and ends the pipeline.

## Out of Scope

- Adding new `ExecutionStatus` fields
- Adding a new structured escalation field to `TradingState`
- Changing the Fund Manager model tier
- Introducing a deterministic post-processor after the Fund Manager response
- Refactoring unrelated risk or trading workflow components
