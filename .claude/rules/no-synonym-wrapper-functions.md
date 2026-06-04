# No Synonym Wrapper Functions

A function whose entire body is a call to **one other in-scope function**, with
the same behavior and an identical (or trivially renamed) signature, is a
**synonym wrapper**. It adds a second name for one behavior and nothing else.
Delete it and call the underlying function directly.

```rust
// ── banned: pure pass-through, adds no behavior ──────────────────────────────
fn should_retry_typed_error(err: &TradingError) -> bool {
    should_retry_trading_error(err)
}
fn should_retry_text_error(err: &TradingError) -> bool {
    should_retry_trading_error(err)
}
#[cfg(test)]
fn is_transient_error(err: &PromptError) -> bool {
    transient_prompt_error_summary(err).is_some()
}
```

A synonym wrapper is worse than no function:

- **Misleading specificity.** `should_retry_typed_error` / `should_retry_text_error`
  imply per-path retry logic that does not exist — both are the one
  `should_retry_trading_error`. A reader burns time looking for the difference,
  and the next author "fixes a bug for typed prompts" by editing one synonym,
  silently diverging the two paths that were supposed to be identical.
- **Drift surface.** Two names for one rule is two places that can fall out of
  sync. The wrapper invites exactly the divergence it pretends to abstract.
- **Indirection tax.** Every reader follows the call to discover it does
  nothing. That is pure cost (cf. CLAUDE.md §2 "Simplicity First" — "No
  abstractions for single-use code").

## The rule

Remove the function and inline the call when **all** are true:

1. **The body is a single delegating expression** — one call to another function
   (optionally with `.is_some()` / `.is_none()` / `.unwrap_or_default()` style
   trivial post-fixes), with no branching, logging, or local work.
2. **No signature value is added** — no type conversion at the boundary, no error
   mapping, no defaulting, no argument binding/partial application, no narrowing.
   The wrapper's parameters and return type are the callee's (modulo the trivial
   post-fix in #1).
3. **No deliberate boundary is added** — it is not a `pub` re-export that defines
   an intentional public API surface, and not a trait-method delegation required
   by the trait contract.

Inline it: replace `wrapper(x)` call sites with `callee(x)` (or
`callee(x).is_some()` etc.), then delete the wrapper. If the wrapper had its own
doc comment worth keeping, fold it onto the callee.

## When NOT to remove (the wrapper earns its keep)

Keep the function when it does real adapter work — these are **not** synonym
wrappers:

- **Type/shape adaptation** at the boundary: `fn validate(resp: &Dto) { let d:
  Domain = resp.clone().into(); validate_domain(&d) }` converts before
  delegating.
- **Error mapping / defaulting:** `serde_json::from_str(s).map_err(|e|
  SchemaViolation { .. })`, or supplying a default the callee lacks.
- **Argument binding (partial application):** the wrapper fixes one parameter so
  callers pass fewer (`fn save_user_config(c) { save_user_config_at(c,
  user_config_path()?) }`). The bound value must be a genuinely meaningful
  default — not just a trivial constant that callers should pass explicitly for
  clarity.
- **A `pub` re-export defining the crate's public API** over a private impl, or a
  **trait method** that delegates because the trait requires the method to exist.
- **A genuinely different return contract** (e.g. `Result` → `Option`, a
  narrowed newtype) that callers depend on.

The distinguishing test: *"Delete the wrapper and call the callee directly — does
anything of value disappear (a type conversion, an error map, a default, an
argument binding, a visibility boundary, a trait obligation)?"* If the only loss
is one name, it was a synonym — inline it.

## Accessor methods on struct fields

A method that only returns a struct field is a synonym wrapper for the field
itself. The fix is not a new wrapper — it is **making the field public** and
deleting the method.

```rust
// ── banned: accessor wrappers add nothing over pub fields ────────────────────
impl LlmAgent {
    pub fn provider_name(&self) -> &'static str { self.provider.as_str() }
    pub fn provider_id(&self) -> ProviderId     { self.provider }
    pub fn model_id(&self) -> &str              { &self.model_id }
    pub fn rate_limiter(&self) -> Option<&_>    { self.rate_limiter.as_ref() }
}

// ── correct: public fields, callers access directly ───────────────────────────
pub struct LlmAgent {
    pub provider:      ProviderId,
    pub model_id:      String,
    pub rate_limiter:  Option<SharedRateLimiter>,
    inner:             LlmAgentInner,   // stays private: callers have no business here
}
// call sites: agent.provider.as_str(), agent.model_id, agent.rate_limiter.as_ref()
```

The boundary added by a getter — visibility promotion — is genuinely valuable
when the field must stay private (e.g., `inner` above: exposing the dispatch enum
would couple callers to implementation variants). But when there is no reason
to hide the field, the getter is a synonym and `pub field` is the honest choice.

## Layered synonym chains

A chain A → B → C where each level is a synonym of the next collapses all at once,
not layer by layer. Find the deepest function that does real work and wire callers
to it directly; delete every level in between.

```
// ── banned chain ──────────────────────────────────────────────────────────────
pub fn prompt_with_retry(agent, prompt, timeout, policy)
    → prompt_with_retry_budget(agent, prompt, timeout, policy.total_budget(timeout), policy)
    → retry_prompt_budget_loop(agent, timeout, total_budget, policy, || agent.prompt_details(prompt))
    → [real loop logic]

// ── correct: callers reach the real function directly ─────────────────────────
pub fn retry_prompt_budget_loop(agent, timeout, total_budget, policy, call_fn)  // real work here
// callers:
let policy = RetryPolicy { … };
let timeout = Duration::from_millis(50);
retry_prompt_budget_loop(&agent, timeout, policy.total_budget(timeout), &policy,
    || agent.prompt_details(prompt)).await
```

When the intermediate layers exist because callers varied in *one detail*
(e.g. return type, budget source), the duplication is the signal: the difference
should be expressed as a closure or type parameter at the canonical function
boundary, not as parallel entry points.

## Test-infrastructure synonyms

Test code is not exempt. Mock helpers are as susceptible to the synonym smell as
production code.

```rust
// ── banned ────────────────────────────────────────────────────────────────────
fn mock_prompt_response(output: &str, usage: Usage) -> PromptResponse {
    PromptResponse::new(output, usage)      // pure synonym for PromptResponse::new
}
fn mock_llm_agent_with_provider(provider, model, prompts, chats)
    → mock_llm_agent_with_provider_id(provider, model, prompts, chats)   // identical signature
```

Delete and inline: call `PromptResponse::new(...)` and the canonical constructor
directly. When two constructors exist and one is just the other with a constant
bound, delete the shorter one and have callers pass the constant explicitly.

## Test-control methods on production types

Placing `#[cfg(test)]` control methods (`push_typed_ok`, `set_prompt_delay`,
`typed_attempts`, …) on a production struct is doubly wrong:

1. It puts test concerns inside the production type (see [[mock-at-the-right-seam-not-in-production]]).
2. Each method is a synonym wrapper for direct field access on the underlying mock.

The correct seam is a dedicated **controller** struct returned alongside the mock
by the constructor. With `pub(crate)` fields on the controller, callers access
mock state directly — no wrapper methods needed:

```rust
// ── banned: test methods on the production type ───────────────────────────────
impl LlmAgent {
    #[cfg(test)]
    pub(crate) fn push_typed_ok<T>(&self, r: TypedPromptResponse<T>) { … }
    #[cfg(test)]
    pub(crate) fn typed_attempts(&self) -> usize { … }
}

// ── correct: pub(crate) fields on the controller ──────────────────────────────
pub(crate) struct MockLlmAgentController {
    pub(crate) typed_results:  TypedResultQueue,
    pub(crate) typed_attempts: Arc<Mutex<usize>>,
    …
}
// call sites: ctrl.typed_results.lock().unwrap().push_back(Ok(Box::new(r)));
//             *ctrl.typed_attempts.lock().unwrap()
```

The same applies if wrapper methods exist on the controller itself — if every
method body is just `.lock().unwrap()` boilerplate, the methods are synonyms.
Make the fields `pub(crate)` and delete the methods.

## A related but distinct smell: redundant duplicate DI seams

An **argument-binding** wrapper is exempt above — it is *not* a synonym. But the
same wrapper can still be collapsible for a *different* reason: when it is a
**free-function dependency-injection seam that merely duplicates a deeper seam
the tests already use.** This is redundant indirection (CLAUDE.md §2), not a
synonym, and the fix is different — you don't just rename a call, you fold the
wrapper's real work inward and delete the duplicate seam.

The tell: a production entry point `outer(args)` binds a test-only injection
parameter to its sole production impl and forwards to `outer_with_dep<D>(args,
dep)`, whose body is real work (build a client, construct an object, then call a
**method** that is itself generic over the same `D`). If that method-level seam
is the one almost every test injects through, the free-function seam exists only
to serve a handful of tests (often one) and adds a second injection point for the
same dependency. Collapse it:

1. Inline `outer_with_dep`'s body into `outer` so `outer` does the real work
   directly and passes the production impl to the method seam. `outer` is now a
   genuine entry point, **not** a one-line delegator — the argument-binding
   exemption was always correct; the *indirection* was the problem.
2. Delete the redundant free-function seam, and delete or fold the lone test that
   used it (its coverage is subsumed by the method-seam tests plus the
   constructor's own tests).
3. Keep the method-level seam — that is the canonical injection point
   ([[mock-at-the-right-seam-not-in-production]]: one seam, not two).

Do **not** "fix" this by reclassifying argument binding as a synonym. The lesson
is narrower: *a DI seam that duplicates another DI seam for the same dependency
is redundant.*

### Example

`agents/trader/mod.rs` had `run_trader(state, config)` bind `&RigTraderInference`
and forward to `run_trader_with_inference<I>(state, config, inference)`, whose
body did `create_completion_model` + `TraderAgent::new` + `agent
.run_with_inference(state, inference)`. The method `run_with_inference<I:
TraderInference>` is the seam ~15 tests inject `StubInference` through; the
free-function seam `run_trader_with_inference` served exactly **one** test. It was
collapsed: `run_trader_with_inference` was deleted and its body inlined into
`run_trader` (which now builds the handle, constructs the agent, and runs via the
method seam with `&RigTraderInference`), and the lone duplicate-seam test was
removed (its assertions were covered by the proposal-writing method-seam test and
the `TraderAgent::new` model-tier test). `run_trader` is now real work, not a
delegator — and it never was a synonym.

## Worked examples

**`providers/factory/retry.rs` — synonym chain + two-name duplication.** The
module carried `should_retry_typed_error → should_retry_trading_error` and
`should_retry_text_error → should_retry_trading_error`; a third,
`#[cfg(test)] is_transient_error → transient_prompt_error_summary(err).is_some()`.
Also a three-level chain: `prompt_with_retry` → `prompt_with_retry_budget` →
`retry_prompt_budget_loop`, and a parallel `prompt_with_retry_details` →
`retry_prompt_budget_loop`. All were collapsed: `retry_prompt_budget_loop` became
`pub` and the canonical entry point; callers pass `|| agent.prompt_details(prompt)`
as a closure and compute `policy.total_budget(timeout)` inline. All intermediate
layers were deleted.

**`providers/factory/agent.rs` — accessor methods and test-control on production
type.** `LlmAgent` had four getter methods (`provider_name`, `provider_id`,
`model_id`, `rate_limiter`) and ten `#[cfg(test)]` control methods
(`push_typed_ok`, `set_prompt_delay`, `typed_attempts`, …). The getters were
replaced by `pub` fields; `MockLlmAgentController` was expanded with `pub(crate)`
fields so tests access mock state directly without wrapper methods.
`mock_prompt_response` (synonym for `PromptResponse::new`) and the old
`mock_llm_agent_with_provider` (identical signature to `mock_llm_agent_with_provider_id`)
were deleted. `agent_test_support.rs` — a module whose only content was one
wrapper function — was deleted entirely.

**`providers/factory/retry.rs` — `prepare_attempt_text`.** A `pub(super)` function
whose only body was `prepare_attempt(…, &RetryMessages { … })` with hardcoded
text-prompt messages. Deleted; the `RetryMessages` value was extracted to a
`pub(super) const TEXT_RETRY_MESSAGES` and callers pass it directly to
`prepare_attempt`.

See CLAUDE.md §2 "Simplicity First" / §3 "Surgical Changes", and the sibling
rules [[no-write-only-placeholder-fields]] (deletes redundant *data* slots),
[[infallible-constructor-for-process-fatal-failures]] (deletes redundant
*error-handling* slots), and [[mock-at-the-right-seam-not-in-production]] — this
one deletes redundant *naming/indirection* slots.
