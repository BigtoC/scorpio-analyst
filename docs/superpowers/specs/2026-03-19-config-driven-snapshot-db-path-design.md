# Design: Config-driven `snapshot_db_path`

**Date:** 2026-03-19
**Status:** Approved

## Goal

Make the SQLite database path used by `SnapshotStore` configurable via `config.toml` and environment variables, with a sensible default of `~/.scorpio-analyst/phase_snapshots.db`. Currently the path is hardcoded inside `SnapshotStore::resolve_db_path`.

## Approach

Approach A: thin `StorageConfig` struct with a free `expand_path` helper function. The struct is plain data; path expansion happens at the call site before constructing `SnapshotStore`. No changes to `SnapshotStore` itself.

## Architecture

### `src/config.rs`

Add a new `StorageConfig` struct alongside the existing `LlmConfig`, `TradingConfig`, and `ApiConfig`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_snapshot_db_path")]
    pub snapshot_db_path: String,
}

fn default_snapshot_db_path() -> String {
    "~/.scorpio-analyst/phase_snapshots.db".to_string()
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { snapshot_db_path: default_snapshot_db_path() }
    }
}
```

Add a `storage` field to the root `Config` struct:

```rust
pub struct Config {
    // ... existing fields ...
    #[serde(default)]
    pub storage: StorageConfig,
}
```

Add a free function `expand_path(s: &str) -> PathBuf` in `config.rs`:

- If `s` starts with `~/`, replace leading `~` with the value of `$HOME` env var (fallback: current dir).
- If `s` starts with `$HOME/`, substitute the `$HOME` env var.
- Otherwise, return `PathBuf::from(s)` unchanged (handles absolute and relative paths).

The function is `pub` so `main.rs` can use it directly.

### `config.toml`

Add a `[storage]` section:

```toml
[storage]
# Path to the SQLite snapshot database. Supports ~ and $HOME expansion.
# Default: ~/.scorpio-analyst/phase_snapshots.db
snapshot_db_path = "~/.scorpio-analyst/phase_snapshots.db"
```

Env override follows the existing `config` crate convention:
`SCORPIO_STORAGE__SNAPSHOT_DB_PATH=/custom/path.db`

### Call site (`main.rs` / CLI)

```rust
let db_path = expand_path(&config.storage.snapshot_db_path);
let snapshot_store = SnapshotStore::new(Some(&db_path)).await?;
```

## Components and Responsibilities

| Component | Responsibility |
|---|---|
| `StorageConfig` | Hold raw (unexpanded) string path from config/env |
| `default_snapshot_db_path()` | Provide the serde default literal `"~/.scorpio-analyst/phase_snapshots.db"` |
| `expand_path(s)` | Resolve `~` and `$HOME` at runtime; return `PathBuf` |
| `config.toml [storage]` | Document the default; allow user override |
| `main.rs` | Read config, call `expand_path`, pass `PathBuf` to `SnapshotStore::new` |
| `SnapshotStore` | Unchanged — still accepts `Option<&Path>` |

## Data Flow

```
config.toml [storage.snapshot_db_path]
    │
    ▼
Config::load() → StorageConfig { snapshot_db_path: String }
    │
    ▼
expand_path(&config.storage.snapshot_db_path) → PathBuf
    │
    ▼
SnapshotStore::new(Some(&path)) → SnapshotStore
```

## Error Handling

- If `$HOME` is not set and the path contains `~` or `$HOME`, `expand_path` falls back to the current working directory and logs a warning via `tracing::warn!`.
- `SnapshotStore::new` already handles SQLite open errors; no new error surface is added.

## Testing

**Unit tests in `src/config.rs`** covering `expand_path`:

| Input | Expected output |
|---|---|
| `~/foo/bar` | `$HOME/foo/bar` |
| `$HOME/foo/bar` | `$HOME/foo/bar` |
| `/absolute/path.db` | `/absolute/path.db` (unchanged) |
| `relative/path.db` | `relative/path.db` (unchanged) |

Tests for `~/` and `$HOME/` cases set `HOME` env var explicitly before asserting.

**Existing tests unchanged:** All test helpers in `src/workflow/tasks.rs`, `tests/workflow_pipeline.rs`, and `tests/workflow_observability.rs` pass explicit temp paths to `SnapshotStore::new` and are not affected.

## Non-Goals

- No changes to `SnapshotStore::new` signature.
- No changes to existing test helpers.
- No new crate dependencies (home dir resolved via `std::env::var("HOME")`).
- No support for arbitrary env var substitution beyond `~` and `$HOME`.
