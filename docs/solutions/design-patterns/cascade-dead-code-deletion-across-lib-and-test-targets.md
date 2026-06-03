---
title: "Cascade dead-code deletion across lib and test targets (clippy --all-targets)"
date: 2026-06-03
category: design-patterns
module: rust-dead-code-removal
problem_type: design_pattern
component: development_workflow
severity: high
applies_when:
  - "deleting a pub wrapper, orchestrator, or re-export from a Rust library crate"
  - "removing synonym-wrapper or dead/obsolete functions during a workspace cleanup"
  - "a private or pub(crate) helper's only remaining callers live inside #[cfg(test)] modules"
  - "CI gates on cargo clippy --workspace --all-targets -- -D warnings"
  - "tempted to #[cfg(test)]-gate a helper to preserve test coverage after deleting its caller"
tags:
  - rust
  - clippy
  - dead-code
  - cfg-test
  - lib-vs-test-target
  - safe-deletion
  - cargo-workspace
related_components:
  - tooling
  - testing_framework
---

# Cascade dead-code deletion across lib and test targets (clippy --all-targets)

## Context

This came out of a workspace-wide cleanup pass (commit `3a3fb87`, "remove synonym
wrappers and dead/obsolete code") run against
[`.claude/rules/no-synonym-wrapper-functions.md`](../../../.claude/rules/no-synonym-wrapper-functions.md)
and a broader sweep for unreachable code across all four crates — net `+96/−2750`
lines. Several deletions targeted superseded `pub` orchestrators that the dynamic
per-task pipeline had replaced: `run_analyst_team`, `run_risk_discussion`, and the
dead `chat_with_retry` chat-prompt wrapper. Each looked completely safe to delete —
they were `pub`, had zero production callers, and removing them produced a
clean-looking diff.

The recurring surprise: after deleting the visibly-dead `pub` function and assuming
the job was done, `cargo clippy --workspace --all-targets -- -D warnings` (the CI
gate) failed — **not on the symbol that was deleted, but on a different, untouched
private symbol downstream.** A `pub` wrapper silently masks the deadness of
everything it privately calls, so its removal converts those private callees into
lib-build dead code all at once. The clean-looking deletion was the first domino in
a cascade, and clippy reported only the next domino, one symbol at a time.

## Guidance

Follow this checklist whenever you delete a wrapper, orchestrator, or any function
with non-trivial private callees:

1. **Before deleting, grep each callee's non-test reachability and note its
   visibility.** A `pub` item in a *library* crate is exempt from the `dead_code`
   lint (assumed external API), so its deadness is invisible. A `fn` (private) or
   `pub(crate)` item is *not* exempt — once its last non-test caller disappears,
   clippy flags it. For every function the wrapper calls, ask: "after this deletion,
   does any caller outside `#[cfg(test)]` remain?"

   ```rust
   // run_risk_discussion (pub) was the ONLY non-test caller of this private helper:
   async fn run_risk_discussion_with_executor<E>(/* ... */) -> Result<_> { /* the loop */ }
   // delete the pub wrapper → this private fn is reachable only from #[cfg(test)] → lib-build dead.
   ```

2. **Cascade the deletion.** Delete every callee, test, struct field, or import that
   becomes test-only as a result. The cascade can be several layers deep: deleting
   `run_analyst_team` (pub) orphaned `apply_analyst_results` (`pub(crate)`) and
   `flatten_task_result` (private), which in turn orphaned their tests and the
   `sample_*` fixtures those tests used. Deleting `chat_with_retry` (pub) orphaned
   `chat_with_retry_budget` (`pub(crate)`), and deleting its one test orphaned a
   private mock field `observed_history_ptrs` (written in the mock, read only by that
   test) plus its plumbing sites.

3. **Do NOT `#[cfg(test)]`-gate a helper just to preserve its tests.** This is the
   tempting "fix" and it is wrong. A test that exercises only test-only code verifies
   nothing about production — exactly the project's
   [mock-at-the-right-seam-not-in-production](mock-at-the-right-seam-not-in-production.md)
   rule. An adversarial reviewer in this session explicitly advised "keep
   `run_risk_discussion_with_executor`, it's the live test seam"; that was incorrect
   because the helper is *private* and, post-deletion, reachable only from
   `#[cfg(test)]`. Delete the helper and its now-vacuous tests; don't quarantine them.

4. **For imports/consts genuinely needed by *surviving* tests, move them under
   `#[cfg(test)]` rather than deleting.** When a `use` was reached by non-test code
   only via the deleted orchestrator, but a real surviving test still needs it, gate
   the import instead of dropping it:

   ```rust
   // agents/analyst/mod.rs — only non-test-reachable through the deleted orchestrator,
   // but the surviving timeout tests still need them:
   #[cfg(test)]
   use crate::{config::LlmConfig, error::RetryPolicy};
   #[cfg(test)]
   use std::time::Duration;
   ```

5. **Run `cargo clippy --workspace --all-targets -- -D warnings` after each deletion
   batch — use it as the discovery tool, not just the final gate.** It compiles both
   the lib target (cfg-not-test) and the test target (cfg-test), and names the exact
   orphaned symbol/import/field. Treat each failure as the next domino, fix it, and
   re-run until green. Don't assume one clean-looking deletion is complete.

## Why This Matters

`cargo clippy --workspace --all-targets` compiles a crate's targets under *two
distinct cfg states*: the **lib build** (`cfg(test)` is false) and the **test
build** (`cfg(test)` is true). Dead-code analysis runs per target. A private or
`pub(crate)` helper called only from `#[cfg(test)]` blocks is *referenced* in the
test build but *unreferenced* in the lib build — so it's dead in the lib build, and
`-D warnings` turns that into a hard CI failure. Meanwhile `cargo test` /
`cargo nextest` compile only the test configuration, where the helper *is*
referenced, so they stay green. This is precisely why the failure surprises you:
your local test run passes, the deletion looks done, and the dedicated CI gate fails
on a symbol you never touched.

The `pub`-masks-deadness effect compounds the trap. In a library crate, `pub` items
are presumed external API, so the `dead_code` lint never fires on them — a dead
`pub fn` reports nothing in *either* build. The wrapper acts as a fake "live caller"
for its private callees right up until you delete it, at which point all of them
flip to dead in the lib build simultaneously.

The cost is concrete: repeated CI-gate failures on symbols you didn't edit, each
requiring another investigate-and-fix cycle. And the instinctive fix —
`#[cfg(test)]`-gating the orphaned helper so its tests still compile — preserves
tests that now assert nothing about the production path. Cascading the deletion is
the principled fix; gating is papering over vacuous coverage.

## When to Apply

- Deleting any `pub` wrapper, orchestrator, synonym function, or superseded entry
  point — especially in a **library crate**, where `pub` hides callee deadness.
- Removing obsolete or unreachable code where the deleted item had non-trivial
  private / `pub(crate)` callees (helpers, loop bodies, executors).
- Any time a deleted function's only remaining callers would live in `#[cfg(test)]`
  after the deletion — that callee is about to become lib-build dead.
- When a deleted test was the **sole reader** of a struct field, mock-controller
  field, fixture, or constant — clippy will report `field is never read` or
  `function is never used` for it next.
- When surviving tests still need an import/const previously reached through the
  deleted code — gate it with `#[cfg(test)] use` rather than deleting (and rather
  than reviving the dead code).

## Examples

**Case 1 — `run_risk_discussion` orphaning a private loop helper (the
wrong-reviewer-advice case).** The `pub async fn run_risk_discussion` was the only
non-test caller of the private `run_risk_discussion_with_executor<E>`, which held the
actual round loop. The advice to keep the helper as "the live test seam" was wrong:
it is private, so once the `pub` wrapper is gone the only references are in
`#[cfg(test)]`.

```rust
// BEFORE — pub wrapper masks the private helper's deadness:
pub async fn run_risk_discussion(state: &mut TradingState, /* ... */) -> Result<_> {
    let mut executor = RealRiskExecutor { /* ... */ };
    run_risk_discussion_with_executor(state, max_rounds, &mut executor).await   // sole prod caller
}
async fn run_risk_discussion_with_executor<E>(/* ... */) -> Result<_> { /* the loop */ }

// AFTER — wrapper + private helper + the loop-only test module + MockRiskExecutor all deleted.
// Deleting only run_risk_discussion would have produced:
//   error: function `run_risk_discussion_with_executor` is never used
```

**Case 2 — `#[cfg(test)] use` migration.** Deleting the analyst-team orchestrator
removed the only non-test reachability for several imports in
`agents/analyst/mod.rs`, but the timeout tests still needed them, so they were gated
rather than dropped:

```rust
#[cfg(test)]
use crate::{config::LlmConfig, error::RetryPolicy};
#[cfg(test)]
use std::time::Duration;
```

Without gating, the lib build emits `unused imports: RetryPolicy, LlmConfig` (and
`Duration`); deleting them outright instead breaks the surviving tests' compilation.

**Case 3 — deleted test orphaning a write-only mock field.** Deleting
`chat_with_retry` (pub) made `chat_with_retry_budget` (`pub(crate)`) test-only;
deleting its lone test left the private mock-controller field `observed_history_ptrs`
written but never read — `error: field observed_history_ptrs is never read`. The fix
removed the field and its plumbing, not just the test.

**The exact clippy strings that drive the cascade** (each names a symbol you did not
edit):

```text
error: function `X` is never used
error: method `X` is never used
error: field `Y` is never read
error: unused imports: `A`, `B`
```

## Related

This is the operational consequence that the repo's "delete the false-affordance
slot" rule family triggers — when you remove a redundant slot, this is what fans out
in the lib build:

- [`.claude/rules/no-synonym-wrapper-functions.md`](../../../.claude/rules/no-synonym-wrapper-functions.md) — deleting the *naming/indirection* slot (the wrapper itself); this doc covers the cascade that deletion sets off.
- [infallible-constructor-when-failure-is-process-fatal.md](infallible-constructor-when-failure-is-process-fatal.md) — nearest cascade precedent (deleting a `Result` false affordance and following `Option`/`match`/`?` removals downstream; also notes the test-isolation side effect).
- [mock-at-the-right-seam-not-in-production.md](mock-at-the-right-seam-not-in-production.md) — why you delete (not `#[cfg(test)]`-gate) a helper whose only remaining callers are tests.
- [`.claude/rules/no-write-only-placeholder-fields.md`](../../../.claude/rules/no-write-only-placeholder-fields.md) — the `field is never read` variant of the same cascade (write-only data/mock slots).
- CLAUDE.md §2 "Simplicity First" / §3 "Surgical Changes" — clean up only the orphans *your* deletion created.
