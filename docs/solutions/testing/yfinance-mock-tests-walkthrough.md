# How the `YFinanceData` mock tests work

A walkthrough of the trait + `mockall` test seam that replaced the old
`#[cfg(test)] stubbed_financials` short-circuit. The enforceable rule lives in
[`.claude/rules/mock-at-the-right-seam-not-in-production.md`](../../../.claude/rules/mock-at-the-right-seam-not-in-production.md);
this doc is the *how-to* companion.

**The core idea:** the substitution sits at the trait boundary the consumer
already calls through, so a test runs the entire real method *except* the one
I/O call — versus the old stub branch, which returned canned data *before* any
real logic ran.

---

## 1. The seam: one attribute generates the mock

`crates/scorpio-core/src/data/yfinance/data_source.rs`

```rust
#[cfg_attr(test, mockall::automock)]   // in test builds, generates `MockYFinanceData`
#[async_trait]
pub trait YFinanceData: Send + Sync {
    async fn get_earnings_trend_result(&self, symbol: &str)
        -> Result<Option<Vec<EarningsTrendRow>>, TradingError>;
    async fn get_quote(&self, symbol: &str) -> Option<EtfQuote>;
    async fn get_quarterly_cashflow(&self, symbol: &str) -> Option<Vec<CashflowRow>>;
    // … one method per fetch
}
```

- `mockall::automock` is a proc-macro. Under `cfg(test)` it reads the trait and
  **generates a struct `MockYFinanceData`** that implements `YFinanceData`. For
  every trait method `foo` the mock gains an `expect_foo()` builder.
- In production builds the attribute is absent — nothing is generated, zero
  runtime cost.
- The real `YFinanceClient` also implements the trait (a blanket impl that
  delegates to its inherent methods), so the same `Arc<dyn YFinanceData>` slot
  accepts either the real client or the mock.

## 2. The consumer depends on the trait, not the concrete client

`crates/scorpio-core/src/data/adapters/estimates.rs`

```rust
pub struct YFinanceEstimatesProvider {
    client: Arc<dyn YFinanceData>,   // any impl: real client OR mock
    price_target: Option<UpstreamPriceTarget>,
    recommendations: Option<UpstreamRecommendationSummary>,
}

impl YFinanceEstimatesProvider {
    pub fn new(client: Arc<dyn YFinanceData>) -> Self { /* … */ }
}
```

Holding `Arc<dyn YFinanceData>` is the swap point: production injects a real
`YFinanceClient`, tests inject a `MockYFinanceData`, and the provider can't tell
the difference.

## 3. The code under test calls the trait method

```rust
async fn fetch_consensus(&self, symbol, as_of_date) -> Result<ConsensusOutcome, TradingError> {
    let trend_res = self.client.get_earnings_trend_result(symbol).await;  // dispatches to the mock in tests
    let trend_branch = classify_branch(trend_res, …);
    // …~50 lines of real branching: error → ProviderDegraded, empty → NoCoverage, data → Data…
}
```

## 4. The test: set a canned return, then run the *real* logic

```rust
#[tokio::test]
async fn fetch_consensus_classifies_lone_trend_error_as_provider_degraded() {
    let mut mock = MockYFinanceData::new();
    mock.expect_get_earnings_trend_result()              // configure ONE method
        .returning(|_| Err(TradingError::SchemaViolation {    // the closure IS the canned response
            message: "Yahoo Finance response could not be parsed".to_owned(),
        }));

    let provider = YFinanceEstimatesProvider::new(Arc::new(mock));  // inject the mock

    let outcome = provider
        .fetch_consensus("AAPL", "2025-04-01")   // runs the REAL fetch_consensus body
        .await
        .expect("single-branch error with empty siblings is degraded, not Err");

    assert_eq!(outcome, ConsensusOutcome::ProviderDegraded);  // assert on real output
}
```

What happens at runtime:

1. `expect_get_earnings_trend_result()` registers an expectation.
   `.returning(|args| …)` stores a closure mockall invokes whenever the method is
   called; its return value is what the method yields. (mockall + `async_trait`
   lets the closure return the resolved value directly — no manual future
   wrapping.)
2. `Arc::new(mock)` coerces `MockYFinanceData` → `Arc<dyn YFinanceData>`.
3. `provider.fetch_consensus(...)` executes the **actual production branching**
   (`classify_branch`, the error/empty/data taxonomy). The only faked thing is
   the single fetch; everything the test asserts is real logic.

Happy path is the same shape, returning data so the real normalization runs:

```rust
mock.expect_get_earnings_trend_result()
    .returning(|_| Ok(Some(vec![make_trend_row(Some(2.50), Some(95_000_000_000.0), Some(35))])));
// then assert: evidence.eps_estimate == Some(2.50), price_target / recommendations populated, …
```

## 5. A mock can also assert a call *never* happens

`crates/scorpio-core/src/workflow/tasks/analyst.rs` —
`etf_baseline_fetch_skips_equity_statement_fanout`:

```rust
let mut mock = MockYFinanceData::new();
mock.expect_get_quote().returning(|_| None);
mock.expect_get_distribution_yield_ttm().returning(|_| None);
mock.expect_get_ohlcv().returning(|_, _, _| Err(/* … */));
// get_quarterly_cashflow / balance / income / shares: NO expect_* set

let inputs = fetch_valuation_inputs(&mock, None, PackId::EtfBaseline, "SPY", &today, …).await;
assert!(inputs.cashflow.is_none());
```

mockall **panics if a method is called with no matching expectation**. So *not*
configuring `expect_get_quarterly_cashflow()` turns "the ETF path must not fetch
equity statements" into a hard assertion at the call boundary — something a
hand-written stub can't express (it would just return `None` silently).

> Note: `fetch_valuation_inputs` takes `&dyn YFinanceData`, so the test passes
> `&mock` directly (no `Arc` needed); `YFinanceEstimatesProvider` stores
> `Arc<dyn YFinanceData>`, so its tests pass `Arc::new(mock)`.

## When to use which seam

| Code under test                                 | Seam                             | Example                                                                    |
|-------------------------------------------------|----------------------------------|----------------------------------------------------------------------------|
| Pure transformation (no I/O)                    | Call the function directly       | `assemble_snapshot`, `build_yahoo_news_data`, `empty_price_target_to_none` |
| A unit that *calls* a fetch                     | `mockall` trait mock (this doc)  | `fetch_consensus`, `fetch_valuation_inputs`                                |
| Real request construction (paths/headers/query) | `wiremock` + injectable base URL | Reddit client, Alpha Vantage                                               |

See [`docs/solutions/design-patterns/mock-at-the-right-seam-not-in-production.md`](../design-patterns/mock-at-the-right-seam-not-in-production.md)
for the full rationale and the `EdgarHttp` precedent (a hand-rolled `reqwest`
client that also mocks at a trait seam, "without wiremock").
