# Query Analysis Report by Execution ID

**Date:** 2026-05-07
**Status:** Proposed

## Problem

After running analyses, there is no way to query past reports. Users cannot list previous executions or retrieve a full report for a given execution ID. The SQLite `phase_snapshots` table stores all the data, but there is no CLI surface or public core API to read it back.

## Goal

Add two CLI commands, the supporting core API, and updated documentation:

1. `scorpio report list` — list all past executions with minimal metadata
2. `scorpio report show <ID>` — display the full report for a specific execution
3. Update `README.md` to document both new commands

## Design

### Core: New types and methods on `SnapshotStore`

**File:** `crates/scorpio-core/src/workflow/snapshot.rs` (the root file — not a submodule). Note: `snapshot.rs` has a companion `snapshot/` directory; `thesis.rs` lives there for thesis-specific queries. The new `ExecutionSummary`, `execution_exists`, `list_executions`, and `load_full_report` items are generic cross-execution queries and belong directly in `snapshot.rs` alongside `save_snapshot` and `load_snapshot`.

#### New struct

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionSummary {
    pub execution_id: uuid::Uuid,
    pub symbol: Option<String>,
    pub created_at: String,
}
```

`Serialize` is derived upfront so a future `list --json` flag requires no structural change.

#### `list_executions`

```rust
pub async fn list_executions(&self) -> Result<Vec<ExecutionSummary>, TradingError>
```

SQL:

```sql
SELECT execution_id, symbol, MIN(created_at) as created_at
FROM phase_snapshots
GROUP BY execution_id
ORDER BY MIN(created_at) DESC
LIMIT 100
```

Returns the 100 most recent executions. A `--limit N` CLI flag with a corresponding `limit: u32` parameter on this method can be added in a follow-up. `run_list` must print `(showing 100 most recent)` after the table when exactly 100 rows are returned, so users know results may be truncated.

#### `execution_exists`

```rust
pub async fn execution_exists(&self, execution_id: &str) -> Result<bool, TradingError>
```

SQL: `SELECT COUNT(*) FROM phase_snapshots WHERE execution_id = ?`. Used by `run_show` to distinguish "ID not found" from "all phases schema-version mismatched" when `load_full_report` returns an empty vec.

#### `load_full_report`

```rust
pub async fn load_full_report(
    &self,
    execution_id: &str,
) -> Result<Vec<LoadedSnapshot>, TradingError>
```

SQL:

```sql
SELECT schema_version, phase_number, trading_state_json, token_usage_json
FROM phase_snapshots
WHERE execution_id = ?
ORDER BY phase_number ASC
```

Returns all phases (up to 5) for an execution. Soft-skips rows with mismatched `schema_version` (emit `debug!`, consistent with `load_prior_thesis_for_symbol`); emits `warn!` only on deserialization failure. This is a deliberate departure from `load_snapshot` which hard-errors on mismatch — the report path is non-transactional and showing partial data is better than failing, whereas the pipeline path treats a mismatch as a real error. Returns `Ok(vec![])` if no rows match — the caller decides how to handle "not found."

**SQL binding:** `LoadedSnapshot` has only `state` and `token_usage` fields; the 4-column SELECT adds `schema_version` and `phase_number` which are needed for soft-skip logic and phase selection. Use a raw tuple `(Option<i64>, i64, String, Option<String>)` (matching `(schema_version, phase_number, trading_state_json, token_usage_json)`) for the `sqlx::query_as` binding, then construct `LoadedSnapshot` from the `trading_state_json` / `token_usage_json` columns after applying the schema-version skip check. This mirrors the tuple binding pattern in `thesis.rs`.

**`created_at` format:** The column is always RFC 3339 UTC from `Utc::now().to_rfc3339()` (see `save_snapshot`). String ordering is safe for grouping and sorting.

### CLI: New `Report` subcommand

**File:** `crates/scorpio-cli/src/cli/mod.rs`

New args:

```rust
#[derive(Debug, Clone, Args)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub subcommand: ReportSubcommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ReportSubcommand {
    /// List all past analysis executions.
    List,
    /// Show the full report for a specific execution.
    Show {
        /// Execution ID to look up.
        #[arg(value_name = "ID")]
        execution_id: String,

        /// Output raw JSON instead of the terminal report.
        #[arg(long)]
        json: bool,
    },
}
```

New variant in `Commands`:

```rust
pub enum Commands {
    Analyze(AnalyzeArgs),
    Setup,
    Upgrade,
    Report(ReportArgs),
}
```

**New file:** `crates/scorpio-cli/src/cli/report.rs`

- `pub fn run(args: &ReportArgs)` — dispatches to `run_list` or `run_show`
- `pub fn run_list()` — loads `Config` (for DB path), opens `SnapshotStore`, calls `list_executions()`, prints a formatted table to stdout
- `pub fn run_show(execution_id: &str, json: bool)` — opens `SnapshotStore`, calls `load_full_report(id)`, then:
  - If the returned vec is empty: call `execution_exists(id)`. If `false` → print "No report found for execution ID: {id}". If `true` → print "Report exists but is incompatible with the current binary (schema version mismatch).".
  - If non-empty: select the snapshot with the **highest `phase_number`** from the returned vec (i.e., `last()` on the sorted-by-`phase_number ASC` result). If `phase_number < 5`, prepend a one-line notice: `(incomplete run — N of 5 phases present)` where N = `vec.len()` (count of returned phases).
  - If `--json`: serialize the selected `TradingState` as pretty-printed JSON to stdout (the `TradingState` already contains `execution_id` and `asset_symbol` internally, so no metadata is lost)
  - Otherwise: replay the terminal report using `scorpio_reporters::terminal::render_final_report(&TradingState) -> String` (exists at `crates/scorpio-reporters/src/terminal/mod.rs`; `scorpio-reporters` is already a dependency of `scorpio-cli`)

Note: `run_list` and `run_show` are sync functions that internally build a tokio runtime (same pattern as `analyze::run`) because `SnapshotStore` methods are async. `setup::run` is fully synchronous and is not the same pattern.

**Dispatch in `main.rs`:**

```rust
Commands::Report(args) => {
    let args = args.clone();
    tokio::task::spawn_blocking(move || scorpio_cli::cli::report::run(&args))
        .await
        .map_err(|e| anyhow::anyhow!("report task failed to join: {e}"))
        .and_then(|r| r)
}
```

### Output format

**List output:**

```
Execution ID                           Symbol   Date
────────────────────────────────────── ──────── ──────────────────────
a1b2c3d4-...                           AAPL     2026-05-07T14:30:00Z
e5f6g7h8-...                           NVDA     2026-05-06T10:15:00Z
```

No `phase_count` column.

**Show output (terminal):** Uses the final phase (phase 5 / FundManager) which contains the complete `TradingState`. Passes it through the existing terminal report formatting logic.

**Show output (--json):** Pretty-prints the final `TradingState` as JSON to stdout.

### Error handling

| Scenario                                  | Behavior                                                                                                   |
|-------------------------------------------|------------------------------------------------------------------------------------------------------------|
| DB doesn't exist / can't open             | `SnapshotStore::new` returns `Err` — propagate with context                                                |
| Config file missing (no `scorpio setup`)  | `Config::load()` returns `Err` — propagate; no interactive wizard is triggered                             |
| `list_executions` on empty DB             | Return `Ok(vec![])` — print "No executions found."                                                         |
| `load_full_report` with unknown ID        | Return `Ok(vec![])` — print "No report found for execution ID: {id}"                                       |
| Schema version mismatch on some phases    | Soft-skip those phases (`debug!`), return remaining phases                                                 |
| All phases mismatched (binary upgrade)    | Return `Ok(vec![])` — print "Report exists but is incompatible with the current binary (schema mismatch)." |
| Only some phases present (incomplete run) | Return whatever phases exist — `run_show` selects highest phase and prepends an "(incomplete run)" notice  |

### Files to change

| File                                                             | Change                                                                                                          |
|------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/workflow/snapshot.rs`                   | Add `ExecutionSummary`, `execution_exists()`, `list_executions()`, `load_full_report()`                         |
| `crates/scorpio-cli/src/cli/mod.rs`                              | Add `Report` variant to `Commands`, `ReportArgs`, `ReportSubcommand` structs; add `pub mod report;` declaration |
| `crates/scorpio-cli/Cargo.toml`                                  | Add `comfy-table.workspace = true` to `[dependencies]`                                                          |
| `crates/scorpio-cli/src/cli/report.rs`                           | New file: `run`, `run_list`, `run_show`                                                                         |
| `crates/scorpio-cli/src/main.rs`                                 | Add `Commands::Report` dispatch arm                                                                             |
| `crates/scorpio-core/migrations/0003_add_execution_id_index.sql` | New file: `CREATE INDEX IF NOT EXISTS idx_phase_snapshots_execution_id ON phase_snapshots(execution_id);`       |
| `Cargo.toml` (workspace root)                                    | Add `"uuid"` to sqlx features: `features = ["runtime-tokio-rustls", "sqlite", "macros", "migrate", "uuid"]`     |
| `README.md`                                                      | Add `scorpio report list` and `scorpio report show <ID>` usage under CLI commands section                       |

### Testing

- **Core unit tests** in `snapshot.rs` (using existing `tempfile`-based pattern):
  - `list_executions` returns correct summaries, ordered by `created_at DESC`
  - `list_executions` on empty DB returns `Ok(vec![])`
  - `list_executions` returns at most 100 rows (LIMIT honored)
  - `load_full_report` returns all 5 phases for a known execution
  - `load_full_report` with unknown ID returns `Ok(vec![])`
  - `load_full_report` soft-skips phases with mismatched `schema_version` (emits `debug!`), and soft-skips phases that fail deserialization (emits `warn!` with `schema_version` and `error.kind = "deserialize"` only — no serde error text), returns remaining phases
  - `load_full_report` with all phases mismatched returns `Ok(vec![])`
  - `load_full_report` with partial phases (incomplete run) returns whatever exists
  - `execution_exists` returns `true` for a known execution ID
  - `execution_exists` returns `false` for an unknown ID
- **CLI integration test** in `crates/scorpio-cli/tests/`: verify `report list` and `report show <ID>` subcommands parse correctly (clap integration)
- **CLI integration test**: verify `report show <ID> --json` emits valid JSON that round-trips as `TradingState`
- No external services required — all tests use in-memory or temp SQLite databases
