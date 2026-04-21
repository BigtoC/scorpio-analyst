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

- [x] **Step 1: Write the failing tri-state helper tests first and update existing binary-wording fixtures**

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

**Existing tests and fixtures that carry old binary wording and must be updated in this step** (not discovered later via the full nextest run):

In `src/agents/risk/common.rs`:
- `validate_moderator_output_accepts_valid` — currently passes the old binary sentence with `true`; rewrite to pass each `DualRiskStatus` variant with its matching tri-state sentence.
- `validate_moderator_output_rejects_missing_required_violation_sentence` — rewrite in tri-state terms.

In `src/agents/risk/moderator.rs`:
- Helper fixtures `valid_synthesis()` and `valid_dual_violation_synthesis()` — update their embedded `Violation status: ...` line to the matching tri-state form. Keep the lowercase words `conservative` and `neutral` somewhere in the body so `run_synthesis_mentions_conservative_and_neutral_violation` at lines 337–356 continues to pass without being rewritten.
- `build_moderator_result_redacts_secret_from_stored_output` — if it asserts on moderator sentence content, update to tri-state.

In `src/agents/risk/mod.rs`:
- `MockRiskExecutor::moderate()` literal at around line 584 — covered in Step 6 below but listed here for completeness.

**Case sensitivity of the new validator:** the tri-state `validate_moderator_output` must preserve the existing case-insensitive substring check (`to_ascii_lowercase` on both sides). Sloppy model casing (`"dual-risk escalation PRESENT"`) must still validate — this is the moderator output surface, not the Fund Manager rationale first-line contract (the latter IS case-sensitive, per spec).

- [x] **Step 2: Run the focused tri-state slice to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_risk_status_is_present_when_both_reports_flag_violation dual_risk_status_is_absent_when_both_reports_exist_but_not_both_flagged dual_risk_status_is_unknown_when_either_report_is_missing expected_moderator_violation_sentence_is_tri_state validate_moderator_output_accepts_unknown_sentence validate_moderator_output_rejects_wrong_sentence_for_present validate_moderator_output_rejects_wrong_sentence_for_absent validate_moderator_output_rejects_wrong_sentence_for_unknown run_synthesis_mentions_conservative_and_neutral_violation
```

Expected: FAIL because `risk/common.rs` and `risk/moderator.rs` are still binary-only.

- [x] **Step 3: Add `DualRiskStatus` and helper methods in `risk/common.rs`**

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

- [x] **Step 4: Re-export `DualRiskStatus` from `risk/mod.rs` for sibling modules**

In `src/agents/risk/mod.rs`, add or update the re-export so sibling modules can use a stable path:

```rust
pub(crate) use self::common::DualRiskStatus;
```

Keep it adjacent to the other `mod.rs` re-exports.

**Visibility pattern note:** the enum itself stays `pub(super)` inside `common.rs` (visible only within the `risk` module), and `mod.rs` re-exports it as `pub(crate)` to widen visibility to sibling agent modules. This "`pub(super)` + `pub(crate) use`" pattern is intentional — do not widen the enum definition to `pub(crate)` directly.

- [x] **Step 5: Update `risk/moderator.rs` to use the tri-state helper everywhere and add the moderator drift guard**

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

It must scan every Risk Moderator prompt constant or prompt fragment defined in `src/agents/risk/moderator.rs`, not just one assembled string, and assert they do not contain (case-insensitive):

```text
must reject
automatic rejection
deterministic rejection
deterministic reject
deterministic safety rule
required to reject
mandatory rejection
presumptive rejection
```

The list is intentionally narrow: bare `deterministic` and bare `safety rule` are excluded because the Fund Manager system prompt legitimately retains the phrase "pre-computed deterministic valuation" (kept by Task 4 Step 4 under "Keep valuation and missing-data instructions intact"), and "safety rule" is a common English phrase that would false-positive on any future unrelated risk-management wording. The forbidden set targets the specific dual-risk-rejection phrasing only.

- [x] **Step 6: Update the risk discussion mock fixture in `risk/mod.rs`**

Replace the hardcoded moderation text currently returned by `MockRiskExecutor::moderate()` with the new tri-state wording, for example:

```rust
"Violation status: dual-risk escalation present. Proceed with caution.".to_owned()
```

Only update the fixture strings needed for alignment. Do not refactor unrelated risk discussion logic.

- [x] **Step 7: Re-run the focused tri-state slice**

Run the command from Step 2.

Expected: PASS.

- [x] **Step 8: Commit the shared tri-state signal work**

```bash
git add src/agents/risk/common.rs src/agents/risk/moderator.rs src/agents/risk/mod.rs
git commit -m "feat(risk): add tri-state dual-risk escalation signal"
```

### Task 2: Remove the deterministic bypass and thread `DualRiskStatus` through the Fund Manager runtime path

**Files:**
- Modify: `src/agents/fund_manager/agent.rs`
- Modify: `src/agents/fund_manager/mod.rs`
- Modify: `src/agents/fund_manager/tests.rs`

> **Commit-boundary note:** Tasks 2, 3, and **4** all mutate the same Fund Manager interface surface (`agent.rs` calls a 4-arg `build_prompt_context` introduced in Task 2, the validator signature widens in Task 3, and `prompt.rs` gains the 4th parameter in Task 4). A commit at the end of Task 3 alone would land a non-compiling tree. The single combined commit is now deferred to Task 4 Step 7 and covers all four files. Tasks 2 and 3 still produce green focused-test slices, but they do so by keeping the intermediate state uncommitted.

- [x] **Step 1: Write the failing runtime-path tests first and enumerate old tests to delete**

In `src/agents/fund_manager/tests.rs`, **delete these existing tests by name** (their premises — zero tokens, no LLM call on bypass, hardcoded `DETERMINISTIC_REJECT_RATIONALE` — no longer exist and cannot be rewritten in place):

- `deterministic_rejection_when_both_conservative_and_neutral_flag_violation` (currently around lines 263–296)
- `agent_token_usage_for_deterministic_bypass_has_zero_tokens_and_measured_latency` (currently around lines 566–592)

Also identify (and flag for Task 3 update, not deletion) tests whose fixtures depend on the old rationale shape:

- `missing_risk_reports_invoke_llm_path` — uses `approved_json_with_missing_data_ack()` whose rationale `"Approved with reduced confidence because one or more upstream inputs are missing."` will fail the new `DualRiskStatus::Unknown` first-line contract once Task 3 lands. Task 3 Step 1 adds the dedicated fixture fix.
- `missing_analyst_inputs_invoke_llm_path` — same fixture, same fix.

After deleting the two bypass tests, add these async tests:

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

- [x] **Step 2: Run the focused runtime slice to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_violation_still_invokes_llm_path llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status
```

Expected: FAIL because `run_with_inference()` still returns before calling the LLM when both flags are true.

- [x] **Step 3: Remove deterministic imports and the bypass branch in `agent.rs`**

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

**Rustdoc intra-doc link follow-up:** `src/agents/fund_manager/agent.rs` line 58 currently has a rustdoc link `/// a validated [\`ExecutionStatus\`].`. Removing `ExecutionStatus` from the imports tuple downgrades this to an unresolved intra-doc link; with rustdoc warnings-as-errors on `-D warnings` this can break CI. Fix in the same step: either rewrite the doc comment without the link syntax (e.g. `/// a validated ExecutionStatus.`), or keep `ExecutionStatus` in scope via a separate `use crate::state::ExecutionStatus;` placed at module scope for the doc link only. Verify `cargo doc --no-deps` is clean before the combined commit in Task 4 Step 7.

- [x] **Step 4: Derive typed context and pass it into prompt building and validation**

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

- [x] **Step 5: Update the public module doc comment in `mod.rs`**

Replace the stale deterministic-safety-net module comment with a current one, for example:

```rust
//! The Fund Manager always uses the deep-thinking model and treats the
//! Conservative+Neutral dual-violation condition as a high-severity escalation
//! signal rather than a deterministic rejection branch.
```

- [x] **Step 6: Re-run the focused runtime slice**

Run the command from Step 2.

Expected: compilation will still fail until **both** Task 3 (validator signature) and Task 4 (prompt signature) land. This is the expected intermediate red state — do not try to patch around it. Proceed to Task 3 without a commit.

- [x] **Step 7: Do not commit yet**

Tasks 2, 3, and 4 all mutate the Fund Manager interface surface atomically. The single combined commit lands in Task 4 Step 7 after the prompt signature change also goes in, so the intermediate signature mismatch never becomes a standalone commit. If Task 3's green checkpoint is reached before Task 4 lands, the crate will still fail to compile — that is expected; do not commit at Task 3 Step 8.

### Task 3: Implement the Fund Manager dual-risk validation contract

**Files:**
- Modify: `src/agents/fund_manager/validation.rs`
- Modify: `src/agents/fund_manager/tests.rs`

- [x] **Step 1: Write the full failing validator matrix first and fix fixtures flagged in Task 2**

In `src/agents/fund_manager/tests.rs`, add synchronous tests for the exact contract surface. `parse_and_validate_execution_status` is a sync function (no `.await`), so use `#[test] fn` — **not** `#[tokio::test] async fn`:

```rust
#[test]
fn dual_risk_present_accepts_upheld_reject() { ... }

#[test]
fn dual_risk_present_accepts_deferred_approved_hold() { ... }

#[test]
fn dual_risk_present_accepts_overridden_directional_approval() { ... }

#[test]
fn dual_risk_present_rejects_missing_first_line_prefix() { ... }

#[test]
fn dual_risk_present_rejects_wrong_disposition_for_approved_hold() { ... }

#[test]
fn dual_risk_present_rejects_prefix_when_not_first_line() { ... }

// The four prefix-variant cases below can optionally collapse into one table-driven
// test over (label, malformed_input) pairs — same contract coverage, ~60 fewer lines
// of boilerplate. Either form is acceptable; list them separately here for clarity.
#[test]
fn dual_risk_present_rejects_lowercase_prefix_variant() { ... }

#[test]
fn dual_risk_present_rejects_mixed_case_prefix_variant() { ... }

#[test]
fn dual_risk_present_rejects_em_dash_prefix_variant() { ... }

#[test]
fn dual_risk_present_rejects_markdown_fenced_prefix() { ... }

#[test]
fn dual_risk_present_rejects_two_leading_newlines_before_prefix() { ... }

#[test]
fn dual_risk_present_allows_single_leading_newline_before_prefix() { ... }

#[test]
fn dual_risk_present_rejects_same_direction_reject_for_buy() { ... }

#[test]
fn dual_risk_present_rejects_same_direction_reject_for_sell() { ... }

#[test]
fn dual_risk_present_allows_rejected_hold_against_directional_proposal() { ... }

#[test]
fn dual_risk_present_allows_rejected_direction_when_trader_proposed_hold() { ... }

#[test]
fn dual_risk_unknown_requires_indeterminate_prefix() { ... }
```

Only the two runtime-path tests added in Task 2 (`dual_violation_still_invokes_llm_path`, `llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status`) genuinely need `#[tokio::test]` — they call `run_with_inference` which is async.

**Also update fixtures flagged in Task 2 that break under the new contract:**
- `approved_json_with_missing_data_ack()` — prepend a `Dual-risk escalation: indeterminate because the upstream inputs required for dual-risk evaluation are missing.` first line so `missing_risk_reports_invoke_llm_path` and `missing_analyst_inputs_invoke_llm_path` continue to pass once the Unknown-first-line rule is enforced in Step 5.

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

- [x] **Step 2: Run the focused validator matrix to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_risk_present_accepts_upheld_reject dual_risk_present_accepts_deferred_approved_hold dual_risk_present_accepts_overridden_directional_approval dual_risk_present_rejects_missing_first_line_prefix dual_risk_present_rejects_wrong_disposition_for_approved_hold dual_risk_present_rejects_prefix_when_not_first_line dual_risk_present_rejects_lowercase_prefix_variant dual_risk_present_rejects_mixed_case_prefix_variant dual_risk_present_rejects_em_dash_prefix_variant dual_risk_present_rejects_markdown_fenced_prefix dual_risk_present_rejects_two_leading_newlines_before_prefix dual_risk_present_allows_single_leading_newline_before_prefix dual_risk_present_rejects_same_direction_reject_for_buy dual_risk_present_rejects_same_direction_reject_for_sell dual_risk_present_allows_rejected_hold_against_directional_proposal dual_risk_present_allows_rejected_direction_when_trader_proposed_hold dual_risk_unknown_requires_indeterminate_prefix
```

Expected: FAIL because `validation.rs` still has no dual-risk context, no first-line parser, and no same-direction rejection rule.

- [x] **Step 3: Change the validator signature and remove deterministic helpers**

In `src/agents/fund_manager/validation.rs`:

```rust
use crate::agents::risk::DualRiskStatus;
use crate::state::proposal::TradeAction;

pub(super) fn parse_and_validate_execution_status(
    raw_output: &str,
    requires_missing_data_acknowledgment: bool,
    target_date: &str,
    dual_risk_status: DualRiskStatus,
    trader_proposal_action: TradeAction,
) -> Result<ExecutionStatus, TradingError>
```

Import path specifics:
- `DualRiskStatus` comes from the `pub(crate)` re-export added in Task 1 Step 4 at `crate::agents::risk::DualRiskStatus`. Do **not** import directly from `crate::agents::risk::common` — the re-export is the stable path.
- `TradeAction` lives in `crate::state::proposal::TradeAction`. Do not create a local alias.

Delete:

```rust
pub(super) const DETERMINISTIC_REJECT_RATIONALE: &str = ...
pub(super) fn deterministic_reject(state: &TradingState) -> bool { ... }
```

Also remove the now-unused `TradingState` import if this file no longer needs it outside the missing-input helpers. Post-deletion, run `rg -n 'deterministic_reject|DETERMINISTIC_REJECT_RATIONALE' src/` — it must return zero matches. Any stray `pub(super)` re-export of either symbol will surface here.

- [x] **Step 4: Add dedicated first-line parsing helpers**

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

- [x] **Step 5: Encode the exact disposition and action-direction contract**

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

- [x] **Step 6: Keep the generic `ExecutionStatus` validation minimal**

`validate_execution_status()` should still only guard:

```text
- non-empty rationale
- max rationale length
- control-character rejection
```

`MAX_RATIONALE_CHARS` in `src/constants.rs` is currently `usize::MAX` by design, so the `chars().count() > MAX_RATIONALE_CHARS` check is a pass-through today. Preserve it unchanged — do not introduce a new bound in this change.

All dual-risk-specific logic belongs in the dedicated helper invoked from `parse_and_validate_execution_status()`.

- [x] **Step 7: Re-run the focused validator matrix**

Run the command from Step 2.

Expected: the validator matrix tests PASS in isolation **only once Task 4 Step 3 also lands** — `agent.rs` calls the 4-arg `build_prompt_context` but `prompt.rs` still has the 3-arg signature at this point, so the crate will not yet compile. This is the expected intermediate red state carried over from Task 2 Step 6. Do **not** attempt to commit here; proceed directly to Task 4.

If the implementing agent wants to confirm validator-matrix greenness in isolation before Task 4, they may temporarily run `cargo nextest run --all-features --locked --no-fail-fast --test-threads=1 -p scorpio-analyst` with a throwaway 4-arg `build_prompt_context` shim; but this is optional and not required by the plan.

- [x] **Step 8: Do not commit yet — combined commit lands in Task 4 Step 7**

The `feat(fund-manager): enforce dual-risk judgment contract` commit now covers `agent.rs`, `mod.rs`, `validation.rs`, `prompt.rs`, and `tests.rs` together to avoid a non-compiling intermediate commit. See Task 4 Step 7.

### Task 4: Rebuild the Fund Manager prompt contract around `DualRiskStatus`

**Files:**
- Modify: `src/agents/fund_manager/prompt.rs`
- Modify: `src/agents/fund_manager/tests.rs`

- [x] **Step 1: Write the failing prompt-contract tests first**

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

For the drift guard, assert **both** Fund Manager prompt surfaces avoid these strings (case-insensitive) — narrowed to the specific dual-risk-rejection phrasing so the preserved "pre-computed deterministic valuation" instruction does not false-positive:

```text
must reject
automatic rejection
deterministic rejection
deterministic reject
deterministic safety rule
required to reject
mandatory rejection
presumptive rejection
```

Surfaces to check (in this file's scope only):
- `FUND_MANAGER_SYSTEM_PROMPT`
- assembled Fund Manager user prompt via `build_user_prompt(...)`

Do **not** reach into `src/agents/risk/moderator.rs` from this test — `RISK_MODERATOR_SYSTEM_PROMPT` is a private `const`, not `pub`, and adding a visibility hack just to assert from a sibling module is abstraction without runtime value. The moderator drift guard added in Task 1 Step 5 (`risk_moderator_prompt_drift_guard_forbids_deterministic_phrases`) already covers that surface in its own module.

- [x] **Step 2: Run the focused prompt-contract slice to confirm the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast fund_manager_prompt_includes_present_indicator_near_top fund_manager_prompt_includes_absent_indicator_near_top fund_manager_prompt_uses_unknown_indicator_when_report_missing fund_manager_prompt_places_unknown_indicator_near_top fund_manager_system_prompt_contains_exact_first_line_contract fund_manager_prompt_drift_guard_forbids_deterministic_phrases
```

Expected: FAIL because the current prompt still says “Apply the deterministic safety rule” and has no tri-state indicator line.

- [x] **Step 3: Update the `build_prompt_context()` signature and thread `DualRiskStatus` through the user prompt**

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

- [x] **Step 4: Replace the deterministic system-prompt instruction with the exact contract**

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

Keep valuation and missing-data instructions intact. Specifically, the existing instruction line `Ground the decision in the pre-computed deterministic valuation provided in the user context` must remain verbatim — the phrase `pre-computed deterministic valuation` is asserted by the pre-existing test `fund_manager_system_prompt_references_precomputed_valuation` and is the exact reason the drift-guard forbidden list above uses `deterministic rejection`/`deterministic reject`/`deterministic safety rule` rather than bare `deterministic`.

- [x] **Step 5: Insert the tri-state indicator near the top of the user prompt**

In `build_user_prompt()`, add this immediately after the “Produce an ExecutionStatus JSON...” line:

```rust
push_bounded_line(
    &mut prompt,
    &format!("Dual-risk escalation: {}", dual_risk_status.as_prompt_value()),
    MAX_USER_PROMPT_CHARS,
);
```

Do not move this below serialized reports or valuation context.

- [x] **Step 6: Re-run the focused prompt-contract slice**

Run the command from Step 2.

Expected: PASS.

- [x] **Step 7: Combined commit for Tasks 2, 3, and 4**

This is the atomic commit that lands `agent.rs`, `mod.rs`, `validation.rs`, `prompt.rs`, and `tests.rs` together. It replaces the separate commits originally planned at Task 3 Step 8 and Task 4 Step 7, to avoid a non-compiling intermediate tree.

Before committing, verify the crate compiles and the full focused matrix is green:

```bash
cargo build --tests
cargo nextest run --all-features --locked --no-fail-fast -E 'test(=dual_violation_still_invokes_llm_path) + test(=llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status) + test(=dual_risk_present_accepts_upheld_reject) + test(=dual_risk_present_accepts_deferred_approved_hold) + test(=dual_risk_present_accepts_overridden_directional_approval) + test(=dual_risk_present_rejects_missing_first_line_prefix) + test(=dual_risk_present_rejects_wrong_disposition_for_approved_hold) + test(=dual_risk_present_rejects_prefix_when_not_first_line) + test(=dual_risk_present_rejects_lowercase_prefix_variant) + test(=dual_risk_present_rejects_mixed_case_prefix_variant) + test(=dual_risk_present_rejects_em_dash_prefix_variant) + test(=dual_risk_present_rejects_markdown_fenced_prefix) + test(=dual_risk_present_rejects_two_leading_newlines_before_prefix) + test(=dual_risk_present_allows_single_leading_newline_before_prefix) + test(=dual_risk_present_rejects_same_direction_reject_for_buy) + test(=dual_risk_present_rejects_same_direction_reject_for_sell) + test(=dual_risk_present_allows_rejected_hold_against_directional_proposal) + test(=dual_risk_present_allows_rejected_direction_when_trader_proposed_hold) + test(=dual_risk_unknown_requires_indeterminate_prefix) + test(=fund_manager_prompt_includes_present_indicator_near_top) + test(=fund_manager_prompt_includes_absent_indicator_near_top) + test(=fund_manager_prompt_uses_unknown_indicator_when_report_missing) + test(=fund_manager_prompt_places_unknown_indicator_near_top) + test(=fund_manager_system_prompt_contains_exact_first_line_contract) + test(=fund_manager_prompt_drift_guard_forbids_deterministic_phrases)'
```

Note: the `-E 'test(=<name>) + test(=<name>)'` form uses nextest's exact-filter expression language so substring collisions cannot silently match (or fail to match) unintended tests. Do not rely on the space-separated substring filter shown in earlier focused-slice steps when correctness depends on exact binding.

Then commit:

```bash
git add src/agents/fund_manager/agent.rs src/agents/fund_manager/mod.rs src/agents/fund_manager/validation.rs src/agents/fund_manager/prompt.rs src/agents/fund_manager/tests.rs
git commit -m "feat(fund-manager): enforce dual-risk judgment contract"
```

---

## Chunk 2: Current-Behavior Docs and Full Verification

### Task 5: Update current behavior docs and spec references

**Files:**
- Modify: `docs/prompts.md`
- Modify: `PRD.md`
- Modify: `openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md`

- [x] **Step 1: Update `docs/prompts.md`**

Replace the deterministic wording in both sections.

Risk Moderator section should say the moderator records dual-risk escalation status for downstream review, not deterministic rejection.

Fund Manager section should say, in substance:

```text
If the prompt says `Dual-risk escalation: present`, weigh it as a high-severity signal.
Do not treat it as automatic rejection.
The first line of `rationale` must classify the outcome using the approved exact form.
```

- [x] **Step 2: Update `PRD.md` to current behavior truth**

Replace:

```text
The graph terminates at the Fund Manager node, which executes a deterministic logic check across the three risk reports to approve or reject the trade.
```

with wording in this shape:

```text
The graph terminates at the Fund Manager node, which uses the deep-thinking model to render the final decision after weighing the trader proposal, risk reports, and analyst context; dual Conservative+Neutral objection is treated as a high-severity escalation signal, not a deterministic reject rule.
```

- [x] **Step 3: Update the current OpenSpec wrapper scenario**

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

- [x] **Step 4: Sanity-check the canonical file list for leftover deterministic wording**

Run:

```bash
rg -n -i "must reject|automatic rejection|deterministic rejection|deterministic reject|deterministic safety rule|required to reject|mandatory rejection|presumptive rejection" \
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

Expected: no matches. The regex matches the same narrowed set as the Task 1 and Task 4 drift-guard tests — bare `deterministic` and bare `safety rule` are intentionally excluded because Task 4 Step 4 preserves the phrase `pre-computed deterministic valuation` in the Fund Manager system prompt (see Task 4 Step 4: "Keep valuation and missing-data instructions intact"). Test files are excluded because drift-guard tests contain the forbidden list as literals. Historical/archive files are out of scope and may still match.

- [x] **Step 5: Commit the current-behavior doc/spec updates**

```bash
git add docs/prompts.md PRD.md openspec/changes/add-graph-orchestration/specs/graph-orchestration/spec.md
git commit -m "docs: align dual-risk behavior with fund manager judgment"
```

### Task 6: Run full verification and capture the final state

**Files:**
- Modify: none expected

- [x] **Step 1: Run the focused Fund Manager contract tests**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_violation_still_invokes_llm_path llm_retry_exhaustion_under_dual_risk_returns_typed_error_without_fallback_status dual_risk_present_accepts_upheld_reject dual_risk_present_accepts_deferred_approved_hold dual_risk_present_accepts_overridden_directional_approval dual_risk_present_rejects_missing_first_line_prefix dual_risk_present_rejects_wrong_disposition_for_approved_hold dual_risk_present_rejects_prefix_when_not_first_line dual_risk_present_rejects_lowercase_prefix_variant dual_risk_present_rejects_mixed_case_prefix_variant dual_risk_present_rejects_em_dash_prefix_variant dual_risk_present_rejects_markdown_fenced_prefix dual_risk_present_rejects_two_leading_newlines_before_prefix dual_risk_present_allows_single_leading_newline_before_prefix dual_risk_present_rejects_same_direction_reject_for_buy dual_risk_present_rejects_same_direction_reject_for_sell dual_risk_present_allows_rejected_hold_against_directional_proposal dual_risk_present_allows_rejected_direction_when_trader_proposed_hold dual_risk_unknown_requires_indeterminate_prefix fund_manager_prompt_includes_present_indicator_near_top fund_manager_prompt_includes_absent_indicator_near_top fund_manager_prompt_uses_unknown_indicator_when_report_missing fund_manager_prompt_places_unknown_indicator_near_top fund_manager_system_prompt_contains_exact_first_line_contract fund_manager_prompt_drift_guard_forbids_deterministic_phrases
```

Expected: PASS.

- [x] **Step 2: Run the focused Risk Moderator / common helper slice**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast dual_risk_status_is_present_when_both_reports_flag_violation dual_risk_status_is_absent_when_both_reports_exist_but_not_both_flagged dual_risk_status_is_unknown_when_either_report_is_missing expected_moderator_violation_sentence_is_tri_state validate_moderator_output_accepts_unknown_sentence validate_moderator_output_rejects_wrong_sentence_for_present validate_moderator_output_rejects_wrong_sentence_for_absent validate_moderator_output_rejects_wrong_sentence_for_unknown run_synthesis_mentions_conservative_and_neutral_violation risk_moderator_prompt_drift_guard_forbids_deterministic_phrases
```

Expected: PASS.

- [x] **Step 2.5: Confirm the comparison base exists before the final diff check**

Run:

```bash
git fetch origin main --quiet || true
BASE_REF=$(git rev-parse --verify origin/main 2>/dev/null || git rev-parse --verify main)
echo "BASE_REF=$BASE_REF"
```

Expected: prints a commit hash. The `|| true` and fallback chain tolerate airgapped or sandboxed worker environments where `origin` is unreachable (offline reviewers, CI sandboxes, isolated worktrees); the local `main` branch is an acceptable diff base in those cases. Export the resolved ref for use in Step 6.

- [x] **Step 3: Run formatting**

Run:

```bash
cargo fmt -- --check
```

Expected: exit 0.

- [x] **Step 4: Run clippy**

Run:

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: PASS with zero warnings.

- [x] **Step 5: Run the full CI-equivalent nextest command**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast
```

Expected: PASS.

- [x] **Step 6: Review the final diff against the implementation scope**

Run (substituting the `BASE_REF` resolved in Step 2.5):

```bash
git diff --stat "$BASE_REF"...HEAD
git status --short
```

Expected:
- diff stat shows only the files listed in the File Map
- `git status --short` is clean unless verification required a final unstaged fix

If `BASE_REF` was not exported in this shell session, resolve it inline with the same fallback chain: `BASE_REF=$(git rev-parse --verify origin/main 2>/dev/null || git rev-parse --verify main)`.

- [x] **Step 7: Final commit if verification required follow-up edits**

If any verification step required code changes, create one final commit describing the verification fix, for example:

```bash
git add <exact files>
git commit -m "test: fix dual-risk escalation verification coverage"
```

If no follow-up edits were needed, skip this step.
