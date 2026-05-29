---
title: "Stub seam baked into production fetchers (`#[cfg(test)] stubbed_financials`)"
date: 2026-05-29
category: testing-patterns
module: data/yfinance
problem_type: testing_anti_pattern
component: testing
symptoms:
  - "Production methods carry a `#[cfg(test)]`/`feature = \"test-helpers\"` branch that returns canned data and skips the real fetch/parse path"
  - "A cfg-gated stub field + `with_stubbed_*` constructor on a production struct"
  - "Unit tests pass while asserting behavior production does not have (e.g. an error 'reason' that production discards via a generic mapper)"
  - "A test-only helper duplicates production logic (a parallel `fetch_from_stub` reimplementation)"
root_cause: test_concern_in_production_code
resolution_type: refactor
severity: medium
related_components:
  - yfinance
  - mockall
  - wiremock
  - testing
tags: [testing, unit-tests, mocking, mockall, wiremock, trait-seam, anti-pattern, rust, yfinance]
---

# Stub seam baked into production fetchers

## Problem

`YFinanceClient` (a thin wrapper over the `yfinance-rs` library) carried a
`#[cfg(any(test, feature = "test-helpers"))] stubbed_financials:
Option<Arc<StubbedFinancialResponses>>` field and a `#[cfg(test)] if let
Some(stubbed) = &self.stubbed_financials { return … }` short-circuit at the top
of all ten fetchers, plus a `with_stubbed_financials` constructor, a
`synthesize_stub_info` helper, and a ~130-line `fetch_from_stub` that
**duplicated** the options-assembly logic. Tests injected canned domain objects
and never ran the real fetch → parse → `map_yf_err` path.

## Symptoms

- Test concerns (a stub field, a `with_stubbed_*` ctor, a branch per method)
  living in production code.
- Tests that validate the stub instead of production — `*_preserves_yahoo_failure_reason`
  passed only because the stub echoed the message, while production maps every
  error through the generic `map_yf_err` and drops the reason.
- A parallel reimplementation (`fetch_from_stub`) that can drift from the real
  `fetch_snapshot_impl` it shadows.

## What Didn't Work

- **HTTP-mocking under the library (`wiremock` against `yfinance-rs`).** Initially
  attempted: point `YfClient`'s `base_*`/`cookie_url`/`crumb_url` at a
  `MockServer` and serve recorded Yahoo JSON. It compiles and a proof test
  passes, but it's brittle fixture-wrangling that tests `yfinance-rs`'s parser,
  not our code, and the quarterly-cashflow/balance timeseries fixtures don't even
  exist upstream. Wrong seam for a library-backed call.

## Solution

Pick the seam by what the code does:

1. **Pure logic → test the function directly.** Extracted `assemble_snapshot`
   (options) from `fetch_snapshot_impl`; tested `build_yahoo_news_data` (news)
   and `empty_price_target_to_none` / `empty_recommendation_summary_to_none`
   (financials) directly with constructed inputs — no client, no HTTP.

2. **Library-backed I/O → mock at the function boundary with a trait.** Added
   `#[cfg_attr(test, mockall::automock)] trait YFinanceData` over the
   consumer-facing fetch methods, with a blanket impl for `YFinanceClient`
   (mirroring `sec_edgar`'s `EdgarHttp`). Data-only consumers
   (`YFinanceEstimatesProvider`, `AnalystSyncTask`, `fetch_valuation_inputs`,
   `hydrate_consensus`) switched to `Arc<dyn YFinanceData>` and are tested with
   the generated `MockYFinanceData` (`mock.expect_get_earnings_trend_result()…`).
   A mock with no expectation for a method also *proves it is never called*
   (e.g. "the ETF path skips equity-statement fetches").

3. **Hand-rolled HTTP → `wiremock`.** Reserved for code that issues `reqwest`
   calls directly (the Reddit client, SEC EDGAR), via an injectable base URL.

Then deleted `StubbedFinancialResponses`, the field, all branches,
`with_stubbed_financials`, `synthesize_stub_info`, and `fetch_from_stub`, and
removed tests whose only coverage was the stub or was already unit-tested
elsewhere (`apply_consensus_half_life_policy`, `classify_runtime_pack`,
`serialize_*`). `fmt` + `clippy -D warnings` + `nextest` (2112 tests) green.

## Key constraint discovered

A consumer that needs the **raw** client (the analyst fan-out builds `rig` tools
and the options/news providers from the live `YfClient` session) must keep the
concrete `YFinanceClient`; only data-only consumers take the trait. That's why
`AnalystSyncTask` (data-only) became `Arc<dyn YFinanceData>` while
`TechnicalAnalystTask` (tool-building) stayed concrete.

## See also

- `.claude/rules/mock-at-the-right-seam-not-in-production.md` — the enforceable rule.
- `crates/scorpio-core/src/data/yfinance/data_source.rs` — the `YFinanceData` seam.
- `crates/scorpio-core/src/data/sec_edgar/mod.rs` — the `EdgarHttp` precedent.
- `docs/solutions/design-patterns/` — sibling rules on deleting false-affordance
  slots (infallible constructors, write-only placeholder fields).
