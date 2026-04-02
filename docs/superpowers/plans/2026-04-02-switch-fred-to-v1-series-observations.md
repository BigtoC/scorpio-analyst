# Switch FRED To V1 Series Observations Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current release-wide FRED lookup path with direct FRED v1 `series/observations` requests so macro indicator fetches stop timing out on bulk release scans.

**Architecture:** Keep the existing `FredClient`, `GetEconomicIndicators` tool surface, retry loop, and macro-event classification flow, but swap the runtime fetch path from release-based pagination to direct series-based requests. Add small pure helpers in `src/data/fred.rs` for request-shape and status/error classification so the endpoint change can be covered with narrow regression tests.

**Tech Stack:** Rust, `reqwest`, `tokio`, `serde`, existing `TradingError` mapping, existing inline tests in `src/data/fred.rs`.

---

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Modify | `src/data/fred.rs:1-177` | Replace v2 release endpoint constants and request path with v1 series-observations request helpers and fetching logic |
| Modify | `src/data/fred.rs:201-260` | Preserve macro collection flow but tighten degradation semantics to transient-only failures |
| Modify | `src/data/fred.rs:314-394` | Replace v2 response models with v1 observation response model and explicit status classification helpers |
| Modify | `src/data/fred.rs:418-652` | Replace v2-oriented tests with v1 request/helper/response/error regression coverage |
| Reference | `docs/superpowers/specs/2026-04-02-fred-v1-series-observations-design.md` | Approved design and error-handling rules for this change |

## Chunk 1: FRED Client Endpoint Swap

### Task 1: Add failing tests for the v1 helper boundaries

**Files:**
- Modify: `src/data/fred.rs:418-652`
- Reference: `docs/superpowers/specs/2026-04-02-fred-v1-series-observations-design.md`

- [ ] **Step 1: Write the failing request-helper test**

Add a test near the existing `fred.rs` test module that asserts a pure helper returns the v1 endpoint path and exact query parameters for `FEDFUNDS`:

```rust
#[test]
fn series_observations_query_targets_v1_endpoint_with_latest_only_params() {
    let query = series_observations_query(SERIES_FEDFUNDS);

    assert_eq!(query.path, "/fred/series/observations");
    assert_eq!(query.params, [
        ("series_id", "FEDFUNDS"),
        ("file_type", "json"),
        ("sort_order", "desc"),
        ("limit", "1"),
    ]);
}
```

- [ ] **Step 2: Run the single test and verify it fails**

Run: `cargo test --lib data::fred::tests::series_observations_query_targets_v1_endpoint_with_latest_only_params`
Expected: FAIL because the helper does not exist yet.

- [ ] **Step 3: Write the failing status-classification tests**

Add focused tests for the approved error boundary:

```rust
#[test]
fn fred_status_429_maps_to_rate_limit() {
    assert!(matches!(
        classify_fred_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
        FredStatusClass::RateLimited
    ));
}

#[test]
fn fred_status_500_is_retryable_transient() {
    assert!(matches!(
        classify_fred_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
        FredStatusClass::TransientServer
    ));
}

#[test]
fn fred_status_401_is_permanent() {
    assert!(matches!(
        classify_fred_status(reqwest::StatusCode::UNAUTHORIZED),
        FredStatusClass::PermanentClient
    ));
}

#[test]
fn fred_status_404_is_permanent() {
    assert!(matches!(
        classify_fred_status(reqwest::StatusCode::NOT_FOUND),
        FredStatusClass::PermanentClient
    ));
}
```

- [ ] **Step 4: Run the status-classification tests and verify they fail**

Run: `cargo test --lib data::fred::tests::fred_status_429_maps_to_rate_limit`
Expected: FAIL because the classification helper and enum do not exist yet.

- [ ] **Step 5: Write the failing v1 response-shape tests**

Add tests that prove the new response model expects top-level `observations` and that `"."` still maps to `None`:

```rust
#[test]
fn deserialize_v1_series_observations_response() {
    let json = r#"{
        "observations": [
            {"date": "2026-03-01", "value": "4.33"}
        ]
    }"#;

    let resp: FredSeriesObservationsResponse = serde_json::from_str(json).expect("should parse");
    assert_eq!(resp.observations.len(), 1);
    assert_eq!(resp.observations[0].parse_value(), Some(4.33));
}
```

Also add a malformed JSON regression test at the decode boundary, for example by testing a small helper that attempts to deserialize a bad payload into `FredSeriesObservationsResponse` and maps it through the dedicated decode error path.

- [ ] **Step 6: Run the v1 response test and verify it fails**

Run: `cargo test --lib data::fred::tests::deserialize_v1_series_observations_response`
Expected: FAIL because the v1 response type does not exist yet.

### Task 2: Implement the pure helpers and v1 response model

**Files:**
- Modify: `src/data/fred.rs:27-41`
- Modify: `src/data/fred.rs:314-394`
- Modify: `src/data/fred.rs:418-652`

- [ ] **Step 1: Add the v1 endpoint constant and request-helper struct**

Replace the current release endpoint constant with a v1 series-observations constant and add a tiny pure helper return type:

```rust
const FRED_V1_SERIES_OBSERVATIONS_PATH: &str = "/fred/series/observations";

#[derive(Debug, PartialEq, Eq)]
struct FredSeriesQuery<'a> {
    path: &'static str,
    params: [(&'static str, &'a str); 4],
}
```

- [ ] **Step 2: Implement the request helper minimally**

Add the helper near the constants:

```rust
fn series_observations_query(series_id: &str) -> FredSeriesQuery<'_> {
    FredSeriesQuery {
        path: FRED_V1_SERIES_OBSERVATIONS_PATH,
        params: [
            ("series_id", series_id),
            ("file_type", "json"),
            ("sort_order", "desc"),
            ("limit", "1"),
        ],
    }
}
```

- [ ] **Step 3: Add the status-classification enum and helper**

Implement only the categories needed by the approved design:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FredStatusClass {
    RateLimited,
    TransientServer,
    PermanentClient,
}

fn classify_fred_status(status: reqwest::StatusCode) -> FredStatusClass {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        FredStatusClass::RateLimited
    } else if status.is_server_error() {
        FredStatusClass::TransientServer
    } else {
        FredStatusClass::PermanentClient
    }
}
```

- [ ] **Step 4: Replace the v2 response structs with the v1 response model**

Use the smallest response model that matches the new endpoint:

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct FredSeriesObservationsResponse {
    pub observations: Vec<FredObservation>,
}
```

Keep `FredObservation` and `parse_value()`.

- [ ] **Step 5: Run the targeted tests and verify they pass**

Run: `cargo test --lib data::fred::tests`
Expected: PASS.

### Task 3: Replace the runtime fetch path with direct series lookups

**Files:**
- Modify: `src/data/fred.rs:101-177`
- Modify: `src/data/fred.rs:201-215`

- [ ] **Step 1: Replace `get_release_series_latest()` with `get_series_latest()`**

Change the method signature and implementation so it accepts only `series_id` and builds the request through `series_observations_query()`.

Implementation target:

```rust
pub async fn get_series_latest(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
    self.limiter.acquire().await;

    let mut last_err = None;
    for attempt in 0..Self::MAX_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Self::RETRY_BASE_DELAY * attempt).await;
        }
        match self.send_series_request(series_id).await {
            Ok(value) => return Ok(value),
            Err(error) if is_retryable_fred_err(&error) => last_err = Some(error),
            Err(error) => return Err(error),
        }
    }

    Err(last_err.expect("loop executed at least once"))
}
```

- [ ] **Step 2: Implement `send_series_request()` for the v1 endpoint**

Use the pure helper and existing reqwest client:

```rust
async fn send_series_request(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
    let query = series_observations_query(series_id);
    let resp = self
        .http
        .get(format!("https://api.stlouisfed.org{}", query.path))
        .query(&query.params)
        .query(&[("api_key", self.api_key.expose_secret())])
        .send()
        .await
        .map_err(map_fred_err)?;

    let resp = map_fred_response_status(resp)?;
    let body = resp
        .json::<FredSeriesObservationsResponse>()
        .await
        .map_err(map_fred_decode_err)?;

    Ok(body.observations.first().and_then(FredObservation::parse_value))
}
```

Keep the client’s `.http1_only()` transport setup unchanged.

- [ ] **Step 3: Update `get_economic_indicators()` to call the new helper**

Replace:

```rust
let interest_fut = self.get_release_series_latest(RELEASE_INTEREST_RATES, SERIES_FEDFUNDS);
let inflation_fut = self.get_release_series_latest(RELEASE_MEI, SERIES_CPALTT01);
```

With:

```rust
let interest_fut = self.get_series_latest(SERIES_FEDFUNDS);
let inflation_fut = self.get_series_latest(SERIES_CPALTT01);
```

- [ ] **Step 4: Run the `fred.rs` test module**

Run: `cargo test --lib data::fred::tests`
Expected: PASS, or fail only on the next known error-boundary tests not yet implemented.

### Task 4: Tighten retry and degradation semantics to transient-only failures

**Files:**
- Modify: `src/data/fred.rs:246-394`
- Modify: `src/data/fred.rs:418-652`

- [ ] **Step 1: Write the failing permanent-failure tests**

Add tests that encode the approved behavior:

```rust
#[test]
fn economic_indicator_collection_propagates_non_transient_analyst_failures() {
    let result = collect_macro_events_from_series_results(
        Err(TradingError::AnalystError {
            agent: "fred".to_owned(),
            message: "FRED request failed: 401 Unauthorized".to_owned(),
        }),
        Ok(Some(2.5)),
    );

    assert!(matches!(result, Err(TradingError::AnalystError { .. })));
}
```

Add a parallel positive test for transient server errors if you introduce a dedicated transient error message path.

Also add a malformed-decode regression test that proves a bad JSON payload maps to a hard failure and is not silently degraded.

- [ ] **Step 2: Run the new permanent-failure test and verify it fails**

Run: `cargo test --lib data::fred::tests::economic_indicator_collection_propagates_non_transient_analyst_failures`
Expected: FAIL because the current degradation rule treats all `AnalystError` values as degradable.

- [ ] **Step 3: Split transport/status/decode mapping into explicit helpers**

Implement narrow helpers so the runtime behavior matches the approved design:

```rust
fn map_fred_response_status(resp: reqwest::Response) -> Result<reqwest::Response, TradingError> { ... }

fn map_fred_decode_err(err: reqwest::Error) -> TradingError { ... }

fn is_retryable_fred_err(err: &TradingError) -> bool { ... }

fn is_degradable_fred_err(err: &TradingError) -> bool { ... }
```

Rules:

- retry and degrade `NetworkTimeout`
- retry and degrade `RateLimitExceeded`
- retry and degrade transient HTTP 5xx failures
- do not retry or degrade non-429 HTTP 4xx failures
- do not retry or degrade JSON decode failures

- [ ] **Step 4: Update `degrade_transient_series_error()` to use the explicit helper**

Replace the broad `match` arm over all `AnalystError` values with:

```rust
Err(error) if is_degradable_fred_err(&error) => {
    tracing::warn!(series = series_name, error = %error, "FRED series fetch failed; degrading to partial macro snapshot");
    Ok(None)
}
```

- [ ] **Step 5: Run the focused semantics tests**

Run: `cargo test --lib data::fred::tests`
Expected: PASS.

### Task 5: Run final verification for the FRED change

**Files:**
- Modify: `src/data/fred.rs`
- Reference: `docs/superpowers/specs/2026-04-02-fred-v1-series-observations-design.md`

- [ ] **Step 1: Run the full `fred.rs` test module**

Run: `cargo test --lib data::fred::tests`
Expected: PASS.

- [ ] **Step 2: Run formatter check**

Run: `cargo fmt -- --check`
Expected: PASS.

- [ ] **Step 3: Run a targeted library check**

Run: `cargo check --lib`
Expected: PASS.

- [ ] **Step 4: Do not create a commit unless the user explicitly asks**

The repository instructions for this session do not permit creating a git commit unless the user requests one.
