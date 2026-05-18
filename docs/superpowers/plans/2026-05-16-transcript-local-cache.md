# Earnings Call Transcript Local Cache Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist stable Alpha Vantage transcript results in a dedicated local SQLite cache so repeated analyses for the same symbol and quarter skip the external API call entirely.

**Architecture:** Add a new `TranscriptCacheStore` in `scorpio-core::data` that owns its own SQLite database, migrations, and uppercase-symbol normalization. The store caches only `TranscriptFetch::Found` results — negative outcomes (`NotPublished`, `Throttled`, `Unavailable`) bypass the cache and stay re-fetchable. Cache write failures degrade silently to direct API calls and increment `AlphaVantageClient::cache_failure_count`; read-side cache failures degrade to cache misses with sanitized `warn!` logs. Caching is fully internal to `AlphaVantageClient` by injecting an optional store at construction time, so the `TranscriptProvider` trait and all downstream callers remain unchanged; runtime startup degrades gracefully to uncached API calls if the cache cannot be opened.

**Tech Stack:** Rust 2024, `sqlx` SQLite migrations, `serde`/`serde_json`, `reqwest`, `tokio`, `tracing`, existing `Config`/`AnalysisRuntime`/`SharedRateLimiter` patterns. No new dependencies.

---

## Read First

- `docs/superpowers/specs/2026-05-16-transcript-local-cache-design.md` — note: this plan intentionally implements a simpler scope than the spec (no schema versioning, no `NotPublished` caching, no settings-boundary changes); the deferred items are listed in **Scope Check** below.
- `AGENTS.md`
- `CLAUDE.md`
- `.github/instructions/rust.instructions.md`

## Preconditions

- Work in a dedicated worktree.
- Keep Rust/code changes inside `crates/scorpio-core`; the only planned edit outside it is the doc update in `AGENTS.md`. `scorpio-cli` stays untouched.
- Follow `@superpowers:test-driven-development` discipline for each task.
- After implementation, run `@ce:review`, then capture the cache pattern in `@ce:compound`.
- Use `cargo nextest`, not `cargo test`.
- Full verification must end with:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

- If local verification fails because `protoc` is missing, install it first:

```bash
brew install protobuf
```

## Scope Check

This is one coherent subsystem: a transcript cache store plus the `AlphaVantageClient` wiring that consumes it.

Out of scope for this plan:

- New CLI commands such as `scorpio cache prune`
- TTL/eviction logic for cached transcripts
- Changes to `TranscriptProvider` or transcript prompt rendering
- A generic shared storage abstraction for every SQLite-backed store
- Caching of negative outcomes (`NotPublished`, `Throttled`, `Unavailable`) — keep re-fetchable; only `Found` is cacheable
- Schema versioning of cache rows — `serde_json` deserialization failure is the sole stale-row signal; treated as a cache miss with a sanitized warn
- User-file `[storage]` overrides for the new path — env var only. The same pre-plan limitation already applies to `snapshot_db_path`; out of scope here, addressable in a follow-on settings PR if user demand appears
- Restructuring the existing flat `crates/scorpio-core/migrations/` directory; the new cache migration sits in a new `migrations/transcript_cache/` subdirectory which `SnapshotStore`'s `sqlx::migrate!()` does not recurse into

## File Structure

| File                                                                               | Action | Responsibility                                                                                                                                            |
|------------------------------------------------------------------------------------|--------|-----------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql` | Create | Define the transcript cache table keyed by `(symbol, quarter)` with payload JSON                                                                          |
| `crates/scorpio-core/src/data/transcript_cache.rs`                                 | Create | Own `TranscriptCacheStore`, uppercase symbol normalization, SQLite open/setup, cache read/write, and focused store tests                                  |
| `crates/scorpio-core/src/data/mod.rs`                                              | Modify | Expose the new `transcript_cache` module                                                                                                                  |
| `crates/scorpio-core/src/data/alpha_vantage.rs`                                    | Modify | Add optional cache wiring, cache hit/miss flow, `cache_failure_count` counter, integration-style cache tests                                              |
| `crates/scorpio-core/src/config.rs`                                                | Modify | Add `StorageConfig::transcript_cache_db_path` with `serde(default)` and update the `Default` impl; add focused storage-config tests                       |
| `crates/scorpio-core/src/app/mod.rs`                                               | Modify | Construct the transcript cache best-effort and pass it into `AlphaVantageClient::new` only when Alpha Vantage is enabled                                  |
| `crates/scorpio-core/tests/app_runtime.rs`                                         | Modify | Fix the `StorageConfig` struct literal using `..StorageConfig::default()` after the new field is added                                                    |
| `AGENTS.md`                                                                        | Modify | Document the new transcript cache database location, the `migrations/transcript_cache/` subdirectory, and the `cache_failure_count` observability surface |

Notes for the implementing engineer:

- Keep `TranscriptCacheStore` as a single focused file.
- Do not introduce a `TranscriptCache` trait. The cache has one production implementation and one caller.
- Keep cache hits ahead of `self.rate_limiter.acquire().await` so a local hit does not spend rate-limit budget or request latency.
- Cache only `TranscriptFetch::Found(_)`. All other variants bypass cache writes.
- Normalize cache keys to uppercase at the storage boundary. `validate_symbol()` preserves caller casing in this repo.
- The existing `SnapshotStore::new` keeps using `sqlx::migrate!()` (which defaults to `migrations/` and reads only top-level `.sql` files — sqlx does not recurse into subdirectories). No snapshot migrations move; no `SnapshotStore` code changes.

## Chunk 1: Persistence Layer

### Task 1: Add cache migration and bootstrap store

**Files:**
- Create: `crates/scorpio-core/src/data/transcript_cache.rs`
- Create: `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql`
- Modify: `crates/scorpio-core/src/data/mod.rs`
- Test: `crates/scorpio-core/src/data/transcript_cache.rs`
- Regression test: `crates/scorpio-core/src/workflow/snapshot/tests/path.rs`
- Regression test: `crates/scorpio-core/src/workflow/snapshot/tests/core_roundtrip.rs`

- [ ] **Step 1: Export the new module first so the red test will actually compile into the crate**

Add the module declaration in `crates/scorpio-core/src/data/mod.rs`:

```rust
pub mod transcript_cache;
```

This is a prerequisite for TDD in this repo: without the export, the new file is not in the module tree and the failing test will not be compiled.

- [ ] **Step 2: Write the failing bootstrap test**

Add this test near the bottom of the new `crates/scorpio-core/src/data/transcript_cache.rs` file. Keep the file intentionally incomplete for now so the test fails to compile.

```rust
#[tokio::test]
async fn new_creates_transcript_cache_table() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");

    let store = TranscriptCacheStore::new(Some(&path))
        .await
        .expect("store should open");

    assert!(store.get("AAPL", "2025Q1").await.is_none());
}
```

- [ ] **Step 3: Run the bootstrap and snapshot regression tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(new_creates_transcript_cache_table) | test(parent_directory_is_created) | test(save_and_load_round_trip)'
```

Expected: the new test FAILS with a compile error about `TranscriptCacheStore` / `transcript_cache` not existing yet. The two snapshot regression tests should still pass — confirming snapshots are unaffected.

- [ ] **Step 4: Add the new transcript-cache migration**

Create `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql`:

```sql
CREATE TABLE IF NOT EXISTS transcript_cache (
    symbol       TEXT NOT NULL,
    quarter      TEXT NOT NULL CHECK (quarter GLOB '[0-9][0-9][0-9][0-9]Q[1-4]'),
    payload_json TEXT NOT NULL,
    cached_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (symbol, quarter)
);
```

The new subdirectory lives alongside the existing flat snapshot migrations. `SnapshotStore::new`'s `sqlx::migrate!()` reads `migrations/*.sql` only (no recursion), so it will not pick up this file; `TranscriptCacheStore::new` reads `migrations/transcript_cache/*.sql` only via the explicit path argument.

- [ ] **Step 5: Add the minimal `TranscriptCacheStore` bootstrap implementation**

In `crates/scorpio-core/src/data/transcript_cache.rs`, add only enough code to open the database, apply the two SQLite pragmas, run the new migration, and satisfy the bootstrap test:

```rust
#[derive(Clone, Debug)]
pub struct TranscriptCacheStore {
    pool: SqlitePool,
}

impl TranscriptCacheStore {
    pub async fn new(db_path: Option<&Path>) -> Result<Self, TradingError> {
        let resolved = resolve_db_path(db_path)?;

        if let Some(parent) = resolved.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))
                .map_err(TradingError::Config)?;
        }

        let db_url = format!("sqlite://{}?mode=rwc", resolved.display());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("PRAGMA journal_mode=WAL").execute(conn).await?;
                    sqlx::query("PRAGMA busy_timeout = 5000").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&db_url)
            .await
            .with_context(|| format!("failed to open SQLite pool at {}", resolved.display()))
            .map_err(TradingError::Config)?;

        sqlx::migrate!("migrations/transcript_cache")
            .run(&pool)
            .await
            .map_err(TradingError::Config)?;

        Ok(Self { pool })
    }

    pub async fn get(&self, _symbol: &str, _quarter: &str) -> Option<TranscriptFetch> {
        None
    }
}
```

Also add a private `resolve_db_path()` helper in the same file. Mirror `SnapshotStore` path validation rules (non-empty, no null bytes, no bare traversal, no dot-only); do not extract a shared generic helper for two call sites.

- [ ] **Step 6: Run the bootstrap and snapshot regression tests again**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(new_creates_transcript_cache_table) | test(parent_directory_is_created) | test(save_and_load_round_trip)'
```

Expected: PASS. Confirms the new cache store opens cleanly and the snapshot store is unaffected by the new subdirectory.

- [ ] **Step 7: Commit the bootstrap slice**

```bash
git add crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/data/transcript_cache.rs
git commit -m "feat(data): add transcript cache bootstrap store"
```

### Task 2: Implement transcript cache reads and writes

**Files:**
- Modify: `crates/scorpio-core/src/data/transcript_cache.rs`
- Test: `crates/scorpio-core/src/data/transcript_cache.rs`

- [ ] **Step 1: Write the failing read/write tests**

First define `test_store()` and `sample_found()` helpers in the test module so the tests below compile:

```rust
async fn test_store() -> TranscriptCacheStore {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");
    // Leak the TempDir for the lifetime of the test pool so the file
    // is not deleted while the connection is open. Tests are short-lived;
    // tempfile cleans up on process exit.
    std::mem::forget(dir);
    TranscriptCacheStore::new(Some(&path))
        .await
        .expect("store should open")
}

fn sample_found(symbol: &str, quarter: &str) -> TranscriptFetch {
    // Construct a minimal Found payload using whatever the existing
    // TranscriptFetch::Found tuple/struct shape requires. Mirror the
    // payload pattern already used in alpha_vantage.rs tests.
    TranscriptFetch::Found(/* ... single TranscriptEvidence ... */)
}
```

Then add the tests:

```rust
#[tokio::test]
async fn put_and_get_round_trip_found() {
    let store = test_store().await;
    let fetch = sample_found("AAPL", "2025Q1");

    store.put("AAPL", "2025Q1", &fetch).await.expect("put");

    assert_eq!(store.get("AAPL", "2025Q1").await, Some(fetch));
}

#[tokio::test]
async fn put_skips_negative_outcomes() {
    let store = test_store().await;

    store.put("AAPL", "2025Q1", &TranscriptFetch::NotPublished).await.expect("put");
    store.put("AAPL", "2025Q2", &TranscriptFetch::Throttled).await.expect("put");
    store.put("AAPL", "2025Q3", &TranscriptFetch::Unavailable).await.expect("put");

    assert!(store.get("AAPL", "2025Q1").await.is_none());
    assert!(store.get("AAPL", "2025Q2").await.is_none());
    assert!(store.get("AAPL", "2025Q3").await.is_none());
}
```

Symbol casing is normalized inside `put`/`get` (`to_ascii_uppercase()`) so the cache key is canonical. The production call path goes through `validate_symbol()` first, which today preserves caller casing — the uppercase normalization is a one-liner defensive guard, but a dedicated case-mixing test is out of scope.

- [ ] **Step 2: Run the tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_and_get_round_trip_found) | test(put_skips_negative_outcomes)'
```

Expected: FAIL because `put()` does not exist yet.

- [ ] **Step 3: Implement `put()` and the happy-path `get()`**

```rust
pub async fn put(
    &self,
    symbol: &str,
    quarter: &str,
    fetch: &TranscriptFetch,
) -> Result<(), TradingError> {
    validate_quarter_for_storage(quarter)?;

    if !matches!(fetch, TranscriptFetch::Found(_)) {
        return Ok(());
    }

    let normalized_symbol = symbol.to_ascii_uppercase();
    let payload_json = serde_json::to_string(fetch)
        .with_context(|| "failed to serialize transcript cache payload")
        .map_err(TradingError::Storage)?;

    sqlx::query(
        "INSERT INTO transcript_cache (symbol, quarter, payload_json)
         VALUES (?, ?, ?)
         ON CONFLICT(symbol, quarter) DO UPDATE SET
             payload_json = excluded.payload_json,
             cached_at = datetime('now')",
    )
    .bind(&normalized_symbol)
    .bind(quarter)
    .bind(&payload_json)
    .execute(&self.pool)
    .await
    .with_context(|| format!("failed to write transcript cache entry {symbol} {quarter}"))
    .map_err(TradingError::Storage)?;

    Ok(())
}

pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch> {
    let normalized_symbol = symbol.to_ascii_uppercase();
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT payload_json FROM transcript_cache
         WHERE symbol = ? AND quarter = ?",
    )
    .bind(&normalized_symbol)
    .bind(quarter)
    .fetch_optional(&self.pool)
    .await
    .ok()??;

    let (payload_json,) = row;
    serde_json::from_str(&payload_json).ok()
}
```

Add a small `validate_quarter_for_storage()` helper enforcing the canonical `YYYYQN` shape before any SQL runs. This makes `put("AAPL", "2025q1", ...)` fail fast even for negative outcomes, instead of being silently skipped before the SQL `CHECK` constraint is reached.

This `get()` is the happy path only; Task 3 adds explicit query/deserialization warning paths and the test seam needed by the client integration tests.

- [ ] **Step 4: Run the read/write tests again**

Expected: PASS.

- [ ] **Step 5: Commit the read/write slice**

```bash
git add crates/scorpio-core/src/data/transcript_cache.rs
git commit -m "feat(data): implement transcript cache reads and writes"
```

### Task 3: Harden cache reads, overwrite semantics, and test seam

**Files:**
- Modify: `crates/scorpio-core/src/data/transcript_cache.rs`
- Test: `crates/scorpio-core/src/data/transcript_cache.rs`

- [ ] **Step 1: Write the failing resilience tests**

Add a `sample_found_with_speaker(symbol, quarter, speaker)` helper that returns a `Found` with a specific speaker so the overwrite test asserts an actual payload change. Reuse the small `SharedLogBuffer` test-helper pattern already present in `crates/scorpio-core/src/providers/factory/error.rs` so the corrupt-payload test can assert on warning output without adding a dependency. Then add:

```rust
#[tokio::test]
async fn put_overwrites_existing_entry() {
    let store = test_store().await;

    let first = sample_found("AAPL", "2025Q1");
    store.put("AAPL", "2025Q1", &first).await.expect("first put");

    let second = sample_found_with_speaker("AAPL", "2025Q1", "Luca Maestri");
    store.put("AAPL", "2025Q1", &second).await.expect("second put");

    assert_eq!(store.get("AAPL", "2025Q1").await, Some(second));
}

#[test]
fn get_logs_sanitized_warn_for_corrupt_payload() {
    let logs = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_writer(logs.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        runtime.block_on(async {
            let store = test_store().await;

            sqlx::query(
                "INSERT INTO transcript_cache (symbol, quarter, payload_json)
                 VALUES (?, ?, ?)",
            )
            .bind("AAPL")
            .bind("2025Q1")
            .bind("not valid json")
            .execute(&store.pool)
            .await
            .expect("seed row");

            assert!(store.get("AAPL", "2025Q1").await.is_none());
        });
    });

    let output = logs.contents();
    assert!(output.contains("transcript cache deserialize failed"));
    assert!(!output.contains("not valid json"));
}

#[tokio::test]
async fn put_rejects_invalid_quarter_at_storage_boundary() {
    let store = test_store().await;
    let err = store
        .put("AAPL", "2025q1", &sample_found("AAPL", "2025q1"))
        .await
        .expect_err("invalid quarter should fail");

    assert!(format!("{err:#}").contains("invalid quarter format"));
}

#[tokio::test]
async fn two_store_instances_can_open_the_same_db_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");

    let first = TranscriptCacheStore::new(Some(&path)).await.expect("first store");
    let second = TranscriptCacheStore::new(Some(&path)).await.expect("second store");

    first
        .put("AAPL", "2025Q1", &sample_found("AAPL", "2025Q1"))
        .await
        .expect("write");
    assert!(second.get("AAPL", "2025Q1").await.is_some());
}
```

- [ ] **Step 2: Run the resilience tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_overwrites_existing_entry) | test(get_logs_sanitized_warn_for_corrupt_payload) | test(put_rejects_invalid_quarter_at_storage_boundary) | test(two_store_instances_can_open_the_same_db_path)'
```

Expected: FAIL because Task 2's `get()` still silently `ok()`s deserialization failures, so the corrupt-row test sees no warning output yet. The overwrite and shared-path tests may already pass from Task 2; the important red signal is the missing sanitized warn.

- [ ] **Step 3: Harden `get()` so cache failures degrade to misses with sanitized WARN logs**

Replace the happy-path implementation with explicit handling for query errors and deserialization failure:

```rust
pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch> {
    let normalized_symbol = symbol.to_ascii_uppercase();
    let row = match sqlx::query_as::<_, (String,)>(
        "SELECT payload_json FROM transcript_cache
         WHERE symbol = ? AND quarter = ?",
    )
    .bind(&normalized_symbol)
    .bind(quarter)
    .fetch_optional(&self.pool)
    .await
    {
        Ok(row) => row,
        Err(_err) => {
            warn!(symbol, quarter, error.kind = "query", "transcript cache read failed");
            return None;
        }
    };

    let Some((payload_json,)) = row else {
        return None;
    };

    match serde_json::from_str::<TranscriptFetch>(&payload_json) {
        Ok(fetch) => Some(fetch),
        Err(_err) => {
            warn!(symbol, quarter, error.kind = "deserialize", "transcript cache deserialize failed");
            None
        }
    }
}
```

Do not log raw `serde_json` error text — it can echo payload bytes. `AlphaVantageClient::cache_failure_count` (added in Chunk 2) is only the operator-facing signal for chronic write failures; read-side cache failures rely on these sanitized warn logs.

- [ ] **Step 4: Add the test-only store failure seam**

Mirror the existing `SnapshotStore::close_for_test()` pattern so `AlphaVantageClient` tests can force cache read/write failures without adding a trait abstraction:

```rust
#[cfg(test)]
pub(crate) async fn close_for_test(&self) {
    self.pool.close().await;
}
```

This seam is required for Chunk 2's `fetch_transcript_uses_api_when_cache_is_unavailable` test, which verifies the client falls back to the API and bumps `cache_failure_count` when the cache pool is closed.

- [ ] **Step 5: Re-run the resilience tests and one earlier happy-path test**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_and_get_round_trip_found) | test(put_overwrites_existing_entry) | test(get_logs_sanitized_warn_for_corrupt_payload) | test(put_rejects_invalid_quarter_at_storage_boundary) | test(two_store_instances_can_open_the_same_db_path)'
```

Expected: PASS.

- [ ] **Step 6: Commit the hardened store slice**

```bash
git add crates/scorpio-core/src/data/transcript_cache.rs
git commit -m "feat(data): harden transcript cache reads"
```

## Chunk 2: Alpha Vantage Integration

### Task 4: Add optional cache wiring to `AlphaVantageClient`

> Depends on Tasks 1–3: requires `TranscriptCacheStore`, its public API (`new`, `put`, `get`), and the `close_for_test` seam.

**Files:**
- Modify: `crates/scorpio-core/src/data/alpha_vantage.rs`
- Test: `crates/scorpio-core/src/data/alpha_vantage.rs`

- [ ] **Step 1: Write the failing cache-hit test**

Add this test to `crates/scorpio-core/src/data/alpha_vantage.rs`:

```rust
#[tokio::test]
async fn fetch_transcript_returns_cached_result_before_network() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");
    let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
    let cached = sample_found("AAPL", "2025Q1");

    cache.put("AAPL", "2025Q1", &cached).await.expect("seed cache");

    let client = AlphaVantageClient::new_with_base_url(
        SecretString::from("test-dummy-key"),
        SharedRateLimiter::disabled("test"),
        "http://127.0.0.1:1/query".to_owned(),
        Some(cache),
    );

    let result = client
        .fetch_transcript("AAPL", "2025Q1")
        .await
        .expect("cache hit should succeed");

    assert_eq!(result, cached);
}
```

Also add a `sample_found()` helper to this test module so the transcript payload is not repeated inline.

- [ ] **Step 2: Run the cache-hit test to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(fetch_transcript_returns_cached_result_before_network) | test(debug_does_not_leak_secret) | test(constructor_missing_key_uses_static_error_message)'
```

Expected: FAIL because `AlphaVantageClient` does not yet accept a cache and `new_with_base_url()` still takes three arguments.

- [ ] **Step 3: Add the optional cache field, observability counter, and update constructors**

Add the field and counter to the struct (the existing imports already pull in `AtomicU64` and `Ordering` per the other counters in this file):

```rust
pub struct AlphaVantageClient {
    key: SecretString,
    rate_limiter: SharedRateLimiter,
    http: reqwest::Client,
    base_url: String,
    cache: Option<TranscriptCacheStore>,
    cache_failure_count: AtomicU64,
    // existing counters (found_count, not_published_count, throttled_count, ...) stay as-is
}
```

Update both constructor signatures and their field-init blocks:

```rust
pub fn new(
    api: &ApiConfig,
    limiter: SharedRateLimiter,
    cache: Option<TranscriptCacheStore>,
) -> Result<Self, TradingError> {
    // ...existing setup...
    Ok(Self {
        // ...existing field inits...
        cache,
        cache_failure_count: AtomicU64::new(0),
    })
}

fn new_with_base_url(
    key: SecretString,
    limiter: SharedRateLimiter,
    base_url: String,
    cache: Option<TranscriptCacheStore>,
) -> Self {
    Self {
        // ...existing field inits...
        cache,
        cache_failure_count: AtomicU64::new(0),
    }
}
```

The existing field literals in both constructors are exhaustive (no `..Default::default()` fallback) — both initializer lines are required or you'll hit `E0063: missing field cache_failure_count`.

Add a public accessor:

```rust
pub fn cache_failure_count(&self) -> u64 {
    self.cache_failure_count.load(Ordering::Relaxed)
}
```

The existing `Debug` impl in this file is hand-rolled (not auto-derived). Add one more `.field(...)` line to it:

```rust
.field("cache_failure_count", &self.cache_failure_count.load(Ordering::Relaxed))
```

Keep `for_test()` cache-free:

```rust
pub fn for_test() -> Self {
    Self::new_with_base_url(
        SecretString::from("test-dummy-key"),
        SharedRateLimiter::disabled("test"),
        "http://127.0.0.1:1/query".to_owned(),
        None,
    )
}
```

Update the two existing constructor tests in this file to pass `None` into `AlphaVantageClient::new(...)`.

- [ ] **Step 4: Add the cache hit/miss path before the rate limiter**

Inside `fetch_transcript()`, keep input validation first, then add the cache lookup before `self.rate_limiter.acquire().await`:

```rust
validate_symbol(symbol)?;
Self::validate_quarter(as_of_date)?;

if let Some(cache) = &self.cache {
    if let Some(cached) = cache.get(symbol, as_of_date).await {
        debug!(symbol, quarter = as_of_date, "transcript cache hit");
        return Ok(cached);
    }
}

debug!(symbol, quarter = as_of_date, "transcript cache miss, fetching from Alpha Vantage");
```

Important:

- Leave `validate_symbol` and `validate_quarter` ahead of the cache so invalid input still fails fast.
- Do not increment `found_count` / `not_published_count` on cache hits; those counters continue to describe provider outcomes, not local reads.
- Do not add a `with_cache()` builder. The constructor parameter makes call sites explicit.

- [ ] **Step 5: Run the cache-hit and constructor tests again**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(fetch_transcript_returns_cached_result_before_network) | test(debug_does_not_leak_secret) | test(constructor_missing_key_uses_static_error_message) | test(invalid_quarter_format_rejected)'
```

Expected: PASS.

- [ ] **Step 6: Commit the cache-hit slice**

```bash
git add crates/scorpio-core/src/data/alpha_vantage.rs
git commit -m "feat(alpha-vantage): add transcript cache reads"
```

### Task 5: Cache successful API results with best-effort writeback and observability

**Files:**
- Modify: `crates/scorpio-core/src/data/alpha_vantage.rs`
- Test: `crates/scorpio-core/src/data/alpha_vantage.rs`

- [ ] **Step 1: Write the failing integration-style cache tests**

Add a tiny loopback HTTP helper in the `alpha_vantage.rs` test module using `std::net::TcpListener`; do not add `wiremock` or another new dev dependency.

Then add these tests:

```rust
#[tokio::test]
async fn fetch_transcript_caches_found_results_after_first_api_call() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");
    let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
    let calls = Arc::new(AtomicUsize::new(0));

    let base_url = spawn_transcript_server(
        r#"{"symbol":"AAPL","quarter":"2025Q1","transcript":[{"speaker":"Tim Cook","title":"CEO","content":"Hello","sentiment":0.5}]}"#,
        Arc::clone(&calls),
    );

    let client = AlphaVantageClient::new_with_base_url(
        SecretString::from("test-dummy-key"),
        SharedRateLimiter::disabled("test"),
        base_url,
        Some(cache),
    );

    let first = client.fetch_transcript("AAPL", "2025Q1").await.expect("first call");
    let second = client.fetch_transcript("AAPL", "2025Q1").await.expect("second call");

    assert!(matches!(first, TranscriptFetch::Found(_)));
    assert_eq!(first, second);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(client.cache_failure_count(), 0);
}

#[tokio::test]
async fn fetch_transcript_uses_api_when_cache_is_unavailable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");
    let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
    cache.close_for_test().await;
    let calls = Arc::new(AtomicUsize::new(0));

    // Use a Found-shaped body so the writeback path actually touches the
    // closed pool. `put()` early-returns Ok for NotPublished/Throttled/
    // Unavailable outcomes (cacheability policy), so an empty-transcript
    // body would never reach the pool and the counter would never bump.
    let base_url = spawn_transcript_server(
        r#"{"symbol":"AAPL","quarter":"2025Q1","transcript":[{"speaker":"Tim Cook","title":"CEO","content":"Hello","sentiment":0.5}]}"#,
        Arc::clone(&calls),
    );

    let client = AlphaVantageClient::new_with_base_url(
        SecretString::from("test-dummy-key"),
        SharedRateLimiter::disabled("test"),
        base_url,
        Some(cache),
    );

    let result = client.fetch_transcript("AAPL", "2025Q1").await.expect("api fallback");

    assert!(matches!(result, TranscriptFetch::Found(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(client.cache_failure_count() > 0);
}
```

The first test exercises the primary `Found` path. The second test asserts that closed-pool failures bump the observability counter, so chronic failure is visible to operators without scanning warn logs.

- [ ] **Step 2: Run the integration-style tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(fetch_transcript_caches_found_results_after_first_api_call) | test(fetch_transcript_uses_api_when_cache_is_unavailable)'
```

Expected: FAIL because `fetch_transcript()` still returns the API result without writing it back to the cache, and the failure counter is never incremented.

- [ ] **Step 3: Add best-effort cache writeback with counter bumps**

At the end of `fetch_transcript()`, after `outcome` is computed and before the final `Ok(outcome)`, add:

```rust
if let Some(cache) = &self.cache {
    if let Err(_err) = cache.put(symbol, as_of_date, &outcome).await {
        self.cache_failure_count.fetch_add(1, Ordering::Relaxed);
        warn!(
            symbol,
            quarter = as_of_date,
            error.kind = "storage",
            "transcript cache put failed"
        );
    }
}
```

The cache-hit `if let` block introduced in Task 4 Step 4 stays as-is — do not rewrite it. This step only adds the writeback at the end of `fetch_transcript()`.

The cacheability rule (only `Found`) already lives in `TranscriptCacheStore::put()`; `AlphaVantageClient` only forwards the outcome.

**Counter semantics:** `cache_failure_count` tracks **write** failures only. Read-side failures (corrupt payload, query error) emit sanitized `warn!` logs from `TranscriptCacheStore::get()` but do not bump the counter. Rationale: a corrupted row is one extra API call; a sustained write failure indicates a disk or permissions problem that operators need to see at a glance. The AGENTS.md update (Task 7 Step 1) must reflect this — the counter is not a complete cache-health signal.

- [ ] **Step 4: Add the tiny loopback server helper in the test module**

Use the pattern already present in `crates/scorpio-cli/src/cli/update.rs:898` rather than adding a new mocking library:

```rust
fn spawn_transcript_server(body: &'static str, calls: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = std::io::Read::read(&mut stream, &mut [0u8; 1024]);
        calls.fetch_add(1, Ordering::SeqCst);

        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        std::io::Write::write_all(&mut stream, response.as_bytes()).expect("write");
    });

    format!("http://{addr}/query")
}
```

- [ ] **Step 5: Run the integration-style tests again**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(fetch_transcript_returns_cached_result_before_network) | test(fetch_transcript_caches_found_results_after_first_api_call) | test(fetch_transcript_uses_api_when_cache_is_unavailable)'
```

Expected: PASS.

- [ ] **Step 6: Commit the writeback slice**

```bash
git add crates/scorpio-core/src/data/alpha_vantage.rs
git commit -m "feat(alpha-vantage): cache transcript fetch results with observability"
```

## Chunk 3: Runtime Wiring, Docs, and Verification

### Task 6: Add transcript cache configuration and runtime wiring

**Files:**
- Modify: `crates/scorpio-core/src/config.rs`
- Modify: `crates/scorpio-core/src/data/transcript_cache.rs`
- Modify: `crates/scorpio-core/src/app/mod.rs`
- Modify: `crates/scorpio-core/tests/app_runtime.rs`
- Test: `crates/scorpio-core/src/config.rs`

- [ ] **Step 1: Write the failing storage-config tests**

Add these tests to `crates/scorpio-core/src/config.rs`:

```rust
#[test]
fn storage_config_transcript_cache_defaults_to_tilde_path() {
    let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
    let cfg = Config::load_from(&path).expect("config should load");

    assert_eq!(
        cfg.storage.transcript_cache_db_path,
        "~/.scorpio-analyst/transcript_cache.db"
    );
}

#[test]
fn storage_config_transcript_cache_can_be_overridden_via_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);

    unsafe {
        std::env::set_var(
            "SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH",
            "/tmp/transcript-cache.db",
        );
    }
    let result = Config::load_from(&path);
    unsafe {
        std::env::remove_var("SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH");
    }

    let cfg = result.expect("config should load");
    assert_eq!(cfg.storage.transcript_cache_db_path, "/tmp/transcript-cache.db");
}

#[test]
fn load_storage_honors_transcript_cache_env_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().expect("temp home");
    let config_dir = home.path().join(".scorpio-analyst");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(config_dir.join("config.toml"), MINIMAL_CONFIG_TOML).expect("write config");

    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var(
            "SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH",
            "/tmp/env-only-transcript.db",
        );
    }
    let result = Config::load_storage();
    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH");
    }

    let storage = result.expect("storage loading should succeed");
    assert_eq!(
        storage.transcript_cache_db_path,
        "/tmp/env-only-transcript.db"
    );
}
```

The third test specifically verifies env-var symmetry through `Config::load_storage()` (the env-only path), not just `Config::load_from()`, so the new field is honored through the same builder pipeline as `snapshot_db_path`.

- [ ] **Step 2: Run the storage-config tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(storage_config_transcript_cache_defaults_to_tilde_path) | test(storage_config_transcript_cache_can_be_overridden_via_env) | test(load_storage_honors_transcript_cache_env_override)'
```

Expected: FAIL because `StorageConfig::transcript_cache_db_path` does not exist yet.

- [ ] **Step 3: Extend `StorageConfig` and remove the snapshot-only early return**

In `crates/scorpio-core/src/config.rs`:

1. Add the new field to the `StorageConfig` struct:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_snapshot_db_path")]
    pub snapshot_db_path: String,
    #[serde(default = "default_transcript_cache_db_path")]
    pub transcript_cache_db_path: String,
}

fn default_transcript_cache_db_path() -> String {
    "~/.scorpio-analyst/transcript_cache.db".to_string()
}
```

2. **Replace** the existing `impl Default for StorageConfig` (currently around `config.rs:368`) with:

```rust
impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            snapshot_db_path: default_snapshot_db_path(),
            transcript_cache_db_path: default_transcript_cache_db_path(),
        }
    }
}
```

(Do not add a second `impl Default` — `E0119: conflicting implementations`.)

3. **Remove** the early-return inside `Config::load_storage()` at approximately `config.rs:454-456`:

```rust
// BEFORE — delete this block:
if let Ok(snapshot_db_path) = std::env::var("SCORPIO__STORAGE__SNAPSHOT_DB_PATH") {
    return Ok(StorageConfig { snapshot_db_path });
}
```

The early-return constructs `StorageConfig` with a single named field — that becomes a compile error (`E0063: missing field transcript_cache_db_path`) the moment the new field is added. Removing it is also correct on the merits: `SCORPIO__STORAGE__SNAPSHOT_DB_PATH` is already covered by the subsequent `config::Environment::with_prefix("SCORPIO").separator("__")` builder, so the early-return was redundant defense-in-depth.

That is the entire `config.rs` change. Do not modify `PartialConfig`, `UserConfigFile`, or `partial_to_nested_toml_non_secrets()` — user-file `[storage]` overrides are not in scope for this plan. (The same pre-plan limitation already applies to `snapshot_db_path`; users override via env vars.) `Config::load_storage()` already parses `[storage]` directly from the user file via `StorageOnlyConfig`, so default + env-var overrides work without further surgery.

- [ ] **Step 4: Add `TranscriptCacheStore::from_config()`**

Back in `crates/scorpio-core/src/data/transcript_cache.rs`, add:

```rust
pub async fn from_config(config: &Config) -> Result<Self, TradingError> {
    let path = crate::config::expand_path(&config.storage.transcript_cache_db_path);
    Self::new(Some(&path)).await
}
```

Keep this thin. Do not move unrelated config logic into the store.

- [ ] **Step 5: Wire the cache into `AnalysisRuntime::new()` only when Alpha Vantage is enabled**

In `crates/scorpio-core/src/app/mod.rs`, keep the existing `if cfg.api.alpha_vantage_api_key.is_some()` guard. Inside that branch, build the cache best-effort, warn on failure, then pass it into the client constructor:

```rust
let transcript_cache = match crate::data::transcript_cache::TranscriptCacheStore::from_config(&cfg).await {
    Ok(store) => Some(store),
    Err(_err) => {
        tracing::warn!(error.kind = "config", "failed to initialize transcript cache; continuing without cache");
        None
    }
};

match crate::data::AlphaVantageClient::new(&cfg.api, av_limiter, transcript_cache) {
    // existing success/failure handling
}
```

Do not create the store outside the Alpha Vantage branch. Without an Alpha Vantage key, transcript enrichment is already disabled, so eagerly opening the cache is wasted work and emits pointless warnings.

- [ ] **Step 6: Fix the `StorageConfig` struct literal in the runtime integration test**

Update `crates/scorpio-core/tests/app_runtime.rs` to use the new defaulted field rather than spelling both fields manually:

```rust
storage: StorageConfig {
    snapshot_db_path: "/dev/null/scorpio-phase-snapshots.db".to_owned(),
    ..StorageConfig::default()
},
```

This keeps the test focused on snapshot-store failure instead of transcript-cache configuration noise.

- [ ] **Step 7: Run the focused config, runtime, and cache tests**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(storage_config_transcript_cache_defaults_to_tilde_path) | test(storage_config_transcript_cache_can_be_overridden_via_env) | test(load_storage_honors_transcript_cache_env_override) | test(new_wraps_snapshot_store_initialization_failures) | test(fetch_transcript_caches_found_results_after_first_api_call)'
```

`new_wraps_snapshot_store_initialization_failures` is a pre-existing regression test in `tests/app_runtime.rs`; this filter runs it to confirm the new `StorageConfig` field and the optional cache wiring do not break runtime assembly.

Expected: PASS.

- [ ] **Step 8: Commit the runtime wiring slice**

```bash
git add crates/scorpio-core/src/config.rs crates/scorpio-core/src/data/transcript_cache.rs crates/scorpio-core/src/app/mod.rs crates/scorpio-core/tests/app_runtime.rs
git commit -m "feat(runtime): wire transcript cache configuration"
```

### Task 7: Update docs, run full verification, and do a manual smoke pass

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: Update `AGENTS.md` storage section**

Update the storage/migration section in `AGENTS.md` to describe:

- `crates/scorpio-core/migrations/` (existing, flat) for snapshot migrations
- `crates/scorpio-core/migrations/transcript_cache/` (new subdirectory) for cache migrations
- `SnapshotStore` continues to use `sqlx::migrate!()` (defaults to `migrations/`); `TranscriptCacheStore` uses `sqlx::migrate!("migrations/transcript_cache")`
- The dedicated cache file `~/.scorpio-analyst/transcript_cache.db`, overridable via `SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH`
- `AlphaVantageClient::cache_failure_count` is exposed in its `Debug` impl as the operator-facing signal for chronic cache **write** failure (storage layer falls back to direct API calls on failure). Read-side failures (corrupt payload, query error) emit sanitized `warn!` logs but do not increment this counter — a `cache_failure_count == 0` does NOT mean the cache is fully healthy, only that writes are succeeding

Keep the wording short and factual. This is a doc-accuracy pass, not a narrative rewrite. `CLAUDE.md` does not need a parallel update — the convention applies to one new subdirectory in one crate's storage layer, not project-wide.

- [ ] **Step 2: Run the required repository-wide verification commands**

Run, in order:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

Expected: all three commands exit 0.

- [ ] **Step 3: Run a manual smoke check with debug logging**

Use any symbol that currently resolves to a published transcript in your environment. `AAPL` is a reasonable starting point if your local config has the required keys.

First run:

```bash
RUST_LOG=debug cargo run -p scorpio-cli -- analyze AAPL
```

Expected: one `transcript cache miss` log the first time that transcript is fetched.

Second run:

```bash
RUST_LOG=debug cargo run -p scorpio-cli -- analyze AAPL
```

Expected: a `transcript cache hit` log for the same internally resolved quarter, with the external transcript API call skipped.

If you need to force a refetch during testing:

```bash
rm ~/.scorpio-analyst/transcript_cache.db
```

- [ ] **Step 4: Commit the docs/verification slice**

```bash
git add AGENTS.md
git commit -m "docs(storage): document transcript cache"
```

## Execution Handoff

- Implement this plan with `@superpowers:subagent-driven-development`.
- Keep the commits in task order; do not batch Chunk 1 into one giant commit.
- After the last task, run `@ce:review` before opening a PR.
- After merge readiness, capture the store/migration pattern with `@ce:compound` so the next SQLite-backed cache does not repeat this archaeology.
