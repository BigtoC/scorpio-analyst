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
| Fund Manager execution path | Always call the deep-thinking LLM | Keep final judgment with the Fund Manager on every fully-formed run |
| `ExecutionStatus` schema | No changes | Existing `decision`, `action`, `rationale`, and timing fields already express the outcome |
| Risk Moderator wording | Explicitly note the dual-risk condition without calling it deterministic rejection | Keep the signal visible downstream while removing policy drift |
| Validation scope | Keep existing schema and missing-data validation only | Avoid brittle keyword checks for judgment reasoning |
| Workflow topology | No new tasks or branches | This is a policy correction inside the existing Phase 5 boundary |

## Architecture

The Fund Manager remains the terminal decision-maker in Phase 5. The dual `flags_violation` signal from the Conservative and Neutral risk agents is still preserved and emphasized, but it no longer controls execution flow on its own.

Phase 4 continues to produce three `RiskReport` objects plus a moderator synthesis. Phase 5 continues to produce one `ExecutionStatus`. The only architectural change is the meaning of the dual-risk signal:

- before: deterministic bypass of the LLM
- after: high-severity evidence that the Fund Manager must weigh in its final judgment

No new state type, no schema expansion, and no extra workflow phase are needed.

## Component Changes

### `src/agents/fund_manager/agent.rs`

- Remove the deterministic early-return branch that currently writes `Rejected + Hold` without model inference.
- Always build prompt context and call the deep-thinking Fund Manager model.
- Keep the existing runtime timestamp overwrite, state write, and token accounting behavior.

### `src/agents/fund_manager/validation.rs`

- Remove the deterministic-reject helper and the hardcoded deterministic-reject rationale constant.
- Keep JSON parsing, `ExecutionStatus` validation, missing-data acknowledgment checks, and runtime timestamp normalization unchanged.
- Do not add a new keyword-based validator for dual-risk acknowledgment; that requirement should live in prompt guidance and tests rather than brittle string matching.

### `src/agents/fund_manager/prompt.rs`

- Replace the current instruction that says the Fund Manager must reject when both Conservative and Neutral flag a material violation.
- Reword it so the dual-risk condition is treated as a high-severity signal that must be weighed explicitly in the rationale, alongside the trader proposal, analyst outputs, valuation context, and prior learnings.
- Preserve the existing instruction that the Fund Manager may return any valid `ExecutionStatus` it endorses after review.

### `src/agents/risk/moderator.rs` and shared risk wording

- Keep the requirement that the Risk Moderator explicitly states whether Conservative and Neutral both flag a material violation.
- Remove wording that claims the Fund Manager uses that condition as a deterministic rejection rule.
- Reframe the sentence as downstream escalation context for the Fund Manager.

### Current docs and specs

Update current, non-archived sources that describe runtime truth so they match the new policy. This includes at least:

- `docs/prompts.md`
- `PRD.md`
- current OpenSpec or non-archived spec documents that still describe deterministic rejection as active runtime behavior

Archived historical documents may remain unchanged unless they are still being treated as authoritative references.

## Data Flow

The runtime flow stays the same, but the interpretation of the dual-risk signal changes.

1. Risk agents produce their `RiskReport`s.
2. The Risk Moderator produces a plain-text synthesis that explicitly records whether Conservative and Neutral both flagged a material violation.
3. The Fund Manager receives:
   - the trader proposal
   - all three risk reports
   - the moderator synthesis
   - analyst outputs
   - valuation context
   - prior thesis memory
4. The Fund Manager deep-thinking LLM returns the final `ExecutionStatus`.
5. Validation checks the response and writes it to `TradingState::final_execution_status`.

The key behavioral difference is that the dual-risk condition remains visible and emphasized end-to-end, but it no longer short-circuits the model path or predetermines the outcome.

## Error Handling

The failure model stays conservative.

- Missing `trader_proposal` remains a hard `TradingError::SchemaViolation`.
- LLM timeout, provider failure, or retry exhaustion still surface through the existing error paths.
- Invalid JSON or invalid `ExecutionStatus` still fails validation and does not write partial state.
- Missing analyst or risk inputs still require the Fund Manager rationale to acknowledge that gap.

The dual Conservative+Neutral violation condition is not an error and is not a deterministic branch. It is a required consideration in prompt guidance and audit reasoning, handled through the normal LLM-plus-validation pipeline.

## Testing

Update tests from bypass-oriented assertions to judgment-path assertions.

- Replace tests that currently assert "no LLM call when both flags are true" with tests asserting that the Fund Manager still invokes the deep-thinking LLM in that case.
- Keep coverage showing that one-sided or no-violation cases also use the normal LLM path.
- Add or update coverage so dual-violation runs may produce any valid `ExecutionStatus` permitted by the prompt and schema.
- Update prompt tests so the Fund Manager prompt and Risk Moderator prompt both describe the dual-risk condition as a serious signal, not a deterministic rejection rule.
- Update any integration-level tests or spec assertions that currently name a deterministic rejection path through `FundManagerTask`.
- Preserve existing tests for malformed JSON, missing rationale, timestamp overwrite, missing-input acknowledgment, and deep-thinking model enforcement.

`FundManagerTask` itself should remain unchanged at the workflow boundary: it still writes `ExecutionStatus`, persists the final snapshot, and ends the pipeline.

## Out of Scope

- Adding new `ExecutionStatus` fields
- Adding a new structured escalation field to `TradingState`
- Changing the Fund Manager model tier
- Introducing a deterministic post-processor after the Fund Manager response
- Refactoring unrelated risk or trading workflow components
