# Earnings Call Transcript Local Cache Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist stable Alpha Vantage transcript results in a dedicated local SQLite cache so repeated analyses for the same symbol and quarter skip the external API call entirely.

**Architecture:** Add a new `TranscriptCacheStore` in `scorpio-core::data` that owns its own SQLite database, migrations, schema-version gating, and cacheability rules. Keep caching fully internal to `AlphaVantageClient` by injecting an optional store at construction time, so the `TranscriptProvider` trait and all downstream callers remain unchanged; runtime startup should degrade gracefully to uncached API calls if the cache cannot be opened.

**Tech Stack:** Rust 2024, `sqlx` SQLite migrations, `serde`/`serde_json`, `chrono`, `reqwest`, `tokio`, `tracing`, existing `Config`/`AnalysisRuntime`/`SharedRateLimiter` patterns.

---

## Read First

- `docs/superpowers/specs/2026-05-16-transcript-local-cache-design.md`
- `AGENTS.md`
- `CLAUDE.md`
- `.github/instructions/rust.instructions.md`

## Preconditions

- Work in a dedicated worktree.
- Keep Rust/code changes inside `crates/scorpio-core`; the only planned edits outside it are the final doc updates in `CLAUDE.md` and `AGENTS.md`. `scorpio-cli` should stay untouched.
- Follow `@superpowers:test-driven-development` discipline for each task.
- After implementation, run `@ce:review`, then capture the migration/cache pattern in `@ce:compound`.
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

This is one coherent subsystem: a transcript cache store plus the `AlphaVantageClient` wiring that consumes it. Do not split it further unless you intentionally land the migration-directory refactor as a preparatory PR.

Out of scope for this plan:

- New CLI commands such as `scorpio cache prune`
- TTL/eviction logic for published transcripts
- Changes to `TranscriptProvider` or transcript prompt rendering
- A generic shared storage abstraction for every SQLite-backed store

## File Structure

| File                                                                               | Action | Responsibility                                                                                                                                                                          |
|------------------------------------------------------------------------------------|--------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/migrations/snapshots/0001_create_phase_snapshots.sql`         | Move   | Preserve the existing snapshot migration byte-for-byte so sqlx checksums stay valid after the per-store directory split                                                                 |
| `crates/scorpio-core/migrations/snapshots/0002_add_symbol_and_schema_version.sql`  | Move   | Same as above for the second snapshot migration                                                                                                                                         |
| `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql` | Create | Define the transcript cache table keyed by `(symbol, quarter)` with payload JSON and schema version                                                                                     |
| `crates/scorpio-core/src/workflow/snapshot.rs`                                     | Modify | Point `SnapshotStore::new` at `migrations/snapshots` instead of the crate-root migration directory                                                                                      |
| `crates/scorpio-core/src/data/transcript_cache.rs`                                 | Create | Own `TranscriptCacheStore`, `TRANSCRIPT_CACHE_SCHEMA_VERSION`, quarter-age logic, uppercase symbol normalization, SQLite open/setup, cache read/write behavior, and focused store tests |
| `crates/scorpio-core/src/data/mod.rs`                                              | Modify | Expose the new `transcript_cache` module                                                                                                                                                |
| `crates/scorpio-core/src/data/alpha_vantage.rs`                                    | Modify | Add optional cache wiring, cache hit/miss flow, and integration-style cache tests using the private test constructor                                                                    |
| `crates/scorpio-core/src/config.rs`                                                | Modify | Add `StorageConfig::transcript_cache_db_path`, remove the snapshot-only storage fast path, and extend storage tests                                                                     |
| `crates/scorpio-core/src/settings.rs`                                              | Modify | Extend the user-config boundary so `Config::load_from_user_path()` can preserve nested storage overrides from `~/.scorpio-analyst/config.toml`                                          |
| `crates/scorpio-core/src/app/mod.rs`                                               | Modify | Construct the transcript cache best-effort and pass it into `AlphaVantageClient::new` only when Alpha Vantage is enabled                                                                |
| `crates/scorpio-core/tests/app_runtime.rs`                                         | Modify | Fix the `StorageConfig` struct literal after the new storage field is added                                                                                                             |
| `CLAUDE.md`                                                                        | Modify | Document the per-store migration directory convention and transcript cache database                                                                                                     |
| `AGENTS.md`                                                                        | Modify | Keep the agent instructions aligned with the new migration layout and cache store boundary                                                                                              |

Notes for the implementing engineer:

- Keep `TranscriptCacheStore` as a single focused file unless it grows well past what is comfortable to review in one pass.
- Do not introduce a `TranscriptCache` trait. The cache has one production implementation and one caller.
- Keep the cacheability decision inside `TranscriptCacheStore::put()`. `AlphaVantageClient` should only know hit, miss, and best-effort writeback.
- Keep cache hits ahead of `self.rate_limiter.acquire().await` so a local hit does not spend rate-limit budget or request latency.
- The runtime config seam matters: `Config::load_from_user_path()` is rebuilt from `settings::PartialConfig`, so nested `[storage]` data from the user config file will be dropped unless this plan explicitly adds a storage-preserving path.
- Normalize cache keys to uppercase at the storage boundary. `validate_symbol()` preserves caller casing in this repo, so without explicit normalization `AAPL` and `aapl` would double-store and miss the cache.

## Chunk 1: Persistence Layer

### Task 1: Split SQLite migrations by store and add cache bootstrap

**Files:**
- Create: `crates/scorpio-core/src/data/transcript_cache.rs`
- Create: `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql`
- Move: `crates/scorpio-core/migrations/0001_create_phase_snapshots.sql` -> `crates/scorpio-core/migrations/snapshots/0001_create_phase_snapshots.sql`
- Move: `crates/scorpio-core/migrations/0002_add_symbol_and_schema_version.sql` -> `crates/scorpio-core/migrations/snapshots/0002_add_symbol_and_schema_version.sql`
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs` (`SnapshotStore::new`)
- Modify: `crates/scorpio-core/src/data/mod.rs`
- Test: `crates/scorpio-core/src/data/transcript_cache.rs`
- Regression test: `crates/scorpio-core/src/workflow/snapshot/tests/path.rs`
- Regression test: `crates/scorpio-core/src/workflow/snapshot/tests/core_roundtrip.rs`

- [ ] **Step 1: Export the new module first so the red test will actually compile into the crate**

Before writing the first test, add the module declaration in `crates/scorpio-core/src/data/mod.rs`:

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

Expected: FAIL with a compile error about `TranscriptCacheStore` / `transcript_cache` not existing yet.

- [ ] **Step 4: Move the snapshot migrations without editing their contents**

Create the new `snapshots/` directory and move the SQL files byte-for-byte. Use `git mv` so history stays readable:

```bash
git mv crates/scorpio-core/migrations/0001_create_phase_snapshots.sql crates/scorpio-core/migrations/snapshots/0001_create_phase_snapshots.sql
git mv crates/scorpio-core/migrations/0002_add_symbol_and_schema_version.sql crates/scorpio-core/migrations/snapshots/0002_add_symbol_and_schema_version.sql
```

Then update `SnapshotStore::new` in `crates/scorpio-core/src/workflow/snapshot.rs` to use the explicit migration path:

```rust
sqlx::migrate!("migrations/snapshots")
    .run(&pool)
    .await
```

- [ ] **Step 5: Add the new transcript-cache migration**

Create `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql` with the exact table shape from the spec:

```sql
CREATE TABLE IF NOT EXISTS transcript_cache (
    symbol         TEXT    NOT NULL,
    quarter        TEXT    NOT NULL CHECK (quarter GLOB '[0-9][0-9][0-9][0-9]Q[1-4]'),
    payload_json   TEXT    NOT NULL,
    schema_version INTEGER NOT NULL,
    cached_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (symbol, quarter)
);
```

- [ ] **Step 6: Add the minimal `TranscriptCacheStore` bootstrap implementation**

In `crates/scorpio-core/src/data/transcript_cache.rs`, add only enough code to open the database, apply the two SQLite pragmas, run the new migration, and satisfy the bootstrap test:

```rust
pub const TRANSCRIPT_CACHE_SCHEMA_VERSION: i64 = 1;

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

Also add a private `resolve_db_path()` helper in the same file. Mirror `SnapshotStore` path validation rules; do not extract a shared generic helper for two call sites.

- [ ] **Step 7: Run the bootstrap and snapshot regression tests again**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(new_creates_transcript_cache_table) | test(parent_directory_is_created) | test(save_and_load_round_trip)'
```

Expected: PASS.

- [ ] **Step 8: Commit the bootstrap slice**

```bash
git add crates/scorpio-core/migrations/snapshots/0001_create_phase_snapshots.sql crates/scorpio-core/migrations/snapshots/0002_add_symbol_and_schema_version.sql crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/data/transcript_cache.rs
git commit -m "refactor(storage): split snapshot and transcript migrations"
```

### Task 2: Implement transcript cache write policy and quarter-age logic

**Files:**
- Modify: `crates/scorpio-core/src/data/transcript_cache.rs`
- Test: `crates/scorpio-core/src/data/transcript_cache.rs`

- [ ] **Step 1: Write the failing write-policy tests**

Add these tests to `crates/scorpio-core/src/data/transcript_cache.rs`:

```rust
#[tokio::test]
async fn put_and_get_round_trip_found() {
    let store = test_store().await;
    let fetch = sample_found("AAPL", "2025Q1");

    store.put("AAPL", "2025Q1", &fetch).await.expect("put");

    assert_eq!(store.get("AAPL", "2025Q1").await, Some(fetch));
}

#[tokio::test]
async fn put_and_get_normalize_symbol_case() {
    let store = test_store().await;
    let fetch = sample_found("AAPL", "2025Q1");

    store.put("aapl", "2025Q1", &fetch).await.expect("put");

    assert_eq!(store.get("AAPL", "2025Q1").await, Some(fetch.clone()));
    assert_eq!(store.get("aapl", "2025Q1").await, Some(fetch));
}

#[tokio::test]
async fn put_caches_old_not_published() {
    let store = test_store().await;

    store
        .put_with_today(
            "AAPL",
            "2025Q1",
            &TranscriptFetch::NotPublished,
            NaiveDate::from_ymd_opt(2026, 5, 16).unwrap(),
        )
        .await
        .expect("put");

    assert_eq!(
        store.get("AAPL", "2025Q1").await,
        Some(TranscriptFetch::NotPublished)
    );
}

#[tokio::test]
async fn put_skips_recent_not_published_throttled_and_unavailable() {
    let store = test_store().await;

    store
        .put_with_today(
            "AAPL",
            "2026Q1",
            &TranscriptFetch::NotPublished,
            NaiveDate::from_ymd_opt(2026, 5, 16).unwrap(),
        )
        .await
        .expect("put");
    store
        .put("AAPL", "2025Q2", &TranscriptFetch::Throttled)
        .await
        .expect("put");
    store
        .put("AAPL", "2025Q3", &TranscriptFetch::Unavailable)
        .await
        .expect("put");

    assert!(store.get("AAPL", "2026Q1").await.is_none());
    assert!(store.get("AAPL", "2025Q2").await.is_none());
    assert!(store.get("AAPL", "2025Q3").await.is_none());
}

#[test]
fn quarter_is_old_enough_to_cache_not_published_handles_boundaries() {
    let today = NaiveDate::from_ymd_opt(2026, 5, 16).unwrap();

    assert!(quarter_is_old_enough_to_cache_not_published("2025Q4", today));
    assert!(!quarter_is_old_enough_to_cache_not_published("2026Q1", today));
}
```

Add a small local `test_store()` helper and `sample_found()` helper to keep the test module readable.

- [ ] **Step 2: Run the write-policy tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_and_get_round_trip_found) | test(put_caches_old_not_published) | test(put_skips_recent_not_published_throttled_and_unavailable) | test(quarter_is_old_enough_to_cache_not_published_handles_boundaries)'
```

Expected: FAIL because `put`, `put_with_today`, and the quarter-age helper are still missing.

- [ ] **Step 3: Add the minimal quarter parser and age helper**

Keep the logic private to this file. Do not introduce a shared date helper for one cache store.

```rust
fn quarter_is_old_enough_to_cache_not_published(quarter: &str, today: NaiveDate) -> bool {
    let Some((year, quarter_num)) = parse_quarter(quarter) else {
        return false;
    };

    let quarter_end = match quarter_num {
        1 => NaiveDate::from_ymd_opt(year, 3, 31).unwrap(),
        2 => NaiveDate::from_ymd_opt(year, 6, 30).unwrap(),
        3 => NaiveDate::from_ymd_opt(year, 9, 30).unwrap(),
        4 => NaiveDate::from_ymd_opt(year, 12, 31).unwrap(),
        _ => return false,
    };

    quarter_end
        .checked_add_days(chrono::Days::new(90))
        .is_some_and(|threshold| threshold <= today)
}
```

Use byte-level parsing for `parse_quarter()`; do not bring in a regex crate for a six-character format check.

Add one exact-threshold assertion so the implementation matches the spec's `>= 90 days ago` rule:

```rust
#[test]
fn quarter_is_old_enough_to_cache_not_published_is_inclusive_at_ninety_days() {
    let today = NaiveDate::from_ymd_opt(2025, 6, 29).unwrap();
    assert!(quarter_is_old_enough_to_cache_not_published("2025Q1", today));
}
```

- [ ] **Step 4: Implement cache writes with the policy owned by `put()`**

Add `put()` and a small `put_with_today()` test seam so the quarter-age tests stay deterministic:

```rust
pub async fn put(
    &self,
    symbol: &str,
    quarter: &str,
    fetch: &TranscriptFetch,
) -> Result<(), TradingError> {
    self.put_with_today(symbol, quarter, fetch, Utc::now().date_naive())
        .await
}

async fn put_with_today(
    &self,
    symbol: &str,
    quarter: &str,
    fetch: &TranscriptFetch,
    today: NaiveDate,
) -> Result<(), TradingError> {
    validate_quarter_for_storage(quarter)?;
    let normalized_symbol = symbol.to_ascii_uppercase();

    let should_cache = match fetch {
        TranscriptFetch::Found(_) => true,
        TranscriptFetch::NotPublished => {
            quarter_is_old_enough_to_cache_not_published(quarter, today)
        }
        TranscriptFetch::Throttled | TranscriptFetch::Unavailable => false,
    };

    if !should_cache {
        return Ok(());
    }

    let payload_json = serde_json::to_string(fetch)
        .with_context(|| "failed to serialize transcript cache payload")
        .map_err(TradingError::Storage)?;

    sqlx::query(
        "INSERT INTO transcript_cache (symbol, quarter, payload_json, schema_version)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(symbol, quarter) DO UPDATE SET
             payload_json = excluded.payload_json,
             schema_version = excluded.schema_version,
             cached_at = datetime('now')",
    )
    .bind(&normalized_symbol)
    .bind(quarter)
    .bind(&payload_json)
    .bind(TRANSCRIPT_CACHE_SCHEMA_VERSION)
    .execute(&self.pool)
    .await
    .with_context(|| format!("failed to write transcript cache entry {symbol} {quarter}"))
    .map_err(TradingError::Storage)?;

    Ok(())
}
```

Add a small `validate_quarter_for_storage()` helper that enforces the canonical `YYYYQN` shape before cacheability gating. This is required so malformed input like `"2025q1"` fails fast even for `TranscriptFetch::NotPublished`, instead of being silently skipped before the SQL `CHECK` constraint is reached.

Also normalize the symbol with `to_ascii_uppercase()` before every SQL read/write bind so the cache key is canonical regardless of caller casing.

- [ ] **Step 5: Implement the happy-path `get()` read**

Replace the bootstrap stub with a real lookup:

```rust
pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch> {
    let normalized_symbol = symbol.to_ascii_uppercase();
    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT payload_json, schema_version FROM transcript_cache
         WHERE symbol = ? AND quarter = ?",
    )
    .bind(&normalized_symbol)
    .bind(quarter)
    .fetch_optional(&self.pool)
    .await
    .ok()??;

    let (payload_json, _schema_version) = row;
    serde_json::from_str(&payload_json).ok()
}
```

This is intentionally only the happy path. The next task will harden version mismatch, deserialization failure, and query-failure behavior.

- [ ] **Step 6: Run the write-policy tests again**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_and_get_round_trip_found) | test(put_caches_old_not_published) | test(put_skips_recent_not_published_throttled_and_unavailable) | test(quarter_is_old_enough_to_cache_not_published_handles_boundaries)'
```

Expected: PASS.

- [ ] **Step 7: Commit the write-policy slice**

```bash
git add crates/scorpio-core/src/data/transcript_cache.rs
git commit -m "feat(data): add transcript cache write policy"
```

### Task 3: Harden transcript cache reads, overwrite semantics, and test seams

**Files:**
- Modify: `crates/scorpio-core/src/data/transcript_cache.rs`
- Test: `crates/scorpio-core/src/data/transcript_cache.rs`

- [ ] **Step 1: Write the failing resilience tests**

Add these tests to `crates/scorpio-core/src/data/transcript_cache.rs`:

```rust
#[tokio::test]
async fn put_overwrites_existing_entry() {
    let store = test_store().await;

    store
        .put("AAPL", "2025Q1", &sample_found("AAPL", "2025Q1"))
        .await
        .expect("first put");
    store
        .put_with_today(
            "AAPL",
            "2025Q1",
            &TranscriptFetch::NotPublished,
            NaiveDate::from_ymd_opt(2026, 5, 16).unwrap(),
        )
        .await
        .expect("second put");

    assert_eq!(
        store.get("AAPL", "2025Q1").await,
        Some(TranscriptFetch::NotPublished)
    );
}

#[tokio::test]
async fn get_treats_corrupt_payload_as_cache_miss() {
    let store = test_store().await;

    sqlx::query(
        "INSERT INTO transcript_cache (symbol, quarter, payload_json, schema_version)
         VALUES (?, ?, ?, ?)",
    )
    .bind("AAPL")
    .bind("2025Q1")
    .bind("not valid json")
    .bind(TRANSCRIPT_CACHE_SCHEMA_VERSION)
    .execute(&store.pool)
    .await
    .expect("seed row");

    assert!(store.get("AAPL", "2025Q1").await.is_none());
}

#[tokio::test]
async fn get_treats_version_mismatch_as_cache_miss() {
    let store = test_store().await;
    let payload = serde_json::to_string(&sample_found("AAPL", "2025Q1")).unwrap();

    sqlx::query(
        "INSERT INTO transcript_cache (symbol, quarter, payload_json, schema_version)
         VALUES (?, ?, ?, ?)",
    )
    .bind("AAPL")
    .bind("2025Q1")
    .bind(payload)
    .bind(TRANSCRIPT_CACHE_SCHEMA_VERSION + 1)
    .execute(&store.pool)
    .await
    .expect("seed row");

    assert!(store.get("AAPL", "2025Q1").await.is_none());
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

Use an explicitly old quarter in this test. The point is to confirm ordinary upsert semantics for a cacheable `NotPublished` row, not to bless downgrading `Found` over a recent non-cacheable miss.

- [ ] **Step 2: Run the resilience tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_overwrites_existing_entry) | test(get_treats_corrupt_payload_as_cache_miss) | test(get_treats_version_mismatch_as_cache_miss) | test(put_rejects_invalid_quarter_at_storage_boundary) | test(two_store_instances_can_open_the_same_db_path)'
```

Expected: FAIL because `get()` still silently `ok()`s everything. Some other tests in this group may already pass from earlier tasks; the important red signal here is that corrupt rows and version-mismatch rows are not yet treated as sanitized cache misses.

- [ ] **Step 3: Harden `get()` so cache failures degrade to misses with sanitized WARN logs**

Replace the happy-path implementation with explicit handling for query errors, schema-version mismatch, and deserialization failure:

```rust
pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch> {
    let normalized_symbol = symbol.to_ascii_uppercase();
    let row = match sqlx::query_as::<_, (String, i64)>(
        "SELECT payload_json, schema_version FROM transcript_cache
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

    let Some((payload_json, stored_version)) = row else {
        return None;
    };

    if stored_version != TRANSCRIPT_CACHE_SCHEMA_VERSION {
        warn!(
            symbol,
            quarter,
            stored = stored_version,
            current = TRANSCRIPT_CACHE_SCHEMA_VERSION,
            "transcript cache version mismatch"
        );
        return None;
    }

    match serde_json::from_str::<TranscriptFetch>(&payload_json) {
        Ok(fetch) => Some(fetch),
        Err(_err) => {
            warn!(symbol, quarter, error.kind = "deserialize", "transcript cache deserialize failed");
            None
        }
    }
}
```

Do not log raw `serde_json` error text.

- [ ] **Step 4: Add the test-only store failure seam**

Mirror the existing `SnapshotStore::close_for_test()` pattern so `AlphaVantageClient` tests can force cache read/write failures without adding a trait abstraction:

```rust
#[cfg(test)]
pub(crate) async fn close_for_test(&self) {
    self.pool.close().await;
}
```

This seam is preparation for Chunk 2's client-level cache-fallback tests; it is not required for the failure expectation in this chunk.

- [ ] **Step 5: Re-run the resilience tests and one earlier happy-path test**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(put_and_get_round_trip_found) | test(put_and_get_normalize_symbol_case) | test(put_overwrites_existing_entry) | test(get_treats_corrupt_payload_as_cache_miss) | test(get_treats_version_mismatch_as_cache_miss) | test(put_rejects_invalid_quarter_at_storage_boundary) | test(two_store_instances_can_open_the_same_db_path)'
```

Expected: PASS.

- [ ] **Step 6: Commit the hardened store slice**

```bash
git add crates/scorpio-core/src/data/transcript_cache.rs
git commit -m "feat(data): harden transcript cache reads"
```

## Chunk 2: Alpha Vantage Integration

### Task 4: Add optional cache wiring to `AlphaVantageClient`

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

- [ ] **Step 3: Add the optional cache field and update constructors**

Modify the struct and constructor signatures in `crates/scorpio-core/src/data/alpha_vantage.rs`:

```rust
pub struct AlphaVantageClient {
    key: SecretString,
    rate_limiter: SharedRateLimiter,
    http: reqwest::Client,
    base_url: String,
    cache: Option<TranscriptCacheStore>,
    // existing counters...
}

pub fn new(
    api: &ApiConfig,
    limiter: SharedRateLimiter,
    cache: Option<TranscriptCacheStore>,
) -> Result<Self, TradingError>

fn new_with_base_url(
    key: SecretString,
    limiter: SharedRateLimiter,
    base_url: String,
    cache: Option<TranscriptCacheStore>,
) -> Self
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
- Do not increment `found_count` / `not_published_count` on cache hits; those counters should continue to describe provider outcomes, not local reads.
- Do not add a `with_cache()` builder. The constructor parameter makes call sites explicit and prevents partial wiring.

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

### Task 5: Cache successful API results and ignore cache failures

**Files:**
- Modify: `crates/scorpio-core/src/data/alpha_vantage.rs`
- Regression seam: `crates/scorpio-core/src/data/transcript_cache.rs` (`close_for_test` already added in Task 3)
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
}

#[tokio::test]
async fn fetch_transcript_uses_api_when_cache_is_unavailable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("transcript-cache.db");
    let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
    cache.close_for_test().await;
    let calls = Arc::new(AtomicUsize::new(0));

    let base_url = spawn_transcript_server(
        r#"{"symbol":"AAPL","quarter":"2025Q1","transcript":[]}"#,
        Arc::clone(&calls),
    );

    let client = AlphaVantageClient::new_with_base_url(
        SecretString::from("test-dummy-key"),
        SharedRateLimiter::disabled("test"),
        base_url,
        Some(cache),
    );

    let result = client.fetch_transcript("AAPL", "2025Q1").await.expect("api fallback");

    assert_eq!(result, TranscriptFetch::NotPublished);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
```

The first test must exercise the primary `Found` path, not just `NotPublished`. That is the main steady-state cache win this feature exists to provide, and unlike `NotPublished` it is not date-sensitive.

- [ ] **Step 2: Run the integration-style tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(fetch_transcript_caches_found_results_after_first_api_call) | test(fetch_transcript_uses_api_when_cache_is_unavailable)'
```

Expected: FAIL because `fetch_transcript()` still returns the API result without writing it back to the cache.

- [ ] **Step 3: Add best-effort cache writeback after the API result is known**

At the end of `fetch_transcript()`, after `outcome` is computed and before the final `Ok(outcome)`, add:

```rust
if let Some(cache) = &self.cache {
    if let Err(_err) = cache.put(symbol, as_of_date, &outcome).await {
        warn!(
            symbol,
            quarter = as_of_date,
            error.kind = "storage",
            "transcript cache put failed"
        );
    }
}
```

Do not move the quarter-age rule into this client. `put()` already owns it.

- [ ] **Step 4: Add the tiny loopback server helper in the test module**

Use the pattern already present in `crates/scorpio-cli/src/cli/update.rs` rather than adding a new mocking library:

```rust
fn spawn_transcript_server(body: &'static str, calls: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
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
git commit -m "feat(alpha-vantage): cache transcript fetch results"
```

## Chunk 3: Runtime Wiring, Docs, and Verification

### Task 6: Add transcript cache configuration and runtime wiring

**Files:**
- Modify: `crates/scorpio-core/src/config.rs`
- Modify: `crates/scorpio-core/src/settings.rs`
- Modify: `crates/scorpio-core/src/data/transcript_cache.rs`
- Modify: `crates/scorpio-core/src/app/mod.rs`
- Modify: `crates/scorpio-core/tests/app_runtime.rs`
- Test: `crates/scorpio-core/src/config.rs`
- Test: `crates/scorpio-core/src/settings.rs`

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
fn load_storage_reads_both_storage_paths_from_user_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    let home = tempfile::tempdir().expect("temp home");
    let config_dir = home.path().join(".scorpio-analyst");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[storage]
snapshot_db_path = "/tmp/report.db"
transcript_cache_db_path = "/tmp/transcript-cache.db"

[llm]
quick_thinking_provider = "definitely-invalid-provider"
"#,
    )
    .expect("write config");

    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let result = Config::load_storage();
    unsafe {
        std::env::remove_var("HOME");
    }

    let storage = result.expect("storage-only loading should ignore unrelated runtime fields");
    assert_eq!(storage.snapshot_db_path, "/tmp/report.db");
    assert_eq!(storage.transcript_cache_db_path, "/tmp/transcript-cache.db");
}

#[test]
fn load_from_user_path_preserves_storage_config_from_user_file() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
quick_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_provider = "openai"
deep_thinking_model = "o3"

[storage]
snapshot_db_path = "/tmp/report.db"
transcript_cache_db_path = "/tmp/transcript-cache.db"
"#,
    )
    .unwrap();

    let cfg = Config::load_from_user_path(&path).expect("config should load");

    assert_eq!(cfg.storage.snapshot_db_path, "/tmp/report.db");
    assert_eq!(cfg.storage.transcript_cache_db_path, "/tmp/transcript-cache.db");
}
```

- [ ] **Step 2: Run the storage-config tests to verify failure**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(storage_config_transcript_cache_defaults_to_tilde_path) | test(storage_config_transcript_cache_can_be_overridden_via_env) | test(load_storage_reads_both_storage_paths_from_user_config) | test(load_from_user_path_preserves_storage_config_from_user_file)'
```

Expected: FAIL because `StorageConfig::transcript_cache_db_path` does not exist yet and `load_from_user_path()` does not currently preserve nested `[storage]` config from the user file.

- [ ] **Step 3: Extend `StorageConfig` and simplify `Config::load_storage()`**

In `crates/scorpio-core/src/config.rs`, add the new field and default:

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

Also update `impl Default for StorageConfig`.

Then remove the snapshot-only early return from `Config::load_storage()` instead of extending it. The builder already deserializes `StorageOnlyConfig`; deleting the special case keeps both storage fields symmetrical and avoids another one-off branch.

- [ ] **Step 4: Preserve nested `[storage]` values when loading full runtime config from the user file**

`Config::load_from_user_path()` currently loads `settings::PartialConfig` and then rebuilds runtime config from `partial_to_nested_toml_non_secrets()`, which only emits `[llm]` and `[providers]`. If you do nothing, a user file with `[storage] transcript_cache_db_path = ...` will still be dropped in production `Config::load()`.

Use the minimal fix:

- extend `crate::settings::PartialConfig` with two new flat optional fields:

```rust
pub snapshot_db_path: Option<String>,
pub transcript_cache_db_path: Option<String>,
```

- extend `UserConfigFile` with a nested storage section so manually-authored user config files can deserialize correctly:

```rust
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
struct UserConfigStorage {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    snapshot_db_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    transcript_cache_db_path: Option<String>,
}
```

- add `storage: UserConfigStorage` to `UserConfigFile` with `#[serde(default, skip_serializing_if = "UserConfigStorage::is_empty")]`
- map `UserConfigFile.storage.*` to and from the new flat `PartialConfig` fields in the existing `From<UserConfigFile> for PartialConfig` and `From<&PartialConfig> for UserConfigFile` impls
- teach `partial_to_nested_toml_non_secrets()` to emit a `[storage]` table when either value is present
- add a focused round-trip test in `crates/scorpio-core/src/settings.rs` for the new fields

For the user-config boundary, keep `PartialConfig` flat because the setup flow and existing tests already reason about a flat struct. The persisted TOML may contain a nested `[storage]` section, just like it already contains nested `[providers.*]` sections.

Keep this storage support narrow. Do not redesign the whole settings format.

- [ ] **Step 5: Add `TranscriptCacheStore::from_config()`**

Back in `crates/scorpio-core/src/data/transcript_cache.rs`, add:

```rust
pub async fn from_config(config: &Config) -> Result<Self, TradingError> {
    let path = crate::config::expand_path(&config.storage.transcript_cache_db_path);
    Self::new(Some(&path)).await
}
```

Keep this thin. Do not move unrelated config logic into the store.

- [ ] **Step 6: Wire the cache into `AnalysisRuntime::new()` only when Alpha Vantage is enabled**

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

Do not create the store outside the Alpha Vantage branch. Without an Alpha Vantage key, transcript enrichment is already disabled, so eagerly opening the cache is wasted work and can emit pointless warnings.

- [ ] **Step 7: Fix the `StorageConfig` struct literal in the runtime integration test**

Update `crates/scorpio-core/tests/app_runtime.rs` to use the new defaulted field rather than spelling both fields manually:

```rust
storage: StorageConfig {
    snapshot_db_path: "/dev/null/scorpio-phase-snapshots.db".to_owned(),
    ..StorageConfig::default()
},
```

This keeps the test focused on snapshot-store failure instead of transcript-cache configuration noise.

- [ ] **Step 8: Add the `settings.rs` round-trip test for the new storage fields**

In `crates/scorpio-core/src/settings.rs`, add:

```rust
#[test]
fn storage_paths_round_trip_through_user_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut partial = PartialConfig::default();
    partial.snapshot_db_path = Some("/tmp/report.db".into());
    partial.transcript_cache_db_path = Some("/tmp/transcript-cache.db".into());

    save_user_config_at(&partial, &path).expect("save");
    let loaded = load_user_config_at(&path).expect("load");

    assert_eq!(loaded.snapshot_db_path, Some("/tmp/report.db".into()));
    assert_eq!(
        loaded.transcript_cache_db_path,
        Some("/tmp/transcript-cache.db".into())
    );
}
```

- [ ] **Step 9: Run the focused config, runtime, and cache tests again**

Run:

```bash
cargo nextest run -p scorpio-core --all-features -E 'test(storage_config_transcript_cache_defaults_to_tilde_path) | test(storage_config_transcript_cache_can_be_overridden_via_env) | test(load_storage_reads_both_storage_paths_from_user_config) | test(load_from_user_path_preserves_storage_config_from_user_file) | test(storage_paths_round_trip_through_user_config_file) | test(new_wraps_snapshot_store_initialization_failures) | test(fetch_transcript_caches_found_results_after_first_api_call)'
```

Expected: PASS.

- [ ] **Step 10: Commit the runtime wiring slice**

```bash
git add crates/scorpio-core/src/config.rs crates/scorpio-core/src/settings.rs crates/scorpio-core/src/data/transcript_cache.rs crates/scorpio-core/src/app/mod.rs crates/scorpio-core/tests/app_runtime.rs
git commit -m "feat(runtime): wire transcript cache configuration"
```

### Task 7: Update docs, run full verification, and do a manual smoke pass

**Files:**
- Modify: `CLAUDE.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: Update the repository docs that describe SQLite stores**

In both `CLAUDE.md` and `AGENTS.md`, update the storage/migration sections so they describe:

- `crates/scorpio-core/migrations/snapshots/`
- `crates/scorpio-core/migrations/transcript_cache/`
- `SnapshotStore` using `sqlx::migrate!("migrations/snapshots")`
- `TranscriptCacheStore` using `sqlx::migrate!("migrations/transcript_cache")`
- The dedicated cache file `~/.scorpio-analyst/transcript_cache.db`

Keep the wording short and factual. This is a doc-accuracy pass, not a narrative rewrite.

- [ ] **Step 2: Run the required repository-wide verification commands**

Run, in order:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

Expected: all three commands exit 0.

- [ ] **Step 3: Run a manual smoke check with debug logging**

Use any symbol that currently resolves to a published transcript in your environment. `AAPL` is a reasonable starting point if your local config has the required keys. The quarter is resolved internally by the existing analysis flow; you are validating cache hit/miss behavior for whatever quarter that run fetches.

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
git add CLAUDE.md AGENTS.md
git commit -m "docs(storage): document transcript cache migrations"
```

## Execution Handoff

- Implement this plan with `@superpowers:subagent-driven-development`.
- Keep the commits in task order; do not batch Chunk 1 into one giant commit.
- After the last task, run `@ce:review` before opening a PR.
- After merge readiness, capture the store/migration pattern with `@ce:compound` so the next SQLite-backed cache does not repeat this archaeology.
