# Design: Switch macro indicator fetches to FRED v1 series observations

**Date:** 2026-04-02
**Status:** Approved

## Goal

Stop the recurring FRED timeouts in the macro indicator tool by replacing release-wide fetches with direct series-level lookups against FRED v1 `fred/series/observations`.

## Why this design is needed

The current implementation in `src/data/fred.rs` uses FRED v2 `fred/v2/release/observations` and scans a full release until it finds the target series. That is the wrong data shape for this tool.

- `get_economic_indicators()` only needs the latest value for `FEDFUNDS`.
- `get_economic_indicators()` only needs the latest value for `CPALTT01USM657N`.
- The current code fetches release pages and may traverse a large payload before finding either series.
- The observed logs show both lookups repeatedly timing out at 30 seconds and degrading to partial output.

FRED's v1 documentation for `fred/series/observations` is a better match because it supports direct lookup by `series_id` and allows requesting only the latest observation with `sort_order=desc` and `limit=1`.

## Chosen approach

Replace the release-based lookup path with a series-based lookup path that calls:

- `GET https://api.stlouisfed.org/fred/series/observations`
- `series_id=<target series>`
- `file_type=json`
- `sort_order=desc`
- `limit=1`

The client will keep the existing timeout budget, rate limiting, and partial-degradation behavior for transient failures while making retry and degradation rules explicit for the v1 path. The existing reqwest transport configuration, including `.http1_only()`, remains unchanged.

## Scope

This design includes:

- changing `FredClient` in `src/data/fred.rs` from release-based fetches to series-based fetches
- updating response deserialization to the FRED v1 `observations` payload shape
- keeping the existing `GetEconomicIndicators` tool surface unchanged
- adding a small pure helper boundary for request-shape and error-classification tests
- adding regression tests for request shape and latest-value extraction

This design does not include:

- any change to the agent/tool interface name or arguments
- broader FRED client expansion beyond the two existing macro indicators
- changes to analyst prompts, workflow wiring, or retry policy semantics
- caching, batching, or additional macro series in this change

## Assumptions

- This tool continues to fetch the latest live FRED value, not the value as of the analyst `target_date`.
- That matches the current macro snapshot behavior and keeps this fix narrowly focused on the timeout problem.
- If backtest-accurate macro snapshots become necessary, a follow-up change should thread date context into FRED query parameters such as `observation_end` and the real-time period fields.

## Root-cause summary

### 1. Wrong endpoint for the job

`src/data/fred.rs` currently uses the bulk release endpoint, which is designed for retrieving observations for all series on a release. The macro tool only needs one latest observation per series.

### 2. Timeout pressure comes from oversized fetches

Because the code walks release pages until it finds the requested series, latency is tied to release size and pagination rather than to a single direct series lookup. That makes 30 second request deadlines much easier to hit.

### 3. The failure is inside the data client, not the rig tool boundary

The tool call itself is valid. The logged retries and degradations originate from `FredClient` request execution and error mapping, so the fix belongs in the FRED client implementation.

## Interface summary

| Unit | Responsibility | Input contract | Output contract |
|---|---|---|---|
| `series_observations_query` helper | Build the endpoint path and query parameters for one series lookup | `series_id: &str` | testable request descriptor for `fred/series/observations` |
| `FredClient::get_series_latest` | Fetch the latest observation for one FRED series | `series_id: &str` | `Result<Option<f64>, TradingError>` |
| `classify_fred_status` helper | Convert FRED HTTP status codes into retry/degrade classes | `StatusCode` | mapped `TradingError` or retry class |
| `FredClient::get_economic_indicators` | Fetch the fixed macro snapshot concurrently | none | `Result<Vec<MacroEvent>, TradingError>` |
| `GetEconomicIndicators` | Preserve the existing rig tool boundary | `{}` | `Vec<MacroEvent>` |

## Architecture changes

### 1. Replace release-based lookup with direct series lookup

The current `get_release_series_latest(release_id, target_series)` flow should be replaced with a narrower `get_series_latest(series_id)` flow.

Required behavior:

- build one request per target series
- send requests to `fred/series/observations`
- include `api_key`, `series_id`, `file_type=json`, `sort_order=desc`, and `limit=1`
- parse the first returned observation as the latest value
- treat `"."` as missing data and return `Ok(None)`

Implementation note:

- request construction should be factored through a small pure helper so tests can assert the endpoint path and query parameters without requiring a mock HTTP dependency or a live FRED call

This removes release IDs and cursor pagination from the runtime path.

### 2. Update response deserialization to the v1 shape

The v2 release response structs should be replaced by a smaller v1 observations response model:

- top-level `observations: Vec<FredObservation>`
- `FredObservation { date, value }`

No series-group wrapper or pagination fields are needed for this path.

### 3. Preserve transport behavior and make retry/degradation rules explicit

The existing resilience behavior remains useful, but the new path should define the boundary more precisely than the current broad `AnalystError` retry/degrade behavior.

Required behavior:

- keep the 30 second reqwest timeout unless a separate timeout change is needed later
- keep `.http1_only()` on the reqwest client because the codebase already documents a FRED CDN HTTP/2 interoperability problem
- retry on request timeout and transport-level failures where no valid FRED HTTP status was received
- treat HTTP 5xx as transient and eligible for retry
- do not retry HTTP 4xx request/auth failures other than `429`
- keep `429` mapped to `TradingError::RateLimitExceeded`
- do not silently degrade malformed JSON or other decode failures; surface them as hard failures

Partial macro snapshot degradation rule:

- degrade a single series to `None` for `NetworkTimeout`, transport-level transient failures, HTTP 5xx, and `RateLimitExceeded`
- do not degrade configuration errors, non-429 HTTP 4xx responses, or JSON decode failures

This keeps the endpoint swap narrow while aligning behavior with the intended meaning of "transient".

## Data flow

1. `GetEconomicIndicators::call()` invokes `FredClient::get_economic_indicators()`.
2. `FredClient::get_economic_indicators()` launches concurrent lookups for `FEDFUNDS` and `CPALTT01USM657N`.
3. Each lookup calls `fred/series/observations` directly for its `series_id`.
4. The latest observation value is parsed into `Option<f64>`.
5. The existing macro classification logic converts available values into `MacroEvent`s.
6. Only transient single-series failures degrade to a partial macro snapshot.

## Error handling

- missing FRED API key remains a configuration error
- HTTP timeouts remain `TradingError::NetworkTimeout`
- HTTP 429 remains `TradingError::RateLimitExceeded`
- transient HTTP 5xx and transport failures remain retryable and degradable
- non-429 HTTP 4xx failures fail fast and do not degrade silently
- malformed JSON and decode failures fail fast and do not degrade silently
- a missing or `"."` latest observation remains non-fatal and returns `Ok(None)`

## Testing

Regression coverage for this change should include:

- pure-helper request-building test proving the client targets `fred/series/observations` with `series_id`, `file_type=json`, `sort_order=desc`, and `limit=1`
- response parsing test for a valid latest observation payload
- response parsing test for `"."` mapping to `None`
- status-classification tests for `401`, `404`, `429`, and `500`
- response decode test proving malformed JSON is a hard failure
- macro collection test showing transient per-series failures still degrade instead of aborting the whole macro snapshot

## Risks and trade-offs

- This change is intentionally narrow and does not preserve the release-based v2 path for future bulk use.
- The client will still depend on FRED availability and network quality, but each request becomes much smaller and better aligned to the tool's actual needs.
- If future work requires release-wide historical ingestion, that should be implemented as a separate code path rather than overloaded into the macro snapshot helper.
