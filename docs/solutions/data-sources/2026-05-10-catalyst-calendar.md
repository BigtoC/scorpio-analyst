---
title: Catalyst calendar Tier 1+2 wiring — free-tier forward-looking event data
date: 2026-05-10
category: data-sources
module: data/adapters/catalysts, data/sec_edgar, workflow/pipeline/runtime
problem_type: feature_implementation
component: data_pipeline
severity: medium
applies_when:
  - Adding a forward-looking catalyst data source to the pipeline
  - Composing multiple fail-soft data providers behind a trait seam
  - Mapping SEC EDGAR 8-K Item codes to structured catalyst events
tags:
  - data-sources
  - prompts
  - catalyst-calendar
  - theme-g
  - attribution
  - sec-edgar
  - fail-soft
---

# Catalyst calendar Tier 1+2 wiring

## Problem

The news analyst's Theme G prompt (catalyst taxonomy with H/M/L impact) shipped in
degraded mode: it could only classify catalyst events it *discovered* in news headlines,
never projecting forward. The user-facing output said `degraded mode: news-discovered
events only` because no structured forward-looking calendar was wired.

## Root Cause

No calendar data source was integrated. The relevant structured feeds are:
- Finnhub free-tier: `/calendar/earnings`, `/calendar/ipo` — both available
- FRED: `/fred/release/dates` — scheduled macro release dates for CPI, NFP, FOMC, GDP, ISM, Retail Sales
- yfinance: `Ticker.calendar()` — per-ticker ex-dividend and earnings dates
- SEC EDGAR: submissions JSON at `data.sec.gov/submissions/CIK<10digit>.json` — 8-K Item codes and 13D/G filings

## Fix Applied

### Tier 1 (`CatalystCalendarProvider` trait + `Tier1CatalystProvider`)

Created `crates/scorpio-core/src/data/adapters/catalysts.rs`:
- `CatalystEvent` payload (symbol, event_date, category, impact, headline, source_url, source)
- `CatalystCalendarProvider` trait — the seam all providers implement
- `Tier1CatalystProvider` — fans out to Finnhub earnings/IPO, FRED release dates, and yfinance calendar via `tokio::join!` (not `try_join!`)
- Each source is wrapped in a `try_*` helper that warn-logs and returns `vec![]` on failure — one source failing never zeros out the others

FRED release IDs (verified at `fred.stlouisfed.org/releases/`):

| Release          | ID  | Impact |
|------------------|-----|--------|
| CPI              | 10  | H      |
| Nonfarm Payrolls | 50  | H      |
| FOMC Decision    | 101 | H      |
| GDP              | 53  | M      |
| ISM Mfg          | 21  | M      |
| Retail Sales     | 14  | M      |

### Tier 2 (`SecEdgarClient` + `SecEdgar8kProvider` + `Tier2CatalystProvider`)

Created `crates/scorpio-core/src/data/sec_edgar.rs`:
- Hardcoded `User-Agent: Scorpio Analyst scorpio@ledgerlylab.com` (SEC EDGAR fair-use policy)
- Lazy-loaded ticker→CIK map from `https://www.sec.gov/files/company_tickers.json` (cached per client instance)
- Submissions JSON from `https://data.sec.gov/submissions/CIK<10-digit-padded>.json`
- Internal `EdgarHttp` trait (with `mockall::automock`) enables unit-test coverage without `wiremock`
- Per-instance circuit breaker (5 consecutive failures → 60s cooldown) prevents rate-limit storms

8-K Item → `CatalystEvent` mapping:

| Form / Item | Category             | Impact |
|-------------|----------------------|--------|
| 8-K 1.01    | CorporateEvents      | H      |
| 8-K 2.01    | CorporateEvents      | H      |
| 8-K 2.02    | EarningsAndFinancial | H      |
| 8-K 5.07    | CorporateEvents      | M      |
| 8-K 7.01    | CorporateEvents      | M      |
| 8-K 8.01    | CorporateEvents      | M      |
| SC 13D      | CorporateEvents      | H      |
| SC 13G      | CorporateEvents      | M      |

### Runtime wiring (`workflow/pipeline/runtime.rs`)

`hydrate_catalysts` now accepts `&dyn CatalystCalendarProvider`. During prefetch:
1. Try `SecEdgarClient::new(...)` — uses hardcoded UA, virtually always succeeds
2. If `Ok` → wrap as `Tier2CatalystProvider { tier1, sec_edgar }`; log `catalyst provider: Tier 2`
3. If `Err` → fall back to `Arc<Tier1CatalystProvider>`; log `falling back to Tier 1`
4. Pass `catalyst_provider.as_ref()` to `hydrate_catalysts` alongside the existing `tokio::join!` block

`EnrichmentState<Vec<CatalystEvent>>` semantics:
- `payload: None` → prefetch was skipped (never happens in current wiring)
- `payload: Some(vec![])` → ran, nothing to report (all sources quiet or all failed)
- `payload: Some(events)` → at least one source returned events

## Key Non-obvious Decisions

1. **`mockall::automock` on internal `EdgarHttp` trait** — `wiremock` is not in the project. The internal trait approach lets all 8 failure-mode scenarios (403, 404, 429, 500, transport error, malformed JSON, bogus CIK, missing `items` field) be tested as unit tests without spinning up a real HTTP server or adding a new dependency.

2. **`tokio::join!` not `try_join!`** throughout — this is the invariant that makes the composition fail-soft. Any `try_join!` would short-circuit all sources when one fails.

3. **`items` field in EDGAR submissions JSON** — the field is a string per filing (e.g., `"2.02"` or `"1.01, 2.01"`), not an array. Deserialize with `#[serde(default)]` because some older records omit it entirely. Normalize `", "` → `","` before splitting.

4. **SEC EDGAR lookback of 14 days** — 8-K filings may arrive several days after the underlying event (shareholder votes, Reg FD disclosures). The lookback window ensures recently filed events appear in the catalyst block even when the analyst runs the day after the event.

5. **`_MACRO` sentinel symbol** — FOMC, CPI, GDP, etc. apply to all tickers. The prompt renderer interleaves macro events with per-ticker events sorted by `event_date`. Dedup key is `(symbol, event_date, category)` — macro events from FRED and SEC EDGAR don't collide because their symbols differ.

## Open: Tier 3 (Deferred)

The following remain out of scope pending a separate plan:
- FDA PDUFA / Advisory Committee calendar — paid feed or filing-body text extraction
- S-1 lockup expiry dates — requires parsing lockup language from the filing body
- DEF M14A expected M&A close dates — requires parsing proxy statement body
- Conferences and investor-day calendars — no free structured feed; SEC EDGAR item 7.01 announces them but does not project the forward date

Track these in a future Tier 3 plan.

## Related

- [Finnhub `EarningsRelease.quarter` is the reported quarter, not the announcement quarter](../logic-errors/finnhub-earnings-release-quarter-semantics-2026-05-16.md) — same Finnhub endpoint (`/calendar/earnings`), different code path: that doc covers backward-looking transcript-quarter resolution; this doc covers forward-looking catalyst projection. Don't conflate the two when reading runtime logs.
