---
title: Concrete enrichment provider pattern behind adapter trait seams
date: 2026-04-10
category: best-practices
module: data/adapters
problem_type: best_practice
component: data_pipeline
severity: medium
applies_when:
  - Adding a new data vendor behind an existing adapter trait
  - Implementing fail-open enrichment in a pipeline startup path
  - Normalizing vendor payloads into shared evidence types
tags:
  - enrichment
  - adapters
  - providers
  - fail-open
  - timeout
  - normalization
  - vendor-integration
---

# Concrete enrichment provider pattern behind adapter trait seams

## Context

The scorpio-analyst pipeline uses Stage 1 adapter trait seams (`EventNewsProvider`,
`EstimatesProvider`, `TranscriptProvider`) with `null` placeholder values seeded by
`PreflightTask`. Milestone 7 required turning these contract-only traits into working
enrichment flows backed by the free-tier vendors (Finnhub for events, yfinance-rs for
consensus estimates), without disrupting the existing pipeline or making enrichment
mandatory.

The challenge: enrichment providers live in the startup path (before the graph runs),
must be fail-open (no blocking the run), must respect target-date authority (no future
data in backtests), and must integrate cleanly with the existing context-key and
state-based data flow.

## Guidance

### 1. Implement concrete providers in the adapter module itself

When a provider is backed by a single vendor, keep the implementation in the adapter
file alongside the trait. No vendor-specific sub-module needed until complexity warrants
it.

```rust
// src/data/adapters/events.rs
pub struct FinnhubEventNewsProvider {
    client: FinnhubClient,
}

impl FinnhubEventNewsProvider {
    pub fn new(client: FinnhubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EventNewsProvider for FinnhubEventNewsProvider {
    async fn fetch_event_news(
        &self, symbol: &str, as_of_date: &str,
    ) -> Result<Vec<EventNewsEvidence>, TradingError> {
        let target = parse_date(as_of_date)?;
        // Filter by target_date for time-authority safety
        let raw = self.client.fetch_company_news(symbol, &from_str, &to_str).await?;
        Ok(raw.into_iter()
            .filter(|n| n.datetime <= target_end_of_day)
            .map(|n| normalize_company_news(symbol, n))
            .collect())
    }
}
```

### 2. Use a three-state `EnrichmentResult<T>` at the adapter boundary

`Option<T>` conflates "not fetched" with "fetched, nothing found." Use an explicit enum:

```rust
pub enum EnrichmentResult<T> {
    Available(T),      // Provider returned usable data
    NotAvailable,      // Provider confirmed no data exists
    FetchFailed(String), // Fetch attempt failed with reason
}

impl<T> EnrichmentResult<T> {
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Available(v) => Some(v),
            Self::NotAvailable | Self::FetchFailed(_) => None,
        }
    }
}
```

Downstream fail-open consumers call `.into_option()`. Observability consumers can
match on `FetchFailed` to surface fetch errors.

### 3. Hydrate enrichment in `run_analysis_cycle` with timeout-bounded fetch

Enrichment lives in the startup path, not inside LLM agents or `PreflightTask`.
Wrap each fetch in `tokio::time::timeout` with a config-driven duration:

```rust
async fn hydrate_event_news(
    finnhub: &FinnhubClient, symbol: &str, target_date: &str,
    timeout: Duration,
) -> EnrichmentResult<Vec<EventNewsEvidence>> {
    let provider = FinnhubEventNewsProvider::new(finnhub.clone());
    match tokio::time::timeout(timeout, provider.fetch_event_news(symbol, target_date)).await {
        Ok(Ok(events)) if events.is_empty() => EnrichmentResult::NotAvailable,
        Ok(Ok(events)) => EnrichmentResult::Available(events),
        Ok(Err(e)) => EnrichmentResult::FetchFailed(e.to_string()),
        Err(_) => EnrichmentResult::FetchFailed("enrichment fetch timed out".to_owned()),
    }
}
```

### 4. Use `seed_if_absent` to preserve pre-hydrated enrichment

`PreflightTask` seeds cache keys with `"null"`. After enrichment hydration writes real
data to the context, preflight must not overwrite it:

```rust
async fn seed_if_absent(context: &Context, key: &str) {
    let existing: Option<String> = context.get(key).await;
    match existing.as_deref() {
        None | Some("null") => { context.set(key, "null".to_owned()).await; }
        Some(_) => { /* Already populated — do not overwrite */ }
    }
}
```

### 5. Cache shared API responses across callers

When multiple consumers call the same vendor API with identical parameters (e.g.,
the news analyst pre-fetch and the enrichment provider both need company news for
the same symbol and date range), cache at the client level to avoid duplicate
network requests and rate-limit consumption.

```rust
// src/data/finnhub.rs
type NewsCacheKey = (String, String, String); // (symbol, from, to)

#[derive(Clone)]
pub struct FinnhubClient {
    inner: FhClient,
    limiter: SharedRateLimiter,
    news_cache: Arc<tokio::sync::RwLock<HashMap<NewsCacheKey, Vec<CompanyNews>>>>,
}

pub async fn fetch_company_news(&self, symbol: &str, from: &str, to: &str)
    -> Result<Vec<CompanyNews>, TradingError>
{
    let key = (symbol.to_owned(), from.to_owned(), to.to_owned());

    // Fast path: return cached result.
    {
        let cache = self.news_cache.read().await;
        if let Some(cached) = cache.get(&key) {
            return Ok(cached.clone());
        }
    }

    // Cache miss: fetch from API, then store.
    self.limiter.acquire().await;
    let result = self.inner.news().company_news(symbol, from, to).await?;
    self.news_cache.write().await.insert(key, result.clone());
    Ok(result)
}
```

Higher-level methods (e.g., `get_structured_news`) and enrichment providers both
delegate to this cached method. Since `FinnhubClient` is `Clone` with the cache
behind `Arc`, all cloned instances share the same cache.

### 6. Normalize vendor payloads through shared adapter types

Each vendor has different field names and units. Normalize at the boundary:

- Finnhub `CompanyNews.datetime` (unix timestamp) -> `EventNewsEvidence.event_timestamp` (ISO-8601)
- yfinance `EarningsTrendRow.revenue_estimate.avg` (raw Money) -> `ConsensusEvidence.revenue_estimate_m` (USD millions via `money_to_f64(m) / 1_000_000.0`)
- Always use fallback values that satisfy the contract (e.g., `"1970-01-01T00:00:00Z"` not `"<unix_ts>Z"`)

## Why This Matters

- **Fail-open enrichment** prevents optional data vendors from blocking production runs.
  A 10-second timeout on an unresponsive vendor degrades gracefully.
- **Three-state results** enable observability: operators can distinguish "data doesn't
  exist" from "vendor is down" without inspecting logs.
- **Time-authority filtering** prevents backtest contamination with future-published data.
- **Trait-based providers** allow swapping vendors without changing downstream consumers.
- **Client-level caching** prevents duplicate API calls when multiple pipeline
  stages need the same vendor data (e.g., news analyst + enrichment provider).

## When to Apply

- Adding a new data vendor to the enrichment system (e.g., premium transcript provider)
- Implementing any optional, fail-open data enrichment in a pipeline startup path
- Normalizing external API responses into internal evidence types
- Adding timeout-bounded network calls to a critical path
- Multiple pipeline stages consuming the same vendor API with identical parameters

## Examples

**Before (Stage 1 placeholder):**
```rust
// PreflightTask always wrote null
context.set(KEY_CACHED_CONSENSUS, "null".to_owned()).await;
context.set(KEY_CACHED_EVENT_FEED, "null".to_owned()).await;
```

**After (concrete enrichment):**
```rust
// run_analysis_cycle hydrates real data when enabled
let event_enrichment = if enrichment_cfg.enable_event_news {
    hydrate_event_news(&finnhub, &symbol, &date, fetch_timeout).await
} else {
    EnrichmentResult::NotAvailable
};
initial_state.enrichment_event_news = event_enrichment.into_option();

// PreflightTask preserves pre-hydrated data
seed_if_absent(&context, KEY_CACHED_EVENT_FEED).await;
```

**Review finding fixed:** The original `normalize_company_news` used `format!("{}Z", news.datetime)` as a timestamp fallback, producing `"1706475600Z"` which breaks the ISO-8601 contract. Fixed to `"1970-01-01T00:00:00Z"` — always satisfy the contract shape, even in degenerate cases.

## Related

- Plan: `docs/plans/2026-04-07-004-feat-concrete-enrichment-providers-plan.md`
- Stage 1 contracts: `src/data/adapters/` (events.rs, estimates.rs, transcripts.rs)
- Runtime hydration: `src/workflow/pipeline/runtime.rs`
