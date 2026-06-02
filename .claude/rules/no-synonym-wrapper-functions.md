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
  user_config_path()?) }`).
- **A `pub` re-export defining the crate's public API** over a private impl, or a
  **trait method** that delegates because the trait requires the method to exist.
- **A genuinely different return contract** (e.g. `Result` → `Option`, a
  narrowed newtype) that callers depend on.

The distinguishing test: *"Delete the wrapper and call the callee directly — does
anything of value disappear (a type conversion, an error map, a default, an
argument binding, a visibility boundary, a trait obligation)?"* If the only loss
is one name, it was a synonym — inline it.

## Worked example

`providers/factory/retry.rs` carried `should_retry_typed_error(err) ->
should_retry_trading_error(err)` and `text_retry.rs` carried the identical
`should_retry_text_error(err) -> should_retry_trading_error(err)`; a third,
`#[cfg(test)] is_transient_error(err) -> transient_prompt_error_summary(err)
.is_some()`. None converted types, mapped errors, or added a boundary — each was
one alias for one behavior, and the `typed`/`text` names falsely implied
per-path logic. All three were deleted: the two `should_retry_*` call sites
(production and tests) now call `should_retry_trading_error` directly, and the
five `is_transient_error` test assertions call `transient_prompt_error_summary(&err)
.is_some()` / `.is_none()`. The `transient_prompt_error_summary` doc absorbed the
classification note that lived on the deleted wrapper.

See CLAUDE.md §2 "Simplicity First" / §3 "Surgical Changes", and the sibling
rules [[no-write-only-placeholder-fields]] (deletes redundant *data* slots),
[[infallible-constructor-for-process-fatal-failures]] (deletes redundant
*error-handling* slots), and [[mock-at-the-right-seam-not-in-production]] — this
one deletes redundant *naming/indirection* slots.
