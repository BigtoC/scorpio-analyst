# Design — yfinance news, options snapshot, and extended consensus evidence

**Date:** 2026-04-24 (revised 2026-04-26 to reflect the prompt-bundle centralization refactor)
**Author:** brainstorming session with BigtoC
**Status:** Draft — pending implementation plan

## Summary

Add four yfinance-rs data streams to the equity analyst pipeline:

1. **Company news** from yfinance, supplementing the existing Finnhub news feed.
2. **Options & derivatives snapshot** (summary metrics + near-the-money slice) as a new tool for the Technical Analyst.
3. **Analyst price targets** as an extension of the existing `ConsensusEvidence` enrichment.
4. **Recommendations summary** as an extension of the existing `ConsensusEvidence` enrichment.

The design uses a **mixed integration pattern** — streams (3) and (4) are pre-fetched enrichment (visible to all downstream agents), stream (1) is merged into the existing news prefetch, and stream (2) is an analyst-scoped tool pulled on demand by the Technical Analyst. Each pattern is chosen to match the shape of the data and how it's used.

## Goals

- Realize the yfinance-rs capabilities that PRD.md §4.2 already promises but that are not yet implemented: options chains, analyst estimates, price targets, recommendations summary, and yfinance-sourced company news. (Historical upgrade/downgrade event streams remain explicitly deferred — see Non-goals.)
- Keep analyst responsibilities symmetric with the asset-class-generalization refactor: equity-only today; crypto pack will later supply analogous providers via the same trait seams.
- Ship as a single integrated change (one plan, one review cycle) — the four streams are loosely coupled via existing scaffolding.

## Non-goals

- No crypto coverage. yfinance-rs is equity-only; the existing `DerivativesProvider` stub (crypto-oriented, opaque `raw: String` payload) remains unused. Crypto pack defines its own derivatives/news/consensus contracts.
- No expansion of the final CLI report. New data is internal to agent reasoning and audit-trail SQLite phase snapshots. Report surface is a potential follow-up.
- No new user-facing configuration keys. yfinance-rs is already wired; no new credentials.
- No historical upgrade/downgrade event stream (`upgrades_downgrades` endpoint). Point-in-time recommendations summary only; event history deferred.
- No unusual options activity detection (high-volume-versus-OI flagging). Summary metrics + near-the-money slice only.

## Design choices (decided during brainstorming)

| Decision            | Choice                                                                                       | Rationale                                                               |
|---------------------|----------------------------------------------------------------------------------------------|-------------------------------------------------------------------------|
| Integration pattern | Mixed — enrichment for consensus, orchestration merge for news, analyst tool for options     | Each pattern matches the data's shape and consumers                     |
| News strategy       | Supplement (yfinance + Finnhub, deduped and merged)                                          | More coverage; low merge cost                                           |
| Options shape       | Summary metrics + near-the-money slice (nearest expiration, ±5% of spot)                     | Covers vol-regime + event-aware strikes without prompt bloat            |
| Consensus fields    | Standard (mean/median/high/low price target, analyst count, recommendation counts by bucket) | Balanced detail; count breakdown preserves consensus dispersion         |
| Abstraction level   | Pragmatic — `OptionsProvider` trait; extend `EstimatesProvider`; news-merge inline           | Trait where multiple future vendors are likely; orchestration where not |
| Rollout             | Single integrated plan                                                                       | Pieces are loosely coupled; trait seams insulate each                   |

## Architecture

Three integration points, three shapes, all living in the equity pack.

```
┌─────────────────────────── Pipeline start (pre-debate) ───────────────────────────┐
│                                                                                   │
│  run_analysis_cycle (workflow/pipeline/runtime.rs)                                │
│    ├─▶ FinnhubEventNewsProvider  ──▶ enrichment_event_news (unchanged)            │
│    └─▶ YFinanceEstimatesProvider ──▶ enrichment_consensus (FIELDS EXTENDED)       │
│         └── now also fetches price targets + recommendations summary              │
└───────────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     ▼
┌────────────────────────── Phase 1: Analyst fan-out ───────────────────────────────┐
│                                                                                   │
│  prefetch_analyst_news (agents/analyst/mod.rs)                                    │
│    ├─▶ FinnhubClient as NewsProvider  ──┐                                         │
│    └─▶ YFinanceNewsProvider           ──┴──▶ merge + dedupe ──▶ Arc<NewsData>     │
│                                         │                                         │
│                                         ├─▶ NewsAnalyst (shared)                  │
│                                         └─▶ SentimentAnalyst (shared)             │
│                                                                                   │
│  TechnicalAnalyst                                                                 │
│    ├── existing tools: GetOhlcv, CalculateAllIndicators, ...                      │
│    └── NEW TOOL: GetOptionsSnapshot (wraps Arc<dyn OptionsProvider>)              │
│                                                                                   │
└───────────────────────────────────────────────────────────────────────────────────┘
                                     │
                                     ▼
         enrichment_consensus (extended) rendered into ALL downstream prompts
             (researchers, trader, risk) via build_enrichment_context()
```

### New and modified files

```
crates/scorpio-core/src/
├── data/
│   ├── traits/options.rs          (NEW — OptionsProvider trait + OptionsSnapshot types)
│   └── yfinance/
│       ├── news.rs                (NEW — YFinanceNewsProvider impl of NewsProvider)
│       ├── options.rs             (NEW — YFinanceOptionsProvider impl + GetOptionsSnapshot rig Tool)
│       └── mod.rs                 (MODIFIED — export new modules)
├── data/adapters/estimates.rs     (MODIFIED — extend ConsensusEvidence; extend provider fetch)
├── state/technical.rs             (MODIFIED — TechnicalData gains options_summary: Option<String>)
├── agents/analyst/mod.rs          (MODIFIED — prefetch_analyst_news refactored to take Arc<dyn NewsProvider>s)
├── agents/analyst/equity/
│   └── technical.rs               (MODIFIED — bind GetOptionsSnapshot; no Rust-side prompt edits)
├── agents/shared/prompt.rs        (MODIFIED — render extended ConsensusEvidence fields)
├── analysis_packs/equity/prompts/
│   └── technical_analyst.md       (MODIFIED — append options-tool guidance paragraph)
├── workflow/pipeline/runtime.rs   (MODIFIED — construct YFinanceNewsProvider + YFinanceOptionsProvider; pass both news providers into prefetch)
├── workflow/snapshot/thesis.rs    (unchanged — schema version stays at 3; see "State and persistence" below)
└── constants.rs                   (MODIFIED — add OPTIONS_NTM_STRIKE_BAND, OPTIONS_FETCH_TIMEOUT_SECS)

crates/scorpio-core/examples/yfinance_live_test.rs  (MODIFIED — add sections 7–10)
```

> Note: `prefetch_analyst_news` currently lives in `agents/analyst/mod.rs` (not `equity/mod.rs`) and takes `&FinnhubClient` directly. This design refactors it to accept two `Arc<dyn NewsProvider>` handles so the merge can stay agnostic of the concrete clients. `FinnhubClient` already implements `NewsProvider` via `data/provider_impls.rs`, so no `FinnhubNewsProvider` wrapper is needed — see "Component: News supplementation" below.

## Component: ConsensusEvidence extension

In-place extension of the existing struct. No new module, no new trait.

```rust
// crates/scorpio-core/src/data/adapters/estimates.rs

pub struct ConsensusEvidence {
    // Existing fields (unchanged)
    pub symbol: String,
    pub eps_estimate: Option<f64>,
    pub revenue_estimate_m: Option<f64>,
    pub analyst_count: Option<u32>,
    pub as_of_date: String,

    // NEW — all Option<T> so older snapshots still deserialize via #[serde(default)]
    #[serde(default)]
    pub price_target: Option<PriceTargetSummary>,
    #[serde(default)]
    pub recommendations: Option<RecommendationsSummary>,
}

pub struct PriceTargetSummary {
    pub mean: Option<f64>,
    pub median: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub analyst_count: Option<u32>,
    pub as_of_date: String,
}

pub struct RecommendationsSummary {
    pub strong_buy: u32,
    pub buy: u32,
    pub hold: u32,
    pub sell: u32,
    pub strong_sell: u32,
    pub as_of_date: String,
}
```

### Provider behavior

`YFinanceEstimatesProvider::fetch_consensus` fetches three yfinance endpoints concurrently via `tokio::join!` (not `try_join!` — `try_join!` short-circuits on the first `Err`, which would defeat field-granular fail-open):

1. `get_earnings_trend` (existing — populates eps/revenue/analyst_count)
2. `get_analyst_price_target` (new — populates `PriceTargetSummary`)
3. `get_recommendations_summary` (new — populates `RecommendationsSummary`)

**Field-granular fail-open** — each `Result` is inspected independently:

- If (1) succeeds and (2) or (3) fail, return `Ok(Some(evidence))` with `price_target=None` or `recommendations=None`. Log `tracing::warn!` per failed endpoint.
- If (1) fails but (2) or (3) succeed, still return `Ok(Some(evidence))` with `eps_estimate=None` / `revenue_estimate_m=None`. Previously (1) failing aborted the evidence; this is a deliberate loosening to let consensus data survive an earnings-trend outage.
- If all three fail, return `Err(TradingError::...)` — the existing enrichment fail-open at `run_analysis_cycle` marks `enrichment_consensus` unavailable and the pipeline continues.

### Prompt rendering

`build_enrichment_context()` in `agents/shared/prompt.rs` gains new formatting branches rendered inline with the existing block. No pre-digested labels — raw numbers, agents interpret:

```
[Analyst Consensus — as of 2026-04-24]
  EPS estimate (next Q):        $2.15  (N=28)
  Revenue estimate (next Q):    $94,200M
  Price target (mean):          $215.00  (N=42)
  Price target range:           $170.00 – $265.00
  Recommendations:              strong_buy=12, buy=18, hold=10, sell=2, strong_sell=0
```

## Component: News supplementation

New `YFinanceNewsProvider` + a small refactor of `prefetch_analyst_news` to consume the existing `NewsProvider` trait so the merge stays agnostic of the concrete client. No `MergedNewsProvider` wrapper, no `FinnhubNewsProvider` wrapper — `FinnhubClient` already implements `NewsProvider` in `data/provider_impls.rs`.

### Pre-existing shape (what we're refactoring)

Today the prefetch lives at `agents/analyst/mod.rs::prefetch_analyst_news` and takes the concrete client:

```rust
pub async fn prefetch_analyst_news(finnhub: &FinnhubClient, symbol: &str) -> Option<Arc<NewsData>> {
    match finnhub.get_structured_news(symbol).await { ... }
}
```

The pipeline calls it as `prefetch_analyst_news(&pipeline.finnhub, &symbol)` from `workflow/pipeline/runtime.rs`. This refactor replaces the concrete `FinnhubClient` parameter with two `Arc<dyn NewsProvider>` handles so we can fan the merge out symmetrically.

### YFinanceNewsProvider

```rust
// crates/scorpio-core/src/data/yfinance/news.rs

pub struct YFinanceNewsProvider {
    client: YFinanceClient,
}

#[async_trait]
impl NewsProvider for YFinanceNewsProvider {
    fn provider_name(&self) -> &'static str { "yfinance" }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> {
        // 1. Call yfinance_rs::news::NewsBuilder::new(symbol).fetch().await
        // 2. Normalize yfinance articles -> state::NewsArticle (same shape used
        //    today for Finnhub articles: title, url, source, published_at, summary).
        //    Filter to the target_date window.
        // 3. Return NewsData { articles, macro_events: vec![], summary: None }
        //    — macro_events stays empty; that's Finnhub's territory.
    }
}
```

### Orchestrator change

```rust
// crates/scorpio-core/src/agents/analyst/mod.rs — prefetch_analyst_news (NEW SIGNATURE)

pub async fn prefetch_analyst_news(
    finnhub_news: &Arc<dyn NewsProvider>,
    yfinance_news: &Arc<dyn NewsProvider>,
    symbol: &Symbol,
) -> Option<Arc<NewsData>> {
    let (finnhub_result, yfinance_result) = tokio::join!(
        finnhub_news.fetch(symbol),
        yfinance_news.fetch(symbol),
    );
    Some(Arc::new(merge_news(finnhub_result, yfinance_result)))
}
```

Pipeline call site updates accordingly: `workflow/pipeline/runtime.rs` constructs both providers (see "Provider construction" below) and passes them in. The single `Option<Arc<NewsData>>` return is preserved so downstream `GetCachedNews` / `GetNews` selection in `sentiment.rs` and `news.rs` is untouched.

### merge_news helper

```rust
fn merge_news(
    a: Result<NewsData, TradingError>,
    b: Result<NewsData, TradingError>,
) -> NewsData {
    // Both Ok:    dedupe by (normalized_url OR normalized_title), combine.
    //             macro_events carried from whoever populated it (Finnhub today).
    // One Err:    log the failing provider, return the other's output.
    // Both Err:   log both, return NewsData::default().
    // Sort merged articles by published_at descending.
    // Cap at NEWS_MAX_ARTICLES so prompt size stays bounded.
}
```

**Dedupe rule:** normalize URL by lowercasing + stripping UTM/querystring fragments, then hash. Fallback to normalized headline (lowercase, punctuation-stripped, first 80 chars) when URL missing. First-seen wins.

**Shared cache:** merged `NewsData` is wrapped in `Arc` and handed to both `NewsAnalyst` and `SentimentAnalyst` via the existing `GetCachedNews` tool path. No change to analyst code.

**`EventNewsEvidence` is untouched** — still Finnhub-sourced. yfinance news feeds the live-analyst path only.

## Component: Options snapshot tool

New trait, new provider, new rig Tool. The `DerivativesProvider` stub remains crypto-oriented; we introduce a separate `OptionsProvider` for structured equity options.

### OptionsProvider trait and types

```rust
// crates/scorpio-core/src/data/traits/options.rs

#[async_trait]
pub trait OptionsProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    /// Fetch summary + near-the-money slice for `symbol` as of `as_of_date`.
    /// Returns Ok(None) if the ticker has no listed options.
    async fn fetch_snapshot(
        &self,
        symbol: &Symbol,
        as_of_date: &str,
    ) -> Result<Option<OptionsSnapshot>, TradingError>;
}

pub struct OptionsSnapshot {
    pub symbol: String,
    pub as_of_date: String,
    pub spot_price: f64,

    // Summary metrics
    pub atm_iv: Option<f64>,                    // front-month ATM IV
    pub iv_term_structure: Vec<IvTermPoint>,    // IV per expiration (front to back)
    pub put_call_volume_ratio: Option<f64>,
    pub put_call_oi_ratio: Option<f64>,
    pub max_pain_strike: Option<f64>,
    pub skew_25d: Option<f64>,                  // 25-delta put IV minus 25-delta call IV

    // Near-the-money slice (nearest expiration only, strikes within ±5% of spot)
    pub near_term_expiration: Option<String>,   // YYYY-MM-DD
    pub near_term_strikes: Vec<NearTermStrike>,
}

pub struct IvTermPoint {
    pub expiration: String,       // YYYY-MM-DD
    pub days_to_expiry: u32,
    pub atm_iv: f64,
}

pub struct NearTermStrike {
    pub strike: f64,
    pub call_volume: u64,
    pub call_oi: u64,
    pub call_iv: Option<f64>,
    pub put_volume: u64,
    pub put_oi: u64,
    pub put_iv: Option<f64>,
}
```

### YFinanceOptionsProvider and GetOptionsSnapshot tool

```rust
// crates/scorpio-core/src/data/yfinance/options.rs

pub struct YFinanceOptionsProvider {
    client: YFinanceClient,
}

#[async_trait]
impl OptionsProvider for YFinanceOptionsProvider {
    // Fetch via yfinance_rs options API (ticker → expirations → full chain per
    // expiration). Compute summary metrics from full chain. Slice near-the-money
    // from nearest expiration. Return OptionsSnapshot; discard raw contracts.
}

pub struct GetOptionsSnapshot {
    provider: Arc<dyn OptionsProvider>,
    symbol: Symbol,
    as_of_date: String,
}

impl Tool for GetOptionsSnapshot {
    const NAME: &'static str = "get_options_snapshot";
    type Args = ();                  // no args — scoped at construction
    type Output = OptionsSnapshot;
    // execute() delegates to self.provider.fetch_snapshot(...).
    // On Ok(None) return an error: "no listed options for {symbol}" —
    // LLM treats as a signal to skip options analysis per prompt guidance.
}
```

### Metric computation (unit-testable against fixtures)

- **ATM IV** — linear interpolation of IV between the two strikes straddling spot, front-month only. Uses the call leg's IV (puts would give the same value at true ATM under put-call parity, but calls are less sensitive to early-exercise effects on dividend-paying tickers).
- **Term structure** — for each expiration, compute the same ATM-interpolated IV using that expiration's own chain; emit sorted by `days_to_expiry` ascending.
- **Put/call volume ratio** — `sum(put_volume) / sum(call_volume)` across all strikes/expirations.
- **Put/call OI ratio** — `sum(put_oi) / sum(call_oi)` across all strikes/expirations.
- **Max pain** — strike that minimizes total dollar loss to option holders at expiration. Computed over front-month only; documented clearly in struct docstring.
- **25-delta skew** — interpolate 25-delta put IV and 25-delta call IV from front-month; subtract. `None` if chain is too thin to interpolate reliably.

### TechnicalAnalyst wiring

```rust
// crates/scorpio-core/src/agents/analyst/equity/technical.rs

let tools = vec![
    Box::new(GetOhlcv::scoped(...)) as Box<dyn ToolDyn>,
    Box::new(CalculateAllIndicators::new()) as Box<dyn ToolDyn>,
    // ... existing indicators
    Box::new(GetOptionsSnapshot::scoped(options_provider, symbol, date)) as Box<dyn ToolDyn>,
];
```

**Prompt addition** is appended to `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` — the equity pack's role markdown is the single source of truth for analyst system prompts after the prompt-bundle centralization refactor. No Rust-side prompt edit is needed; the load-time `analyst_runtime_contract.md` injection in `analysis_packs/equity/baseline.rs::baseline_prompt_bundle` continues to apply automatically.

> If `get_options_snapshot` returns data, incorporate implied-volatility regime (via `atm_iv` and `iv_term_structure`) and positioning skew (put/call ratios, 25-delta skew) into your technical read. The `near_term_strikes` slice is useful when earnings or material events are within the window. If the tool errors with "no listed options", omit options analysis without retrying.

The golden-byte regression gate at `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` will need to be regenerated once the markdown is updated:

```bash
UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core \
    --test prompt_bundle_regression_gate --features test-helpers
```

**`TechnicalData` state extension** — new optional field `options_summary: Option<String>` so the analyst's own options interpretation persists in state alongside the RSI/MACD/ATR summary. Optional to preserve backward compat for tickers without options.

## State and persistence

- `ConsensusEvidence` grows two new optional fields (above). No new top-level state field. The struct does **not** carry `#[serde(deny_unknown_fields)]`, so `#[serde(default)]` on each new field is sufficient for older snapshots to deserialize cleanly.
- `TechnicalData` grows `options_summary: Option<String>` with `#[serde(default)]`. Per CLAUDE.md's snapshotted-state rule, `#[serde(deny_unknown_fields)]` has been removed from all state structs reachable from `TradingState` — so additive fields deserialize cleanly on both old-reading-new and new-reading-old paths.
- `NewsArticle` gains `url: Option<String>` with `#[serde(default)]` to support cross-provider deduplication and provenance. (The original draft said `NewsData` would stay unchanged; the implementation plan guardrails supersede this.)

**Snapshot schema version** — `THESIS_MEMORY_SCHEMA_VERSION` stays at **3**. All new fields are additive with `#[serde(default)]`, so no schema bump is needed. A bump would explicitly retire all v3 rows, which is disproportionate for purely additive changes. See CLAUDE.md's "TradingState schema evolution" rule: bumps are reserved for renames, removals, and backward-incompatible type changes.

## Provider construction (pipeline runtime)

```rust
// crates/scorpio-core/src/workflow/pipeline/runtime.rs

// Existing — FinnhubClient already implements NewsProvider directly
// (see data/provider_impls.rs); we just up-cast to the trait object.
let finnhub_news: Arc<dyn NewsProvider> = Arc::new(pipeline.finnhub.clone());
let estimates_provider = Arc::new(YFinanceEstimatesProvider::new(pipeline.yfinance.clone()));

// NEW
let yfinance_news: Arc<dyn NewsProvider> = Arc::new(YFinanceNewsProvider::new(pipeline.yfinance.clone()));
let options_provider = Arc::new(YFinanceOptionsProvider::new(pipeline.yfinance.clone()));
```

Both `Arc<dyn NewsProvider>` handles are threaded through `run_analyst_team` to the refactored `prefetch_analyst_news`. The options provider is threaded through the same path and bound into the TechnicalAnalyst tool list.

## Configuration

No new user-facing config keys. Internal constants added to `constants.rs`:

- `OPTIONS_NTM_STRIKE_BAND: f64 = 0.05` (±5% band around spot)
- `OPTIONS_FETCH_TIMEOUT_SECS: u64 = 30` (per snapshot fetch)

## Error handling matrix

| Stream                   | Scope                 | Failure mode                            | Behavior                                                                                                                                                               |
|--------------------------|-----------------------|-----------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Extended consensus fetch | Pipeline startup      | Earnings trend fails                    | Existing — log warn, `enrichment_consensus` marked unavailable. Pipeline continues.                                                                                    |
| Extended consensus fetch | Pipeline startup      | Price target or recs fails, earnings OK | NEW — field-granular fail-open. Log warn per failed field, populate `price_target=None` or `recommendations=None`.                                                     |
| yfinance news            | Phase 1 prefetch      | yfinance fails, Finnhub OK              | `merge_news` returns Finnhub feed alone. Log warn with provider name.                                                                                                  |
| yfinance news            | Phase 1 prefetch      | Both providers fail                     | Return empty `NewsData`. Analysts see existing "news unavailable" marker. Pipeline continues.                                                                          |
| Options snapshot         | TechnicalAnalyst turn | Ticker has no listed options            | Provider returns `Ok(None)`; tool returns error "no listed options for {symbol}". LLM omits options analysis per prompt, no retry.                                     |
| Options snapshot         | TechnicalAnalyst turn | Network/parse failure                   | Tool returns error; existing `RetryPolicy` applies (max 3 retries, exponential backoff). If still failing, LLM continues without options data; `options_summary=None`. |

**Timeout wiring:**

- Pre-debate consensus fetch — uses the existing enrichment-prefetch timeout in `run_analysis_cycle` (no new constant).
- yfinance news prefetch — uses the existing news-prefetch timeout that already bounds the Finnhub fetch (no new constant).
- Options snapshot tool call inside TechnicalAnalyst turn — wrapped in `tokio::time::timeout(OPTIONS_FETCH_TIMEOUT_SECS, ...)` at the provider call site so a slow yfinance chain fetch doesn't stall the analyst's tool turn budget. This is why `OPTIONS_FETCH_TIMEOUT_SECS` is added as a new constant.

## Backward compatibility

- Old `ConsensusEvidence` snapshots: load with `price_target = None`, `recommendations = None` via `#[serde(default)]` on each new field.
- Old `TechnicalData` snapshots: load with `options_summary = None` via `#[serde(default)]`. The `#[serde(deny_unknown_fields)]` attribute has been removed from all snapshotted state structs per CLAUDE.md — older binaries reading newer snapshots tolerate unknown keys rather than rejecting them.
- Old `NewsArticle` snapshots: load with `url = None` via `#[serde(default)]`.
- Old prompt templates: unaffected — new fields render only when `Some(...)`.
- Schema version stays at **3** — purely additive changes do not warrant a bump.
- CLI report: unchanged.

## Testing strategy

### Unit tests (hermetic, `cargo nextest`)

**`data/yfinance/news.rs`**
- `fetches_and_normalizes_articles` — fixture yfinance payload, assert `NewsArticle` field mapping and date-window filter.
- `empty_feed_returns_empty_news_data` (not an error).

**`data/yfinance/options.rs`**
- `computes_atm_iv_from_chain` — fixture chain, assert interpolation picks the right strike.
- `computes_put_call_ratios_over_all_strikes` — assert aggregation math.
- `computes_max_pain_front_month_only` — fixture, assert correct strike.
- `near_term_slice_filters_to_ntm_band` — assert ±5% band respected.
- `returns_none_when_no_options_listed` — empty expiration list ⇒ `Ok(None)`.
- `returns_partial_snapshot_when_skew_unavailable` — thin chain; summary populated, `skew_25d=None`.

**`data/adapters/estimates.rs` (modified)**
- `populates_price_target_when_available`
- `returns_partial_when_recommendations_fails` — partial failure, eps+revenue still populated, recs=None.
- `returns_ok_none_when_all_three_fail`.

**`agents/analyst/mod.rs` (new merge helper, alongside the refactored `prefetch_analyst_news`)**
- `merge_dedupes_by_url`
- `merge_dedupes_by_headline_when_url_missing`
- `merge_falls_back_to_single_provider_on_partial_failure`
- `merge_returns_empty_when_both_fail`
- `merge_caps_at_news_max_articles`
- `prefetch_analyst_news_returns_arc_when_either_provider_succeeds` — exercises the new `Arc<dyn NewsProvider>` signature using stub providers.

### Integration tests (`crates/scorpio-core/tests/`)

- `extended_consensus_populates_price_target_and_recommendations` — stub `YFinanceEstimatesProvider`, run `run_analysis_cycle`, assert state slot carries new fields.
- `options_tool_returns_snapshot_to_technical_analyst` — stub `OptionsProvider`, run TechnicalAnalyst in isolation via `run_analyst_inference`, assert `TechnicalData.options_summary` populated.

### Live smoke test — `examples/yfinance_live_test.rs`

Extend the existing manual smoke test (not in CI) with four new sections, preserving the pass/fail tracker format and exit-1-on-failure:

- **Section 7 (new)**: `YFinanceNewsProvider::fetch(AAPL)` — assert non-empty `articles`, every `published_at` parses as RFC3339, URLs non-empty.
- **Section 8 (new)**: extended `YFinanceEstimatesProvider::fetch_consensus(AAPL, today)` — assert `price_target.mean > 0` and at least one recommendation bucket > 0. Partial success (missing one extra endpoint) passes with a WARN line so the test is resilient to temporary per-endpoint outages.
- **Section 9 (new)**: `YFinanceOptionsProvider::fetch_snapshot(AAPL, today)` — assert `spot_price > 0`, `atm_iv` in plausible range (0.05–2.0), `iv_term_structure` non-empty, `near_term_strikes` non-empty.
- **Section 10 (new)**: ETF/degradation paths.
  - `YFinanceNewsProvider::fetch(SPY)` — returns without panicking; empty feed is acceptable (ETF news coverage can be sparse). WARN line on empty, no FAIL.
  - `YFinanceOptionsProvider::fetch_snapshot(SPY, today)` — SPY has a deep listed options chain, so this is expected to succeed (assert `spot_price > 0`, non-empty `iv_term_structure`).
  - `YFinanceEstimatesProvider::fetch_consensus(SPY, today)` — ETF has no analyst coverage; assert provider returns `Ok(None)` or `Ok(Some(evidence))` with all Option fields `None`, and does not panic. WARN line, no FAIL.

Existing sections 1–6 are unchanged.

## Deferred decisions

**Cross-analyst options routing.** Options data is Technical-Analyst-scoped for v1. The `OptionsProvider` trait lives in `data/traits/options.rs` and is consumed only by `TechnicalAnalyst`. Cross-analyst access (Sentiment Agent or Risk Agents reading the same snapshot) would require routing through `data/routing.rs` and reconciling `OptionsProvider` with the existing `DerivativesProvider` placeholder in `data/traits/derivatives.rs`. That reconciliation is a deferred decision pending a concrete written request from a Sentiment or Risk agent author — it should not be pulled forward on an unowned demand signal.

## Out of scope / deferred

- **Crypto derivatives coverage.** The existing `DerivativesProvider` stub stays unused; crypto pack will wire its own providers when that pack ships.
- **Upgrade/downgrade event history.** yfinance exposes this as a separate endpoint — a natural phase-2 addition as a new enrichment slot or a bounded history appended to `RecommendationsSummary`.
- **Unusual options activity detection.** Noisy on low-volume tickers; worth its own design once the baseline snapshot is in place.
- **CLI report surfaces.** Data is reachable via SQLite phase snapshots. Report expansion is a separate UX change.
- **Full options chain in prompts.** Summary + near-the-money slice is intentional scope discipline.
- **Config-driven provider toggles.** No `enable_yfinance_news` / `enable_options_snapshot` flags initially; providers are always on, failures are fail-open.

## Open questions

None outstanding. The rollout is a single integrated plan; the implementation plan will sequence the five concrete tasks (consensus extension, news provider, news merge orchestration, options trait/provider/tool, smoke-test updates) internally for review discipline.
