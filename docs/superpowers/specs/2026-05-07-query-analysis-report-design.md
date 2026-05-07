# Query Analysis Report by Execution ID

**Date:** 2026-05-07
**Status:** Proposed

## Problem

After running analyses, there is no way to query past reports. Users cannot list previous executions or retrieve a full report for a given execution ID. The SQLite `phase_snapshots` table stores all the data, but there is no CLI surface or public core API to read it back.

## Goal

Add two CLI commands and the supporting core API:

1. `scorpio report list` — list all past executions with minimal metadata
2. `scorpio report show <ID>` — display the full report for a specific execution

## Design

### Core: New types and methods on `SnapshotStore`

**File:** `crates/scorpio-core/src/workflow/snapshot.rs`

#### New struct

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionSummary {
    pub execution_id: String,
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
ORDER BY created_at DESC
```

Returns all executions, most recent first. No limit.

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

Returns all phases (up to 5) for an execution. Soft-skips rows with mismatched `schema_version` (log warning, consistent with `load_prior_thesis_for_symbol`). This is a deliberate departure from `load_snapshot` which hard-errors on mismatch — the report path is non-transactional and showing partial data is better than failing, whereas the pipeline path treats a mismatch as a real error. Returns `Ok(vec![])` if no rows match — the caller decides how to handle "not found."

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

- `run(args: &ReportArgs)` — dispatches to `run_list` or `run_show`
- `run_list()` — loads `Config` (for DB path), opens `SnapshotStore`, calls `list_executions()`, prints a formatted table to stdout
- `run_show(execution_id: &str, json: bool)` — opens `SnapshotStore`, calls `load_full_report(id)`, then:
  - If `--json`: serialize the final phase's `TradingState` as pretty-printed JSON to stdout (the `TradingState` already contains `execution_id` and `asset_symbol` internally, so no metadata is lost)
  - Otherwise: replay the terminal report using the existing formatting from `scorpio_reporters::terminal::render_final_report(&TradingState) -> String`

Note: `run_list` and `run_show` are sync functions that internally build a tokio runtime (same pattern as `analyze::run` and `setup::run`) because `SnapshotStore` methods are async.

**Dispatch in `main.rs`:**

```rust
Commands::Report(args) => {
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

| Scenario                                  | Behavior                                                             |
|-------------------------------------------|----------------------------------------------------------------------|
| DB doesn't exist / can't open             | `SnapshotStore::new` returns `Err` — propagate with context          |
| `list_executions` on empty DB             | Return `Ok(vec![])` — print "No executions found."                   |
| `load_full_report` with unknown ID        | Return `Ok(vec![])` — print "No report found for execution ID: {id}" |
| Schema version mismatch on some phases    | Soft-skip those phases (log warning), return remaining phases        |
| All phases mismatched                     | Return `Ok(vec![])` — same as "not found"                            |
| Only some phases present (incomplete run) | Return whatever phases exist — the report shows what's available     |

### Files to change

| File                                           | Change                                                                              |
|------------------------------------------------|-------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/workflow/snapshot.rs` | Add `ExecutionSummary`, `list_executions()`, `load_full_report()`                   |
| `crates/scorpio-cli/src/cli/mod.rs`            | Add `Report` variant to `Commands`, add `ReportArgs` and `ReportSubcommand` structs |
| `crates/scorpio-cli/src/cli/report.rs`         | New file: `run`, `run_list`, `run_show`                                             |
| `crates/scorpio-cli/src/main.rs`               | Add `Commands::Report` dispatch arm                                                 |

### Testing

- **Core unit tests** in `snapshot.rs` (using existing `tempfile`-based pattern):
  - `list_executions` returns correct summaries, ordered by `created_at DESC`
  - `list_executions` on empty DB returns `Ok(vec![])`
  - `load_full_report` returns all 5 phases for a known execution
  - `load_full_report` with unknown ID returns `Ok(vec![])`
  - `load_full_report` soft-skips phases with mismatched `schema_version`, returns remaining
  - `load_full_report` with all phases mismatched returns `Ok(vec![])`
  - `load_full_report` with partial phases (incomplete run) returns whatever exists
- **CLI integration test** in `crates/scorpio-cli/tests/`: verify `report list` and `report show <ID>` subcommands parse correctly (clap integration)
- No external services required — all tests use in-memory or temp SQLite databases
