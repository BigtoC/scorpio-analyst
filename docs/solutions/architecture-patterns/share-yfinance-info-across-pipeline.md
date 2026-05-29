---
title: Fetch yfinance Info once per run and share it via TradingState
date: 2026-05-29
category: architecture-patterns
module: data/yfinance
problem_type: architecture_pattern
component: data_pipeline
severity: medium
applies_when:
  - "A composed upstream call (e.g. Ticker::info) supersedes several separate per-category fetches"
  - "The same fetched data (e.g. profile) is read by multiple pipeline stages in one run"
  - "A fetch runs only to populate a field nothing downstream consumes"
  - "Choosing between a raw shared upstream struct and a per-category provenance wrapper"
related_components:
  - YFinanceClient
  - TradingState
  - YFinanceEstimatesProvider
  - CatalystCalendarProvider
tags:
  - yfinance
  - shared-state
  - fetch-once
  - data-provider
  - pipeline
  - serde-default
  - rust
---

# Fetch yfinance Info once per run and share it via TradingState

## Context

`yfinance-rs` 0.8 reshaped `Ticker::info()` into a *composed snapshot* call: a single `info()` await internally fans out — concurrently — to ~7 underlying endpoints (`snapshot`, `key_statistics`, `profile`, `calendar`, `price_target`, `recommendation_summary`, `esg`) and returns one `Info` aggregating all of them.

The analysis pipeline predated this and still fetched several of those categories independently, producing duplicate network work within a single analysis cycle:

- **`profile` was fetched 2–3× per run** — pack classification in `workflow/pipeline/runtime.rs::run_analysis_cycle`, valuation in `workflow/tasks/analyst.rs::fetch_valuation_inputs`, and ETF fund metadata via `get_fund_info`.
- **ETF `get_quote` (`data/yfinance/etf.rs`) issued a full `Ticker::info()`** (the whole 7-endpoint fan-out) purely to populate `EtfQuote.market_cap`, which was write-only / dead downstream.
- **`price_target` / `recommendation_summary`** were fetched inside the consensus provider, and the corporate **`calendar`** inside the Tier 1 catalyst provider.

The refactor fetches `Info` **once per cycle**, stores the raw upstream struct on shared `TradingState`, and routes every consumer to read its slice from that single snapshot.

## Guidance

When several independent consumers in one execution pass each fetch overlapping slices of the same upstream resource — and the upstream offers a single composed call that returns all of them — apply this pattern:

1. **Fetch the composed snapshot once**, with a thin fail-soft wrapper:

   ```rust
   // data/yfinance/financials.rs
   pub async fn get_info(&self, symbol: &str) -> Option<Info> {
       let ticker = Ticker::new(self.session.client(), symbol);
       match self.session.with_rate_limit(ticker.info()).await {
           Ok(info) => Some(info),
           Err(e) => {
               warn!(error = %e, symbol, "failed to fetch yfinance Info");
               None
           }
       }
   }
   ```

2. **Store the raw upstream struct on shared state** as an additive, snapshot-safe field:

   ```rust
   // state/trading_state.rs
   #[serde(default)]
   pub yfinance_info: Option<Info>,
   ```

   Mirror it through `TradingStateWire`, `From<TradingStateWire>`, `TradingState::new`, and clear it in `reset_cycle_outputs` (`state.yfinance_info = None;`). `Info` derives `Serialize`/`Deserialize`/`PartialEq`, so the raw type persists directly with no projection layer.

3. **Fetch once in the orchestrator, then thread slices to consumers.** In `run_analysis_cycle`:

   ```rust
   let yfinance_info = pipeline.yfinance.get_info(&initial_state.asset_symbol).await;
   // pack classification reads profile:
   let profile = yfinance_info.as_ref().and_then(|info| info.profile.clone());
   // ... thread info.calendar into hydrate_catalysts,
   //     info.price_target / info.recommendation_summary into hydrate_consensus ...
   initial_state.yfinance_info = yfinance_info;
   ```

4. **Convert consumers to read their slice instead of fetching.** Change signatures to accept the slice (or the whole `Info`), not the client:
   - Valuation: `fetch_valuation_inputs(..., info: Option<&Info>)` reads `info.profile` and derives ETF `fund_info` from it, dropping the duplicate `get_fund_info` fetch.
   - Catalysts: `fetch_catalysts(&self, symbol, as_of_date, horizon_days, calendar: Option<TickerCalendar>)` — the calendar arrives as a **per-call argument** (the provider is built once and reused across cycles, so it must not be a field). `Tier1CatalystProvider` dropped its `yfinance` field; `build_catalyst_provider` dropped its `yfinance` param (3 call sites updated).
   - Consensus: `YFinanceEstimatesProvider::with_consensus_inputs(client, price_target, recommendations)` seeds the cached slices on provider fields; the `EstimatesProvider` trait signature is unchanged (concrete-only, no `dyn`).
   - ETF `get_quote` dropped its `Ticker::info()` call and sets `market_cap: None`.

5. **Keep the one branch that carries error provenance as a separate live fetch** (see *Why This Matters*). Consensus keeps fetching only `earnings_trend` live.

6. **Decision rule — raw struct vs. owned projection:** default to storing the **raw upstream struct**, not a custom projection with per-category provenance wrappers. Propose the simple shared-struct shape first and surface its tradeoffs; only build a projection layer if a concrete consumer requires it. An initial `InfoContext` + `CategoryOutcome<T>` + `InfoFetchPlan` projection layer was built here and **reverted as over-engineered**.

## Why This Matters

**Resource win (primary motivation).** The old shape issued the `profile` endpoint up to three times and ran an entire seven-endpoint fan-out (`Ticker::info()` in ETF `get_quote`) just to fill one dead field. A single composed fetch removes those duplicate round-trips and the wasted fan-out, lowers rate-limit pressure on the provider, and gives every consumer a *consistent* view (no two fetches returning skewed snapshots mid-cycle).

**Provenance tradeoff (the cost to accept knowingly).** `Ticker::info()` internally swallows each sub-fetch failure to `None` (its `log_err_async`). A fetched `Info` therefore **cannot distinguish "the `price_target` endpoint errored" from "`price_target` is genuinely empty."** Error provenance for those categories is lost. This is why the consensus `Data` / `NoCoverage` / `ProviderDegraded` taxonomy now derives error provenance **only from the live `earnings_trend` branch** — the one branch *not* part of `Info`:

```rust
// data/adapters/estimates.rs::fetch_consensus
let trend_res = self.client.get_earnings_trend_result(symbol).await;
let trend_branch = classify_branch(trend_res, |rows| rows.as_ref().is_some_and(|r| !r.is_empty()));

let price_target    = self.price_target.as_ref().filter(|pt| !price_target_is_empty(pt));
let recommendations = self.recommendations.as_ref().filter(|rs| !recommendations_is_empty(rs));

let any_error = trend_branch.is_error();                  // sole error provenance
let any_data  = trend_branch.is_data() || price_target.is_some() || recommendations.is_some();
```

Whole-provider degradation still classifies correctly (a dead provider fails the live trend fetch too), but a *partial* failure isolated to `price_target` now reads as "empty" rather than "degraded." Two pipeline tests were updated to simulate degradation via `trend_error` instead of the now-meaningless `price_target_error`. (Sibling provenance-accuracy precedent: `../logic-errors/reddit-news-source-regressions-2026-05-24.md`.)

**Coupling tradeoff.** Storing the raw `Info` couples persisted `TradingState` snapshots to the upstream type's serialization shape and carries fields (`snapshot`, `key_statistics`, `esg_scores`) with no current consumer ("for future use"). The accepted verdict: a single shared raw struct, with the provenance simplification and the carried-but-unused fields, in exchange for substantially simpler code. Prefer the simple shape and name the tradeoffs out loud.

## When to Apply

Reach for this pattern when **all** of these hold:

- Multiple consumers within a single execution pass each fetch overlapping data from the same source.
- The upstream provides a *composed* call returning those overlapping pieces together (ideally fanning out concurrently itself).
- There is a natural single point early in the run to fetch, and a shared state object to hang the result on.

**And weigh the provenance question:** if a consumer's correctness depends on distinguishing "errored" from "empty" for a category, that category cannot rely solely on the composed snapshot. Either keep it as a separate live fetch that preserves its error signal (the `earnings_trend` approach), or accept the degraded-vs-empty conflation explicitly.

**Don't apply** when the "duplicate" fetches happen across *different* runs (caching, not run-scoped sharing, is the lever — see `../best-practices/concrete-enrichment-provider-pattern-2026-04-10.md`), when consumers need *different freshness* per slice, or when the composed call is materially more expensive than the subset actually used.

## Examples

**ETF quote — before: a full 7-endpoint fan-out for one dead field; after: no fan-out.**

```rust
// before (data/yfinance/etf.rs)
let info = Ticker::new(self.session.client(), symbol).info().await.ok();  // 7-endpoint fan-out
let market_cap = info.and_then(|i| i.key_statistics.market_cap);          // write-only, dead downstream

// after
Some(EtfQuote {
    // Sourced from the shared `Info` snapshot's `key_statistics`, not a second
    // per-quote `info()` fan-out. Left `None`; read
    // `state.yfinance_info.key_statistics.market_cap` when needed.
    market_cap: None,
    // ...
})
```

**Consensus — before: three error-bearing live fetches; after: only `earnings_trend` is live, price_target/recs ride the shared snapshot.**

```rust
// data/adapters/estimates.rs
pub fn with_consensus_inputs(
    client: YFinanceClient,
    price_target: Option<UpstreamPriceTarget>,
    recommendations: Option<UpstreamRecommendationSummary>,
) -> Self {
    Self { client, price_target, recommendations }
}
```

**Catalysts — before: provider held a `yfinance` client and fetched the calendar; after: calendar threaded in as a per-call argument.**

```rust
// data/adapters/catalysts.rs
async fn fetch_catalysts(
    &self,
    symbol: &str,
    as_of_date: &str,
    horizon_days: u32,
    calendar: Option<TickerCalendar>,   // lifted from the shared Info snapshot
) -> Result<Vec<CatalystEvent>, TradingError>;
```

The argument-not-field choice matters: the provider is constructed once and reused across cycles, so the calendar — which changes per cycle — must flow through the call, not live on the struct.

**Test-only synthesizer so stub-driven tests don't need a full upstream payload.**

```rust
// data/yfinance/financials.rs
#[cfg(test)]
fn synthesize_stub_info(stubbed: &super::ohlcv::StubbedFinancialResponses) -> Info {
    use paft_aggregates::Snapshot;
    use yfinance_rs::{AssetKind, Instrument, KeyStatistics};
    // snapshot is never read by any consumer; build a throwaway instrument.
    let instrument = Instrument::from_symbol("AAPL", AssetKind::default()).expect("valid stub instrument");
    Info {
        snapshot: Snapshot::new(instrument),
        key_statistics: KeyStatistics::default(),
        profile: stubbed.profile.clone(),
        calendar: stubbed.calendar.clone().map(ticker_calendar_to_upstream),
        price_target: stubbed.price_target.clone(),
        recommendation_summary: stubbed.recommendation_summary.clone(),
        esg_scores: None,
    }
}
```

This required adding `paft-aggregates` as a **test-only** dependency (workspace `Cargo.toml` plus `[dev-dependencies]` in `crates/scorpio-core/Cargo.toml`) solely to construct the throwaway `Snapshot` — a direct consequence of choosing the raw struct over a projection.

## Related

- `../best-practices/concrete-enrichment-provider-pattern-2026-04-10.md` — the conceptual predecessor; its "cache shared API responses" guidance solved dedup at the *client cache* layer. This pattern moves dedup up to the *run-scoped `TradingState`* layer for the composed-`Info` consumers (consensus, catalysts, valuation, classification, ETF). The two are complementary: caching for cross-run reuse, shared state for within-run sharing.
- `../data-sources/2026-05-10-catalyst-calendar.md` — the catalyst provider is a rerouted consumer (it previously fetched `Ticker.calendar()` itself; the calendar now arrives from `yfinance_info`).
- `../logic-errors/deterministic-valuation-derivation-fixes-2026-04-10.md` — valuation is a rerouted consumer; its determinism/PEG invariants must still hold for the info-sourced path.
- `../logic-errors/reddit-news-source-regressions-2026-05-24.md` — sibling provenance-accuracy fix (provider over-claiming); cite alongside the consensus provenance simplification here.
- Verification: full-workspace `cargo nextest` (2130 passed), `cargo clippy` clean in changed files, `--locked` build OK.
