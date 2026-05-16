# Earnings Call Transcript Local Cache

## Goal

Cache earnings call transcripts locally in SQLite so that repeated analyses for the same symbol/quarter skip the Alpha Vantage API call entirely. Published transcripts are **stable in practice** — once Alpha Vantage publishes a quarter, the payload almost never changes — making them well suited to permanent local caching. The rare exceptions (vendor-side corrections, re-segmented turns) are handled by manual cache invalidation: deleting `~/.scorpio-analyst/transcript_cache.db` forces a re-fetch on the next run.

## Background

- `AlphaVantageClient` fetches transcripts from Alpha Vantage's `EARNINGS_CALL_TRANSCRIPT` API on every run.
- Free tier is 25 requests/day per key, **shared globally across all Alpha Vantage premium endpoints** (transcript, fundamentals, news, insider, etc.). A single `scorpio analyze` run can consume several of those 25 calls. Caching transcripts removes them from this shared budget on subsequent runs, leaving headroom for the other AV endpoints.
- No persistent data caching exists in the codebase — all in-memory caches are session-scoped and lost on process exit.
- The project already uses SQLite for workflow snapshots (`~/.scorpio-analyst/phase_snapshots.db`) via `SnapshotStore` + `sqlx` + migrations.
- Transcripts are keyed by `(symbol, quarter)` where quarter is `"YYYYQN"` format (e.g., `"2025Q1"`). The existing `TranscriptProvider::fetch_transcript` trait names this parameter `as_of_date`, but it carries the same quarter identifier — the cache stores it as `quarter` for clarity.

## Design Decisions

| Decision          | Choice                                                                                                        | Rationale                                                                                                                                                                                                                                                                                                                      |
|-------------------|---------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Storage backend   | SQLite (reusing the project's existing persistence stack)                                                     | Project already depends on `sqlx` for `SnapshotStore`. Flat-file kv (one JSON per `{symbol}_{quarter}`) was considered but rejected: SQLite gives atomic writes, queryability (for footprint reporting), and reuses existing patterns rather than introducing a second storage abstraction.                                    |
| Database file     | Separate SQLite DB (`~/.scorpio-analyst/transcript_cache.db`)                                                 | `phase_snapshots.db` is debug/audit state and may be wiped between runs (e.g., user clears snapshots to force a fresh pipeline). The transcript cache must survive that wipe — its whole value is durability across runs. Splitting the file makes that boundary explicit and lets each store evolve its schema independently. |
| Cache population  | On-demand (check cache → miss → fetch API → store)                                                            | Simplest; no new CLI commands needed.                                                                                                                                                                                                                                                                                          |
| Expiry            | None for `Found` entries; quarter-age gate for `NotPublished` entries (see Cacheable states)                  | Published transcripts are stable in practice. If a correction is ever shipped, the user invalidates with `rm ~/.scorpio-analyst/transcript_cache.db`.                                                                                                                                                                          |
| Integration point | Inside `AlphaVantageClient`                                                                                   | `TranscriptProvider` trait unchanged; callers unaffected.                                                                                                                                                                                                                                                                      |
| Cacheable states  | `Found` always; `NotPublished` only when the quarter ended **≥ 90 days ago**; `Throttled`/`Unavailable` never | Earnings calls typically publish within ~90 days of quarter end. Caching a recent `NotPublished` would trap the user: the transcript could land tomorrow and they'd never see it. Caching an old `NotPublished` (e.g., 2018Q3 with no transcript) is safe because retroactive publication after a year is vanishingly rare.    |
| Crate location    | New `data/transcript_cache.rs` in `scorpio-core`                                                              | Follows existing data module structure.                                                                                                                                                                                                                                                                                        |

## Architecture

```
                    ┌─────────────────────────┐
                    │    AlphaVantageClient    │
                    │                          │
 fetch_transcript() │   ┌───────────────────┐  │
 ──────────────────►│   │ TranscriptCache   │  │
                    │   │ Store (SQLite)    │  │
                    │   └────────┬──────────┘  │
                    │            │              │
                    │    ┌───────▼──────┐       │
                    │    │ Cache HIT?   │       │
                    │    └──┬───────┬───┘       │
                    │   yes │       │ no        │
                    │       ▼       ▼           │
                    │   Return   Alpha Vantage  │
                    │   cached   API call       │
                    │            │              │
                    │            ▼              │
                    │   Store in cache          │
                    │   (if Found/NotPublished) │
                    │            │              │
                    │            ▼              │
                    │        Return             │
                    └──────────────────────────-┘
```

## Components

### 1. Database Schema

**Migration directory layout must split per store.** `sqlx::migrate!()` (no-arg form) resolves to `CARGO_MANIFEST_DIR/migrations/` and applies *every* `.sql` file there. Today both `0001_create_phase_snapshots.sql` and `0002_add_symbol_and_schema_version.sql` live at the crate root's `migrations/`. Adding a transcript migration there would (a) run it against `phase_snapshots.db` on next `SnapshotStore` open, and (b) re-run the snapshot migrations against `transcript_cache.db`. The fix is per-store subdirectories with explicit paths in each store's macro call:

```
crates/scorpio-core/migrations/
├── snapshots/
│   ├── 0001_create_phase_snapshots.sql        # moved, byte-identical (preserves sqlx checksum)
│   └── 0002_add_symbol_and_schema_version.sql # moved, byte-identical
└── transcript_cache/
    └── 0001_create_transcript_cache.sql       # new (renumbered to 0001 for this store)
```

- `SnapshotStore::new` updates its macro call to `sqlx::migrate!("migrations/snapshots").run(&pool).await?`.
- `TranscriptCacheStore::new` calls `sqlx::migrate!("migrations/transcript_cache").run(&pool).await?`.
- Existing `phase_snapshots.db` users are unaffected: sqlx tracks applied migrations in `_sqlx_migrations` by version + checksum, so byte-identical moved files are recognized as already-applied.

```sql
-- crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql
CREATE TABLE IF NOT EXISTS transcript_cache (
    symbol         TEXT    NOT NULL,
    quarter        TEXT    NOT NULL CHECK (quarter GLOB '[0-9][0-9][0-9][0-9]Q[1-4]'),
    payload_json   TEXT    NOT NULL,
    schema_version INTEGER NOT NULL,
    cached_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (symbol, quarter)
);

```

- `payload_json` stores the serialized `TranscriptFetch` enum (`Found` always; `NotPublished` only per the Cacheable states rule).
- `schema_version` records the `TRANSCRIPT_CACHE_SCHEMA_VERSION` constant in effect when the entry was written. On read, rows whose version does not match the current constant are skipped as cache misses (see Section 2). This follows the `THESIS_MEMORY_SCHEMA_VERSION` pattern documented in `CLAUDE.md` for `TradingState` evolution: bump the constant whenever `TranscriptFetch` or any field reachable from it gains a non-`#[serde(default)]` field, is renamed, or has its type changed in a backward-incompatible way.
- Primary key on `(symbol, quarter)` ensures one cache entry per symbol+quarter and implicitly creates an index — no separate index needed.
- `CHECK (quarter GLOB ...)` enforces the `YYYYQN` format at the storage boundary so a caller passing `"2025q1"` or `"2025-Q1"` fails fast instead of silently double-storing.

### 2. TranscriptCacheStore

New file: `crates/scorpio-core/src/data/transcript_cache.rs`

```rust
/// Bump whenever `TranscriptFetch` or any field reachable from it changes shape
/// in a backward-incompatible way (rename, type change, non-`#[serde(default)]` field).
/// Rows with a different version are skipped on read and treated as cache misses.
pub const TRANSCRIPT_CACHE_SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
pub struct TranscriptCacheStore {
    pool: SqlitePool,
}

impl TranscriptCacheStore {
    /// Open or create the cache database at the given path (default if `None`).
    /// Creates the parent directory if it does not exist.
    /// Applies `journal_mode=WAL` and `busy_timeout=5000` pragmas, then runs
    /// `sqlx::migrate!("migrations/transcript_cache")` on every open.
    pub async fn new(db_path: Option<&Path>) -> Result<Self, TradingError>;

    /// Look up a cached transcript. Returns `None` if no entry exists OR if the
    /// stored `schema_version` does not match `TRANSCRIPT_CACHE_SCHEMA_VERSION`
    /// OR if `payload_json` fails to deserialize. Version-mismatch and
    /// deserialize-failure paths emit a WARN log so silent quota burn is visible.
    pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch>;

    /// Store a transcript fetch result. Cacheable states (see Design Decisions):
    ///   - `Found`        — always cached.
    ///   - `NotPublished` — cached only when `quarter_end_date(quarter) + 90 days < today`.
    ///   - `Throttled` / `Unavailable` — silently skipped.
    /// Writes `schema_version = TRANSCRIPT_CACHE_SCHEMA_VERSION` alongside payload.
    pub async fn put(
        &self,
        symbol: &str,
        quarter: &str,
        fetch: &TranscriptFetch,
    ) -> Result<(), TradingError>;

    /// Construct from user config. Reads `config.storage.transcript_cache_db_path`,
    /// expands `~/` and `$HOME/` via the shared `expand_path()` helper, and
    /// delegates to `Self::new(Some(&expanded))`. Returns an error if the path
    /// cannot be opened — callers (e.g., the app facade) typically `.ok()` this
    /// and proceed with `cache = None` so the pipeline still runs.
    pub async fn from_config(config: &Config) -> Result<Self, TradingError>;
}

/// Returns true when the named quarter ended ≥ 90 days before `today` (in UTC).
/// Used to decide whether a `NotPublished` result is safe to cache permanently.
fn quarter_is_old_enough_to_cache_not_published(quarter: &str, today: NaiveDate) -> bool;
```

`TranscriptCacheStore` derives `Clone` (its only field is `SqlitePool`, which is internally `Arc`-backed and cheaply clonable) and is `Send + Sync` via `SqlitePool`. The runtime constructs one cache per process and clones it into `AlphaVantageClient`. The `new(db_path: Option<&Path>)` signature mirrors `SnapshotStore::new` for consistency.

Path resolution and connection setup follow the `SnapshotStore` pattern:
- Default: `~/.scorpio-analyst/transcript_cache.db`.
- `expand_path()` (the same helper used by `SnapshotStore`) handles `~/` and `$HOME/` prefixes.
- Parent directory is created if absent.
- Pool is opened with `?mode=rwc`, then `PRAGMA journal_mode=WAL` and `PRAGMA busy_timeout=5000` are applied so two concurrent `scorpio analyze` processes can coexist without one falling into the "cache open fails → disable cache" branch under contention.

### 3. AlphaVantageClient Integration

**Breaking constructor signature change** in `crates/scorpio-core/src/data/alpha_vantage.rs`. The signature change is intentional rather than a `with_cache()` builder — passing the cache (or `None`) at construction makes the wiring graph explicit and prevents "client exists but cache was never attached" footguns. Every existing caller must be updated in the same PR:

| Caller                                                             | What changes                                                                                                 |
|--------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/app/mod.rs` (production)                  | Construct `TranscriptCacheStore::from_config(&cfg).await.ok()` first, then pass into `::new(.., .., cache)`. |
| `new_with_base_url(..)` (private helper inside `alpha_vantage.rs`) | Initialize the new struct field `cache: Option<TranscriptCacheStore>` to `None`.                             |
| `AlphaVantageClient::for_test()`                                   | No signature change for callers — `for_test()` delegates to `new_with_base_url`, which sets `cache = None`.  |

```rust
// BEFORE
pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError>

// AFTER
pub fn new(
    api: &ApiConfig,
    limiter: SharedRateLimiter,
    cache: Option<TranscriptCacheStore>,  // NEW parameter
) -> Result<Self, TradingError>
```

**Fetch flow change** in `fetch_transcript()`:

```
1. If self.cache is Some:
   a. cache.get(symbol, quarter).await
   b. If Some(cached) → log DEBUG "transcript cache hit: {symbol} {quarter}"
      → return cached
2. Log DEBUG "transcript cache miss: {symbol} {quarter}, fetching from Alpha Vantage"
3. Call Alpha Vantage API (existing logic unchanged)
4. If self.cache is Some:
   a. cache.put(symbol, quarter, &result).await
      → put() decides internally what to persist:
        - Found              → write
        - Old NotPublished   → write
        - Recent NotPublished, Throttled, Unavailable → silently skip
   b. Log any put errors at WARN (sanitized, non-fatal — proceed with result)
5. Return result
```

- Cache errors are non-fatal: if `get()` or `put()` fails, fall through to API call.
- The cacheability decision lives inside `put()`, not in the caller — `AlphaVantageClient` does not need to know the quarter-age rule.
- `for_test()` sets `cache` to `None` internally. Specifically, the private `new_with_base_url()` helper (which `for_test()` delegates to) must initialize the new `cache: Option<TranscriptCacheStore>` field to `None` — no DB dependency in unit tests.
- `TranscriptProvider` trait is unchanged — caching is fully internal.

### 4. Wiring

**StorageConfig extension** in `crates/scorpio-core/src/config.rs`:

```rust
pub struct StorageConfig {
    pub snapshot_db_path: String,           // existing
    #[serde(default = "default_transcript_cache_db_path")]
    pub transcript_cache_db_path: String,   // NEW
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            snapshot_db_path: default_snapshot_db_path(),
            transcript_cache_db_path: default_transcript_cache_db_path(),
        }
    }
}

fn default_transcript_cache_db_path() -> String {
    "~/.scorpio-analyst/transcript_cache.db".to_string()
}
```

Default value: `"~/.scorpio-analyst/transcript_cache.db"`

`#[serde(default = "...")]` is required for backward compatibility — existing user config files won't have this field, and without a default, deserialization would fail on upgrade.

**Also update `Config::load_storage()`** in the same file: the existing env-var fast path early-returns `StorageConfig { snapshot_db_path }` when `SCORPIO__STORAGE__SNAPSHOT_DB_PATH` is set, which (a) will no longer compile against the new struct field and (b) would silently drop any user-configured `transcript_cache_db_path`. Honor `SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH` symmetrically, or remove the early-return and rely on the `config` crate pipeline to populate both fields.

**Runtime construction** — the `TranscriptCacheStore` is created once during runtime setup and passed to `AlphaVantageClient::new()`. This happens in the same place where `SnapshotStore` and data clients are constructed (the app facade / runtime setup in `crates/scorpio-core/src/app/`):

```rust
// In runtime construction (approximate)
let transcript_cache = TranscriptCacheStore::from_config(&config).await.ok();
let av_client = AlphaVantageClient::new(
    &config.api,
    rate_limiters.alpha_vantage(),
    transcript_cache,
)?;
```

**No config wizard changes** — the cache path uses a sensible default. Users who want a custom path can set `transcript_cache_db_path` in their config file, but this is not exposed in the setup wizard.

### 5. Observability

- `DEBUG` on cache hit: `"transcript cache hit: {symbol} {quarter}"`.
- `DEBUG` on cache miss: `"transcript cache miss: {symbol} {quarter}, fetching from Alpha Vantage"`.
- `WARN` on schema_version mismatch (treated as miss): `"transcript cache version mismatch: {symbol} {quarter} stored={stored} current={current}"`. This signal exists specifically to make silent quota burn visible after a schema bump — without it, an upgraded binary would re-fetch every cached entry with no operator hint.
- `WARN` on deserialize failure (treated as miss): `"transcript cache deserialize failed: {symbol} {quarter} error.kind={kind}"`. **Do not include the raw `serde_json` error text** — payload bytes can be echoed back via the error string. This mirrors the `phase_snapshots` thesis-lookup rule documented in `CLAUDE.md`.
- `WARN` on cache put failure: `"transcript cache put failed: {symbol} {quarter} error.kind={kind}"` (non-fatal; same sanitization rule).
- No new metrics or dashboard changes.

## Data Flow

```
scorpio analyze AAPL
  └─ Phase 0: PreflightTask
  └─ Phase 1: AnalystSyncTask
       └─ hydrate_transcript()
            └─ AlphaVantageClient::fetch_transcript("AAPL", "2025Q1")
                 ├─ TranscriptCacheStore::get("AAPL", "2025Q1")
                 │   ├─ HIT → return cached TranscriptFetch::Found(...)
                 │   └─ MISS ↓
                 ├─ HTTP GET Alpha Vantage API
                 ├─ Parse response → TranscriptFetch::Found(...)
                 ├─ TranscriptCacheStore::put("AAPL", "2025Q1", &found)
                 └─ return TranscriptFetch::Found(...)
```

Next run for AAPL in the same quarter:
```
                 ├─ TranscriptCacheStore::get("AAPL", "2025Q1")
                 │   └─ HIT → return cached (API call skipped entirely)
```

## Error Handling

| Scenario                                          | Behavior                                                                                                                                                                                                                     |
|---------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Cache DB open fails                               | Log WARN (sanitized), disable cache (`cache = None`), proceed without caching                                                                                                                                                |
| Cache read fails                                  | Log WARN (sanitized), fall through to API call                                                                                                                                                                               |
| Cache write fails                                 | Log WARN (sanitized), return API result to caller (non-fatal)                                                                                                                                                                |
| `schema_version` mismatch on read                 | Log WARN, treat as cache miss, re-fetch and overwrite with current version                                                                                                                                                   |
| Corrupted cache entry (payload deserialize fails) | Log WARN with `error.kind` only (no raw error text), treat as cache miss, re-fetch and overwrite                                                                                                                             |
| API returns `Throttled`                           | Not cached; next run retries the API                                                                                                                                                                                         |
| API returns `Unavailable`                         | Not cached; next run retries the API                                                                                                                                                                                         |
| Concurrent processes both open the DB             | WAL mode + 5s `busy_timeout` lets both proceed; migrations are recorded in `_sqlx_migrations` so the second opener sees them as already-applied. Concurrent first-fetches both hit the API (no singleflight — out of scope). |
| Recent `NotPublished` returned by API             | Not cached (quarter ended < 90 days ago); next run re-fetches in case AV publishes                                                                                                                                           |
| Old `NotPublished` returned by API                | Cached permanently (quarter ended ≥ 90 days ago); user invalidates via `rm ~/.scorpio-analyst/transcript_cache.db` if AV retroactively publishes                                                                             |

## Storage Footprint

The cache grows unbounded by design — there is no eviction policy and no `scorpio cache prune` CLI subcommand (intentionally out of scope; users invalidate by deleting the file). Back-of-envelope ceiling for a worst-case power user:

| Symbols analyzed | Quarters per symbol | Avg transcript JSON | Total           |
|------------------|---------------------|---------------------|-----------------|
| 500              | 20 (~5 years)       | ~50 KB              | **~500 MB**     |
| 50               | 20                  | ~50 KB              | ~50 MB          |
| Typical (1–10)   | 4–8                 | ~50 KB              | < 5 MB          |

500 MB is the conservative upper bound; realistic single-analyst usage lands well under that. If a user ever wants to reclaim space (or invalidate after a suspected AV correction), the recovery path is a single command:

```
rm ~/.scorpio-analyst/transcript_cache.db
```

The next `scorpio analyze` re-creates the file via the migration path.

## Testing

- **Unit tests**: `TranscriptCacheStore` with in-memory SQLite (`":memory:"`)
  - `put` + `get` round-trip for `Found` and old-quarter `NotPublished`
  - `put` silently skips `Throttled` and `Unavailable`
  - `put` silently skips `NotPublished` when the quarter ended < 90 days ago (inject a clock so the test is deterministic)
  - `put` writes `NotPublished` when the quarter ended ≥ 90 days ago
  - `get` returns `None` for missing entries
  - Overwrite semantics: `put` with same `(symbol, quarter)` replaces existing
  - Corrupted cache entry: insert malformed JSON → `get` returns `None` (treats as miss)
  - `schema_version` mismatch: insert a row with `schema_version = TRANSCRIPT_CACHE_SCHEMA_VERSION + 1` → `get` returns `None` (treats as miss)
  - `CHECK` constraint: `put` with `quarter = "2025q1"` fails fast at the DB layer
  - `quarter_is_old_enough_to_cache_not_published`: pure-function unit tests across quarter boundaries
- **Integration test**: `AlphaVantageClient` with mock HTTP + real cache DB (tempfile)
  - First call hits API, second call returns cached (verify API called only once)
  - Cache error falls through to API gracefully
  - Two stores opened concurrently against the same tempfile both succeed (WAL + busy_timeout)
- **`for_test()`**: Verify `None` cache doesn't break existing test patterns

## Files Changed

| File                                                                                          | Change                                                                                                                                                                            |
|-----------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/migrations/snapshots/0001_create_phase_snapshots.sql`                    | **Moved** from `migrations/0001_...`. Byte-identical to preserve the sqlx checksum so existing `phase_snapshots.db` files keep working                                            |
| `crates/scorpio-core/migrations/snapshots/0002_add_symbol_and_schema_version.sql`             | **Moved** from `migrations/0002_...`. Byte-identical                                                                                                                              |
| `crates/scorpio-core/migrations/transcript_cache/0001_create_transcript_cache.sql`            | New migration (per-store numbering starts at 0001)                                                                                                                                |
| `crates/scorpio-core/src/workflow/snapshot/store.rs` (or wherever `SnapshotStore::new` lives) | Update `sqlx::migrate!()` to `sqlx::migrate!("migrations/snapshots")`                                                                                                             |
| `crates/scorpio-core/src/data/transcript_cache.rs`                                            | New `TranscriptCacheStore` + `TRANSCRIPT_CACHE_SCHEMA_VERSION` constant + `quarter_is_old_enough_to_cache_not_published` helper                                                   |
| `crates/scorpio-core/src/data/mod.rs`                                                         | Add `pub mod transcript_cache;`                                                                                                                                                   |
| `crates/scorpio-core/src/data/alpha_vantage.rs`                                               | Add `cache: Option<TranscriptCacheStore>` field, modify `new` and `new_with_base_url`, modify `fetch_transcript`                                                                  |
| `crates/scorpio-core/src/config.rs`                                                           | Add `transcript_cache_db_path` to `StorageConfig`, add `default_transcript_cache_db_path()`, extend `StorageConfig::default()`, update `Config::load_storage()` env-var fast path |
| `crates/scorpio-core/src/app/mod.rs`                                                          | Construct `TranscriptCacheStore::from_config(&cfg).await.ok()` (best-effort, WARN on failure) and pass into `AlphaVantageClient::new()`                                           |
| `CLAUDE.md`                                                                                   | Document the per-store migration directory convention so future stores follow it                                                                                                  |
