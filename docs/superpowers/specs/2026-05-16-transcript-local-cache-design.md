# Earnings Call Transcript Local Cache

## Goal

Cache earnings call transcripts locally in SQLite so that repeated analyses for the same symbol/quarter skip the Alpha Vantage API call entirely. Transcripts are immutable once published (a 2025Q1 transcript never changes), making them ideal for permanent local caching.

## Background

- `AlphaVantageClient` fetches transcripts from Alpha Vantage's `EARNINGS_CALL_TRANSCRIPT` API on every run.
- Free tier is 25 requests/day per key. Repeated analyses for the same symbol burn quota unnecessarily.
- No persistent data caching exists in the codebase — all in-memory caches are session-scoped and lost on process exit.
- The project already uses SQLite for workflow snapshots (`~/.scorpio-analyst/phase_snapshots.db`) via `SnapshotStore` + `sqlx` + migrations.
- Transcripts are keyed by `(symbol, quarter)` where quarter is `"YYYYQN"` format (e.g., `"2025Q1"`).

## Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Storage | Separate SQLite DB (`~/.scorpio-analyst/transcript_cache.db`) | Clean separation from snapshot lifecycle; independent schema evolution |
| Cache population | On-demand (check cache → miss → fetch API → store) | Simplest; no new CLI commands needed |
| Expiry | None — transcripts are immutable once published | No TTL complexity; a 2025Q1 transcript never changes |
| Integration point | Inside `AlphaVantageClient` | `TranscriptProvider` trait unchanged; callers unaffected |
| Cacheable states | `Found` and `NotPublished` only | `Throttled`/`Unavailable` are transient — next run should retry. Note: if Alpha Vantage retroactively publishes a transcript for a quarter previously returned as `NotPublished`, the cached `NotPublished` won't refresh. This is acceptable — worst case, user clears the cache manually or runs again after the new transcript appears. |
| Crate location | New `data/transcript_cache.rs` in `scorpio-core` | Follows existing data module structure |

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

New migration: `crates/scorpio-core/migrations/0003_create_transcript_cache.sql` (verify no intervening migration exists before implementing)

```sql
CREATE TABLE IF NOT EXISTS transcript_cache (
    symbol       TEXT NOT NULL,
    quarter      TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    cached_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (symbol, quarter)
);

```

- `payload_json` stores serialized `TranscriptFetch` enum (Found or NotPublished only)
- Primary key on `(symbol, quarter)` ensures one cache entry per symbol+quarter and implicitly creates an index — no separate index needed

### 2. TranscriptCacheStore

New file: `crates/scorpio-core/src/data/transcript_cache.rs`

```rust
pub struct TranscriptCacheStore {
    pool: SqlitePool,
}

impl TranscriptCacheStore {
    /// Open or create the cache database at the given path.
    /// Runs migrations on every open.
    pub async fn new(db_path: &str) -> Result<Self, TradingError>;

    /// Look up a cached transcript. Returns None if no entry exists.
    pub async fn get(&self, symbol: &str, quarter: &str) -> Option<TranscriptFetch>;

    /// Store a transcript fetch result. Only caches Found and NotPublished;
    /// Throttled and Unavailable are silently skipped.
    pub async fn put(
        &self,
        symbol: &str,
        quarter: &str,
        fetch: &TranscriptFetch,
    ) -> Result<(), TradingError>;

    /// Construct from user config, defaulting path to
    /// ~/.scorpio-analyst/transcript_cache.db
    pub async fn from_config(config: &Config) -> Result<Self, TradingError>;
}
```

`TranscriptCacheStore` is `Send + Sync` (required for `AlphaVantageClient` usage across async boundaries). It wraps `SqlitePool` which is already `Clone`, so `TranscriptCacheStore` can also derive `Clone` if needed.

Path resolution follows `SnapshotStore` pattern:
- Default: `~/.scorpio-analyst/transcript_cache.db`
- `expand_path()` handles `~/` and `$HOME/` prefixes
- Creates parent directory if it doesn't exist
- Opens with `?mode=rwc` SQLite pragma

### 3. AlphaVantageClient Integration

**Constructor change** in `crates/scorpio-core/src/data/alpha_vantage.rs`:

```rust
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
2. Log DEBUG "transcript cache miss: {symbol} {quarter}"
3. Call Alpha Vantage API (existing logic unchanged)
4. If result is Found or NotPublished:
   a. cache.put(symbol, quarter, &result).await
   b. Log any put errors at WARN level (non-fatal, proceed with result)
5. Return result
```

- Cache errors are non-fatal: if `get()` or `put()` fails, fall through to API call
- `for_test()` sets `cache` to `None` internally (the underlying struct gains a `cache: Option<TranscriptCacheStore>` field initialized to `None` in the test constructor) — no DB dependency in unit tests
- `TranscriptProvider` trait is unchanged — caching is fully internal

### 4. Wiring

**StorageConfig extension** in `crates/scorpio-core/src/config.rs`:

```rust
pub struct StorageConfig {
    pub snapshot_db_path: String,           // existing
    #[serde(default = "default_transcript_cache_db_path")]
    pub transcript_cache_db_path: String,   // NEW
}
```

Default value: `"~/.scorpio-analyst/transcript_cache.db"`

`#[serde(default = "...")]` is required for backward compatibility — existing user config files won't have this field, and without a default, deserialization would fail on upgrade.

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

- `DEBUG` log on cache hit: `"transcript cache hit: {symbol} {quarter}"`
- `DEBUG` log on cache miss: `"transcript cache miss: {symbol} {quarter}, fetching from Alpha Vantage"`
- `WARN` log on cache put failure: `"transcript cache put failed: {error}"` (non-fatal)
- No new metrics or dashboard changes

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

| Scenario | Behavior |
|---|---|
| Cache DB open fails | Log WARN, disable cache (`cache = None`), proceed without caching |
| Cache read fails | Log WARN, fall through to API call |
| Cache write fails | Log WARN, return API result to caller (non-fatal) |
| Corrupted cache entry | Deserialization fails → treat as cache miss, re-fetch from API |
| API returns Throttled | Not cached; next run retries the API |
| API returns Unavailable | Not cached; next run retries the API |

## Testing

- **Unit tests**: `TranscriptCacheStore` with in-memory SQLite (`":memory:"`)
  - `put` + `get` round-trip for `Found` and `NotPublished`
  - `put` silently skips `Throttled` and `Unavailable`
  - `get` returns `None` for missing entries
  - Overwrite semantics: `put` with same `(symbol, quarter)` replaces existing
  - Corrupted cache entry: insert malformed JSON → `get` returns `None` (treats as miss)
- **Integration test**: `AlphaVantageClient` with mock HTTP + real cache DB (tempfile)
  - First call hits API, second call returns cached (verify API called only once)
  - Cache error falls through to API gracefully
- **`for_test()`**: Verify `None` cache doesn't break existing test patterns

## Files Changed

| File | Change |
|---|---|
| `crates/scorpio-core/migrations/0003_create_transcript_cache.sql` | New migration |
| `crates/scorpio-core/src/data/transcript_cache.rs` | New `TranscriptCacheStore` |
| `crates/scorpio-core/src/data/mod.rs` | Add `pub mod transcript_cache;` |
| `crates/scorpio-core/src/data/alpha_vantage.rs` | Add `cache` field, modify constructor and `fetch_transcript` |
| `crates/scorpio-core/src/config.rs` | Add `transcript_cache_db_path` to `StorageConfig` |
