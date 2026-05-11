# Catalyst Calendar Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the "degraded mode: news-discovered events only" caveat in Theme G of `2026-05-10-analytical-themes-port.md` with a real forward-looking catalyst calendar built from free-tier data sources we already have keys for: Finnhub earnings + IPO calendars, FRED scheduled-release endpoint, yfinance per-ticker calendar, and a new SEC EDGAR 8-K monitor.

**Architecture:** Three tiers shipped independently.

1. **Tier 1 — Structured APIs only.** Earnings (Finnhub range), IPO debut (Finnhub range), economic releases (FRED `/fred/releases/dates`), ex-dividend (yfinance per-ticker). All return parseable JSON; no text parsing. Covers the highest-impact catalyst categories (Earnings, Macro, IPO Debut). Ships first.
2. **Tier 2 — SEC EDGAR 8-K monitor.** Pull recent 8-Ks for the analysed ticker, tag by Item code (1.01, 2.01, 2.02, 5.07, 7.01, 8.01) plus 13D/G filings, expose `(filing_date, item, primary_doc_url)` without parsing the body. Adds activist / buyback / M&A-announcement / shareholder-vote / item-7.01 (RegFD) coverage cheaply because the categorisation comes from the filing's own Item header.
3. **Tier 3 — Optional filing-body parsing.** S-1 lockup language → lockup expiry date; DEF M14A → expected close date; FDA AdComm scraping. **Out of scope for this plan.** Documented as a follow-up gate so Tier 1+2 can ship first.

**What this plan does NOT cover:** FDA PDUFA / advisory-committee calendar, conferences and investor-day calendars, IPO lockup expiry dates, M&A expected-close dates. These require either a paid feed (Finnhub Premium, Wall Street Horizon) or non-trivial filing-text parsing (Tier 3). The user-facing prompt continues to say `data not wired` for these specific subcategories until Tier 3 lands.

**Tech Stack:** Rust, existing `finnhub` 0.2.2, existing `yfinance-rs` 0.7.2, existing FRED HTTP wrapper, new lightweight `reqwest` client for SEC EDGAR (no new top-level crate dependency required).

---

## Failure-Mode Discipline

The catalyst calendar is **non-blocking enrichment**: the analysis pipeline must always proceed, even when every catalyst source fails. This is the same contract `enrichment_consensus` and `enrichment_event_news` already follow (`crates/scorpio-core/src/state/trading_state.rs`). Catalysts are **decorative for the analyst prompt**, not a structural input — the news analyst can still produce a valid `NewsData` output with zero forward catalysts. SEC EDGAR is the highest-risk source (free public service with no SLA, undocumented throttling beyond the 10 req/sec ceiling, JSON-shape changes have happened historically), so the failure-mode rules are written defensively around it.

**Per-source invariants (all providers in this plan must satisfy):**

1. **Construct-time fallibility, runtime infallibility.** A provider may return `Err` from `new(...)` (e.g. SEC EDGAR refuses to construct without a valid `User-Agent`). Once constructed, `fetch_catalysts(...)` **never** returns `Err` — it returns `Ok(Vec::new())` and emits a `tracing::warn!` with a `kind = "catalyst_fetch_failed"` field for any of: HTTP 4xx (including auth/UA rejection at runtime), HTTP 5xx, transport timeout, malformed JSON, missing CIK lookup, rate-limit exhaustion after backoff.
2. **Partial-source success on composition.** `Tier1CatalystProvider` and `Tier2CatalystProvider` fan out their sub-providers via `tokio::join!` (not `tokio::try_join!`) and concatenate whatever succeeded. One source erroring zeros out only that source's contribution; the others still flow through.
3. **No source-aware branching in prompts.** The renderer's existing `(no upcoming catalysts in the next 30 days)` literal is the user-visible signal for "fetched, none returned." A separate `(no upcoming catalysts: data unavailable)` literal is reserved for the case where preflight chose to skip the catalyst prefetch entirely (e.g. baseline pack opt-out). Per-source debug status lives in `tracing` only — the prompt does not say "SEC EDGAR was down."
4. **EnrichmentState semantics:**
    - `payload: None` ⇒ catalyst fetch was not attempted (preflight skipped).
    - `payload: Some(Vec::new())` ⇒ fetch ran; zero events to report (could be all-sources-failed or genuine quiet window — indistinguishable from the prompt's perspective, by design).
    - `payload: Some(events)` ⇒ at least one source returned events.
5. **Construction failure of SEC EDGAR (Tier 2) falls back to Tier 1.** Logged once at startup as `info!("falling back to Tier 1 catalyst provider: <reason>")`. Never aborts pipeline construction.

These invariants are enforced by the live-API smoke-test examples (Task 3 Step 4, Task 8 Step 5): each example asserts that injecting fault conditions (bad UA, fake CIK, network unreachable) produces an empty result with a structured warn, **not** an error return.

---

## Decision Summary

| Question                     | Answer                                                                                                                                                                                                                                                                                                                                                                                                |
|------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **Effort**                   | Tier 1 ≈ 1.5–2 days. Tier 2 ≈ 3–4 days (mostly SEC EDGAR client + auth/UA discipline + Item-code tagging). Tier 3 ≈ multi-week, deferred.                                                                                                                                                                                                                                                             |
| **Risk**                     | Low for Tier 1 (existing crates, structured JSON). Medium for Tier 2 (SEC EDGAR mandates `User-Agent: Name email`, 10 req/sec cap; filing index format is HTML/Atom feed depending on endpoint).                                                                                                                                                                                                      |
| **Data source dependencies** | Adds three endpoint calls to existing crates + one new SEC EDGAR HTTP client. No new top-level Cargo dependencies.                                                                                                                                                                                                                                                                                    |
| **Schema migrations**        | NONE. New `CatalystEvent` evidence type + new `EnrichmentState<Vec<CatalystEvent>>` field on `TradingState`. Field is `#[serde(default)]` per CLAUDE.md so old snapshots deserialize unchanged.                                                                                                                                                                                                       |
| **Provider compatibility**   | Free-tier Finnhub returns 403 on `/calendar/economic` and `misc().fda_calendar()` — **do not call them**. Free-tier `/calendar/earnings` and `/calendar/ipo` are confirmed available (see `~/.cargo/registry/src/index.crates.io-*/finnhub-0.2.2/src/endpoints/calendar.rs`). FRED `/fred/releases/dates` is free with the existing API key. SEC EDGAR is free with no key, but mandates a UA header. |
| **Highest-leverage payoff**  | Tier 1 alone covers Earnings + Macro + IPO debut, which are the H-impact rows in the Theme G impact-tier table. That alone retires the user-facing `degraded mode: news-discovered events only` caveat for those three categories.                                                                                                                                                                    |

**Recommendation:** Ship Tier 1 first behind a feature gate / config flag, validate prompt-context cost, then ship Tier 2. Tier 3 stays a follow-up plan.

---

## Why a separate plan?

`2026-05-10-analytical-themes-port.md` is a prompt port. Its Theme G ships in **degraded mode** explicitly because no catalyst data source is wired. This plan is the data-wiring counterpart — strictly Rust runtime work that produces a `CatalystEvent` stream, plus the prompt-context substitution that lets the news analyst's already-shipped Theme G prompt template stop saying `degraded mode`.

Splitting them keeps the prompt port shippable today (no runtime work blocks it) and keeps this work independently reviewable as a data-source change. Theme G's degraded-mode caveat in the prompt is removed only when Tier 1 of this plan lands.

---

## What each source covers (free tier only)

| Catalyst category            | yfinance-rs                        | finnhub-rs free                              | FRED                                  | SEC EDGAR                               | Coverage after Tier 1+2 |
|------------------------------|------------------------------------|----------------------------------------------|---------------------------------------|-----------------------------------------|-------------------------|
| Earnings releases            | per-ticker via `Ticker.calendar()` | range via `calendar().earnings(from,to,sym)` | —                                     | 8-K item 2.02 (announce)                | ✅ strong                |
| Economic releases            | —                                  | ❌ premium-only                               | `/fred/releases/dates` per release-id | —                                       | ✅ strong                |
| IPO debut                    | —                                  | range via `calendar().ipo(from,to)`          | —                                     | S-1 effectiveness (Tier 3)              | ✅ strong                |
| Ex-dividend                  | per-ticker via `Ticker.calendar()` | ❌                                            | —                                     | —                                       | ✅ per-ticker            |
| Activist filings             | —                                  | —                                            | —                                     | 13D / 13G                               | ✅ via Tier 2            |
| Buyback announcements        | —                                  | —                                            | —                                     | 8-K item 8.01                           | ✅ via Tier 2            |
| M&A announcement (signed)    | —                                  | —                                            | —                                     | 8-K item 1.01                           | ✅ via Tier 2            |
| M&A close (actual)           | —                                  | —                                            | —                                     | 8-K item 2.01                           | ✅ via Tier 2            |
| Shareholder vote             | —                                  | —                                            | —                                     | 8-K item 5.07                           | ✅ via Tier 2            |
| RegFD-disclosed material     | —                                  | —                                            | —                                     | 8-K item 7.01                           | ✅ via Tier 2            |
| FDA decisions (PDUFA/AdComm) | —                                  | ❌ premium-only                               | —                                     | partial via 8-K body (Tier 3)           | ❌ Tier 3 / paid only    |
| Conferences / investor days  | —                                  | —                                            | —                                     | partial via 8-K item 7.01 announcements | ❌ Tier 3 / paid only    |
| IPO lockup expiries          | —                                  | —                                            | —                                     | S-1 lockup language (Tier 3)            | ❌ Tier 3                |
| M&A expected-close dates     | —                                  | —                                            | —                                     | DEF M14A proxy (Tier 3)                 | ❌ Tier 3                |

`Ticker.calendar()` and `CalendarEndpoints::{earnings, ipo, economic}` already exist in the pinned crate versions (verified in `~/.cargo/registry/src/index.crates.io-*/finnhub-0.2.2/src/endpoints/calendar.rs` and `yfinance-rs-0.7.2/src/fundamentals/api.rs`). FRED `/fred/releases/dates` is a public endpoint we just have not wrapped yet.

---

## File Structure

### Files to create

| Path                                                         | Responsibility                                                                                                                                                                              |
|--------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/data/adapters/catalysts.rs`         | `CatalystEvent` payload + `CatalystCategory` / `ImpactLevel` enums + `CatalystCalendarProvider` trait. Mirrors the `EventNewsEvidence` / `EventNewsProvider` shape in `adapters/events.rs`. |
| `crates/scorpio-core/src/data/sec_edgar.rs`                  | New SEC EDGAR HTTP client (UA-mandated, 10 req/sec rate-limited via existing `SharedRateLimiter`). Exposes `fetch_recent_filings(cik, form_types, from, to)`.                               |
| `crates/scorpio-core/tests/catalyst_calendar_integration.rs` | Integration test: each provider returns deterministically-shaped `CatalystEvent`s on canned/fixture responses; live tests gated on `#[ignore]` per existing pattern.                        |
| `crates/scorpio-core/examples/fred_live_test.rs`             | Live FRED smoke test (Task 3 Step 4). Mirrors `finnhub_live_test.rs` PASS/FAIL aggregator. Asserts every release-id constant returns ≥1 row in a 60-day window.                             |
| `crates/scorpio-core/examples/sec_edgar_live_test.rs`        | Live SEC EDGAR smoke test (Task 8 Step 5). Asserts construct-time UA validation, happy-path filings fetch, and fail-soft contracts (bogus CIK → `Ok(empty)`).                               |

### Files to modify

| Path                                                                    | Change                                                                                                                                                                                                                                                              |
|-------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/data/mod.rs`                                   | Re-export `sec_edgar::SecEdgarClient` + `adapters::catalysts::*`.                                                                                                                                                                                                   |
| `crates/scorpio-core/src/data/finnhub.rs`                               | Add `pub async fn fetch_earnings_calendar(&self, from: &str, to: &str, symbol: Option<&str>) -> ...` + `fetch_ipo_calendar(&self, from, to)`. Both wrap `CalendarEndpoints`.                                                                                        |
| `crates/scorpio-core/src/data/fred.rs`                                  | Add `pub async fn release_dates(&self, release_id: u32, from: &str, to: &str) -> ...` + a `RELEASE_IDS` constant table for {CPI=10, NFP=50, FOMC=101, GDP=53, ISM=21, Retail Sales=14}. Numbers verified at https://fred.stlouisfed.org/releases.                   |
| `crates/scorpio-core/src/data/yfinance/financials.rs`                   | Surface the existing `Calendar { earnings_dates, ex_dividend_date, dividend_date }` via a public method on `YFinanceClient`. (`Ticker::calendar()` already exists internally.)                                                                                      |
| `crates/scorpio-core/src/data/adapters/mod.rs`                          | `pub mod catalysts;`                                                                                                                                                                                                                                                |
| `crates/scorpio-core/src/state/trading_state.rs`                        | Add `pub enrichment_catalysts: EnrichmentState<Vec<CatalystEvent>>` to `TradingState` with `#[serde(default)]`. Mirror the existing `enrichment_event_news` field declaration.                                                                                      |
| `crates/scorpio-core/src/state/news.rs`                                 | Move `CatalystCategory` and `ImpactLevel` enums here so the news analyst's `NewsArticle` can reference them (per Theme G's plan at `analytical-themes-port.md:106`). Both enums derive `Serialize/Deserialize/JsonSchema`.                                          |
| `crates/scorpio-core/src/agents/shared/prompt.rs`                       | Add `build_catalyst_calendar_block(state)` that renders the active catalyst window into the analyst-context body, with a `(no upcoming catalysts)` literal fallback when the field is empty.                                                                        |
| `crates/scorpio-core/src/analysis_packs/equity/prompts/news_analyst.md` | Replace the `<!-- TODO(catalyst-calendar): ... -->` HTML comment block (analytical-themes-port.md:436) with a real `## Upcoming Catalysts` section that consumes `{catalyst_calendar}`. Keep the H/M/L impact tier guidance from the original Theme G port.         |
| `crates/scorpio-core/src/analysis_packs/equity/baseline.rs`             | Add `event_news: true` already exists; do NOT add a new enrichment flag — Tier 1 catalysts run unconditionally for the equity baseline pack because the cost is bounded (one Finnhub range call shared across watchlist + one FRED call shared across all symbols). |
| `crates/scorpio-core/src/workflow/tasks/preflight.rs`                   | Add the catalyst prefetch alongside the existing news prefetch (`tokio::join!`-style fan-out). Catalyst fetch is per-pipeline-run, NOT per-analyst-clone.                                                                                                           |
| `crates/scorpio-core/src/workflow/pipeline/runtime.rs`                  | Wire the catalyst prefetch result into `state.enrichment_catalysts` before `analyst_fan_out` fires.                                                                                                                                                                 |
| `docs/superpowers/plans/2026-05-10-analytical-themes-port.md`           | Add a "Depends on" line at the top + downgrade Theme G's degraded-mode caveat to "until 2026-05-10-catalyst-calendar-integration Tier 1 lands". Touched in Task 7 of this plan.                                                                                     |

---

## Type Shapes (canonical)

```rust
// crates/scorpio-core/src/data/adapters/catalysts.rs
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::TradingError;
use crate::state::news::{CatalystCategory, ImpactLevel};

/// A single forward-looking catalyst event for a ticker.
///
/// Distinct from `EventNewsEvidence` (which is *backward-looking* news that
/// already happened). `CatalystEvent` always has `event_date >= as_of_date`
/// at the time the provider returned it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CatalystEvent {
    /// Canonical uppercase ticker, or `"_MACRO"` for ticker-agnostic macro releases.
    pub symbol: String,
    /// ISO-8601 date `"YYYY-MM-DD"`. Time-of-day is intentionally omitted —
    /// providers disagree on it and the prompt only needs the day.
    pub event_date: String,
    pub category: CatalystCategory,
    pub impact: ImpactLevel,
    /// Short label, e.g. `"AAPL Q3 earnings"`, `"FOMC rate decision"`,
    /// `"Activist 13D filed by <filer>"`. Sanitized through `sanitize_prompt_context`
    /// before reaching prompts.
    pub headline: String,
    /// Optional canonical source URL (SEC EDGAR primary doc, FRED release page).
    /// Provided for tier-3 follow-up fetches and audit, not currently surfaced
    /// to prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Identifier of the upstream provider (`"finnhub"`, `"fred"`, `"sec_edgar"`,
    /// `"yfinance"`). Stored for diagnostics; do not branch on this in prompt
    /// rendering.
    pub source: &'static str,
}

#[async_trait]
pub trait CatalystCalendarProvider: Send + Sync {
    /// Fetch upcoming catalysts for `symbol` in the half-open window
    /// `[as_of_date, as_of_date + horizon_days)`. Returns an empty `Vec` rather
    /// than `Err` for "no events" so the analyst-context renderer treats absence
    /// as a domain-valid signal rather than a fetch failure.
    async fn fetch_catalysts(
        &self,
        symbol: &str,
        as_of_date: &str,
        horizon_days: u32,
    ) -> Result<Vec<CatalystEvent>, TradingError>;
}
```

```rust
// crates/scorpio-core/src/state/news.rs (additions only)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalystCategory {
    EarningsAndFinancial,
    CorporateEvents,
    IndustryEvents,
    MacroEvents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum ImpactLevel {
    H,
    M,
    L,
}
```

The `_MACRO` sentinel symbol is intentional: FOMC and CPI releases apply to every ticker. The prompt-context renderer in `agents/shared/prompt.rs` interleaves them with the per-ticker events sorted by `event_date`.

---

## Phased Task Breakdown

### Tier 1 — Structured APIs only

#### Task 1: Add `CatalystEvent` types and `CatalystCalendarProvider` trait

**Files:**
- Create: `crates/scorpio-core/src/data/adapters/catalysts.rs`
- Modify: `crates/scorpio-core/src/data/adapters/mod.rs` (add `pub mod catalysts;`)
- Modify: `crates/scorpio-core/src/state/news.rs` (add `CatalystCategory` + `ImpactLevel`)

- [x] **Step 1: Write failing test for type roundtrip**

In `crates/scorpio-core/src/data/adapters/catalysts.rs::tests`, mirror the shape of `events.rs::tests::serialization_round_trip` for `EventNewsEvidence`:

```rust
#[test]
fn catalyst_event_round_trip() {
    let event = CatalystEvent { /* ... */ };
    let json = serde_json::to_string(&event).expect("serialize");
    let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, recovered);
}
```

- [x] **Step 2: Define types and trait**

Use the canonical `CatalystEvent` and `CatalystCalendarProvider` shapes from the **Type Shapes** section above. Both `CatalystCategory` and `ImpactLevel` go in `state/news.rs` (not in `adapters/catalysts.rs`) so the news analyst's `NewsArticle` can later reference them without a circular import.

- [x] **Step 3: Run tests**

Run: `cargo test -p scorpio-core data::adapters::catalysts`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-core/src/data/adapters/catalysts.rs crates/scorpio-core/src/data/adapters/mod.rs crates/scorpio-core/src/state/news.rs
git commit -m "feat(data): add CatalystEvent contract and CatalystCalendarProvider trait"
```

---

#### Task 2: Wrap `CalendarEndpoints::earnings` and `::ipo` in `FinnhubClient`

**Files:**
- Modify: `crates/scorpio-core/src/data/finnhub.rs`

The crate already exposes the endpoints we need (verified at `~/.cargo/registry/src/index.crates.io-*/finnhub-0.2.2/src/endpoints/calendar.rs`). The work is plumbing through `FinnhubClient` with the existing rate-limit + symbol-validation discipline.

- [x] **Step 1: Add `fetch_earnings_calendar` and `fetch_ipo_calendar`**

```rust
// In crates/scorpio-core/src/data/finnhub.rs alongside the existing fetch_company_news.
pub async fn fetch_earnings_calendar(
    &self,
    from: &str,
    to: &str,
    symbol: Option<&str>,
) -> Result<Arc<Vec<finnhub::models::calendar::EarningsCalendarEvent>>, TradingError> {
    self.limiter.acquire().await;
    let result = self
        .inner
        .calendar()
        .earnings(Some(from), Some(to), symbol)
        .await
        .map_err(map_finnhub_err)?;
    Ok(Arc::new(result.earnings_calendar))
}

pub async fn fetch_ipo_calendar(
    &self,
    from: &str,
    to: &str,
) -> Result<Arc<Vec<finnhub::models::calendar::IPOCalendarEvent>>, TradingError> {
    self.limiter.acquire().await;
    let result = self
        .inner
        .calendar()
        .ipo(from, to)
        .await
        .map_err(map_finnhub_err)?;
    Ok(Arc::new(result.ipo_calendar))
}
```

The exact re-export type names (`EarningsCalendarEvent`, `ipo_calendar` field) must be confirmed against `finnhub-0.2.2/src/models/calendar.rs` before commit.

- [x] **Step 2: Add a smoke test gated on a real API key**

Mirror the existing `#[ignore = "requires API key"]` pattern in `data/finnhub.rs` tests. Local CI does not run it; the developer running the migration runs it once to confirm the response shape.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(data): expose Finnhub earnings + IPO calendars on FinnhubClient"
```

---

#### Task 3: Add FRED `/fred/releases/dates` wrapper

**Files:**
- Modify: `crates/scorpio-core/src/data/fred.rs`

- [x] **Step 1: Add release-id constants**

```rust
// In crates/scorpio-core/src/data/fred.rs
//
// FRED release IDs for high-impact macro events. Verified at
// https://fred.stlouisfed.org/releases/ — release IDs are stable.
// Keep this list short (only H-impact releases per the Theme G impact-tier
// table) so the catalyst calendar stays focused.
pub mod release_id {
    pub const CPI: u32 = 10;
    pub const NONFARM_PAYROLLS: u32 = 50;
    pub const FOMC_DECISION: u32 = 101;
    pub const GDP: u32 = 53;
    pub const ISM_MANUFACTURING: u32 = 21;
    pub const RETAIL_SALES: u32 = 14;
}
```

(Numeric IDs **must** be verified against `https://api.stlouisfed.org/fred/releases?api_key=...` before commit. Do not guess.)

- [x] **Step 2: Add `release_dates` method**

```rust
pub async fn release_dates(
    &self,
    release_id: u32,
    from: &str,
    to: &str,
) -> Result<Vec<NaiveDate>, TradingError> {
    // Endpoint: /fred/release/dates?release_id=...&realtime_start=from&realtime_end=to
    // Documented at https://fred.stlouisfed.org/docs/api/fred/release_dates.html
    // ...
}
```

Reuse the existing `FredClient::TOTAL_RETRY_BUDGET` retry shape. Surface the `release_id` field in the JSON envelope as the source of truth (FRED includes `release_id`/`release_name`/`date` per record).

- [x] **Step 3: Smoke test gated on real API key**

Mirror the existing `#[ignore = "requires API key"]` pattern in `data/fred.rs` tests. Local CI does not run it; smoke validation belongs in the live-API example file added in Step 4.

- [x] **Step 4: Add `crates/scorpio-core/examples/fred_live_test.rs`**

Mirror the structure of the existing `crates/scorpio-core/examples/finnhub_live_test.rs` (PASS/FAIL aggregator, env-driven API key, exit non-zero on any failure). Cover the existing `get_series_latest` and `get_economic_indicators` plus the **new** `release_dates(release_id, from, to)` for every entry in the `release_id::*` constant table:

```rust
//! Live FRED API smoke test.
//!
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example fred_live_test
//! ```
//!
//! Requires `SCORPIO_FRED_API_KEY` in the environment.

use chrono::{Duration, Utc};
use scorpio_core::{
    config::ApiConfig,
    data::{FredClient, fred::release_id},
    rate_limit::SharedRateLimiter,
};
use secrecy::SecretString;

// (Reuse the `Results` aggregator pattern from finnhub_live_test.rs verbatim.)

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // ... key load, client construction, then per-endpoint checks:

    let to = Utc::now().date_naive();
    let from = to - Duration::days(60);

    for (label, id) in [
        ("CPI",                 release_id::CPI),
        ("Nonfarm Payrolls",    release_id::NONFARM_PAYROLLS),
        ("FOMC decision",       release_id::FOMC_DECISION),
        ("GDP",                 release_id::GDP),
        ("ISM Manufacturing",   release_id::ISM_MANUFACTURING),
        ("Retail Sales",        release_id::RETAIL_SALES),
    ] {
        let res = client.release_dates(id, &from.to_string(), &to.to_string()).await;
        results.check(
            &format!("release_dates({label}) returns >= 1 row in 60-day window"),
            res.as_ref().map(|v| !v.is_empty()).unwrap_or(false),
        );
    }
    // Existing get_series_latest, get_economic_indicators checks here too.
}
```

The example **must** assert that every release-id constant returns at least one date within a 60-day historical window. If any release ID is wrong (the plan flags them for verification before commit), this smoke test catches it deterministically — guessing a bad ID returns 200 OK with an empty list, which would otherwise fail silently in production.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/data/fred.rs crates/scorpio-core/examples/fred_live_test.rs
git commit -m "feat(data): add FRED release-dates wrapper + live smoke test"
```

---

#### Task 4: Surface `Ticker.calendar()` on `YFinanceClient`

**Files:**
- Modify: `crates/scorpio-core/src/data/yfinance/financials.rs` (or `client.rs`, depending on layout)

The crate already exposes `Ticker.calendar()` returning `Calendar { earnings_dates, ex_dividend_date, dividend_date }`. Surface it through `YFinanceClient` with the same session/rate-limit discipline as other methods.

- [x] **Step 1: Add `fetch_calendar(symbol, target_date)`**

Returns a thin domain wrapper, not the upstream type, so callers don't depend on `yfinance_rs::fundamentals::Calendar`.

- [ ] **Step 2: Run + commit**

```bash
git commit -m "feat(data): expose yfinance per-ticker calendar"
```

---

#### Task 5: Implement `Tier1CatalystProvider`

**Files:**
- Modify: `crates/scorpio-core/src/data/adapters/catalysts.rs`

- [x] **Step 1: Compose the three sources with fail-soft semantics**

Per the **Failure-Mode Discipline** section: each per-source helper returns `Vec<CatalystEvent>` (no `Result`) and warn-logs on internal error. The composing `fetch_catalysts` uses `tokio::join!` (NOT `try_join!`) so one failed source doesn't zero out the others.

```rust
pub struct Tier1CatalystProvider {
    finnhub: crate::data::FinnhubClient,
    fred: crate::data::FredClient,
    yfinance: crate::data::YFinanceClient,
}

impl Tier1CatalystProvider {
    /// Soft-fail wrapper: turns any provider error into a warn-and-empty result
    /// so the composer never propagates a per-source failure.
    async fn try_finnhub_earnings(&self, symbol: &str, from: &str, to: &str) -> Vec<CatalystEvent> {
        match self.finnhub.fetch_earnings_calendar(from, to, Some(symbol)).await {
            Ok(rows) => rows.iter().map(map_finnhub_earnings_to_catalyst).collect(),
            Err(err) => {
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "finnhub_earnings",
                    symbol,
                    error = %err,
                    "catalyst source failed; continuing with empty contribution"
                );
                Vec::new()
            }
        }
    }
    // ... try_finnhub_ipo, try_fred_releases, try_yfinance_calendar mirror this shape.
}

#[async_trait]
impl CatalystCalendarProvider for Tier1CatalystProvider {
    async fn fetch_catalysts(
        &self,
        symbol: &str,
        as_of_date: &str,
        horizon_days: u32,
    ) -> Result<Vec<CatalystEvent>, TradingError> {
        let from = as_of_date;
        let to = /* as_of_date + horizon_days, NaiveDate arithmetic */;

        let (earnings, ipos, macros, dividends) = tokio::join!(
            self.try_finnhub_earnings(symbol, from, &to),
            self.try_finnhub_ipo(from, &to),
            self.try_fred_releases(from, &to),
            self.try_yfinance_calendar(symbol, as_of_date),
        );

        let mut all = Vec::with_capacity(earnings.len() + ipos.len() + macros.len() + dividends.len());
        all.extend(earnings);
        all.extend(ipos);
        all.extend(macros);
        all.extend(dividends);

        // Dedupe by (symbol, event_date, category) — see Out of Scope note.
        all.sort_by(|a, b| (&a.event_date, &a.symbol, &a.category).cmp(&(&b.event_date, &b.symbol, &b.category)));
        all.dedup_by(|a, b| a.event_date == b.event_date && a.symbol == b.symbol && a.category == b.category);

        Ok(all)
    }
}
```

Note `fetch_catalysts` returns `Result` to satisfy the trait, but the only `Err` it can return is from arithmetic on `as_of_date` (`chrono::ParseError` → `TradingError::SchemaViolation`). Network/upstream errors are swallowed by the `try_*` wrappers.

Map each source-specific row into `CatalystEvent`:
- Finnhub earnings → `category: EarningsAndFinancial`, `impact: H`.
- Finnhub IPO → `category: CorporateEvents`, `impact: M` for non-watchlist tickers, `H` for the ticker under analysis.
- FRED release dates → `symbol: "_MACRO"`, `category: MacroEvents`. Impact map per release (CPI/NFP/FOMC = H; GDP/ISM/Retail = M).
- yfinance ex-dividend → `category: EarningsAndFinancial`, `impact: L`.

- [x] **Step 2: Tests**

Three test variants per source: happy path, empty response, upstream error. Use mocked clients — do not hit live APIs in unit tests. **One additional test must verify the composition invariant:** when one source's mock returns `Err`, `fetch_catalysts` still returns `Ok` with the surviving sources' events plus a `tracing::warn!` captured via a `tracing-test` subscriber.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(data): implement Tier1 catalyst calendar provider (Finnhub + FRED + yfinance)"
```

---

#### Task 6: Wire `enrichment_catalysts` into `TradingState` and prefetch

**Files:**
- Modify: `crates/scorpio-core/src/state/trading_state.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/preflight.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs`
- Modify: `crates/scorpio-core/src/agents/shared/prompt.rs`

- [x] **Step 1: Add the field**

```rust
// In crates/scorpio-core/src/state/trading_state.rs alongside enrichment_event_news
#[serde(default)]
pub enrichment_catalysts: EnrichmentState<Vec<CatalystEvent>>,
```

`#[serde(default)]` is mandatory per the CLAUDE.md TradingState schema-evolution rule. No `THESIS_MEMORY_SCHEMA_VERSION` bump required because this is purely additive.

- [x] **Step 2: Prefetch alongside existing news prefetch**

In `workflow/pipeline/runtime.rs`, the existing prefetch already does `tokio::join!(price, vix, news)`. Extend to `tokio::join!(price, vix, news, catalysts)`. Catalyst horizon = 30 days from `state.target_date` (matches `NEWS_ANALYSIS_DAYS` window for symmetry).

- [x] **Step 3: Add `build_catalyst_calendar_block` renderer**

```rust
// In crates/scorpio-core/src/agents/shared/prompt.rs
pub(crate) fn build_catalyst_calendar_block(state: &TradingState) -> String {
    let Some(events) = state.enrichment_catalysts.payload.as_ref() else {
        return "(no upcoming catalysts: data unavailable)".to_owned();
    };
    if events.is_empty() {
        return "(no upcoming catalysts in the next 30 days)".to_owned();
    }
    // Sort by event_date, format as a bulleted list, sanitize each headline.
    // Cap at 25 lines so prompt context doesn't blow up on a noisy ticker.
    // ...
}
```

Sort by `event_date` ascending; tag each line with `[H]` / `[M]` / `[L]` so the prompt's H/M/L tier rule has structured input.

- [x] **Step 4: Tests**

Snapshot tests for the prompt block: empty, single event, mixed `_MACRO` + per-ticker, oversize cap.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(workflow): prefetch catalysts and surface in analyst-context body"
```

---

#### Task 7: Update Theme G prompt + downgrade analytical-themes-port dependency caveat

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/news_analyst.md`
- Modify: `docs/superpowers/plans/2026-05-10-analytical-themes-port.md`

- [x] **Step 1: Replace the TODO marker with a real `## Upcoming Catalysts` section**

In `news_analyst.md`, find the `<!-- TODO(catalyst-calendar): scorpio currently does not have a catalyst calendar data source. -->` block (referenced in `analytical-themes-port.md:436`). Replace with:

```markdown
## Upcoming Catalysts

The following confirmed forward-looking catalysts apply to {ticker} or the
broader macro calendar in the analysis window. Each line is tagged with an
impact tier and the source category. Reason H/M/L impact decisions against
this list rather than inventing forward dates from training-data recall.

{catalyst_calendar}

If the block above says `(no upcoming catalysts: data unavailable)`, fall back
to news-discovered events only and say so explicitly in your summary. If it
says `(no upcoming catalysts in the next 30 days)`, that is a domain-valid
signal — analysed name is in a quiet window.
```

Keep the rest of the Theme G H/M/L impact-tier guidance intact (it's still the prompt's classification rule).

- [x] **Step 2: Update the dependent plan**

In `docs/superpowers/plans/2026-05-10-analytical-themes-port.md`:
- Above the **Goal** line, add a dependency note referencing this plan.
- In the Decision Summary table row for Theme G, replace `Partial — full power needs catalyst calendar (NOT WIRED at all)` with `Tier 1 of 2026-05-10-catalyst-calendar-integration provides earnings/IPO/macro/dividend; FDA + conferences + lockup/M&A close are deferred to Tier 3 of that plan`.
- In the Theme G section starting at line 59, replace the `Likely candidates for the look-ahead data:` paragraph with `See 2026-05-10-catalyst-calendar-integration.md for the wiring plan.`
- In the Theme G prompt block at line 436, delete the `<!-- TODO(catalyst-calendar) -->` HTML comment because Step 1 of this task replaces the prompt.

- [ ] **Step 3: Smoke test**

Run a real analyst cycle with `RUST_LOG=info` on a ticker close to earnings season. Verify the news analyst summary cites at least one catalyst from the rendered block (Earnings, FOMC, etc.) rather than saying `degraded mode: news-discovered events only`.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(packs): wire catalyst calendar into news analyst prompt"
```

---

### Tier 2 — SEC EDGAR 8-K monitor

#### Task 8: New SEC EDGAR client

**Files:**
- Create: `crates/scorpio-core/src/data/sec_edgar.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs`

SEC EDGAR is free, no API key, but **mandates** a `User-Agent: <Name> <email>` header per their fair-use policy. Rate limit is 10 req/sec. Reuse the existing `SharedRateLimiter` for pacing.

The User-Agent is hardcoded to `"Scorpio Analyst scorpio@ledgerlylab.com"` — no config field or wizard step required (Task 10 is dropped).

- [ ] **Step 1: HTTP client with hardcoded UA**

```rust
// crates/scorpio-core/src/data/sec_edgar.rs

/// SEC EDGAR fair-use User-Agent. Hardcoded per policy — no config required.
const SEC_EDGAR_USER_AGENT: &str = "Scorpio Analyst scorpio@ledgerlylab.com";

pub struct SecEdgarClient {
    http: Arc<reqwest::Client>,
    limiter: SharedRateLimiter,
    // Per-instance circuit-breaker: after N consecutive failures, all subsequent
    // calls return Ok(empty) without hitting the network until the breaker
    // half-opens again. Prevents a flapping SEC EDGAR from burning the per-cycle
    // rate-limit budget that other tasks share.
    breaker: Arc<tokio::sync::Mutex<CircuitBreakerState>>,
}

impl SecEdgarClient {
    /// Construct a SEC EDGAR client using the hardcoded Scorpio User-Agent.
    pub fn new(limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        // Build reqwest::Client with default-headers carrying SEC_EDGAR_USER_AGENT,
        // plus a 15s request timeout (SEC EDGAR tail-latency is bounded).
        // ...
    }
}
```

**Construction-time fallback contract:** if `SecEdgarClient::new(...)` returns `Err` (e.g. reqwest client build failure), preflight logs `info!("falling back to Tier 1 catalyst provider: <reason>")` and instantiates `Tier1CatalystProvider` instead. The pipeline never aborts.

- [ ] **Step 2: CIK lookup**

SEC publishes the ticker→CIK map at `https://www.sec.gov/files/company_tickers.json`. Cache it in `Arc<RwLock<HashMap<String, u32>>>` with a single load on first use (it's small — about 12k tickers, <2MB).

- [ ] **Step 3: 8-K + 13D/G filings index**

```rust
pub async fn fetch_recent_filings(
    &self,
    cik: u32,
    form_types: &[&str], // e.g. &["8-K", "SC 13D", "SC 13G"]
    from: &str,
    to: &str,
) -> Result<Vec<FilingHeader>, TradingError>;

pub struct FilingHeader {
    pub cik: u32,
    pub accession_number: String,
    pub form_type: String,
    pub filing_date: String, // YYYY-MM-DD
    pub primary_doc_url: String,
    /// Comma-separated 8-K item codes (e.g. "1.01,2.01"). Empty for non-8-K filings.
    pub item_codes: String,
}
```

Use EDGAR's structured submissions JSON: `https://data.sec.gov/submissions/CIK<10-digit-padded-cik>.json` returns recent filings with item codes baked in. **Don't scrape the HTML index** — the JSON endpoint is canonical and stable.

- [ ] **Step 4: Tests with explicit failure-mode coverage**

Use saved fixture JSON snapshots for the canned-response tests in `data/sec_edgar.rs::tests`. Required failure-mode tests (each must assert `Ok(Vec::new())` plus a captured `tracing::warn!` event with `kind = "catalyst_fetch_failed"`):

1. HTTP 403 (UA rejected at runtime even though it parsed)
2. HTTP 404 (CIK not in submissions index — common for delisted tickers)
3. HTTP 429 (rate-limit exhausted; verifies retry+backoff exits cleanly after the budget)
4. HTTP 500 (transient server error)
5. Connection timeout
6. Malformed JSON body (response 200 OK but body is not the documented submissions shape)
7. Unknown ticker → CIK lookup miss
8. Filing with no `items` field on an 8-K (rare but observed) → row mapped with empty `item_codes`

The canned-response harness can use `wiremock` (existing dev-dep across the repo per `Cargo.lock`); do not introduce a new mocking crate.

- [ ] **Step 5: Add `crates/scorpio-core/examples/sec_edgar_live_test.rs`**

Mirror `finnhub_live_test.rs` structure (`Results` aggregator, env-driven config, exit non-zero on FAIL). Cover both happy-path and the failure-mode invariants:

```rust
//! Live SEC EDGAR API smoke test.
//!
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example sec_edgar_live_test
//! ```
//!
//! Uses the hardcoded Scorpio User-Agent. SEC EDGAR is unauthenticated — no API key.

use scorpio_core::{data::SecEdgarClient, rate_limit::SharedRateLimiter};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Reuse the Results aggregator from finnhub_live_test.rs.
    let mut results = Results::new();

    let client = SecEdgarClient::new(SharedRateLimiter::new("sec-edgar-test", 10))
        .expect("hardcoded UA should always construct");

    // ── Happy-path: AAPL has hundreds of filings, expect non-empty ──────────
    let aapl_cik: u32 = client.lookup_cik("AAPL").await.expect("AAPL CIK lookup");
    let recent = client
        .fetch_recent_filings(aapl_cik, &["8-K", "SC 13D", "SC 13G"], "2025-01-01", "2026-12-31")
        .await
        .expect("fetch returns Ok");
    results.check(
        "AAPL recent filings non-empty in 24-month window",
        !recent.is_empty(),
    );

    // ── Fail-soft contract: bogus CIK must return Ok(empty), NOT Err ────────
    let bogus = client
        .fetch_recent_filings(99_999_999, &["8-K"], "2025-01-01", "2026-12-31")
        .await;
    results.check(
        "bogus CIK returns Ok(empty), not Err",
        matches!(&bogus, Ok(v) if v.is_empty()),
    );

    // ── Fail-soft contract: unknown ticker for CIK lookup ────────────────────
    let lookup_miss = client.lookup_cik("ZZZNOTAREALTICKERZZZ").await;
    results.check(
        "unknown ticker CIK lookup returns Ok(None) or domain error mapped to fail-soft",
        match lookup_miss {
            Ok(None) => true,
            // If your design returns Err(NotFound), the Tier2 provider must catch it
            // and continue with empty contribution — verify by integration test, not here.
            _ => false,
        },
    );

    results.report_and_exit()
}
```

This example **must** be runnable in CI of the developer's choosing, with the same `--ignored`-style guard the finnhub example uses (skip on missing env var rather than fail). It is the canonical "did SEC EDGAR change their JSON shape" detector for this codebase.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/data/sec_edgar.rs crates/scorpio-core/examples/sec_edgar_live_test.rs
git commit -m "feat(data): add SEC EDGAR client + fail-soft live smoke test"
```

---

#### Task 9: SEC EDGAR catalyst provider

**Files:**
- Modify: `crates/scorpio-core/src/data/adapters/catalysts.rs`

- [ ] **Step 1: Add `SecEdgar8kProvider`**

For the analysed ticker, fetch recent filings from `[as_of_date - lookback_days, as_of_date + horizon_days]`. The lookback exists because some material events file 8-Ks within a window after the event itself (e.g. shareholder votes are reported retrospectively).

Map filings → `CatalystEvent`:

| Form / Item | Category             | Impact | Headline                                 |
|-------------|----------------------|--------|------------------------------------------|
| 8-K 1.01    | CorporateEvents      | H      | "Material agreement: <accession>"        |
| 8-K 2.01    | CorporateEvents      | H      | "Acquisition / disposition: <accession>" |
| 8-K 2.02    | EarningsAndFinancial | H      | "Earnings results filed (8-K 2.02)"      |
| 8-K 5.07    | CorporateEvents      | M      | "Shareholder vote results"               |
| 8-K 7.01    | CorporateEvents      | M      | "Reg FD disclosure"                      |
| 8-K 8.01    | CorporateEvents      | M      | "Other material event"                   |
| SC 13D      | CorporateEvents      | H      | "Activist 13D filed"                     |
| SC 13G      | CorporateEvents      | M      | "Passive 13G filed"                      |

Headlines stay generic — do NOT parse the filing body. The news analyst's existing news-fetch path can pull the body if it wants to discuss specifics.

- [ ] **Step 2: Compose `Tier2CatalystProvider` with fail-soft semantics**

```rust
pub struct Tier2CatalystProvider {
    tier1: Tier1CatalystProvider,
    sec_edgar: SecEdgar8kProvider,
}

#[async_trait]
impl CatalystCalendarProvider for Tier2CatalystProvider {
    async fn fetch_catalysts(
        &self,
        symbol: &str,
        as_of_date: &str,
        horizon_days: u32,
    ) -> Result<Vec<CatalystEvent>, TradingError> {
        // tokio::join!, NOT try_join!: SEC EDGAR contributing zero events
        // must not zero out the Tier 1 contributions.
        let (mut tier1_events, edgar_events) = tokio::join!(
            self.tier1.fetch_catalysts(symbol, as_of_date, horizon_days),
            self.sec_edgar.fetch_catalysts(symbol, as_of_date, horizon_days),
        );

        // Tier 1 returns Result for arithmetic-error symmetry; if it fails on
        // date arithmetic, that's a real bug — propagate. SEC EDGAR follows
        // the same shape but its provider-level errors are already swallowed
        // internally per the Failure-Mode Discipline contract.
        let mut all = tier1_events?;
        match edgar_events {
            Ok(events) => all.extend(events),
            Err(err) => {
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar",
                    symbol,
                    error = %err,
                    "Tier 2 source failed; Tier 1 events still flow through"
                );
            }
        }

        // Dedupe + sort identical to Tier 1.
        // ...
        Ok(all)
    }
}
```

`SecEdgar8kProvider::fetch_catalysts` itself follows the Tier 1 wrapper pattern: each EDGAR call (CIK lookup, filings index, optional facts) is wrapped in a `try_*` helper that warns and returns empty on failure. The provider-level `fetch_catalysts` aggregates and only returns `Err` for unrecoverable arithmetic / programming errors — never for upstream HTTP/JSON failures.

**Circuit breaker:** the per-instance breaker on `SecEdgarClient` (Task 8 Step 1) is the second layer. After **5 consecutive** runtime failures within a single pipeline run, subsequent calls in the same run short-circuit to `Ok(empty)` without hitting the network. The breaker resets on each new pipeline run (a new `SecEdgarClient` instance, since clients are constructed during preflight — confirm against the actual lifetime of the catalyst-prefetch task before wiring).

- [ ] **Step 3: Wire `Tier2CatalystProvider` into the runtime**

`SecEdgarClient::new(limiter)` uses the hardcoded UA. If construction fails (e.g. reqwest build error), fall back to `Tier1CatalystProvider`. Log once at startup which provider was chosen:

```rust
let catalyst_provider: Arc<dyn CatalystCalendarProvider> =
    match SecEdgarClient::new(SharedRateLimiter::new("sec-edgar", 10)) {
        Ok(edgar_client) => {
            tracing::info!("catalyst provider: Tier 2 (Finnhub + FRED + yfinance + SEC EDGAR)");
            Arc::new(Tier2CatalystProvider { tier1, sec_edgar: SecEdgar8kProvider::new(edgar_client) })
        }
        Err(reason) => {
            tracing::info!(reason = %reason, "falling back to Tier 1 catalyst provider");
            Arc::new(tier1)
        }
    };
```

Pipeline construction never aborts on a SEC EDGAR build failure.

- [ ] **Step 4: Smoke + commit**

```bash
git commit -m "feat(data): add SEC EDGAR Item-coded 8-K catalyst provider"
```

---

#### ~~Task 10: Setup wizard support for SEC EDGAR UA~~ — **DROPPED**

User-Agent is hardcoded to `"Scorpio Analyst scorpio@ledgerlylab.com"`. No config field, no wizard step required.

---

### Documentation

#### Task 11: docs/solutions entry

**Files:**
- Create: `docs/solutions/data-sources/2026-05-10-catalyst-calendar.md`

- [ ] **Step 1: Document the wiring**

Per `/ce:compound` discipline (CLAUDE.md "Knowledge Consolidation"), record:
- Problem: news analyst could only classify discovered events; no forward-looking calendar.
- Fix: composed Tier 1 (Finnhub + FRED + yfinance) and Tier 2 (SEC EDGAR 8-K Item codes) into a `CatalystCalendarProvider`.
- Tags: `data-sources`, `prompts`, `catalyst-calendar`, `theme-g`, `attribution`.
- Open: Tier 3 (FDA AdComm scraping, S-1 lockup parsing, DEF M14A close-date extraction) deferred to a separate plan.

- [ ] **Step 2: Commit**

```bash
git commit -m "docs(solutions): record catalyst calendar Tier 1+2 wiring"
```

---

## Out of Scope (explicitly)

- **Tier 3 — filing-body parsing.** S-1 lockup language, DEF M14A expected-close, FDA AdComm calendar scraping. These need real text extraction infrastructure (LLM-assisted field extraction with regression tests) that is not justified until Tier 1+2 prove the prompt-side value.
- **Paid-tier Finnhub endpoints** (`/calendar/economic`, `misc().fda_calendar()`, `/calendar/dividends`). The free-tier wrapper code must explicitly NOT call these endpoints, since they return 403 and the error path would be misleading. If a paid key is ever wired, gate the calls behind a `tier: Premium` config flag.
- **Conferences and investor-day calendars.** No free structured feed exists. Companies announce these ad-hoc via 8-K item 7.01 — Tier 2 will catch the announcement when it files, but cannot project a forward calendar of upcoming conferences.
- **Streaming / push-based catalyst delivery.** This plan is poll-on-each-pipeline-run only. No incremental update or webhook subscription.
- **Cross-listing handling.** Non-US ADRs whose primary listing is foreign get partial coverage (Finnhub earnings often missing, SEC EDGAR has 20-F/6-K filings instead of 10-K/8-K). Acceptable as a known gap; flagged in source-doc.
- **Catalyst de-duplication across sources.** Tier 1 and Tier 2 may both surface the same earnings event (Finnhub `calendar().earnings` projection + the actual 8-K item 2.02 filing). The renderer dedupes by `(symbol, event_date, category)` — anything more sophisticated is over-engineering for the cost.

---

## Self-Review Checklist

- [x] Every task has exact file paths.
- [x] Every step has either verbatim code, a specific instruction, or a documented external endpoint URL.
- [x] No placeholders. The `release_id` constants are flagged for verification before commit because guessing FRED release IDs would silently produce empty catalogs.
- [x] CLAUDE.md compliance: new `enrichment_catalysts` field carries `#[serde(default)]`; no `THESIS_MEMORY_SCHEMA_VERSION` bump (purely additive). Snapshot deserialization stays compatible with old snapshots.
- [x] Concurrency: each tier's provider fans out independent calls via `tokio::join!` (NOT `try_join!`) so one source failing does not zero out the others. Catalyst prefetch runs once per pipeline run alongside existing news prefetch — not per analyst clone.
- [x] Free-tier discipline: explicit "don't call this" list for paid Finnhub endpoints. SEC EDGAR UA is required to **construct** the Tier 2 provider, but a missing/invalid UA falls back to Tier 1 — pipeline construction never aborts.
- [x] **Failure-mode discipline: explicit invariant section at the top of the plan; every per-source helper warns and returns empty on internal error; runtime `fetch_catalysts` never returns Err for upstream HTTP/JSON failures; SEC EDGAR has both per-call swallow + circuit breaker after 5 consecutive failures per run.**
- [x] **Smoke-test examples cover both happy-path and fault-injection invariants for the two highest-risk sources (FRED at `crates/scorpio-core/examples/fred_live_test.rs`, SEC EDGAR at `crates/scorpio-core/examples/sec_edgar_live_test.rs`). Both follow the existing `finnhub_live_test.rs` PASS/FAIL aggregator convention.**
- [x] Dependent plan updated: `2026-05-10-analytical-themes-port.md` is patched in Task 7 to reference this plan and remove the obsolete `degraded mode: news-discovered events only` caveat once Tier 1 lands.

---

## Attribution

Catalyst categorisation taxonomy (Earnings & Financial / Corporate / Industry / Macro and the H/M/L impact tiers) is adapted from `anthropics/financial-services` (Apache 2.0): `equity-research/skills/catalyst-calendar/SKILL.md`. Already attributed via Theme G of `2026-05-10-analytical-themes-port.md`; this plan does not add new prompt content, only wires data behind the existing taxonomy.
