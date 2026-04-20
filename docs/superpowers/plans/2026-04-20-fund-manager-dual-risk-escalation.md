# Fund Manager Dual-Risk Escalation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace deterministic Conservative+Neutral dual-risk rejection with a Fund Manager LLM judgment path that always runs, while enforcing the approved dual-risk rationale contract and keeping runtime/docs/tests consistent.

**Architecture:** Keep the workflow topology unchanged: Phase 4 still produces `RiskReport`s plus moderator synthesis, and Phase 5 still produces `ExecutionStatus`. Introduce one shared internal tri-state `DualRiskStatus`, use it in both the Risk Moderator and Fund Manager paths, remove the fund-manager deterministic bypass, then enforce the first-line rationale contract in validation and align the current-behavior docs/specs.

**Tech Stack:** Rust 2024, `tokio`, `rig`, `serde`, `chrono`, `cargo nextest`, `cargo fmt`, `cargo clippy`.

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `src/agents/risk/common.rs` | Add `DualRiskStatus`, tri-state sentence helpers, and moderator validator support |
| Modify | `src/agents/risk/moderator.rs` | Switch moderator prompt + validation from binary wording to tri-state escalation wording |
| Modify | `src/agents/risk/mod.rs` | Re-export `DualRiskStatus` for sibling-agent consumption and update risk discussion mock/test fixtures that still hardcode the old moderator sentence |
| Modify | `src/agents/fund_manager/mod.rs` | Remove stale deterministic-safety-net module docs so the public module contract matches runtime behavior |
| Modify | `src/agents/fund_manager/agent.rs` | Remove deterministic bypass, derive typed dual-risk context, and pass it into prompt building and validation |
| Modify | `src/agents/fund_manager/validation.rs` | Replace deterministic-reject helpers with first-line parsing and dual-risk contract validation |
| Modify | `src/agents/fund_manager/prompt.rs` | Remove deterministic rejection prompt wording and add the tri-state dual-risk prompt contract |
| Modify | `src/agents/fund_manager/tests.rs` | Replace bypass tests with LLM-path, prompt, and validation coverage for the new dual-risk contract |
| Modify | `docs/prompts.md` | Update the current prompt reference for Fund Manager and Risk Moderator |
| Modify | `PRD.md` | Correct current architecture wording so Fund Manager is judgment-based rather than deterministic |
| Modify | `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md` | Replace the deterministic-wrapper scenario with the new LLM-judgment scenario |

---

## Chunk 1: Shared Dual-Risk Signal and Fund Manager Runtime Contract

### Task 1: Introduce the shared tri-state `DualRiskStatus`

**Files:**
- Modify: `src/agents/risk/common.rs`
- Modify: `src/agents/risk/moderator.rs`
- Modify: `src/agents/risk/mod.rs`

- [ ] **Step 1: Write the failing tri-state helper tests first**

In `src/agents/risk/common.rs`, add these exact unit tests:

```rust
#[test]
fn dual_risk_status_is_present_when_both_reports_flag_violation() { ... }

#[test]
fn dual_risk_status_is_absent_when_both_reports_exist_but_not_both_flagged() { ... }

#[test]
fn dual_risk_status_is_unknown_when_either_report_is_missing() { ... }

#[test]
fn expected_moderator_violation_sentence_is_tri_state() { ... }

#[test]
fn validate_moderator_output_accepts_unknown_sentence() { ... }

#[test]
fn validate_moderator_output_rejects_wrong_sentence_for_present() { ... }

#[test]
fn validate_moderator_output_rejects_wrong_sentence_for_absent() { ... }

#[test]
fn validate_moderator_output_rejects_wrong_sentence_for_unknown() { ... }
```

In `src/agents/risk/moderator.rs`, update or add tests so the required sentence becomes one of:

```text
Violation status: dual-risk escalation present.
Violation status: dual-risk escalation absent.
Violation status: dual-risk escalation unknown due to missing Conservative or Neutral report.
```

- [ ] **Step 2: Run the focused tri-state slice to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_risk_status_is_present_when_both_reports_flag_violation dual_risk_status_is_absent_when_both_reports_exist_but_not_both_flagged dual_risk_status_is_unknown_when_either_report_is_missing expected_moderator_violation_sentence_is_tri_state validate_moderator_output_accepts_unknown_sentence validate_moderator_output_rejects_wrong_sentence_for_present validate_moderator_output_rejects_wrong_sentence_for_absent validate_moderator_output_rejects_wrong_sentence_for_unknown run_synthesis_mentions_conservative_and_neutral_violation
```

Expected: FAIL because `risk/common.rs` and `risk/moderator.rs` are still binary-only.

- [ ] **Step 3: Add `DualRiskStatus` and helper methods in `risk/common.rs`**

Implement:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DualRiskStatus {
    Present,
    Absent,
    Unknown,
}

impl DualRiskStatus {
    pub(crate) fn from_reports(
        conservative: Option<&RiskReport>,
        neutral: Option<&RiskReport>,
    ) -> Self {
        match (conservative, neutral) {
            (Some(con), Some(neu)) if con.flags_violation && neu.flags_violation => Self::Present,
            (Some(_), Some(_)) => Self::Absent,
            _ => Self::Unknown,
        }
    }

    pub(crate) fn as_prompt_value(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::Unknown => "unknown",
        }
    }
}
```

Change:

```rust
expected_moderator_violation_sentence(expect_both_violation: bool)
validate_moderator_output(content: &str, expect_both_violation: bool)
```

to use `DualRiskStatus`.

- [ ] **Step 4: Re-export `DualRiskStatus` from `risk/mod.rs` for sibling modules**

In `src/agents/risk/mod.rs`, add or update the re-export so sibling modules can use a stable path:

```rust
pub(crate) use self::common::DualRiskStatus;
```

Keep it adjacent to the other `mod.rs` re-exports.

- [ ] **Step 5: Update `risk/moderator.rs` to use the tri-state helper everywhere and add the moderator drift guard**

Replace the inline `expect_both_violation` bool logic in both `build_moderator_prompt()` and `build_moderator_result()` with:

```rust
let dual_risk_status = DualRiskStatus::from_reports(
    state.conservative_risk_report.as_ref(),
    state.neutral_risk_report.as_ref(),
);
```

Update the moderator system prompt instruction 3 to stop mentioning deterministic rejection. Use wording in this shape:

```text
Explicitly note the dual-risk escalation status for downstream Fund Manager review.
```

In the same file, add a dedicated test named:

```rust
#[test]
fn risk_moderator_prompt_drift_guard_forbids_deterministic_phrases() { ... }
```

It must scan every Risk Moderator prompt constant or prompt fragment defined in `src/agents/risk/moderator.rs`, not just one assembled string, and assert they do not contain:

```text
must reject
automatic rejection
deterministic
required to reject
safety rule
deterministic safety rule
mandatory rejection
presumptive rejection
```

- [ ] **Step 6: Update the risk discussion mock fixture in `risk/mod.rs`**

Replace the hardcoded moderation text currently returned by `MockRiskExecutor::moderate()` with the new tri-state wording, for example:

```rust
"Violation status: dual-risk escalation present. Proceed with caution.".to_owned()
```

Only update the fixture strings needed for alignment. Do not refactor unrelated risk discussion logic.

- [ ] **Step 7: Re-run the focused tri-state slice**

Run the command from Step 2.

Expected: PASS.

- [ ] **Step 8: Commit the shared tri-state signal work**

```bash
git add src/agents/risk/common.rs src/agents/risk/moderator.rs src/agents/risk/mod.rs
git commit -m "feat(risk): add tri-state dual-risk escalation signal"
```

### Task 2: Remove the deterministic bypass and thread `DualRiskStatus` through the Fund Manager runtime path

**Files:**
- Modify: `src/agents/fund_manager/agent.rs`
- Modify: `src/agents/fund_manager/mod.rs`
- Modify: `src/agents/fund_manager/tests.rs`

- [ ] **Step 1: Write the failing runtime-path tests first**

In `src/agents/fund_manager/tests.rs`, delete the old deterministic-bypass assumptions and add these async tests:

```rust
#[tokio::test]
async fn dual_violation_still_invokes_llm_path() { ... }

#[tokio::test]
async fn llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status() { ... }
```

Use these expectations:

```rust
assert_eq!(inference.call_count(), 1);
assert!(matches!(result, Err(TradingError::Rig(_)) | Err(TradingError::NetworkTimeout { .. })));
assert!(state.final_execution_status.is_none());
```

- [ ] **Step 2: Run the focused runtime slice to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_violation_still_invokes_llm_path llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status
```

Expected: FAIL because `run_with_inference()` still returns before calling the LLM when both flags are true.

- [ ] **Step 3: Remove deterministic imports and the bypass branch in `agent.rs`**

Delete these imports and the entire early-return branch:

```rust
DETERMINISTIC_REJECT_RATIONALE,
deterministic_reject,
Decision,
ExecutionStatus,
```

and:

```rust
if deterministic_reject(state) {
    ...
    return Ok(AgentTokenUsage::unavailable(...));
}
```

- [ ] **Step 4: Derive typed context and pass it into prompt building and validation**

In `agent.rs`, after the `trader_proposal.is_none()` guard, derive:

```rust
let trader_proposal_action = state
    .trader_proposal
    .as_ref()
    .map(|proposal| proposal.action)
    .expect("checked above");

let dual_risk_status = crate::agents::risk::DualRiskStatus::from_reports(
    state.conservative_risk_report.as_ref(),
    state.neutral_risk_report.as_ref(),
);
```

Then change the calls to:

```rust
let (system_prompt, user_prompt) = build_prompt_context(
    state,
    &self.symbol,
    &self.target_date,
    dual_risk_status,
);

let mut status = parse_and_validate_execution_status(
    &outcome.result.output,
    state_has_missing_inputs(state),
    &state.target_date,
    dual_risk_status,
    trader_proposal_action,
)?;
```

Keep runtime timestamp overwrite and token accounting unchanged.

- [ ] **Step 5: Update the public module doc comment in `mod.rs`**

Replace the stale deterministic-safety-net module comment with a current one, for example:

```rust
//! The Fund Manager always uses the deep-thinking model and treats the
//! Conservative+Neutral dual-violation condition as a high-severity escalation
//! signal rather than a deterministic rejection branch.
```

- [ ] **Step 6: Re-run the focused runtime slice**

Run the command from Step 2.

Expected: compilation may still fail until Task 3 updates the validator signature and Task 4 updates the prompt function signature. If so, record that this is the expected intermediate red state and proceed immediately to Tasks 3 and 4 without trying to patch around it.

- [ ] **Step 7: Do not commit yet**

Task 2 and Task 3 intentionally change the same Fund Manager interface surface. Make one commit only after Task 3 passes so the intermediate signature mismatch never becomes a standalone commit.

### Task 3: Implement the Fund Manager dual-risk validation contract

**Files:**
- Modify: `src/agents/fund_manager/validation.rs`
- Modify: `src/agents/fund_manager/tests.rs`

- [ ] **Step 1: Write the full failing validator matrix first**

In `src/agents/fund_manager/tests.rs`, add async tests for the exact contract surface:

```rust
#[tokio::test]
async fn dual_risk_present_accepts_upheld_reject() { ... }

#[tokio::test]
async fn dual_risk_present_accepts_deferred_approved_hold() { ... }

#[tokio::test]
async fn dual_risk_present_accepts_overridden_directional_approval() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_missing_first_line_prefix() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_wrong_disposition_for_approved_hold() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_prefix_when_not_first_line() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_lowercase_prefix_variant() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_mixed_case_prefix_variant() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_em_dash_prefix_variant() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_markdown_fenced_prefix() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_two_leading_newlines_before_prefix() { ... }

#[tokio::test]
async fn dual_risk_present_allows_single_leading_newline_before_prefix() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_same_direction_reject_for_buy() { ... }

#[tokio::test]
async fn dual_risk_present_rejects_same_direction_reject_for_sell() { ... }

#[tokio::test]
async fn dual_risk_present_allows_rejected_hold_against_directional_proposal() { ... }

#[tokio::test]
async fn dual_risk_present_allows_rejected_direction_when_trader_proposed_hold() { ... }

#[tokio::test]
async fn dual_risk_unknown_requires_indeterminate_prefix() { ... }
```

Use concrete rationale strings from the spec, for example:

```text
Dual-risk escalation: upheld because both conservative reviewers identified a thesis-breaking downside scenario.
Blocking evidence outweighs the trader proposal.
```

```text
Dual-risk escalation: deferred because downside confirmation risk remains unresolved.
Approved with Hold while waiting for confirmation.
```

```text
Dual-risk escalation: overridden because valuation support and explicit stop tightening offset the flagged downside.
Approved with Buy on reduced size.
```

```text
Dual-risk escalation: indeterminate because the Neutral risk report is missing.
Decision uses partial upstream context.
```

- [ ] **Step 2: Run the focused validator matrix to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_risk_present_accepts_upheld_reject dual_risk_present_accepts_deferred_approved_hold dual_risk_present_accepts_overridden_directional_approval dual_risk_present_rejects_missing_first_line_prefix dual_risk_present_rejects_wrong_disposition_for_approved_hold dual_risk_present_rejects_prefix_when_not_first_line dual_risk_present_rejects_lowercase_prefix_variant dual_risk_present_rejects_mixed_case_prefix_variant dual_risk_present_rejects_em_dash_prefix_variant dual_risk_present_rejects_markdown_fenced_prefix dual_risk_present_rejects_two_leading_newlines_before_prefix dual_risk_present_allows_single_leading_newline_before_prefix dual_risk_present_rejects_same_direction_reject_for_buy dual_risk_present_rejects_same_direction_reject_for_sell dual_risk_present_allows_rejected_hold_against_directional_proposal dual_risk_present_allows_rejected_direction_when_trader_proposed_hold dual_risk_unknown_requires_indeterminate_prefix
```

Expected: FAIL because `validation.rs` still has no dual-risk context, no first-line parser, and no same-direction rejection rule.

- [ ] **Step 3: Change the validator signature and remove deterministic helpers**

In `src/agents/fund_manager/validation.rs`:

```rust
pub(super) fn parse_and_validate_execution_status(
    raw_output: &str,
    requires_missing_data_acknowledgment: bool,
    target_date: &str,
    dual_risk_status: DualRiskStatus,
    trader_proposal_action: TradeAction,
) -> Result<ExecutionStatus, TradingError>
```

Delete:

```rust
pub(super) const DETERMINISTIC_REJECT_RATIONALE: &str = ...
pub(super) fn deterministic_reject(state: &TradingState) -> bool { ... }
```

Also remove the now-unused `TradingState` import if this file no longer needs it outside the missing-input helpers.

- [ ] **Step 4: Add dedicated first-line parsing helpers**

Implement minimal helpers in `validation.rs`, for example:

```rust
fn normalized_rationale(rationale: &str) -> Result<&str, TradingError> { ... }
fn first_rationale_line(rationale: &str) -> Result<&str, TradingError> { ... }
fn validate_dual_risk_rationale(
    status: &ExecutionStatus,
    dual_risk_status: DualRiskStatus,
    trader_proposal_action: TradeAction,
) -> Result<(), TradingError> { ... }
```

Parser rules must match the spec exactly:
- tolerate at most one leading `\n`
- reject any other leading whitespace before the prefix
- require the first parsed line to begin with the case-sensitive byte prefix `Dual-risk escalation: `
- reject lowercase, mixed-case, markdown-fenced, or em-dash variants

- [ ] **Step 5: Encode the exact disposition and action-direction contract**

Implement these rules inside `validate_dual_risk_rationale()`:

```text
DualRiskStatus::Present:
- Rejected => first line starts with "Dual-risk escalation: upheld because "
- Approved + Hold => first line starts with "Dual-risk escalation: deferred because "
- Approved + Buy/Sell => first line starts with "Dual-risk escalation: overridden because "
- Rejected + same-direction Buy/Buy or Sell/Sell => SchemaViolation
- Rejected + Hold => allowed
- Rejected + Buy/Sell when trader action is Hold => allowed

DualRiskStatus::Unknown:
- first line starts with "Dual-risk escalation: indeterminate because "

DualRiskStatus::Absent:
- no dedicated first-line requirement
```

Keep the existing `rationale_acknowledges_missing_data()` check and run it against the full rationale string, including any first line.

- [ ] **Step 6: Keep the generic `ExecutionStatus` validation minimal**

`validate_execution_status()` should still only guard:

```text
- non-empty rationale
- max rationale length
- control-character rejection
```

All dual-risk-specific logic belongs in the dedicated helper invoked from `parse_and_validate_execution_status()`.

- [ ] **Step 7: Re-run the focused validator matrix**

Run the command from Step 2.

Expected: PASS.

- [ ] **Step 8: Commit Tasks 2 and 3 together**

```bash
git add src/agents/fund_manager/agent.rs src/agents/fund_manager/mod.rs src/agents/fund_manager/validation.rs src/agents/fund_manager/tests.rs
git commit -m "feat(fund-manager): enforce dual-risk judgment contract"
```

### Task 4: Rebuild the Fund Manager prompt contract around `DualRiskStatus`

**Files:**
- Modify: `src/agents/fund_manager/prompt.rs`
- Modify: `src/agents/fund_manager/tests.rs`

- [ ] **Step 1: Write the failing prompt-contract tests first**

Add focused tests covering:

```rust
#[test]
fn fund_manager_prompt_includes_present_indicator_near_top() { ... }

#[test]
fn fund_manager_prompt_includes_absent_indicator_near_top() { ... }

#[test]
fn fund_manager_prompt_uses_unknown_indicator_when_report_missing() { ... }

#[test]
fn fund_manager_prompt_places_unknown_indicator_near_top() { ... }

#[test]
fn fund_manager_system_prompt_contains_exact_first_line_contract() { ... }

#[test]
fn fund_manager_prompt_drift_guard_forbids_deterministic_phrases() { ... }
```

For the “near top” assertions, compare positions in the user prompt:

```rust
let indicator_pos = user.find("Dual-risk escalation:").unwrap();
let proposal_pos = user.find("Trader proposal:").unwrap();
assert!(indicator_pos < proposal_pos);
```

For the drift guard, assert **all three** prompt surfaces avoid these strings:

```text
must reject
automatic rejection
deterministic
required to reject
safety rule
deterministic safety rule
```

Surfaces to check:
- `FUND_MANAGER_SYSTEM_PROMPT`
- assembled Fund Manager user prompt
- Risk Moderator prompt constant (asserted from `src/agents/risk/moderator.rs` tests, not here)

- [ ] **Step 2: Run the focused prompt-contract slice to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast fund_manager_prompt_includes_present_indicator_near_top fund_manager_prompt_includes_absent_indicator_near_top fund_manager_prompt_uses_unknown_indicator_when_report_missing fund_manager_prompt_places_unknown_indicator_near_top fund_manager_system_prompt_contains_exact_first_line_contract fund_manager_prompt_drift_guard_forbids_deterministic_phrases
```

Expected: FAIL because the current prompt still says “Apply the deterministic safety rule” and has no tri-state indicator line.

- [ ] **Step 3: Update the `build_prompt_context()` signature and thread `DualRiskStatus` through the user prompt**

Change:

```rust
pub(super) fn build_prompt_context(
    state: &TradingState,
    symbol: &str,
    target_date: &str,
    dual_risk_status: DualRiskStatus,
) -> (String, String)
```

Also thread `dual_risk_status` through `build_user_prompt(...)`.

- [ ] **Step 4: Replace the deterministic system-prompt instruction with the exact contract**

Delete the current instruction 2 verbatim:

```text
Apply the deterministic safety rule: if BOTH the Conservative and Neutral risk reports clearly flag a material violation (`flags_violation == true`), reject the proposal.
```

Replace it with wording that explicitly requires byte-for-byte prefix emission and forbids alternate formatting. The replacement must include all of these points:

```text
If the user prompt says `Dual-risk escalation: present`, treat that as a high-severity signal.
Do not treat it as mandatory rejection or presumptive rejection.
The first line of `rationale` must begin exactly with one of:
- `Dual-risk escalation: upheld because <blocking reason>`
- `Dual-risk escalation: deferred because <specific unresolved objection or gating condition>`
- `Dual-risk escalation: overridden because <concrete counter-evidence and risk mitigation>`
If the user prompt says `Dual-risk escalation: unknown`, the first line must be `Dual-risk escalation: indeterminate because <which report is missing>`.
Emit the prefix byte-for-byte. Do not use markdown fences, lowercase variants, mixed-case variants, or em-dashes.
```

Keep valuation and missing-data instructions intact.

- [ ] **Step 5: Insert the tri-state indicator near the top of the user prompt**

In `build_user_prompt()`, add this immediately after the “Produce an ExecutionStatus JSON...” line:

```rust
push_bounded_line(
    &mut prompt,
    &format!("Dual-risk escalation: {}", dual_risk_status.as_prompt_value()),
    MAX_USER_PROMPT_CHARS,
);
```

Do not move this below serialized reports or valuation context.

- [ ] **Step 6: Re-run the focused prompt-contract slice**

Run the command from Step 2.

Expected: PASS.

- [ ] **Step 7: Commit the Fund Manager prompt contract**

```bash
git add src/agents/fund_manager/prompt.rs src/agents/fund_manager/tests.rs
git commit -m "feat(fund-manager): add dual-risk escalation prompt contract"
```

---

## Chunk 2: Current-Behavior Docs and Full Verification

### Task 5: Update current behavior docs and spec references

**Files:**
- Modify: `docs/prompts.md`
- Modify: `PRD.md`
- Modify: `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md`

- [ ] **Step 1: Update `docs/prompts.md`**

Replace the deterministic wording in both sections.

Risk Moderator section should say the moderator records dual-risk escalation status for downstream review, not deterministic rejection.

Fund Manager section should say, in substance:

```text
If the prompt says `Dual-risk escalation: present`, weigh it as a high-severity signal.
Do not treat it as automatic rejection.
The first line of `rationale` must classify the outcome using the approved exact form.
```

- [ ] **Step 2: Update `PRD.md` to current behavior truth**

Replace:

```text
The graph terminates at the Fund Manager node, which executes a deterministic logic check across the three risk reports to approve or reject the trade.
```

with wording in this shape:

```text
The graph terminates at the Fund Manager node, which uses the deep-thinking model to render the final decision after weighing the trader proposal, risk reports, and analyst context; dual Conservative+Neutral objection is treated as a high-severity escalation signal, not a deterministic reject rule.
```

- [ ] **Step 3: Update the current OpenSpec wrapper scenario**

Replace:

```text
#### Scenario: Deterministic Rejection Path Through Wrapper
```

with:

```text
#### Scenario: Dual-Risk Escalation Still Flows Through Wrapper

- WHEN both the Conservative and Neutral RiskReport objects have flags_violation = true
- THEN the Fund Manager task still invokes the Fund Manager judgment path, writes the resulting ExecutionStatus, and ends the pipeline
```

- [ ] **Step 4: Sanity-check the canonical file list for leftover deterministic wording**

Run:

```bash
rg -n "must reject|automatic rejection|deterministic|required to reject|safety rule|deterministic safety rule|mandatory rejection|presumptive rejection" \
  src/agents/fund_manager/mod.rs \
  src/agents/fund_manager/agent.rs \
  src/agents/fund_manager/validation.rs \
  src/agents/fund_manager/prompt.rs \
  src/agents/risk/mod.rs \
  src/agents/risk/moderator.rs \
  src/agents/risk/common.rs \
  docs/prompts.md \
  PRD.md \
  openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md
```

Expected: no matches representing current behavior text. Test files are intentionally excluded here because drift-guard tests may contain the forbidden phrase list as literals. Historical/archive files are out of scope and may still match.

- [ ] **Step 5: Commit the current-behavior doc/spec updates**

```bash
git add docs/prompts.md PRD.md openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md
git commit -m "docs: align dual-risk behavior with fund manager judgment"
```

### Task 6: Run full verification and capture the final state

**Files:**
- Modify: none expected

- [ ] **Step 1: Run the focused Fund Manager contract tests**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_violation_still_invokes_llm_path llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status dual_risk_present_accepts_upheld_reject dual_risk_present_accepts_deferred_approved_hold dual_risk_present_accepts_overridden_directional_approval dual_risk_present_rejects_missing_first_line_prefix dual_risk_present_rejects_wrong_disposition_for_approved_hold dual_risk_present_rejects_prefix_when_not_first_line dual_risk_present_rejects_lowercase_prefix_variant dual_risk_present_rejects_mixed_case_prefix_variant dual_risk_present_rejects_em_dash_prefix_variant dual_risk_present_rejects_markdown_fenced_prefix dual_risk_present_rejects_two_leading_newlines_before_prefix dual_risk_present_allows_single_leading_newline_before_prefix dual_risk_present_rejects_same_direction_reject_for_buy dual_risk_present_rejects_same_direction_reject_for_sell dual_risk_present_allows_rejected_hold_against_directional_proposal dual_risk_present_allows_rejected_direction_when_trader_proposed_hold dual_risk_unknown_requires_indeterminate_prefix fund_manager_prompt_includes_present_indicator_near_top fund_manager_prompt_includes_absent_indicator_near_top fund_manager_prompt_uses_unknown_indicator_when_report_missing fund_manager_prompt_places_unknown_indicator_near_top fund_manager_system_prompt_contains_exact_first_line_contract fund_manager_prompt_drift_guard_forbids_deterministic_phrases
```

Expected: PASS.

- [ ] **Step 2: Run the focused Risk Moderator / common helper slice**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_risk_status_is_present_when_both_reports_flag_violation dual_risk_status_is_absent_when_both_reports_exist_but_not_both_flagged dual_risk_status_is_unknown_when_either_report_is_missing expected_moderator_violation_sentence_is_tri_state validate_moderator_output_accepts_unknown_sentence validate_moderator_output_rejects_wrong_sentence_for_present validate_moderator_output_rejects_wrong_sentence_for_absent validate_moderator_output_rejects_wrong_sentence_for_unknown run_synthesis_mentions_conservative_and_neutral_violation risk_moderator_prompt_drift_guard_forbids_deterministic_phrases
```

Expected: PASS.

- [ ] **Step 2.5: Confirm the comparison base exists before the final diff check**

Run:

```bash
git fetch origin main --quiet
git rev-parse --verify origin/main
```

Expected: prints a commit hash for `origin/main`.

- [ ] **Step 3: Run formatting**

Run:

```bash
cargo fmt -- --check
```

Expected: exit 0.

- [ ] **Step 4: Run clippy**

Run:

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Run the full CI-equivalent nextest command**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast
```

Expected: PASS.

- [ ] **Step 6: Review the final diff against the implementation scope**

Run:

```bash
git diff --stat origin/main...HEAD
git status --short
```

Expected:
- diff stat shows only the files listed in the File Map
- `git status --short` is clean unless verification required a final unstaged fix

- [ ] **Step 7: Final commit if verification required follow-up edits**

If any verification step required code changes, create one final commit describing the verification fix, for example:

```bash
git add <exact files>
git commit -m "test: fix dual-risk escalation verification coverage"
```

If no follow-up edits were needed, skip this step.
