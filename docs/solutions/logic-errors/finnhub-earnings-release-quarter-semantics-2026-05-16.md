---
title: Finnhub `EarningsRelease.quarter` is the reported quarter, not the announcement quarter
date: 2026-05-16
category: logic-errors
module: data/finnhub, workflow/pipeline/runtime
problem_type: logic_error
component: service_object
symptoms:
  - "`select_transcript_quarter` returned `2025Q4` for GLW's 2026-04-28 release whose true reported period was Q1 2026"
  - "Alpha Vantage transcript fetch keyed on the wrong `YYYYQN`, silently downgrading to `TranscriptFetch::NotPublished`/`Unavailable`"
  - "Sentiment and News analysts ran with no transcript evidence even when one existed"
root_cause: wrong_api
resolution_type: code_fix
severity: medium
related_components:
  - data/adapters/transcripts
  - data/alpha_vantage
tags:
  - finnhub
  - earnings-calendar
  - transcripts
  - fiscal-quarter
  - alpha-vantage
  - data-source
---

# Finnhub `EarningsRelease.quarter` is the reported quarter, not the announcement quarter

## Problem

`select_transcript_quarter` derived the wrong `YYYYQN` key from Finnhub's earnings-calendar response, causing the Alpha Vantage transcript lookup to miss real, published transcripts. The function assumed Finnhub returned the *announcement* quarter (the calendar quarter the release lands in) and subtracted one to get the *reported* quarter. In reality, Finnhub already returns the reported quarter, so the subtraction shifted every result one quarter into the past.

## Symptoms

- For Corning (`GLW`) at `as_of_date = 2026-05-15`, Finnhub returned an `EarningsRelease` with `date = "2026-04-28"`, `year = 2026`, `quarter = 1`. The function output was `Some("2025Q4")` — the correct answer is `Some("2026Q1")`.
- Alpha Vantage `EARNINGS_CALL_TRANSCRIPT` keyed on the wrong quarter returned no transcript, so `hydrate_transcript` resolved to `TranscriptFetch::NotPublished` even when a transcript existed.
- Downstream, the `Evidence Provenance: Providers` section never listed `alpha_vantage` because the transcript path silently failed at the key-derivation step rather than the network step.
- A related red-herring log line — `Fetched earnings calendar, from: "2026-05-15", to: "2026-06-14", earnings_calendar: []` — was *not* the same code path; that's `Tier1CatalystProvider::fetch_catalysts` looking forward for upcoming events. Don't conflate it with transcript-quarter resolution.

## What Didn't Work

- **Trusting the doc comment over the data.** The existing code carried a long comment explicitly claiming Finnhub returned the announcement quarter, with an example: *"a release on 2026-05-05 comes back from Finnhub as `quarter = 2`, but the published transcript is `2026Q1`."* No upstream contract supported that claim. The fix only became obvious when we compared a real Finnhub payload (`date = "2026-04-28", quarter = 1`) against the function output (`2025Q4`).
- **Adjusting the test fixtures instead of the function.** Existing tests baked in the wrong assumption (`select_transcript_quarter_maps_announcement_quarter_to_reported_quarter`, `select_transcript_quarter_wraps_q1_to_prior_year_q4`) and passed against the broken logic. Updating only the function while leaving these tests in place would have made the fix look like a regression.

## Solution

Three changes in `crates/scorpio-core/src/workflow/pipeline/runtime.rs`:

1. **Drop the `q - 1` shift.** `Finnhub.quarter` *is* the reported quarter — format it directly.
2. **Filter releases newer than `as_of`.** The earnings calendar can include scheduled-but-not-yet-released events with a future `date`. Those don't have a published transcript yet, so they must not become the "latest" pick. Thread an `as_of: NaiveDate` through `resolve_transcript_quarter_from_fetch` and into `select_transcript_quarter`.
3. **Derive the 120-day lookback from `as_of_date`, not `Utc::now()`.** Keeps the function deterministic for backtests and consistent with the same `as_of_date` used as the `to` bound of the API call.

### Before
```rust
fn select_transcript_quarter(
    releases: &[finnhub::models::calendar::EarningsRelease],
) -> Option<String> {
    releases
        .iter()
        .filter_map(|r| match (r.year, r.quarter, r.date.as_deref()) {
            (Some(y), Some(q), Some(d)) if (1..=4).contains(&q) => Some((d.to_owned(), y, q)),
            _ => None,
        })
        .max_by(|(da, ..), (db, ..)| da.cmp(db))
        .map(|(_d, y, q)| {
            // Finnhub's year/quarter describe the *announcement* period ...
            let (year, quarter) = if q == 1 { (y - 1, 4) } else { (y, q - 1) };
            format!("{year}Q{quarter}")
        })
}
```

### After
```rust
fn select_transcript_quarter(
    releases: &[finnhub::models::calendar::EarningsRelease],
    as_of: NaiveDate,
) -> Option<String> {
    releases
        .iter()
        .filter_map(|r| match (r.year, r.quarter, r.date.as_deref()) {
            (Some(y), Some(q), Some(d)) if (1..=4).contains(&q) => {
                let release_date = NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
                if release_date > as_of {
                    return None;
                }
                Some((release_date, y, q))
            }
            _ => None,
        })
        .max_by_key(|(date, ..)| *date)
        .map(|(_d, y, q)| format!("{y}Q{q}"))
}
```

The caller (`resolve_transcript_quarter`) parses `as_of_date` once and passes the `NaiveDate` through.

## Why This Works

Finnhub's earnings-calendar response uses `year`/`quarter` to describe the **fiscal period being reported**, not the calendar quarter the announcement lands in. The 2026-04-28 GLW release reports Q1 2026 earnings — Finnhub correctly sets `quarter = 1`, no shift needed. Alpha Vantage's `EARNINGS_CALL_TRANSCRIPT` endpoint is also keyed by the reported period (`2026Q1`), so the two systems already agree once we stop massaging Finnhub's value.

The future-date filter handles a separate failure mode: Finnhub returns scheduled-but-unreleased earnings within the queried window. Without the filter, those bubble to the top of `max_by_key`, and you end up asking Alpha Vantage for a transcript that doesn't exist yet — even when a perfectly good prior-quarter transcript is right there in the same response.

## Prevention

- **Pin field semantics from a real upstream payload.** When integrating with a third-party API, write down what each field means in the doc comment and back it with a real captured payload, not a guess. The bad code carried an explicit, plausible-sounding example that turned out to be wrong.
- **Use deterministic clocks at function boundaries.** `resolve_transcript_quarter` originally mixed `as_of_date` (caller-supplied) with `Utc::now()` (wall-clock) — that combination made the 120-day window depend on real time even though every other timing parameter was deterministic. Threading `NaiveDate` through removes the implicit dependency.
- **Test against real-shape fixtures.** The replacement test asserts the GLW payload byte-for-byte:
  ```rust
  finnhub::models::calendar::EarningsRelease {
      symbol: Some("GLW".to_owned()),
      date: Some("2026-04-28".to_owned()),
      hour: Some("bmo".to_owned()),
      year: Some(2026),
      quarter: Some(1),
      // ... ⇒ Some("2026Q1")
  }
  ```
  Plus a future-release filter test that seeds two releases (a past Q1 and a future Q2) and asserts the function returns the past one.
- **When a fix invalidates a test name, change the test, not just the data.** The old `select_transcript_quarter_maps_announcement_quarter_to_reported_quarter` and `..._wraps_q1_to_prior_year_q4` enshrined the wrong model. The fix renamed the survivor (`..._returns_reported_quarter_of_latest_release`) and deleted the wraparound test entirely, since wraparound no longer exists.

## Related Issues

- [Catalyst calendar Tier 1+2 wiring](../data-sources/2026-05-10-catalyst-calendar.md) — adjacent: also touches `FinnhubClient::fetch_earnings_calendar`, but for forward-looking catalyst events. The two code paths share the same endpoint with different windows (forward for catalysts, backward for transcript-quarter resolution) and should not be conflated when reading log output.
