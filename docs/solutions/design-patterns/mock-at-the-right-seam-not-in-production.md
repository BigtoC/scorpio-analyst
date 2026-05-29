---
title: "Mock at the right seam — never stub inside production code"
date: 2026-05-29
problem_type: design_pattern
category: design-patterns
module: data/yfinance
component: testing_framework
severity: medium
applies_when:
  - "A production type carries a test-only stub field or `#[cfg(test)]` short-circuit that returns canned data"
  - "Deciding where a test should substitute: pure logic, a library-backed fetch, or a hand-rolled HTTP call"
  - "A test passes only because the stub echoes data that the production path discards"
  - "Choosing a mock seam, mirroring an existing trait such as `EdgarHttp` or `YFinanceData`"
related_components:
  - tooling
  - testing
tags:
  - testing
  - mockall
  - wiremock
  - test-seam
  - anti-pattern
  - dependency-injection
  - rust
---

# Mock at the right seam — never stub inside production code

## Context

A unit test must exercise the **real production path**. When a test needs to
avoid a network call, the substitution belongs at a seam the production code
does not see — never as a `#[cfg(test)]` branch baked into the production method
itself.

`YFinanceClient` (a thin wrapper over the `yfinance-rs` library) had grown the
banned shape: a `#[cfg(any(test, feature = "test-helpers"))] stubbed_financials:
Option<Arc<StubbedFinancialResponses>>` field and a short-circuit at the top of
all ten fetchers —

```rust
pub async fn get_info(&self, symbol: &str) -> Option<Info> {
    #[cfg(test)]                                     // ← test concern in prod code
    if let Some(stubbed) = &self.stubbed_financials {
        return Some(synthesize_stub_info(stubbed));  // ← canned data; skips the
    }                                                //    real fetch + parse + map_yf_err
    // ...real path the test never runs...
}
```

— plus a `with_stubbed_financials` constructor, a `synthesize_stub_info` helper,
and a ~130-line `fetch_from_stub` that *duplicated* the options assembly.

This is a trap:

- **It tests the stub, not production.** A `*_preserves_yahoo_failure_reason`
  test passed only because the stub echoed the message — production maps every
  error through the generic `map_yf_err` and discards the reason. Green, and wrong.
- **It leaks test concerns into production** (a cfg-gated field, a `with_stubbed_*`
  ctor, a branch per method) — see CLAUDE.md §3 "Surgical Changes".
- **It metastasizes** — every new fetcher copies the branch; the stub struct grows
  a field per method; `fetch_from_stub` drifts from the code it shadows.

## Guidance

Pick the seam by **what the code does**:

1. **Pure transformation → test the function directly.** If the logic under test
   is a pure function of its inputs (no I/O, no clock), call it directly with
   constructed inputs. Extract it from the I/O wrapper if it isn't already
   separate. No client, no HTTP, no stub.
   *In this repo:* `empty_price_target_to_none` / `empty_recommendation_summary_to_none`
   (financials), `build_yahoo_news_data` (news), `parse_summary_response` (summary),
   `assemble_snapshot` (options — extracted from `fetch_snapshot_impl` so the
   NTM/IV/max-pain math tests without a network seam), `apply_consensus_half_life_policy`
   and `classify_runtime_pack` (workflow).

2. **A wrapped fetch → mock at the function boundary with a trait.** When the unit
   under test *calls* a fetch method, define a `#[cfg_attr(test, mockall::automock)]`
   trait over the consumer-facing methods, implement it for the concrete client,
   and have *data-only* consumers depend on `Arc<dyn Trait>`. Tests set per-method
   responses on the generated `Mock…`. This is the right seam for **library-backed**
   clients (you don't want to fake the library's wire format) **and** for
   hand-rolled clients that only need their *return value* substituted.
   *In this repo:* `YFinanceData` (+ `MockYFinanceData`) consumed by
   `YFinanceEstimatesProvider`, `AnalystSyncTask`, `fetch_valuation_inputs`,
   `hydrate_consensus`; and `EdgarHttp` (+ `MockEdgarHttp`) in `sec_edgar` — a
   *hand-rolled* `reqwest` client that still mocks at a trait seam returning
   `(status, body)`, explicitly "without wiremock" (its source comment). A
   `mockall` expectation with `.times(0)` (or no expectation) also *verifies a
   call never happens* — e.g. "the ETF path does not fetch equity statements".

3. **Real URL/header/query construction → mock the HTTP with `wiremock`.** Reach
   for HTTP-level mocking only when the behavior under test *is* the request
   building (paths, headers, query params, status handling) of a hand-rolled
   `reqwest` client. Make the base URL injectable and point a `wiremock::MockServer`
   at it.
   *In this repo:* the Reddit client (`with_base_url` + `MockServer`) and Alpha
   Vantage. (SEC EDGAR is hand-rolled `reqwest` too, yet chooses option 2 — the
   trait seam — over wiremock; "hand-rolled" does not force wiremock.)

**Never** add a `#[cfg(test)]` / `feature = "test-helpers"` branch to a production
method that returns canned domain objects, and never add a stub field to a
production struct to feed one.

### Before / after

```rust
// BEFORE — banned: a test seam inside the production fetcher.
#[cfg(test)]
if let Some(stubbed) = &self.stubbed_financials {
    return Some(synthesize_stub_info(stubbed)); // bypasses the real fetch + map_yf_err
}

// AFTER — option 2: the unit runs for real against a mocked fetch boundary.
let mut mock = MockYFinanceData::new();
mock.expect_get_earnings_trend_result()
    .returning(|_| Err(TradingError::SchemaViolation { message: "…".into() }));
let provider = YFinanceEstimatesProvider::new(Arc::new(mock));
let outcome = provider.fetch_consensus("AAPL", "2025-04-01").await.unwrap();
assert_eq!(outcome, ConsensusOutcome::ProviderDegraded); // real fetch_consensus logic ran
```

## Why This Matters

A test that can only ever observe the stub tests nothing — and worse, it can go
green while asserting behavior production does not have. Moving the seam to a
trait (or to a pure function) means the test runs the real URL building, parsing,
and error mapping, so it fails when those break. It also keeps production code
free of test scaffolding.

This completes a trio of "delete the false-affordance slot" rules in this repo:
[[infallible-constructor-for-process-fatal-failures]] deletes redundant
*error-handling* slots, [[no-write-only-placeholder-fields]] deletes redundant
*data* slots, and this one deletes redundant *test-concern* slots (the
`#[cfg(test)] stubbed_financials` field/branch). The carve-out below is the same
"favor the simple shared shape" instinct as [[prefers-simple-shared-structs]].

## When to Apply

- Whenever a unit needs to avoid I/O. Prefer option 1 (pure) when the behavior is
  a pure function — simplest and least brittle. Use option 2 (trait mock) when the
  unit genuinely calls a fetch as part of what's tested. Use option 3 (wiremock)
  only to exercise real request construction.
- **Do not** HTTP-mock a library-backed call: faking the library's wire format is
  brittle fixture-wrangling that tests the library, not your code.
- **Do not** trait-abstract a consumer that also needs the **raw** client. A task
  that builds `rig` tools or providers from the live `YfClient` session keeps the
  concrete `YFinanceClient`; only data-only consumers take the trait. That is why
  `AnalystSyncTask` (data-only) became `Arc<dyn YFinanceData>` while
  `TechnicalAnalystTask` (tool-building) stayed concrete.

## Examples

When the stub was removed, the tests it fed were triaged, not mechanically
preserved. Delete a test whose only coverage was the stub or whose logic is
already covered by a direct unit test:

- the misleading `*_preserves_yahoo_failure_reason` cases (asserted non-existent
  behavior);
- options provider-gating / network-error wrappers, once pure `assemble_snapshot`
  covers the assembly;
- full-cycle pipeline wrappers whose logic lives in
  `apply_consensus_half_life_policy` / `classify_runtime_pack`;
- the options-outcome smoke test, covered by the `serialize_*` + `assemble_snapshot`
  unit tests.

The full migration (commits on `fix/bad-unit-test-pattern`): add the `YFinanceData`
trait seam → migrate consumers to `Arc<dyn YFinanceData>` + pure tests → delete the
stub seam. `fmt` + `clippy -D warnings` + `nextest` all green afterward.

## See also

- `.claude/rules/mock-at-the-right-seam-not-in-production.md` — the enforceable rule.
- `crates/scorpio-core/src/data/yfinance/data_source.rs` — the `YFinanceData` seam.
- `crates/scorpio-core/src/data/sec_edgar/mod.rs` — the `EdgarHttp` precedent (trait
  mock for a hand-rolled `reqwest` client, no wiremock).
- `docs/solutions/logic-errors/deterministic-valuation-derivation-fixes-2026-04-10.md`
  — originally prescribed the now-removed stub seam (superseded for its testing approach).
- `docs/solutions/design-patterns/infallible-constructor-when-failure-is-process-fatal.md`
  — sibling false-affordance-slot learning.
