# Fund Manager Dual-Risk Escalation Design

**Date:** 2026-04-20
**Status:** Approved

## Goal

Replace the current deterministic "Conservative + Neutral dual violation => immediate rejection" behavior with a judgment-based design where the Fund Manager always uses the deep-thinking LLM and treats the dual-risk condition as a high-severity input rather than an automatic outcome.

## Problem

The underlying question is policy, not consistency: should the Fund Manager's final call on a dual-risk case be LLM judgment informed by the risk reports, or a code-level deterministic reject that the LLM cannot override? The current design answers "deterministic reject"; this change argues for "LLM judgment with a narrow audit contract." The symptoms below are consequences of that policy, distributed across the codebase:

- `src/agents/fund_manager/agent.rs` short-circuits to a deterministic `Rejected + Hold` result without calling the LLM.
- `src/agents/fund_manager/prompt.rs` instructs the Fund Manager to reject when both Conservative and Neutral set `flags_violation = true`.
- `src/agents/risk/moderator.rs` tells the Risk Moderator to frame the dual-risk condition as a deterministic rejection trigger.
- Current tests and some current-facing docs/specs still assert the bypass path.

This conflicts with the desired product behavior: the Fund Manager should remain the final decision-maker after reviewing the trader proposal, risk reports, analyst outputs, valuation context, and prior learnings. The tradeoff involved is documented in **Accepted Tradeoffs**.

## Decisions

| Decision                      | Choice                                                                                                                | Rationale                                                                                                                                                                                                                                                                                     |
|-------------------------------|-----------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Dual-risk semantics           | High-severity escalation signal                                                                                       | Preserve the seriousness of the signal without converting it into an auto-veto                                                                                                                                                                                                                |
| Dual-risk audit contract      | Require a dedicated dual-risk first line in `rationale`; require override explanation for non-rejection outcomes      | Keep approvals and holds audit-distinguishable from ordinary cases without adding schema                                                                                                                                                                                                      |
| Prompt framing                | Serious but non-mandatory escalation                                                                                  | Prevent the old deterministic rule from surviving as soft prompt pressure                                                                                                                                                                                                                     |
| Fund Manager execution path   | Always call the deep-thinking LLM                                                                                     | Keep final judgment with the Fund Manager on every fully-formed run                                                                                                                                                                                                                           |
| `ExecutionStatus` schema      | No changes                                                                                                            | Existing `decision`, `action`, `rationale`, and timing fields already express the outcome                                                                                                                                                                                                     |
| Risk Moderator wording        | Explicitly note the dual-risk condition without calling it deterministic rejection                                    | Keep the signal visible downstream while removing policy drift                                                                                                                                                                                                                                |
| Validation scope              | Keep existing schema and missing-data validation, plus a narrow dual-risk rationale contract when both flags are true | Enforce minimum audit semantics without adding new structured fields                                                                                                                                                                                                                          |
| Workflow topology             | No new tasks or branches                                                                                              | This is a policy correction inside the existing Phase 5 boundary                                                                                                                                                                                                                              |
| Safety-rail tradeoff          | Code-level reject floor replaced by LLM judgment + rationale-form audit contract                                      | Accepts that a deterministic reject cannot be overridden by the model; moves the guarantee into (a) deep-thinking LLM judgment on every fully-formed run and (b) validator-enforced first-line classification. Revisit if replay or operator feedback shows worse outcomes than the old floor |
| Alternative considered        | Keep deterministic reject, add LLM-authored rationale                                                                 | Rejected: keeps the Fund Manager out of the final decision on the hottest cases, which is exactly what this change wants to undo. Useful if the replay evaluation (see Open Questions) later shows the full judgment-based path is too aggressive                                             |
| Escalation signal composition | Conservative + Neutral only (Aggressive excluded)                                                                     | Inherited from the prior rule: the two conservative-leaning voices' concurrent objection carries the strongest "do not trade" signal. Including Aggressive would dilute the conservative-veto intent of the signal                                                                            |

## Architecture

The Fund Manager remains the terminal decision-maker in Phase 5. The dual `flags_violation` signal from the Conservative and Neutral risk agents is still preserved and emphasized, but it no longer controls execution flow on its own.

Phase 4 continues to produce three `RiskReport` objects plus a moderator synthesis. Phase 5 continues to produce one `ExecutionStatus`. The only architectural change is the meaning of the dual-risk signal:

- before: deterministic bypass of the LLM
- after: high-severity evidence that the Fund Manager must weigh in its final judgment

No new state type, no schema expansion, and no extra workflow phase are needed.

## Dual-Risk Decision Contract

Prompt assembly must expose a stable dual-risk indicator derived from the typed risk reports. The indicator is the only signal the Fund Manager receives about this condition that is guaranteed to be in the prompt at a fixed location, regardless of moderator prose drift.

- `Dual-risk escalation: present` when both Conservative and Neutral reports exist and both set `flags_violation = true`
- `Dual-risk escalation: absent` when both reports exist and at least one does not set `flags_violation = true` (i.e., the dual-violation condition is not met)
- `Dual-risk escalation: unknown` when either the Conservative or Neutral report is missing

The Aggressive risk agent's flag is intentionally excluded from the indicator (see Decisions). If a future design wants a tri-agent escalation signal, that is a separate change.

### Rationale first-line contract

When `Dual-risk escalation: present`:

- `ExecutionStatus.rationale` must begin with a dedicated first line classifying the outcome. The first line must use one of these exact forms:
  - `Dual-risk escalation: upheld because <blocking reason>`
  - `Dual-risk escalation: deferred because <specific unresolved objection or gating condition>`
  - `Dual-risk escalation: overridden because <concrete counter-evidence and risk mitigation>`
- The required form is a function of the final `decision` / `action` combination:
  - `decision = Rejected` -> `upheld because`
  - `decision = Approved` and `action = Hold` -> `deferred because`
  - `decision = Approved` and `action = Buy` or `Sell` -> `overridden because`

When `Dual-risk escalation: unknown`, the rationale must also begin with a dedicated first line: `Dual-risk escalation: indeterminate because <which report is missing>`. This preserves audit uniformity — every dual-risk-relevant run is distinguishable from ordinary runs via a single prefix scan, and a silent risk-agent failure cannot erase the audit marker. The existing missing-input acknowledgment rule continues to apply to the rationale body.

When `Dual-risk escalation: absent`, no dedicated first line is required; normal rationale conventions apply.

### First-line parsing and normalization

The validator must parse the first line deterministically:

1. Strip at most one leading `\n` from the rationale (to tolerate common LLM output formatting). Any other leading whitespace is a validation failure.
2. Read characters up to the next `\n` or end of string.
3. Perform a case-sensitive byte-prefix match against `Dual-risk escalation: `.

Markdown code fences (` ``` `), lowercase or mixed-case variants (`dual-risk escalation:`, `Dual-Risk Escalation:`), em-dashes in place of `-`, or additional leading whitespace beyond the one tolerated `\n` fail validation. The prompt must explicitly instruct the model to produce the prefix byte-for-byte.

### Action-direction constraints

For `decision = Rejected` under `Dual-risk escalation: present`:

- `action` must not equal the trader proposal's `action` when both are directional (Buy/Buy or Sell/Sell). "Same-direction reject" is a logical contradiction that does not communicate a meaningful review result.
- "Same direction" is `TradeAction` enum equality restricted to directional variants. `Hold` is neutral, not a direction: `Hold` vs any other value is never "same-direction."
- If the trader proposal action is `Hold`, the same-direction constraint does not apply and the Fund Manager may return `Rejected` with any action (Hold, Buy, or Sell). Rejecting a Hold proposal is rare but valid (e.g., "do not just hold — actively reduce exposure").
- Valid rejection outcomes are therefore: `Rejected + Hold` (always), `Rejected + Sell` when trader proposed Buy, `Rejected + Buy` when trader proposed Sell, and `Rejected + <any>` when trader proposed Hold.

For `decision = Approved` under `Dual-risk escalation: present`:

- Any action (Buy/Sell/Hold) is permitted. Direction-reversed approval (e.g., trader proposed Buy, Fund Manager returns Approved + Sell) is classified as `overridden because` along with same-direction overrides; the asymmetry with rejection is deliberate because an approval always endorses a specific action and is therefore unambiguous on its own.

### Outcome matrix for dual-risk-present cases

- `Rejected + Hold` or `Rejected + opposite direction` — escalation upheld; current proposal rejected.
- `Approved + Hold` — escalation deferred; no immediate execution but the thesis stays alive (audit-distinguishable from `Rejected + Hold` via the first-line classifier).
- `Approved + Buy` or `Approved + Sell` — override outcome; requires unusually strong justification grounded in concrete counter-evidence and explicit mitigation.

### Framing constraints

The prompt must not frame dual-risk as mandatory rejection or presumptive rejection. It may frame it as an elevated-justification case. A CI test (see Testing) asserts that no Fund Manager or Risk Moderator prompt constant contains phrases that would re-introduce deterministic framing.

This preserves the seriousness of the dual-risk condition in a way that remains reviewable and testable without adding a new output field.

## Component Changes

### `src/agents/fund_manager/agent.rs`

- Remove the deterministic early-return branch that currently writes `Rejected + Hold` without model inference.
- Remove the `DETERMINISTIC_REJECT_RATIONALE` import from `validation` (it is deleted in the validation changes below).
- Always build prompt context and call the deep-thinking Fund Manager model.
- Compute two typed values from `TradingState` before prompt building and response validation:
  - a dual-risk indicator of type `DualRiskStatus` (see `risk/common.rs` changes)
  - the trader proposal action (`TradeAction`)
- Pass these as two explicit parameters to `parse_and_validate_execution_status(...)`. Do **not** introduce a wrapper struct: the two values are trivially derivable from typed state and have a single consumer today; a named context struct would be abstraction without a second user. If a future change adds a second consumer, introduce the struct then.
- Keep the existing runtime timestamp overwrite, state write, and token accounting behavior.

### `src/agents/fund_manager/validation.rs`

- Remove the `deterministic_reject()` helper, the `DETERMINISTIC_REJECT_RATIONALE` constant, and any pub(super) re-exports of either.
- Keep JSON parsing, missing-data acknowledgment checks, and runtime timestamp normalization.
- Change `parse_and_validate_execution_status(...)` to accept two additional explicit parameters: `dual_risk_status: DualRiskStatus` and `trader_proposal_action: TradeAction`. Do not recompute either value from raw model output.
- Add a narrow dual-risk validation contract driven by those typed inputs:
  - when `dual_risk_status == Present`, `rationale` must begin with a first line matching the disposition-specific form defined in **Dual-Risk Decision Contract** (upheld / deferred / overridden). Use the parsing rules spelled out there (strip at most one leading `\n`; case-sensitive byte-prefix match).
  - when `dual_risk_status == Unknown`, `rationale` must begin with `Dual-risk escalation: indeterminate because ...`.
  - when `dual_risk_status == Absent`, no dedicated dual-risk first line is required.
  - when `dual_risk_status == Present` and `decision == Rejected`, `action` must not equal `trader_proposal_action` if both are directional (Buy/Buy or Sell/Sell). The Hold and trader-Hold carve-outs spelled out in the contract apply here.
- When the dual-risk contract is violated, return `TradingError::SchemaViolation` — the phase fails. The Fund Manager is not reprompted on contract failure; the existing LLM retry policy (max 3 retries, base 500ms) already covers transient model errors, so a validation-level reprompt would be redundant and would mask genuine contract violations.
- Ensure the missing-data acknowledgment substring check scans the full rationale (including any first line). The first-line classifier and the missing-data ack are independent contracts; when both apply they must both be satisfied in one rationale string.
- Keep the rest of `ExecutionStatus` validation unchanged.

### `src/agents/fund_manager/prompt.rs`

- **Delete the current system-prompt instruction #2** verbatim — it currently reads: *"Apply the deterministic safety rule: if BOTH the Conservative and Neutral risk reports clearly flag a material violation (`flags_violation == true`), reject the proposal."* This is the load-bearing wording that must be removed, not softened.
- Replace it with an instruction that treats the dual-risk condition as a high-severity signal that must be weighed explicitly in the rationale, alongside the trader proposal, analyst outputs, valuation context, and prior learnings.
- Add a canonical prompt line derived from the typed `DualRiskStatus` value using the exact `present` / `absent` / `unknown` strings, so the Fund Manager always receives an unambiguous signal even if moderator prose changes.
- In dual-risk-present cases, require the rationale to use the exact dedicated first-line `Dual-risk escalation:` form appropriate to the chosen outcome. Instruct the model to emit the prefix byte-for-byte (no markdown fences, no case changes, no em-dashes) and state that the validator will reject any other form.
- In dual-risk-unknown cases, require the rationale to use `Dual-risk escalation: indeterminate because <which report is missing>` as its first line.
- Explicitly forbid wording that presents dual-risk as deterministic rejection or as a presumed default rejection.
- Place the `Dual-risk escalation: <value>` indicator near the top of the user prompt, before long serialized context blocks, so it survives the current `MAX_USER_PROMPT_CHARS` budget.
- Keep the exact rationale-form instruction in the **system** prompt (separate budget from user prompt; not subject to `MAX_USER_PROMPT_CHARS`) so it is not truncated.
- Preserve the existing instruction that the Fund Manager may return any valid `ExecutionStatus` it endorses after review.

### `src/agents/risk/moderator.rs` and `src/agents/risk/common.rs`

Introduce a shared tri-state enum and thread it through the moderator validator. This is a breaking internal contract change: every call site and test that passes `true`/`false` to `validate_moderator_output` / `expected_moderator_violation_sentence` must be updated.

In `src/agents/risk/common.rs`:

- Add `pub(super) enum DualRiskStatus { Present, Absent, Unknown }`. Derive `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.
- Add a constructor that maps `(Option<&RiskReport>, Option<&RiskReport>)` (Conservative, Neutral) to `DualRiskStatus`: both present and both `flags_violation = true` → `Present`; both present and not both flagged → `Absent`; either missing → `Unknown`. This replaces the current inline `is_some_and(|r| r.flags_violation) && …` computation in the deterministic-reject helper.
- Change `expected_moderator_violation_sentence(expect_both_violation: bool) -> &'static str` to `expected_moderator_violation_sentence(status: DualRiskStatus) -> &'static str` returning the tri-state sentence.
- Change `validate_moderator_output(content: &str, expect_both_violation: bool) -> Result<(), TradingError>` to `validate_moderator_output(content: &str, status: DualRiskStatus) -> Result<(), TradingError>`. The `Unknown` branch asserts the sentence contains the `unknown due to missing Conservative or Neutral report` form.
- Update the existing unit tests in `common.rs` that currently pass `true` / `false` to cover all three `DualRiskStatus` variants.

In `src/agents/risk/moderator.rs`:

- Replace the current binary moderator sentence with the same tri-state signal used by the Fund Manager path:
  - `Violation status: dual-risk escalation present.`
  - `Violation status: dual-risk escalation absent.`
  - `Violation status: dual-risk escalation unknown due to missing Conservative or Neutral report.`
- Remove any prompt wording that claims the Fund Manager uses the condition as a deterministic rejection rule (the current instruction line that reads approximately *"the Fund Manager uses that as a deterministic rejection rule"*).
- Reframe the sentence as downstream escalation context for the Fund Manager.
- Update call sites to build the `DualRiskStatus` from typed `RiskReport` options and pass it in.

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
- Dual-risk contract violations (missing first line, wrong disposition, same-direction reject under `Present`, malformed prefix) are `TradingError::SchemaViolation`. They fail the phase; the existing LLM retry policy covers transient model errors before validation is reached.
- If dual-risk escalation is `Present` and the Fund Manager model is unavailable after retry exhaustion, the phase still fails; there is no conservative reject/hold fallback. Rationale: a `Rejected + Hold` fallback would weaken the "Fund Manager always decides on fully-formed runs" invariant and reintroduce a code path in which no model call occurs. A typed error that operators can detect is strictly more informative than a stale safety reject that carries no signal. This choice is revisited in **Open Questions** if operator replay shows it hurts decision throughput in practice.

The dual Conservative+Neutral violation condition is not an error and is not a deterministic branch. It is a required consideration in prompt guidance and audit reasoning, handled through the normal LLM-plus-validation pipeline.

## Testing

Update tests from bypass-oriented assertions to judgment-path assertions.

### Tests to delete

Their premises (zero tokens, no LLM call on bypass, hardcoded deterministic rationale string) no longer exist and cannot be rewritten in place:

- `deterministic_rejection_when_both_conservative_and_neutral_flag_violation` in `src/agents/fund_manager/tests.rs`
- `agent_token_usage_for_deterministic_bypass_has_zero_tokens_and_measured_latency` in `src/agents/fund_manager/tests.rs`

### Tests to add

- Fund Manager still invokes the deep-thinking LLM when both flags are true (replacement for the "no LLM call" test above).
- Prompt-composition coverage driven by `DualRiskStatus` variants: `Present` produces the present line; `Absent` produces the absent line; `Unknown` produces the unknown line. Assert the line appears near the top of the user prompt (before long serialized blocks) and the rationale-form instruction appears in the system prompt.
- Outcome-matrix validator tests:
  - schema-valid `Rejected` response using `Dual-risk escalation: upheld because ...` — accepted
  - schema-valid `Approved + Hold` using `Dual-risk escalation: deferred because ...` — accepted
  - schema-valid `Approved + Buy/Sell` using `Dual-risk escalation: overridden because ...` — accepted
- First-line parsing negative tests:
  - rationale missing the prefix entirely → rejected
  - rationale with wrong disposition for the `decision` / `action` combination → rejected
  - rationale with the prefix mid-body but not as the first line → rejected
  - rationale with lowercase / mixed-case / em-dash variant of the prefix → rejected
  - rationale with two or more leading `\n` before the prefix → rejected (only one leading `\n` is tolerated)
  - rationale with a single leading `\n` before a valid prefix → accepted (tolerated normalization)
- Action-direction negative tests under `Present`:
  - `Rejected + Buy` when trader proposed `Buy` → rejected
  - `Rejected + Sell` when trader proposed `Sell` → rejected
  - `Rejected + Hold` when trader proposed any direction → accepted
  - `Rejected + Buy` when trader proposed `Hold` → accepted (Hold carve-out)
- `Unknown` path: rationale without the `Dual-risk escalation: indeterminate because ...` first line → rejected.
- LLM availability failure under `Present`: when `StubInference` is configured to exhaust retries, the phase returns a typed error and does **not** write a silent `Rejected + Hold` to state.
- **Prompt-drift guard (CI test):** assert that `FUND_MANAGER_SYSTEM_PROMPT`, the assembled user prompt, and the Risk Moderator prompt constants do NOT contain any of a forbidden-phrase list: `"must reject"`, `"automatic rejection"`, `"deterministic"`, `"required to reject"`, `"safety rule"`, `"deterministic safety rule"`. This is the coordination-constraint guard that stops deterministic framing from leaking back in across future edits; it runs under the existing `cargo clippy --all-targets -- -D warnings` / `cargo nextest run` gates in `.github/workflows/tests.yml`.
- Moderator validator tri-state coverage: `expected_moderator_violation_sentence(DualRiskStatus::Present|Absent|Unknown)` returns the matching sentence; `validate_moderator_output(content, status)` rejects content that uses the wrong variant for `status`.

### Tests to preserve (unchanged)

- Malformed JSON handling.
- Missing rationale rejection.
- Runtime timestamp normalization / overwrite.
- Missing-input acknowledgment substring check.
- Deep-thinking model tier enforcement.
- `system_prompt_contains_safety_net_instructions` may remain for the substring `"flags_violation"` (the new prompt still references the field) but it no longer proves a safety-net rule exists; the prompt-drift guard above is now the real enforcement.

Update only the assertions inside the canonical file list that still name a deterministic rejection path through `FundManagerTask` or the old safety-net wording. `FundManagerTask` itself stays unchanged at the workflow boundary: it still writes `ExecutionStatus`, persists the final snapshot, and ends the pipeline.

## Out of Scope

- Adding new `ExecutionStatus` fields
- Adding a new structured escalation field to `TradingState`
- Changing the Fund Manager model tier
- Introducing a deterministic post-processor after the Fund Manager response
- Refactoring unrelated risk or trading workflow components
- Exposing the dual-risk indicator publicly on `TradingState` (it stays a per-run typed value derived inside the Fund Manager path)
- Multimodel cross-checks, eval harnesses, or replay infrastructure (out of scope for this change; called out in Open Questions)

## Accepted Tradeoffs

The judgment-based design is a deliberate bet, not a pure cleanup. These tradeoffs are being accepted rather than resolved:

- **Safety floor moved from code to model.** The prior deterministic rule was an unbypassable upper bound on `Approved + Buy/Sell` when Conservative and Neutral both flagged. The new design replaces that bound with (a) deep-thinking LLM judgment on every fully-formed run and (b) the first-line rationale-form contract. The product bet is that model judgment + audit is preferable to a non-overridable code reject on the tail. If operator feedback or historical replay shows worse outcomes under dual-risk, revisit — see Open Questions #1.
- **Prompt-enforced over schema-enforced.** The rationale-form contract lives in prompt text plus a narrow string validator rather than as a new `ExecutionStatus` field. Scope chose this because `ExecutionStatus` is already stable and downstream-wired. The cost is that every prompt edit, model swap, or provider change risks eroding the contract; the prompt-drift CI test is the compensating control.
- **Identity shift surfaced, not reversed.** The system was "multi-agent orchestrator with code-level safety constraints on the tail"; it is now "multi-agent orchestrator with LLM judgment plus an audit contract on the tail." Downstream consumers remain unchanged but the guarantee surface changes. Operators relying on the old property "dual violation → guaranteed no trade" will see different behavior; call this out in release notes.
- **Hard fail over safe fallback on LLM unavailability under dual-risk.** Restoring a narrow `Rejected + Hold` fallback would preserve the safety floor exactly where it matters most, but would also reintroduce a code path where the Fund Manager did not actually decide. Keeping "Fund Manager always decides or phase fails" as an invariant is worth the operational cost; a distinct typed error gives operators a signal that a deterministic Hold cannot.

## Open Questions

These are deliberately deferred — they do not block implementation but should be tracked and revisited.

1. **Replay evaluation before merge:** does the deep-thinking LLM, on a corpus of prior dual-risk cases from `phase_snapshots`, produce outcomes at least as conservative as the deterministic reject? Worth running before flipping the default.
2. **Override budget:** what is the acceptable frequency of `Dual-risk escalation: overridden because ...` outcomes in production? Without a target, drift across model upgrades will only be visible after damage.
3. **Unknown-path observability:** should the `Unknown` branch emit a counter / `tracing::warn!` separate from debug logging, so silent degradation of Conservative or Neutral report coverage becomes visible in dashboards?
4. **LLM-unavailable fallback:** if operator feedback or incident replay shows the hard-fail choice hurts decision throughput, a narrow `Rejected + Hold` fallback keyed on retry-exhaustion-under-`Present` is the least-invasive way to restore the floor without reintroducing the full bypass. Do not add preemptively.
5. **`entry_guidance` for `Rejected + opposite-direction`:** the existing system-prompt rule #8 requires `entry_guidance` when `action` is Hold or Sell. `Rejected + Sell` on a Buy proposal becomes legal under this contract; does `entry_guidance` still make semantic sense there, or should the rule be narrowed to `Approved + {Hold, Sell}`?
6. **Second-model cross-check on override outcomes:** should `Approved + Buy/Sell` under `Present` require a second-model confirmation pass, given the spec flags it as needing "unusually strong justification"?
7. **Dual-risk indicator on `TradingState`:** should the tri-state value be surfaced publicly (versioned, with `#[serde(default)]` per the TradingState schema-evolution rules) so downstream auditors and `risk_discussion_history` consumers can see it uniformly? Today it stays private to the Fund Manager path.
8. **Policy-version tag on phase snapshots:** how should historical snapshots written under the old deterministic policy be audited alongside new snapshots? A `trading_state_json` field like `decision_policy_version: u32` would disambiguate mixed-policy analysis; out of scope here but worth a follow-up.
