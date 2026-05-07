---
title: Flaky LLM output aborts pipeline with SchemaViolation; recover via validator-aware retry and deterministic prepend
date: 2026-05-07
category: docs/solutions/runtime-errors
module: providers/factory + agents/{risk,researcher,analyst}
problem_type: runtime_error
component: service_object
symptoms:
  - "graph-flow error in phase 'risk_discussion' task 'risk_moderator': schema violation: RiskModerator: output must include exact violation-status sentence"
  - DeepSeek/OpenRouter deep-thinking model paraphrases required sentinel sentences, failing strict substring/regex validators
  - Single-shot SchemaViolation propagates immediately with no retry, aborting an entire analysis cycle after minutes of upstream work
  - Analyst text-fallback path (DeepSeek/OpenRouter, Gemini after typed failure) shares the same fail-fast shape on first malformed JSON
root_cause: missing_validation
resolution_type: code_fix
severity: high
related_components:
  - assistant
  - background_job
tags:
  - llm-retry
  - schema-violation
  - validator-aware-retry
  - deterministic-postprocess
  - risk-moderator
  - debate-moderator
  - analyst-text-fallback
  - deepseek
  - openrouter
  - rig-core
  - graph-flow
---

# Flaky LLM output aborts pipeline with SchemaViolation; recover via validator-aware retry and deterministic prepend

## Problem

Strict post-LLM validators (`validate_moderator_output`, analyst JSON parse + validate, debate consensus checks) returned `TradingError::SchemaViolation` on any deviation, surfaced directly to graph-flow as a fatal task error. With DeepSeek as the deep-thinking model — which paraphrases sentinel sentences and occasionally emits malformed JSON — a single rephrase killed the whole analysis cycle with no chance to recover.

## Symptoms

- Production cycle abort: `error="graph-flow error in phase 'risk_discussion' task 'risk_moderator': failed to run moderation: schema violation: RiskModerator: output must include exact violation-status sentence: \"Violation status: dual-risk escalation absent.\""`.
- The error class repeats across stages: risk moderator's required sentinel sentence, analyst JSON shape on the text-fallback path (DeepSeek/OpenRouter), debate moderator's stance/evidence/uncertainty checks.
- Every failure is single-shot — no retry, no corrective feedback, no graceful degradation. Minutes of upstream analyst + debate work are lost on one bad rephrase.

## What Didn't Work

The original design wrapped each LLM call in a strict post-validator and returned `SchemaViolation` on any mismatch. It was the natural choice (defense in depth: never trust the LLM, fail loudly on schema drift). It's wrong for two reasons:

1. The validator can't distinguish "model rephrased a known sentinel" from "model produced garbage" — they look identical at the validator boundary.
2. One-shot LLM calls have no self-correction window. Flaky-but-recoverable rephrasings get treated identically to genuine failures, and `should_retry_trading_error` explicitly classifies `SchemaViolation` as non-retryable (`crates/scorpio-core/src/providers/factory/retry.rs`) because the *same* prompt to the *same* model was assumed unlikely to recover.

## Solution

Two complementary changes — one for content the agent doesn't already know, one for content it does.

### 1. Validator-aware retry helpers

Added at the providers/factory layer. On a `SchemaViolation`, the loop retries with the violation message appended to the prompt as corrective feedback (`"IMPORTANT — your previous response was rejected: <message>. Please re-emit a corrected response that satisfies this requirement."`). Other validator errors propagate immediately without retry.

```rust
// crates/scorpio-core/src/providers/factory/retry.rs
pub async fn prompt_with_retry_validated_details<F>(
    agent: &LlmAgent,
    initial_prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
    validator: F,
) -> Result<RetryOutcome<PromptResponse>, TradingError>
where
    F: Fn(&str) -> Result<(), TradingError>;

// crates/scorpio-core/src/providers/factory/text_retry.rs
pub async fn prompt_text_with_retry_validated<F>(
    agent: &LlmAgent,
    initial_prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
    max_turns: usize,
    validator: F,
) -> Result<RetryOutcome<PromptResponse>, TradingError>
where
    F: Fn(&str) -> Result<(), TradingError>;
```

The text variant is tool-enabled (`prompt_text_details`) and is what analysts use on the DeepSeek/OpenRouter fallback path; the details variant uses the non-tool `prompt_details` path.

Wired in at:
- `crates/scorpio-core/src/agents/risk/moderator.rs` — risk moderator runs `validate_moderator_output_shape` inside the retry loop.
- `crates/scorpio-core/src/agents/researcher/moderator.rs` — debate moderator runs `validate_consensus_summary` inside the retry loop.
- `crates/scorpio-core/src/agents/analyst/equity/common.rs` — `run_text_fallback_inference` runs combined `parse + validate` inside the retry loop. Covers all four equity analysts (fundamental, sentiment, news, technical) on DeepSeek/OpenRouter, plus Gemini's text fallback after typed schema violations.

### 2. Deterministic prepend in the risk moderator

The "Violation status: dual-risk escalation {present|absent|unknown|stage-disabled}" sentence is fully computable from `DualRiskStatus`. The agent already knows the answer. Stop validating — prepend.

Before — `crates/scorpio-core/src/agents/risk/common.rs`:

```rust
fn validate_moderator_output(content: &str, status: DualRiskStatus) -> Result<(), TradingError> {
    // size + empty + control char + strict case-insensitive substring match
    // for the canonical violation-status sentence
}
```

After:

```rust
pub(super) fn validate_moderator_output_shape(content: &str) -> Result<(), TradingError> {
    // size + empty + control char only — no sentence check
}

pub(super) fn prepend_violation_status_if_missing(
    content: &str,
    status: DualRiskStatus,
) -> String {
    // case-insensitive contains check; if missing, prepend the canonical sentence
}
```

`build_moderator_result` in `crates/scorpio-core/src/agents/risk/moderator.rs` now computes the canonical sentence from `DualRiskStatus` and prepends it unconditionally if absent. Downstream consumers always see a normalised, machine-checkable first line regardless of how the LLM phrased the rest.

## Why This Works

Two failure classes, two tools:

- **Content the agent doesn't already know** (analyst JSON shape, debate stance/evidence/uncertainty structure): retry with corrective feedback. Flaky models very often self-correct given the validator's exact complaint — costs one extra round-trip vs. losing the entire run.
- **Content the agent already knows deterministically** (the violation-status sentence, computed from `DualRiskStatus`): prepend, don't validate. Asking the LLM to emit a value we can compute is wasted tokens and a wasted retry budget. The validator was checking "did the model echo back what we told it to" — pure ceremony.

The split keeps shape validation (size/empty/control char) intact — those *are* genuine schema concerns the model can violate in non-rephrasable ways, and the validator-aware retry now gives them a recovery window too.

## Prevention

When adding a new strict post-LLM validator, ask in this order:

1. **Do we already know the answer deterministically?** Prepend/inject it. Don't ask the LLM and then validate. Examples: status sentinels computed from typed state, fixed prefixes, known timestamps, dual-risk escalation status from `DualRiskStatus`.
2. **Is the requirement format-only (JSON shape, length, structure)?** Use `prompt_with_retry_validated_details` or `prompt_text_with_retry_validated`. The model can self-correct given its own complaint.
3. **Is retry genuinely incapable of helping?** Only then fail-fast with `SchemaViolation`. Rare — e.g. policy-level refusals where re-prompting won't change the answer.

Tests for new validators must include a "first response fails, second recovers" path — see `run_recovers_when_first_response_has_control_char` in `agents/risk/moderator.rs` and `run_analyst_inference_recovers_when_first_text_response_fails_validation` in `agents/analyst/equity/common.rs`. If that test is hard to write, the validator is probably too strict.

Existing trader (`prompt_typed_with_retry`) and fund manager (`parse_and_validate_execution_status` post-call) paths still use single-shot validation. Adopt the validator-aware retry pattern there as DeepSeek behaviour proves it out.

## Related Issues

- `docs/solutions/logic-errors/fund-manager-dual-risk-prefix-contract-follow-up-fixes-2026-04-21.md` — hardens a different strict validator (Fund Manager dual-risk prefix). Retry-aware variant of that pattern is a future follow-up; not contradicted by this change.
- `docs/solutions/logic-errors/prompt-bundle-centralization-runtime-contract-2026-04-25.md` — same prompt-rendering surface; unrelated concern (prompt source consolidation).
