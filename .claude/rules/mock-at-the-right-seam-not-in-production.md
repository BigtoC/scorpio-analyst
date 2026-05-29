# Mock at the Right Seam — Never Stub Inside Production Code

A unit test must exercise the **real production path**. When a test needs to
avoid a network call, the substitution belongs at a seam the production code
does not see — never as a `#[cfg(test)]` branch baked into the production
method itself.

The banned shape (and the reason this rule exists):

```rust
pub async fn get_info(&self, symbol: &str) -> Option<Info> {
    #[cfg(test)]                                 // ← test concern in prod code
    if let Some(stubbed) = &self.stubbed_financials {
        return Some(synthesize_stub_info(stubbed));  // ← returns canned data,
    }                                                //    bypasses ALL real logic
    // ...real fetch + parse + error mapping the test never runs...
}
```

This "stub field on the production struct + `#[cfg(test)]` short-circuit" pattern
is a trap:

- **It tests the stub, not production.** A test driving `stubbed_financials`
  validates `synthesize_stub_info`, not `Ticker::info()` + parsing + `map_yf_err`.
  It can even assert behavior production does **not** have — e.g. a
  `preserves_yahoo_failure_reason` test passed only because the stub echoed the
  message, while production maps every error through the generic `map_yf_err`
  and discards the reason. Green, and wrong.
- **It leaks test concerns into production** — a `#[cfg(test)]`/`test-helpers`
  field, a `with_stubbed_*` constructor, and a branch at the top of every
  method (see CLAUDE.md §3 "Surgical Changes").
- **It metastasizes** — every new fetcher copies the branch; the stub struct
  grows a field per method.

## The rule — pick the seam by what the code does

1. **Pure transformation → test the function directly.** If the logic under test
   is a pure function of its inputs (no I/O, no clock), call it directly with
   constructed inputs. Extract it from the I/O wrapper if it isn't already
   separate. No client, no HTTP, no stub.
   *Examples in this repo:* `empty_price_target_to_none` /
   `empty_recommendation_summary_to_none` (financials), `build_yahoo_news_data`
   (news), `parse_summary_response` (summary), `assemble_snapshot` (options —
   extracted from `fetch_snapshot_impl` so the NTM/IV/max-pain math is tested
   without a network seam), `apply_consensus_half_life_policy` and
   `classify_runtime_pack` (workflow).

2. **A wrapped fetch → mock at the function boundary with a trait.** When the
   unit under test *calls* a fetch method, the right seam is *what the call
   returns*, not the HTTP beneath it. Define a `#[cfg_attr(test, mockall::automock)]`
   trait over the consumer-facing methods, implement it for the concrete client,
   and have *data-only* consumers depend on `Arc<dyn Trait>`. Tests set per-method
   responses on the generated `Mock…`. This is the seam for **library-backed**
   clients (don't fake the library's wire format) **and** for hand-rolled clients
   whose tests only need the return value substituted.
   *Examples in this repo:* `YFinanceData` (+ `MockYFinanceData`) consumed by
   `YFinanceEstimatesProvider`, `AnalystSyncTask`, `fetch_valuation_inputs`, and
   `hydrate_consensus`; `EdgarHttp` (+ `MockEdgarHttp`) in `sec_edgar` — a
   *hand-rolled* `reqwest` client that mocks at a trait seam returning
   `(status, body)`, explicitly "without wiremock" (its source comment). A `mockall`
   expectation with `.times(0)` (or simply no expectation) also *verifies a call
   never happens* — e.g. "the ETF path does not fetch equity statements".

3. **Real request construction → mock the HTTP with `wiremock`.** Reach for
   HTTP-level mocking only when the behavior under test *is* the request building
   (paths, headers, query params, status handling) of a hand-rolled `reqwest`
   client. Make the base URL injectable and point a `wiremock::MockServer` at it.
   *Examples in this repo:* the Reddit client (`with_base_url` + `MockServer`) and
   Alpha Vantage. Note "hand-rolled" does **not** force wiremock — `sec_edgar` is
   hand-rolled `reqwest` yet chooses the trait seam (option 2); `summary::SummaryHttp`
   is a candidate if its fetch path ever needs coverage (its parsing is already a
   pure-function test).

**Never** add a `#[cfg(test)]` (or `feature = "test-helpers"`) branch to a
production method that returns canned domain objects, and never add a stub field
to a production struct to feed one.

## Choosing between them

- Don't HTTP-mock a library-backed call (option 3 on an option-2 site): faking
  Yahoo's wire format under `yfinance-rs` is brittle fixture-wrangling that tests
  the library, not your code. Mock at the function boundary (option 2).
- Don't trait-abstract a consumer that also needs the **raw** client (e.g. a task
  that builds `rig` tools or providers from the live `YfClient` session). Keep
  the concrete client there; only the data-only consumers take the trait.
- Prefer option 1 whenever the behavior under test is pure — it's the simplest
  and least brittle. Reach for a mock only when the unit genuinely calls the
  fetch method as part of what's being tested.

## Delete, don't preserve, stub-only coverage

When you remove the stub, audit the tests it fed. A test that **only** exercised
the stub (or whose logic is already covered by a direct unit test) should be
deleted, not laboriously reconstructed:

- the misleading `*_preserves_yahoo_failure_reason` cases (asserted non-existent
  behavior);
- options provider-gating / network-error wrappers once the pure
  `assemble_snapshot` covers the assembly;
- full-cycle pipeline wrappers whose logic lives in
  `apply_consensus_half_life_policy` / `classify_runtime_pack`;
- the options-outcome smoke test, covered by the `serialize_*` + `assemble_snapshot`
  unit tests.

A test that can only ever observe the stub tests nothing (cf. the deleted
`.is_ok()` test in [[infallible-constructor-for-process-fatal-failures]]).

## Worked example

`YFinanceClient` carried a `stubbed_financials: Option<Arc<StubbedFinancialResponses>>`
field and a `#[cfg(test)] if let Some(stubbed) … { return … }` branch at the top
of all ten fetchers (`get_quarterly_*`, `get_earnings_trend*`, `get_profile`,
`fetch_calendar`, `get_info`), plus `with_stubbed_financials`,
`synthesize_stub_info`, and a ~130-line `fetch_from_stub` that *duplicated* the
options assembly. Because `YFinanceClient` is a thin wrapper over `yfinance-rs`,
the fix mocked at the **function** boundary, not the HTTP layer:

1. Extracted/used pure functions and tested them directly (`assemble_snapshot`,
   `build_yahoo_news_data`, `empty_*_to_none`).
2. Added the `YFinanceData` trait (`#[cfg_attr(test, mockall::automock)]`,
   blanket impl for `YFinanceClient`); `YFinanceEstimatesProvider`,
   `AnalystSyncTask`, `fetch_valuation_inputs`, and `hydrate_consensus` switched
   to `Arc<dyn YFinanceData>` and are tested with `MockYFinanceData`.
3. Deleted the field, the branches, the constructor, and the synth/duplicate
   helpers.

HTTP-level mocking stayed reserved for the hand-rolled Reddit client. See
CLAUDE.md §2 "Simplicity First" / §3 "Surgical Changes", the sibling rules
[[infallible-constructor-for-process-fatal-failures]] and
[[no-write-only-placeholder-fields]] (this deletes redundant *test-seam* slots;
those delete redundant *error-handling* and *data* slots), and
`docs/solutions/design-patterns/mock-at-the-right-seam-not-in-production.md`.
