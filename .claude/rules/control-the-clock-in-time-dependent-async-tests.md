# Control the Clock in Time-Dependent Async Tests

A test whose **asserted outcome depends on which of two short durations elapses
first** — a per-attempt timeout vs. a mock delay, a backoff vs. a total budget —
must not race the real wall-clock. Under parallel test load (`cargo nextest`
runs many tests at once), scheduler jitter stretches "5ms" into "11ms" and the
race flips: the test that passes alone fails one run in fifty. Pin time to a
deterministic virtual clock with `#[tokio::test(start_paused = true)]` instead.

```rust
// ── flaky: the asserted branch depends on a real-clock race ──────────────────
#[tokio::test]
async fn returns_attempt_timeout() {
    agent.set_prompt_delay(Duration::from_millis(25)); // tokio::time::sleep
    let err = prompt_with_retry(&agent, "p", Duration::from_millis(5), &policy).await;
    //                                         ↑ 5ms per-attempt timeout
    // Asserts "timed out on attempt 1". But the retry loop also has a budget gate
    // measured on a SECOND clock; under load the budget trips first → wrong message.
    assert!(err_message(err).contains("timed out on attempt 1"));
}
```

The deeper trap that made the above genuinely nondeterministic: the production
code accrued its **budget/elapsed on `std::time::Instant` (the real clock)** while
its **delays and per-attempt timeouts used `tokio::time`** (`sleep` / `timeout`).
Two clocks that drift relative to each other under load is the root cause — not
just the test. `start_paused` only helps if *all* the time-bearing operations
read the *same* clock.

## The rule

Two coordinated requirements:

1. **One clock in production.** Time-sensitive async logic must accrue elapsed,
   deadlines, and budgets on the **same clock** its sleeps/timeouts use. In tokio
   code that means `tokio::time::Instant`, **not** `std::time::Instant`, for
   `now()` / `.elapsed()` / budget arithmetic. In a normal runtime
   `tokio::time::Instant` tracks real time identically — production behavior is
   unchanged — so the cost is zero and the payoff is a clock the test harness can
   freeze. (`tokio::time::Instant::now()` requires a runtime context, which any
   function that already calls `tokio::time::sleep`/`timeout` necessarily has.)

2. **Freeze the clock in the test.** Annotate any test whose correctness depends
   on the relative ordering of timeouts/delays/backoff with
   `#[tokio::test(start_paused = true)]`. The paused clock auto-advances to the
   next pending deadline only when all tasks are idle, so a 5ms timeout racing a
   25ms sleep resolves at exactly virtual `t = 5ms`, every run, with zero jitter.
   The repo precedent is `providers/factory/discovery.rs`.

Never assert a timing-race outcome (timeout-vs-delay, backoff-vs-budget) while
the durations are measured against the real wall-clock with millisecond margins.

## When NOT to apply (leave the default `#[tokio::test]`)

- **The test does not depend on time.** Pure-logic, serialization, and
  error-classification tests touch no clock — adding `start_paused` is noise. Only
  pause tests that await a `tokio` timer whose timing the assertion depends on.
- **You intend to measure real latency.** A test that deliberately asserts a real
  operation completes (e.g. `latency_ms < 5_000`) wants the real clock — but it
  must use a *generous* margin, never a tight race. Keep these on
  `std::time::Instant` and plain `#[tokio::test]`; do not migrate them.
- **The awaited path blocks on something that is not a `tokio` timer** (a `std`
  mutex held across `.await`, real blocking I/O). Under `start_paused` the virtual
  clock will not auto-advance past such a block and the test **hangs**. Fix the
  blocking call; don't paper over it with a paused clock.
- **The two `Instant` types are distinct.** `tokio::time::Instant` and
  `std::time::Instant` do not interconvert — migrating one budget clock must not
  leak a value into an API (e.g. a latency helper) that expects the other.

The distinguishing test: *"If the machine were 3× slower for one scheduling
quantum, would a different assertion branch be taken?"* If yes, the test is
racing the clock — unify the production clock and pause it. If the outcome is
independent of real elapsed time (pure logic) or intentionally measures it
(latency), leave it alone.

## Worked example

`providers/factory/retry.rs` imported `std::time::{Duration, Instant}` and
measured the retry budget (`prepare_attempt`'s `started_at.elapsed() + delay >
total_budget` gate) on the **real** clock, while per-attempt guards used
`tokio::time::timeout` and backoff/mock delays used `tokio::time::sleep`. The test
`prompt_with_retry_public_entrypoint_returns_attempt_timeout_after_budget_exhaustion`
(5ms timeout, 25ms mock delay, 11ms budget) passed in isolation but flaked under
the full parallel suite: when real elapsed drifted past ~10ms the pre-attempt
budget gate returned "retry budget exhausted" instead of letting attempt 1 emit
the asserted "timed out on attempt 1". The fix swapped the import to
`use tokio::time::Instant;` (unifying every `started_at`/`acquire_start` budget
site onto tokio's clock — identical in production) and annotated the three
delay-racing tests in `retry.rs` and `text_retry.rs` with
`#[tokio::test(start_paused = true)]`. They now resolve deterministically in
~0.015s (down from real 25–100ms waits) and passed 10/10 hammered runs. The
wide-margin backoff-only tests (200ms budget vs ~1ms work) were left untouched —
their outcome never depended on a real-clock race.

See CLAUDE.md §4 "Goal-Driven Execution" (verifiable, repeatable success
criteria) and the sibling rules [[no-synonym-wrapper-functions]],
[[no-write-only-placeholder-fields]],
[[infallible-constructor-for-process-fatal-failures]], and
[[mock-at-the-right-seam-not-in-production]] (test against the real production
path; this rule keeps that path's *clock* deterministic).
