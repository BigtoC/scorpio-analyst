# Query Analysis Report Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `scorpio report list` and `scorpio report show <ID>` CLI commands with supporting core API to query past analysis executions from SQLite.

**Architecture:** Extend `SnapshotStore` with three new async methods (`list_executions`, `execution_exists`, `load_full_report`) and add a new `report` CLI module that formats output using `comfy-table` for list view and the existing `render_final_report` for show view. A new migration adds an index on `execution_id` for query performance.

**Tech Stack:** Rust, sqlx (SQLite), comfy-table, clap (derive), tokio

---

## File Structure

| File                                                                | Responsibility                                                      |
|---------------------------------------------------------------------|---------------------------------------------------------------------|
| `crates/scorpio-core/src/workflow/snapshot.rs`                      | Add `ExecutionSummary` struct and three new `SnapshotStore` methods |
| `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs` | Unit tests for new query methods                                    |
| `crates/scorpio-core/migrations/0003_add_execution_id_index.sql`    | Index for execution_id lookups                                      |
| `crates/scorpio-cli/src/cli/mod.rs`                                 | Add `Report` variant, `ReportArgs`, `ReportSubcommand` structs      |
| `crates/scorpio-cli/src/cli/report.rs`                              | New file: `run`, `run_list`, `run_show`                             |
| `crates/scorpio-cli/src/main.rs`                                    | Add `Commands::Report` dispatch arm                                 |
| `crates/scorpio-cli/Cargo.toml`                                     | Add `comfy-table.workspace = true`                                  |
| `Cargo.toml` (workspace root)                                       | Add `"uuid"` to sqlx features                                       |
| `README.md`                                                         | Document new commands                                               |

---

## Chunk 1: Core Query Methods

### Task 1: Add execution_id index migration

**Files:**
- Create: `crates/scorpio-core/migrations/0003_add_execution_id_index.sql`

- [ ] **Step 1: Create the migration file**

```sql
CREATE INDEX IF NOT EXISTS idx_phase_snapshots_execution_id
    ON phase_snapshots(execution_id);
```

- [ ] **Step 2: Verify migration runs**

Run: `cargo test -p scorpio-core --features test-helpers -- snapshot`
Expected: All existing snapshot tests PASS (migration is backward-compatible)

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-core/migrations/0003_add_execution_id_index.sql
git commit -m "feat: add execution_id index migration for report queries"
```

---

### Task 2: Add ExecutionSummary struct

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`

- [ ] **Step 1: Add the struct after LoadedSnapshot**

Add this block after the `LoadedSnapshot` struct definition (line 76):

```rust
/// Summary of a single execution for list display.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionSummary {
    pub execution_id: uuid::Uuid,
    pub symbol: Option<String>,
    pub created_at: String,
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-core`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs
git commit -m "feat: add ExecutionSummary struct for report list"
```

---

### Task 3: Add list_executions method

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`
- Create: `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`:

```rust
use super::{in_memory_store, sample_state};
use crate::workflow::snapshot::SnapshotPhase;

#[tokio::test]
async fn list_executions_returns_correct_summaries_ordered_by_created_at() {
    let store = in_memory_store().await;

    // Save two executions with different timestamps
    let state1 = sample_state();
    let exec_id1 = state1.execution_id.to_string();
    store
        .save_snapshot(&exec_id1, SnapshotPhase::AnalystTeam, &state1, None)
        .await
        .expect("save first");

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let mut state2 = sample_state();
    state2.asset_symbol = "NVDA".to_string();
    let exec_id2 = state2.execution_id.to_string();
    store
        .save_snapshot(&exec_id2, SnapshotPhase::AnalystTeam, &state2, None)
        .await
        .expect("save second");

    let summaries = store.list_executions().await.expect("list should succeed");

    assert_eq!(summaries.len(), 2);
    // Most recent first
    assert_eq!(summaries[0].symbol.as_deref(), Some("NVDA"));
    assert_eq!(summaries[1].symbol.as_deref(), Some("AAPL"));
}

#[tokio::test]
async fn list_executions_on_empty_db_returns_empty_vec() {
    let store = in_memory_store().await;

    let summaries = store.list_executions().await.expect("list should succeed");

    assert!(summaries.is_empty());
}

#[tokio::test]
async fn list_executions_deduplicates_by_execution_id() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Save multiple phases for same execution
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");
    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save phase 5");

    let summaries = store.list_executions().await.expect("list should succeed");

    assert_eq!(summaries.len(), 1, "should deduplicate by execution_id");
}

#[tokio::test]
async fn list_executions_returns_at_most_100_rows() {
    let store = in_memory_store().await;

    // Insert 101 distinct executions
    for i in 0..101 {
        let mut state = sample_state();
        state.asset_symbol = format!("SYM{i}");
        let exec_id = uuid::Uuid::new_v4().to_string();
        store
            .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
            .await
            .expect("save");
    }

    let summaries = store.list_executions().await.expect("list");

    assert_eq!(summaries.len(), 100, "should cap at 100 rows");
}
```

- [ ] **Step 2: Register the test module**

Add to `crates/scorpio-core/src/workflow/snapshot/tests.rs` after `mod thesis_lookup;`:

```rust
mod report_queries;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p scorpio-core --features test-helpers -- report_queries`
Expected: FAIL with "method `list_executions` not found"

- [ ] **Step 4: Implement list_executions**

Add this method to the `impl SnapshotStore` block in `snapshot.rs` (after `load_snapshot`):

```rust
/// List all past executions with minimal metadata.
///
/// Returns the 100 most recent executions, ordered by creation date descending.
/// Each execution appears once regardless of how many phases were saved.
pub async fn list_executions(&self) -> Result<Vec<ExecutionSummary>, TradingError> {
    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT execution_id, symbol, MIN(created_at) as created_at
         FROM phase_snapshots
         GROUP BY execution_id
         ORDER BY MIN(created_at) DESC
         LIMIT 100",
    )
    .fetch_all(&self.pool)
    .await
    .with_context(|| "failed to list executions")
    .map_err(TradingError::Storage)?;

    let summaries = rows
        .into_iter()
        .map(|(exec_id, symbol, created_at)| {
            ExecutionSummary {
                execution_id: uuid::Uuid::parse_str(&exec_id).unwrap_or_else(|_| uuid::Uuid::nil()),
                symbol,
                created_at,
            }
        })
        .collect();

    Ok(summaries)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p scorpio-core --features test-helpers -- report_queries`
Expected: All 4 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs crates/scorpio-core/src/workflow/snapshot/tests.rs
git commit -m "feat: add list_executions method to SnapshotStore"
```

---

### Task 4: Add execution_exists method

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`
- Modify: `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`:

```rust
#[tokio::test]
async fn execution_exists_returns_true_for_known_id() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save");

    let exists = store.execution_exists(&exec_id).await.expect("query");

    assert!(exists);
}

#[tokio::test]
async fn execution_exists_returns_false_for_unknown_id() {
    let store = in_memory_store().await;

    let exists = store
        .execution_exists("non-existent-id")
        .await
        .expect("query");

    assert!(!exists);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p scorpio-core --features test-helpers -- execution_exists`
Expected: FAIL with "method `execution_exists` not found"

- [ ] **Step 3: Implement execution_exists**

Add this method to the `impl SnapshotStore` block in `snapshot.rs`:

```rust
/// Check whether any snapshot exists for the given execution ID.
pub async fn execution_exists(&self, execution_id: &str) -> Result<bool, TradingError> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM phase_snapshots WHERE execution_id = ?",
    )
    .bind(execution_id)
    .fetch_one(&self.pool)
    .await
    .with_context(|| format!("failed to check existence for execution_id={execution_id}"))
    .map_err(TradingError::Storage)?;

    Ok(count.0 > 0)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p scorpio-core --features test-helpers -- execution_exists`
Expected: All 2 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs
git commit -m "feat: add execution_exists method to SnapshotStore"
```

---

### Task 5: Add load_full_report method

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`
- Modify: `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`:

```rust
use crate::workflow::snapshot::THESIS_MEMORY_SCHEMA_VERSION;

#[tokio::test]
async fn load_full_report_returns_all_phases_for_known_execution() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Save all 5 phases
    for phase in [
        SnapshotPhase::AnalystTeam,
        SnapshotPhase::ResearcherDebate,
        SnapshotPhase::Trader,
        SnapshotPhase::RiskDiscussion,
        SnapshotPhase::FundManager,
    ] {
        store
            .save_snapshot(&exec_id, phase, &state, None)
            .await
            .expect("save");
    }

    let snapshots = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(snapshots.len(), 5, "should return all 5 phases");
}

#[tokio::test]
async fn load_full_report_with_unknown_id_returns_empty_vec() {
    let store = in_memory_store().await;

    let snapshots = store
        .load_full_report("non-existent-id")
        .await
        .expect("load");

    assert!(snapshots.is_empty());
}

#[tokio::test]
async fn load_full_report_returns_partial_phases_for_incomplete_run() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Only save phases 1 and 2
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");
    store
        .save_snapshot(&exec_id, SnapshotPhase::ResearcherDebate, &state, None)
        .await
        .expect("save phase 2");

    let snapshots = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(snapshots.len(), 2, "should return only saved phases");
}

#[tokio::test]
async fn load_full_report_soft_skips_mismatched_schema_version() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Save phase 1 with current schema
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");

    // Save phase 2 with mismatched schema_version
    let state_json = serde_json::to_string(&state).expect("serialize");
    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json, token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(2i64)
    .bind("researcher_debate")
    .bind(&state_json)
    .bind(None::<&str>)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind("AAPL")
    .bind(999i64) // Mismatched schema version
    .execute(&store.pool)
    .await
    .expect("insert mismatched");

    let snapshots = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(snapshots.len(), 1, "should skip mismatched phase");
}

#[tokio::test]
async fn load_full_report_with_all_phases_mismatched_returns_empty_vec() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Save all phases with wrong schema version
    let state_json = serde_json::to_string(&state).expect("serialize");
    for phase_num in 1..=5i64 {
        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json, token_usage_json, created_at, symbol, schema_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&exec_id)
        .bind(phase_num)
        .bind("test_phase")
        .bind(&state_json)
        .bind(None::<&str>)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind("AAPL")
        .bind(999i64)
        .execute(&store.pool)
        .await
        .expect("insert");
    }

    let snapshots = store.load_full_report(&exec_id).await.expect("load");

    assert!(snapshots.is_empty(), "all mismatched should return empty");
}

#[tokio::test]
async fn load_full_report_soft_skips_phases_that_fail_deserialization() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Save phase 1 normally
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");

    // Save phase 2 with valid schema_version but invalid JSON
    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json, token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(2i64)
    .bind("researcher_debate")
    .bind("{invalid json")
    .bind(None::<&str>)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind("AAPL")
    .bind(THESIS_MEMORY_SCHEMA_VERSION)
    .execute(&store.pool)
    .await
    .expect("insert invalid json");

    let snapshots = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(snapshots.len(), 1, "should skip deserialization failure");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p scorpio-core --features test-helpers -- load_full_report`
Expected: FAIL with "method `load_full_report` not found"

- [ ] **Step 3: Implement load_full_report**

Add this method to the `impl SnapshotStore` block in `snapshot.rs`:

```rust
/// Load all phase snapshots for a given execution ID.
///
/// Returns snapshots ordered by phase number ascending. Phrases with mismatched
/// schema versions are soft-skipped (emitted as `debug!` logs). Deserialization
/// failures are soft-skipped with `warn!` logs. Returns `Ok(vec![])` if no
/// compatible rows match.
pub async fn load_full_report(
    &self,
    execution_id: &str,
) -> Result<Vec<LoadedSnapshot>, TradingError> {
    let rows: Vec<(Option<i64>, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT schema_version, phase_number, trading_state_json, token_usage_json
         FROM phase_snapshots
         WHERE execution_id = ?
         ORDER BY phase_number ASC",
    )
    .bind(execution_id)
    .fetch_all(&self.pool)
    .await
    .with_context(|| format!("failed to load full report for execution_id={execution_id}"))
    .map_err(TradingError::Storage)?;

    let mut snapshots = Vec::with_capacity(rows.len());

    for (schema_version, _phase_number, state_json, usage_json) in rows {
        let schema_version = schema_version.unwrap_or(0);

        // Soft-skip mismatched schema versions
        if schema_version != THESIS_MEMORY_SCHEMA_VERSION {
            debug!(
                execution_id,
                schema_version,
                active = THESIS_MEMORY_SCHEMA_VERSION,
                "report snapshot schema version mismatch; skipping"
            );
            continue;
        }

        // Attempt deserialization
        let state: TradingState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(_err) => {
                warn!(
                    execution_id,
                    schema_version,
                    error.kind = "deserialize",
                    "report snapshot failed to deserialize; skipping"
                );
                continue;
            }
        };

        let usage = usage_json
            .and_then(|json| {
                match serde_json::from_str::<Vec<AgentTokenUsage>>(&json) {
                    Ok(u) => Some(u),
                    Err(_err) => {
                        warn!(
                            execution_id,
                            schema_version,
                            error.kind = "deserialize",
                            "report token usage failed to deserialize; skipping"
                        );
                        None
                    }
                }
            });

        snapshots.push(LoadedSnapshot {
            state,
            token_usage: usage,
        });
    }

    Ok(snapshots)
}
```

- [ ] **Step 4: Add required import**

Ensure `warn` is imported at the top of `snapshot.rs` (it should already be there from `use tracing::debug;` — add `warn` if missing):

```rust
use tracing::{debug, warn};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p scorpio-core --features test-helpers -- report_queries`
Expected: All tests PASS

- [ ] **Step 6: Run full test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs
git commit -m "feat: add load_full_report method to SnapshotStore"
```

---

## Chunk 2: CLI Report Command

### Task 6: Add uuid feature to sqlx workspace dependency

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Update sqlx features**

In the workspace `Cargo.toml`, change the sqlx line from:

```toml
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "sqlite", "macros", "migrate"] }
```

to:

```toml
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "sqlite", "macros", "migrate", "uuid"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "feat: add uuid feature to sqlx workspace dependency"
```

---

### Task 7: Add comfy-table dependency to scorpio-cli

**Files:**
- Modify: `crates/scorpio-cli/Cargo.toml`

- [ ] **Step 1: Add comfy-table dependency**

Add to the `[dependencies]` section after the `colored` line:

```toml
comfy-table.workspace = true
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-cli`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-cli/Cargo.toml
git commit -m "feat: add comfy-table dependency to scorpio-cli"
```

---

### Task 8: Add Report subcommand types to CLI

**Files:**
- Modify: `crates/scorpio-cli/src/cli/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/scorpio-cli/src/cli/mod.rs`:

```rust
#[test]
fn parse_report_list_subcommand() {
    let cli = Cli::try_parse_from(["scorpio", "report", "list"]).unwrap();
    assert!(matches!(
        &cli.command,
        Commands::Report(args) if matches!(args.subcommand, ReportSubcommand::List)
    ));
}

#[test]
fn parse_report_show_with_id() {
    let cli = Cli::try_parse_from(["scorpio", "report", "show", "abc-123"]).unwrap();
    assert!(matches!(
        &cli.command,
        Commands::Report(args) if matches!(&args.subcommand, ReportSubcommand::Show { execution_id, json: false } if execution_id == "abc-123")
    ));
}

#[test]
fn parse_report_show_with_json_flag() {
    let cli = Cli::try_parse_from(["scorpio", "report", "show", "abc-123", "--json"]).unwrap();
    assert!(matches!(
        &cli.command,
        Commands::Report(args) if matches!(&args.subcommand, ReportSubcommand::Show { json: true, .. })
    ));
}

#[test]
fn parse_report_without_subcommand_yields_error() {
    let err = Cli::try_parse_from(["scorpio", "report"]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::MissingSubcommand);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p scorpio-cli -- parse_report`
Expected: FAIL with "Report" not found in Commands enum

- [ ] **Step 3: Add the types and module declaration**

Add `pub mod report;` after `pub mod update;` at the top of `mod.rs`.

Add the new structs before the `Commands` enum:

```rust
/// Arguments for `scorpio report`.
#[derive(Debug, Clone, Args)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub subcommand: ReportSubcommand,
}

/// Subcommands for `scorpio report`.
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

Add the `Report` variant to the `Commands` enum:

```rust
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the full 5-phase analysis pipeline for a ticker symbol.
    Analyze(AnalyzeArgs),
    /// Interactive wizard to configure API keys and provider routing.
    Setup,
    /// Upgrade scorpio to the latest release from GitHub.
    Upgrade,
    /// Query past analysis executions.
    Report(ReportArgs),
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p scorpio-cli -- parse_report`
Expected: All 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-cli/src/cli/mod.rs
git commit -m "feat: add Report subcommand types to CLI"
```

---

### Task 9: Create report.rs with run_list

**Files:**
- Create: `crates/scorpio-cli/src/cli/report.rs`

- [ ] **Step 1: Create the file with run_list implementation**

```rust
//! `scorpio report` subcommand handler.

use anyhow::Context;
use comfy_table::{Cell, Table};

use scorpio_core::config::Config;
use scorpio_core::workflow::snapshot::SnapshotStore;

use super::{ReportArgs, ReportSubcommand};

const CONFIG_MISSING_MSG: &str = "✗ Config not found or incomplete. Run `scorpio setup` to configure your API keys and providers.";

/// Dispatch `scorpio report` subcommands.
pub fn run(args: &ReportArgs) -> anyhow::Result<()> {
    match &args.subcommand {
        ReportSubcommand::List => run_list(),
        ReportSubcommand::Show { execution_id, json } => run_show(execution_id, *json),
    }
}

/// List all past analysis executions.
fn run_list() -> anyhow::Result<()> {
    let cfg = Config::load().context(CONFIG_MISSING_MSG)?;

    // Use current_thread runtime: report commands are simple queries with no
    // spawned tasks needing OS threads (unlike analyze::run which uses
    // new_multi_thread for parallel reporter tasks).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize async runtime")?;

    let summaries = runtime.block_on(async {
        let store = SnapshotStore::from_config(&cfg).await?;
        store.list_executions().await
    })?;

    if summaries.is_empty() {
        println!("No executions found.");
        return Ok(());
    }

    let truncated = summaries.len() == 100;

    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("Execution ID"),
        Cell::new("Symbol"),
        Cell::new("Date"),
    ]);

    for summary in &summaries {
        table.add_row(vec![
            Cell::new(&summary.execution_id),
            Cell::new(summary.symbol.as_deref().unwrap_or("—")),
            Cell::new(&summary.created_at),
        ]);
    }

    println!("{table}");

    if truncated {
        println!("(showing 100 most recent)");
    }

    Ok(())
}

/// Show the full report for a specific execution.
fn run_show(execution_id: &str, json: bool) -> anyhow::Result<()> {
    // Placeholder — will be implemented in Task 10
    todo!("run_show not yet implemented")
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-cli`
Expected: Compiles (with warning about unreachable code in `run_show`)

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-cli/src/cli/report.rs
git commit -m "feat: add report.rs with run_list implementation"
```

---

### Task 10: Implement run_show

**Files:**
- Modify: `crates/scorpio-cli/src/cli/report.rs`

- [ ] **Step 1: Add import**

Add at the top of `report.rs`:

```rust
use scorpio_reporters::terminal::render_final_report;
```

- [ ] **Step 2: Replace the run_show placeholder**

Replace the `run_show` function with:

```rust
/// Show the full report for a specific execution.
fn run_show(execution_id: &str, json: bool) -> anyhow::Result<()> {
    let cfg = Config::load().context(CONFIG_MISSING_MSG)?;

    // Use current_thread runtime: report commands are simple queries with no
    // spawned tasks needing OS threads (unlike analyze::run which uses
    // new_multi_thread for parallel reporter tasks).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize async runtime")?;

    let (snapshots, exists) = runtime.block_on(async {
        let store = SnapshotStore::from_config(&cfg).await?;
        let snapshots = store.load_full_report(execution_id).await?;
        let exists = if snapshots.is_empty() {
            store.execution_exists(execution_id).await?
        } else {
            false
        };
        Ok::<_, anyhow::Error>((snapshots, exists))
    })?;

    if snapshots.is_empty() {
        if exists {
            println!(
                "Report exists but is incompatible with the current binary (schema version mismatch)."
            );
        } else {
            println!("No report found for execution ID: {execution_id}");
        }
        return Ok(());
    }

    // Select the snapshot with the highest phase_number (last in ASC order)
    let phase_count = snapshots.len();
    let selected = snapshots.last().expect("non-empty vec has a last");

    if phase_count < 5 {
        println!("(incomplete run — {phase_count} of 5 phases present)");
    }

    if json {
        let json_output = serde_json::to_string_pretty(&selected.state)
            .context("failed to serialize TradingState to JSON")?;
        println!("{json_output}");
    } else {
        let report = render_final_report(&selected.state);
        println!("{report}");
    }

    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p scorpio-cli`
Expected: Compiles without errors

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-cli/src/cli/report.rs
git commit -m "feat: implement run_show with terminal and JSON output"
```

---

### Task 11: Add dispatch in main.rs

**Files:**
- Modify: `crates/scorpio-cli/src/main.rs`

- [ ] **Step 1: Add the dispatch arm**

In `main.rs`, add the `Commands::Report` arm to the match expression (after the `Commands::Upgrade` arm):

```rust
Commands::Report(args) => {
    let args = args.clone();
    tokio::task::spawn_blocking(move || scorpio_cli::cli::report::run(&args))
        .await
        .map_err(|e| anyhow::anyhow!("report task failed to join: {e}"))
        .and_then(|r| r)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-cli`
Expected: Compiles without errors

- [ ] **Step 3: Run all CLI tests**

Run: `cargo test -p scorpio-cli`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-cli/src/main.rs
git commit -m "feat: add Report dispatch in main.rs"
```

---

### Task 12: Final verification and cleanup

**Files:**
- Modify: `crates/scorpio-cli/src/cli/mod.rs` (tests already added in Task 8)

- [ ] **Step 1: Run all tests to verify**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: All tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt -- --check`
Expected: No formatting issues

- [ ] **Step 4: Smoke test JSON round-trip**

Run: `cargo run -p scorpio-cli -- report show <some-execution-id> --json | jq .`
Expected: Valid JSON output that parses as a `TradingState` object

Note: A full automated round-trip test (deserializing CLI JSON output back to `TradingState`) requires a populated SQLite DB. The smoke test above validates the JSON path produces valid output. For a complete integration test, create a temp DB with `save_snapshot`, then verify `serde_json::from_str::<TradingState>()` succeeds on the CLI output.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test: verify report CLI integration tests pass"
```

---

## Chunk 3: Documentation

### Task 13: Update README.md

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add report commands to CLI usage section**

Find the CLI commands section in README.md and add:

```markdown
### Report Commands

Query past analysis executions:

```bash
# List all past executions
scorpio report list

# Show the full report for a specific execution
scorpio report show <EXECUTION_ID>

# Output raw JSON instead of terminal report
scorpio report show <EXECUTION_ID> --json
```
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add report commands to README"
```

---

## Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: All tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt -- --check`
Expected: No formatting issues

- [ ] **Step 4: Smoke test**

Run: `cargo run -p scorpio-cli -- report list`
Expected: "No executions found." (or a table if DB has data)
