---
title: "Synonym Wrapper Functions: Six Patterns and How to Remove Them"
date: 2026-06-04
category: docs/solutions/conventions/
module: providers/factory
problem_type: convention
component: testing_framework
severity: medium
applies_when:
  - A method's entire body accesses a single public field (accessor synonym)
  - A wrapper delegates through a chain of wrappers before reaching real logic
  - A test-only method on a production struct is a thin delegator to another function or field
  - A test-infrastructure constructor just binds one argument and forwards to a general constructor
  - A module function fixes only constant arguments and forwards to the underlying function
tags:
  - rust
  - refactoring
  - synonym-wrappers
  - test-infrastructure
  - accessor-methods
  - public-fields
  - providers
  - factory
  - mock
---

# Synonym Wrapper Functions: Six Patterns and How to Remove Them

## Context

The `crates/scorpio-core/src/providers/factory/` module accumulated synonym wrapper functions
over a multi-week development period. As `LlmAgent` matured — gaining `prompt_details`,
`prompt_typed_details`, `prompt_text_details`, and `chat_details` — an earlier generation of
wrapper names (`prompt`, `prompt_with_retry`, `prompt_with_retry_details`,
`prompt_with_retry_budget`, `prepare_attempt_text`) remained and mechanically delegated to the
newer API. A parallel accumulation happened in test infrastructure: `MockLlmAgentController`
grew wrapper methods (`push_typed_ok`, `set_prompt_delay`, `observed_history_lengths`), an
`agent_test_support.rs` module duplicated `mock_llm_agent_with_provider` with an identical
signature, and `mock_prompt_response` was a one-liner over `PromptResponse::new`.

The session applied the [[no-synonym-wrapper-functions]] rule across all six synonym shapes that
appeared in this module, eliminating ~230 lines of delegating code across `agent.rs`,
`retry.rs`, `text_retry.rs`, and a deleted `agent_test_support.rs` with no behavioral change.
All 2071 tests continued to pass.

## Guidance

A synonym wrapper is a function whose body is a single delegating call to one other in-scope
function. It adds a second name for one behavior and nothing else. Six distinct shapes appeared
in this codebase.

### Pattern 1 — Return-type slice on a delegating method

A method that calls a richer sibling and discards part of the return value.

```rust
// BEFORE: wrapper strips .output and re-labels the return type
pub async fn prompt(&self, prompt: &str) -> Result<String, PromptError> {
    Ok(self.prompt_details(prompt).await?.output)
}

// AFTER: deleted; call sites write the extraction inline
let text = agent.prompt_details("...").await?.output;
```

The same shape appeared in the retry layer: `prompt_with_retry` returned
`RetryOutcome<String>` by calling into `retry_prompt_budget_loop` with
`|| async { Ok(agent.prompt_details(prompt).await?.output) }`. All intermediate layers
collapsed into callers using `retry_prompt_budget_loop` directly with the closure spelling
out the `.output` extraction once.

### Pattern 2 — Budget-binding wrappers in a layered chain

A public entry point that only computes a value and immediately forwards it, stacked multiple
levels deep.

```rust
// BEFORE: three-level chain
pub async fn prompt_with_retry_details(
    agent: &LlmAgent, prompt: &str, timeout: Duration, policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    let total_budget = policy.total_budget(timeout);
    retry_prompt_budget_loop(agent, timeout, total_budget, policy,
        || agent.prompt_details(prompt)).await
}
pub(crate) async fn prompt_with_retry_details_budget(
    agent: &LlmAgent, prompt: &str, timeout: Duration,
    total_budget: Duration, policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    retry_prompt_budget_loop(agent, timeout, total_budget, policy,
        || agent.prompt_details(prompt)).await
}
async fn retry_prompt_budget_loop<R, F, Fut>(...) -> ... { /* real logic */ }

// AFTER: core is pub; callers compute budget inline
pub async fn retry_prompt_budget_loop<R, F, Fut>(
    agent: &LlmAgent, timeout: Duration, total_budget: Duration,
    policy: &RetryPolicy, call_fn: F,
) -> Result<RetryOutcome<R>, TradingError>
where F: Fn() -> Fut, Fut: Future<Output = Result<R, PromptError>>
{ /* real logic */ }

// call site
let total_budget = policy.total_budget(timeout);
retry_prompt_budget_loop(&agent, timeout, total_budget, &policy,
    || agent.prompt_details(prompt)).await
```

The key insight: when the intermediate layers exist because callers varied in one detail
(return type, budget source), that difference should be a closure parameter at the canonical
function boundary, not parallel named entry points.

### Pattern 3 — Single-callee module function with fixed constant arguments

`prepare_attempt_text` was `prepare_attempt` with four log-message strings bound into a struct
literal. The bound values were constants, not meaningful runtime defaults.

```rust
// BEFORE: wrapper binds four constant strings into a struct
pub(super) async fn prepare_attempt_text(
    agent: &LlmAgent, started_at: Instant, timeout: Duration,
    total_budget: Duration, policy: &RetryPolicy, attempt: u32,
) -> Result<AttemptBudget, TradingError> {
    prepare_attempt(agent, started_at, timeout, total_budget, policy, attempt,
        &RetryMessages {
            retrying:       "retrying text prompt after transient error",
            retry_budget:   "text prompt retry budget exhausted before next attempt",
            acquire_budget: "text prompt budget exhausted before rate-limit acquire",
            exhausted:      "text prompt retry budget exhausted",
        }).await
}

// AFTER: extract the constant, delete the wrapper, pass it at call sites
pub(super) const TEXT_RETRY_MESSAGES: RetryMessages = RetryMessages {
    retrying:       "retrying text prompt after transient error",
    retry_budget:   "text prompt retry budget exhausted before next attempt",
    acquire_budget: "text prompt budget exhausted before rate-limit acquire",
    exhausted:      "text prompt retry budget exhausted",
};

// text_retry.rs call site
prepare_attempt(agent, started_at, timeout, total_budget, policy, attempt,
    &TEXT_RETRY_MESSAGES).await?;
```

The argument-binding exemption applies only when the bound value is a meaningful runtime
default (a resolved path, a configured setting). Binding constant strings is better expressed
as a named `const`.

### Pattern 4 — Accessor methods over public fields

`LlmAgent` had `provider_name()`, `model_id()`, and `rate_limiter()` — each a one-line getter.
The getter adds only visibility promotion, which `pub` on the field provides more honestly.

```rust
// BEFORE: getter methods over private fields
pub struct LlmAgent {
    provider: ProviderId,
    model_id: String,
    rate_limiter: Option<SharedRateLimiter>,
    inner: LlmAgentInner,      // internal dispatch — stays private
}
impl LlmAgent {
    pub fn provider_name(&self) -> &'static str { self.provider.as_str() }
    pub fn model_id(&self) -> &str              { &self.model_id }
    pub fn rate_limiter(&self) -> Option<&SharedRateLimiter> { self.rate_limiter.as_ref() }
}

// AFTER: fields are pub; methods deleted
pub struct LlmAgent {
    pub provider:     ProviderId,
    pub model_id:     String,
    pub rate_limiter: Option<SharedRateLimiter>,
    inner:            LlmAgentInner,  // stays private: callers have no business here
}

// call site
warn!("...", provider = agent.provider.as_str(), model = agent.model_id.as_str(), ...);
```

The boundary added by a getter is valuable only when the field must stay private. When there is
no reason to hide the field, `pub field` is the honest choice.

### Pattern 5 — Test-control methods on the production type

`LlmAgent` had 10 `#[cfg(test)]` methods — `push_typed_ok/error`, `push_text_turn_ok/error`,
`set_prompt_delay`, `set_text_turn_delay`, `typed_attempts`, `prompt_attempts`,
`text_turn_attempts`, `observed_max_turns` — each a thin wrapper over a mutex-guarded field on
the underlying mock. These are doubly wrong: test concern inside the production type, and a
synonym wrapper over the field access.

```rust
// BEFORE: test methods on LlmAgent
#[cfg(test)]
impl LlmAgent {
    pub(crate) fn push_typed_ok<T: Send + 'static>(&self, r: TypedPromptResponse<T>) {
        if let LlmAgentInner::Mock(m) = &self.inner {
            m.typed_results.lock().unwrap().push_back(Ok(Box::new(r)));
        }
    }
    pub(crate) fn set_prompt_delay(&self, delay: Duration) {
        if let LlmAgentInner::Mock(m) = &self.inner {
            *m.prompt_delay.lock().unwrap() = delay;
        }
    }
}

// AFTER: pub(crate) fields on MockLlmAgentController; tests access directly
pub(crate) struct MockLlmAgentController {
    pub(crate) typed_results:  TypedResultQueue,
    pub(crate) prompt_delay:   Arc<Mutex<Duration>>,
    // ...
}

// test code
ctrl.typed_results.lock().unwrap().push_back(Ok(Box::new(response)));
*ctrl.prompt_delay.lock().unwrap() = Duration::from_millis(25);
```

`MockLlmAgentController` is the test-only handle returned alongside the agent by
`mock_llm_agent`. It is the right home for test-control state; the production struct is not.

### Pattern 6 — Test-infrastructure synonyms (helper aliases)

Three separate aliases appeared in test scaffolding.

```rust
// BEFORE: three separate aliases

// alias 1: wraps PromptResponse::new with no added behavior
pub(crate) fn mock_prompt_response(output: &str, usage: Usage) -> PromptResponse {
    PromptResponse::new(output, usage)
}

// alias 2: binds ProviderId::OpenAI to the general constructor
pub(crate) fn mock_llm_agent(model_id: &str, ...) -> (LlmAgent, MockLlmAgentController) {
    mock_llm_agent_with_provider_id(ProviderId::OpenAI, model_id, ...)
}

// alias 3: in agent_test_support.rs — identical signature to mock_llm_agent_with_provider_id
pub(crate) fn mock_llm_agent_with_provider(provider: ProviderId, model_id: &str, ...)
    -> (LlmAgent, MockLlmAgentController) {
    mock_llm_agent_with_provider_id(provider, model_id, ...)
}

// AFTER: all three aliases deleted
// alias 1 → call sites use PromptResponse::new directly
// alias 2+3 → one canonical factory; provider is explicit
pub(crate) fn mock_llm_agent(
    provider: ProviderId, model_id: &str,
    prompt_results: Vec<Result<PromptResponse, PromptError>>,
    chat_results: Vec<MockChatOutcome>,
) -> (LlmAgent, MockLlmAgentController) { ... }

// call site
let (agent, ctrl) = mock_llm_agent(ProviderId::OpenAI, "o3", vec![], vec![]);
```

`agent_test_support.rs`, whose only content was alias 3, was deleted entirely.

## Why This Matters

**Misleading specificity.** `prompt_with_retry` vs `prompt_with_retry_details`,
`prepare_attempt_text` vs `prepare_attempt` — each pair implies a distinction that does not
exist. A reader spends time looking for the difference and may "fix a bug in the retry path"
by editing only one synonym while the other stays stale.

**Drift surface.** Two names for one rule is two places that can fall out of sync. The
`mock_llm_agent` / `mock_llm_agent_with_provider_id` split already encoded a false assumption
that the provider should always be OpenAI. When a test needed a non-OpenAI provider it had to
reach past the public alias to the `_with_provider_id` variant — a future author might have
re-added the assumption by editing only the short alias.

**Test-concern leak.** `push_typed_ok`, `set_prompt_delay` on the production `LlmAgent` struct
advertise a test API on a production type. Once `MockLlmAgentController` owns those fields and
the production struct does not, the boundary is clear.

**Scale.** Removing the six synonym families eliminated ~230 lines of delegating code across
five files with no behavioral change.

## When to Apply

Remove the function when all three conditions hold:

1. **The body is a single delegating expression.** One call to another function, with at most
   a trivial post-fix (`.output`, `.is_some()`, `.unwrap_or_default()`). No branching, no
   logging, no local variable assignment beyond the call.

2. **No signature value is added.** The wrapper's parameters and return type are the callee's.
   It does not convert types, map errors, supply a meaningful default, or perform partial
   application over a non-trivial runtime value. Binding constant strings into a struct
   (Pattern 3) is better expressed as a named `const`.

3. **No deliberate boundary is added.** It is not a `pub` re-export establishing an
   intentional public API surface, and not a trait-method delegation required by the trait
   contract.

The distinguishing test: *"Delete the wrapper and call the callee directly — does anything of
value disappear (a type conversion, an error map, a default, an argument binding, a visibility
boundary, a trait obligation)?"* If the only loss is one name, it was a synonym.

**When a layered chain collapses**, find the deepest function that does real work and wire
callers to it directly. Delete every level in between, not just the top layer.

## Related

- [Cascade dead-code deletion across lib and test targets](../design-patterns/cascade-dead-code-deletion-across-lib-and-test-targets.md)
  — After inlining synonyms, follow this to chase the downstream dead-code cascade that
  deletion triggers in lib-vs-test build targets.
- [Mock at the right seam, not in production](../design-patterns/mock-at-the-right-seam-not-in-production.md)
  — When removing test-control methods from production types (Pattern 5), this rule governs
  where test substitution belongs instead.
- [Infallible constructor when failure is process-fatal](../design-patterns/infallible-constructor-when-failure-is-process-fatal.md)
  — Sibling "delete the false-affordance slot" pattern: redundant error-handling slots vs.
  redundant naming/indirection slots.
- `.claude/rules/no-synonym-wrapper-functions.md` — the authoritative rule file; updated this
  session to document all six patterns with before/after examples.
- `.claude/rules/no-write-only-placeholder-fields.md` — sibling rule for redundant data
  slots, often orphaned as a secondary cascade when synonyms are removed.
