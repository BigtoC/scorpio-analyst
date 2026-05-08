# Query Analysis Report Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `scorpio report list` and `scorpio report show <ID>` CLI commands with supporting core API to query past analysis executions from SQLite.

**Architecture:** Extend `SnapshotStore` with two new async methods (`list_executions`, `load_full_report`). Both methods filter by the active `THESIS_MEMORY_SCHEMA_VERSION` at the SQL boundary, so callers see only viewable rows — no soft-skip / silent-filter divergence with `load_snapshot`. The CLI gets a new `report` module that renders the list via a helper exposed from `scorpio-reporters` (preserving the existing crate boundary that owns terminal presentation) and the show view via the existing `render_final_report`. `--json` emits a structured `ReportJson` payload (state + token usage + phase metadata) suitable for round-trip parsing.

**Tech Stack:** Rust, sqlx (SQLite), comfy-table (via scorpio-reporters), clap (derive), tokio

---

## Refinement Notes

This plan was refined across two review passes. Final design choices:

- **Schema filtering at SQL.** `WHERE schema_version = ?` lives in both `list_executions` and `load_full_report`. Callers see no rows from older schemas — symmetric with `load_snapshot`'s contract, no soft-skip divergence.
- **Stale-row diagnostic.** `list_executions` *also* runs a parallel COUNT query without the schema filter and returns `(visible, total)`. The CLI prints a stderr banner if `total > visible` ("N runs are not displayed: incompatible with this binary's schema version"). Closes the silent-disappear-after-bump UX cliff.
- **`load_full_report` returns `LoadedReport { snapshots, skipped_phases }`.** Corrupt rows that fail JSON deserialization are tracked so the CLI can surface a stderr warning ("phase N data was unreadable; skipped"), instead of relying on `tracing::warn!` which is invisible by default.
- **`report::run` is `async`.** Dispatched directly with `.await` from `main.rs` — no nested runtime, no `spawn_blocking`. The function has no sync deps that would justify the bridge.
- **`render_execution_list` returns the table only.** Empty-state copy ("No executions found.") moves to the CLI caller, keeping the reporter helper single-responsibility.
- **`ReportJson` includes `is_complete: bool`.** Set when `phase_number == 5`. Canonical completion check for downstream JSON consumers (avoids the "treat phase-2 partial state as final" footgun).
- **`MAX(created_at)` ordering with documented semantics.** Sorts by latest activity. Documented trade-off: for crashed runs MAX = phase-of-crash time, for complete runs MAX = FundManager time. If users want "started at" semantics later, add a `started_at` column to `ExecutionSummary`.
- **`ExecutionSummary.created_at: DateTime<Utc>` with fallback parser.** Tries RFC3339 first, then SQLite's native `YYYY-MM-DD HH:MM:SS` for legacy rows from migration 0001's `datetime('now')` default. Keeps lists working for users with mixed-format DBs.
- **`ExecutionSummary.execution_id: String`** (not `Uuid`) to match the snapshot layer's existing `&str` convention; no silent `Uuid::nil()` fallback.
- **`THESIS_MEMORY_SCHEMA_VERSION` widened to `pub`** (Task 0). Required because `ReportJson.schema_version` embeds it.
- **`LoadedReportSnapshot` is a sibling type to `LoadedSnapshot`.** Adds `phase_number` for the multi-row case without breaking existing `LoadedSnapshot` consumers (PartialEq, struct-pattern matches).
- **Dropped from the plan** vs. initial draft: `execution_exists` method (SQL filter subsumes), sqlx `uuid` feature (unused), direct `comfy-table` dep on scorpio-cli (lives in scorpio-reporters), `0003_add_execution_id_index.sql` migration (UNIQUE constraint already covers it), `LIMIT 100` cap.
- **`report` commands** load `Config` only to resolve the snapshot DB path; do not require API keys.
- **Integration test is hermetic.** Sets `HOME=<tmpdir>` to prevent inheriting the developer's `~/.scorpio-analyst/config.toml`, sets `SCORPIO__STORAGE__SNAPSHOT_DB_PATH` (the actual config field name) to point at the temp DB.

Accepted residuals (documented, not changed):

- **`render_final_report` on partial state.** If a run crashed mid-pipeline, the highest visible phase still gets rendered. Banner makes the partial-ness visible to humans; `is_complete` makes it visible to JSON consumers. If `render_final_report` produces nonsense for non-final phases in practice, replace with a stricter path then.
- **`ON CONFLICT` upsert behavior.** Re-running a phase shifts `MAX(created_at)` forward. List ordering is "last-touched" semantics, not "started" semantics. Acceptable for first cut.
- **Test fixtures using raw INSERTs** for the deserialization-failure case. Locked to migration column shape; if a future migration adds NOT NULL columns the test breaks loudly. Add a `#[cfg(test)]` helper then.

---

## File Structure

| File                                                                | Responsibility                                                           |
|---------------------------------------------------------------------|--------------------------------------------------------------------------|
| `crates/scorpio-core/src/workflow/snapshot.rs`                      | Add `ExecutionSummary` struct, `list_executions`, `load_full_report`     |
| `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs` | Unit tests for new query methods                                         |
| `crates/scorpio-reporters/src/terminal/mod.rs`                      | Add `render_execution_list` helper alongside `render_final_report`       |
| `crates/scorpio-cli/src/cli/mod.rs`                                 | Add `Report` variant, `ReportArgs`, `ReportSubcommand` structs           |
| `crates/scorpio-cli/src/cli/report.rs`                              | New file: `run`, `run_list`, `run_show`, `ReportJson`                    |
| `crates/scorpio-cli/src/main.rs`                                    | Add `Commands::Report` dispatch arm                                      |
| `crates/scorpio-cli/tests/report_json_roundtrip.rs`                 | New: end-to-end `--json` round-trip test against a temp SQLite DB        |
| `README.md`                                                         | Document new commands                                                    |

---

## Chunk 1: Core Query Methods

### Task 0: Expose `THESIS_MEMORY_SCHEMA_VERSION` to consumers

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`
- Modify: `crates/scorpio-core/src/workflow/snapshot/thesis.rs`

The CLI's `ReportJson` payload needs to embed `schema_version` so consumers know which TradingState shape they're parsing. The constant is currently `pub(crate)` and must be widened to `pub`.

- [ ] **Step 1: Make the constant public**

In `thesis.rs`, change:

```rust
pub(crate) const THESIS_MEMORY_SCHEMA_VERSION: i64 = 3;
```

to:

```rust
pub const THESIS_MEMORY_SCHEMA_VERSION: i64 = 3;
```

In `snapshot.rs`, change the re-export:

```rust
pub(crate) use thesis::THESIS_MEMORY_SCHEMA_VERSION;
```

to:

```rust
pub use thesis::THESIS_MEMORY_SCHEMA_VERSION;
```

This is a deliberate widening of the public API. It commits scorpio-core to keeping the constant visible going forward. If you would rather not widen the surface, drop `schema_version` from `ReportJson` instead and skip this task — see the Refinement Notes for the open decision.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-core`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/workflow/snapshot/thesis.rs
git commit -m "feat: expose THESIS_MEMORY_SCHEMA_VERSION for report consumers"
```

---

### Task 1: Add ExecutionSummary struct

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`

- [ ] **Step 1: Add the structs after LoadedSnapshot**

Add this block after the `LoadedSnapshot` struct definition (currently around line 76):

```rust
/// Summary of a single execution for list display.
///
/// Only includes executions whose snapshot rows match the active
/// `THESIS_MEMORY_SCHEMA_VERSION`; runs from older schemas are not visible.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionSummary {
    pub execution_id: String,
    pub symbol: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Result of `list_executions` — visible summaries plus a count of stale
/// executions filtered out by the schema-version check.
///
/// The CLI surfaces `stale_count` as a stderr banner so users notice when a
/// version bump has retired previously-visible runs.
#[derive(Debug, Clone)]
pub struct ExecutionListing {
    pub summaries: Vec<ExecutionSummary>,
    pub stale_count: usize,
}
```

`execution_id` is kept as `String` to match the `&str` storage convention used elsewhere in `SnapshotStore` (and to avoid a silent `Uuid::nil()` fallback when a row's id is malformed). `created_at` is parsed at the boundary so consumers don't re-parse and malformed timestamps surface as `TradingError::Storage`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-core`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs
git commit -m "feat: add ExecutionSummary struct for report list"
```

---

### Task 2: Add list_executions method

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`
- Create: `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`:

```rust
use super::{in_memory_store, sample_state};
use crate::workflow::snapshot::{SnapshotPhase, THESIS_MEMORY_SCHEMA_VERSION};

#[tokio::test]
async fn list_executions_returns_correct_summaries_ordered_by_latest_activity() {
    let store = in_memory_store().await;

    // Save first execution
    let state1 = sample_state();
    let exec_id1 = state1.execution_id.to_string();
    store
        .save_snapshot(&exec_id1, SnapshotPhase::AnalystTeam, &state1, None)
        .await
        .expect("save first");

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Save second execution (newer activity)
    let mut state2 = sample_state();
    state2.asset_symbol = "NVDA".to_string();
    let exec_id2 = state2.execution_id.to_string();
    store
        .save_snapshot(&exec_id2, SnapshotPhase::AnalystTeam, &state2, None)
        .await
        .expect("save second");

    let listing = store.list_executions().await.expect("list should succeed");

    assert_eq!(listing.summaries.len(), 2);
    assert_eq!(listing.stale_count, 0);
    // Most recent activity first
    assert_eq!(listing.summaries[0].symbol.as_deref(), Some("NVDA"));
    assert_eq!(listing.summaries[1].symbol.as_deref(), Some("AAPL"));
}

#[tokio::test]
async fn list_executions_on_empty_db_returns_empty_listing() {
    let store = in_memory_store().await;

    let listing = store.list_executions().await.expect("list should succeed");

    assert!(listing.summaries.is_empty());
    assert_eq!(listing.stale_count, 0);
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

    let listing = store.list_executions().await.expect("list should succeed");

    assert_eq!(listing.summaries.len(), 1, "should deduplicate by execution_id");
}

#[tokio::test]
async fn list_executions_excludes_rows_from_older_schema_versions_and_reports_stale_count() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id_current = state.execution_id.to_string();
    store
        .save_snapshot(&exec_id_current, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save current");

    // Insert two distinct stale executions directly
    let state_json = serde_json::to_string(&state).expect("serialize");
    for _ in 0..2 {
        let stale_exec_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json,
                 token_usage_json, created_at, symbol, schema_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&stale_exec_id)
        .bind(1i64)
        .bind("analyst_team")
        .bind(&state_json)
        .bind(None::<&str>)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind("AAPL")
        .bind(999i64) // stale
        .execute(&store.pool)
        .await
        .expect("insert stale");
    }

    let listing = store.list_executions().await.expect("list");

    assert_eq!(listing.summaries.len(), 1, "only current-schema rows are visible");
    assert_eq!(listing.summaries[0].execution_id, exec_id_current);
    assert_eq!(listing.stale_count, 2, "stale executions counted but not surfaced");
}

#[tokio::test]
async fn list_executions_parses_legacy_sqlite_datetime_format() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();
    let state_json = serde_json::to_string(&state).expect("serialize");

    // Insert a row with SQLite's native `datetime('now')` format (no T, no offset).
    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json,
             token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(1i64)
    .bind("analyst_team")
    .bind(&state_json)
    .bind(None::<&str>)
    .bind("2026-01-15 10:30:00") // legacy format
    .bind("AAPL")
    .bind(THESIS_MEMORY_SCHEMA_VERSION)
    .execute(&store.pool)
    .await
    .expect("insert legacy");

    let listing = store.list_executions().await.expect("list");

    assert_eq!(listing.summaries.len(), 1);
    assert_eq!(listing.summaries[0].execution_id, exec_id);
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
/// List all past executions visible to the current binary.
///
/// Returns visible summaries plus a count of executions filtered out by the
/// schema-version check. Visible rows are ordered by latest activity
/// (`MAX(created_at)`) descending. Only rows whose `schema_version` matches the
/// active `THESIS_MEMORY_SCHEMA_VERSION` are returned in `summaries`; the rest
/// are tallied into `stale_count` so the CLI can surface a stderr banner
/// instead of letting the user think the database is empty.
///
/// Ordering note: `MAX(created_at)` reflects the latest phase save. For a
/// completed run this is the FundManager save time; for a crashed run it is
/// the time of the failing phase. If "started at" semantics are needed later,
/// add a `started_at` field populated from `MIN(created_at)`.
pub async fn list_executions(&self) -> Result<ExecutionListing, TradingError> {
    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT execution_id, symbol, MAX(created_at) as latest_at
         FROM phase_snapshots
         WHERE schema_version = ?
         GROUP BY execution_id
         ORDER BY latest_at DESC",
    )
    .bind(THESIS_MEMORY_SCHEMA_VERSION)
    .fetch_all(&self.pool)
    .await
    .with_context(|| "failed to list executions")
    .map_err(TradingError::Storage)?;

    // Count distinct executions whose ALL rows fail the schema-version filter.
    let total_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT execution_id) FROM phase_snapshots",
    )
    .fetch_one(&self.pool)
    .await
    .with_context(|| "failed to count executions")
    .map_err(TradingError::Storage)?;

    let visible_count = rows.len();
    let stale_count = (total_count.0 as usize).saturating_sub(visible_count);

    let summaries = rows
        .into_iter()
        .map(|(exec_id, symbol, latest_at)| {
            let created_at = parse_snapshot_timestamp(&latest_at).with_context(|| {
                format!("failed to parse created_at='{latest_at}' for execution_id={exec_id}")
            }).map_err(TradingError::Storage)?;
            Ok(ExecutionSummary { execution_id: exec_id, symbol, created_at })
        })
        .collect::<Result<Vec<_>, TradingError>>()?;

    Ok(ExecutionListing { summaries, stale_count })
}
```

Add the timestamp helper as a free function in the same file:

```rust
/// Parse a `created_at` value from `phase_snapshots`.
///
/// Tries RFC3339 first (the format written by current production code via
/// `Utc::now().to_rfc3339()`), then falls back to SQLite's native
/// `YYYY-MM-DD HH:MM:SS` format used by migration 0001's `datetime('now')`
/// default. Returns `Err` on unrecognized formats.
fn parse_snapshot_timestamp(s: &str) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|naive| naive.and_utc())
        .map_err(|e| anyhow::anyhow!("unrecognized timestamp format '{s}': {e}"))
}
```

Note: `total_count` includes both visible and stale execution IDs but uses a single query for simplicity. An execution with a mix of current and stale rows counts as visible (since the filtered query returned at least one row); stale_count thus represents executions whose entire row set is from older schemas.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p scorpio-core --features test-helpers -- report_queries`
Expected: All 4 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs crates/scorpio-core/src/workflow/snapshot/tests.rs
git commit -m "feat: add list_executions method to SnapshotStore"
```

---

### Task 3: Add load_full_report method

**Files:**
- Modify: `crates/scorpio-core/src/workflow/snapshot.rs`
- Modify: `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs` (the `THESIS_MEMORY_SCHEMA_VERSION` import added in Task 2 is reused here):

```rust
#[tokio::test]
async fn load_full_report_returns_all_phases_for_known_execution() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

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

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(report.snapshots.len(), 5);
    assert!(report.skipped_phases.is_empty());
    // Ordered ASC by phase_number — first is AnalystTeam, last is FundManager.
    assert_eq!(report.snapshots.first().unwrap().phase_number, 1);
    assert_eq!(report.snapshots.last().unwrap().phase_number, 5);
}

#[tokio::test]
async fn load_full_report_with_unknown_id_returns_empty_report() {
    let store = in_memory_store().await;

    let report = store
        .load_full_report("non-existent-id")
        .await
        .expect("load");

    assert!(report.snapshots.is_empty());
    assert!(report.skipped_phases.is_empty());
}

#[tokio::test]
async fn load_full_report_returns_partial_phases_for_incomplete_run() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");
    store
        .save_snapshot(&exec_id, SnapshotPhase::ResearcherDebate, &state, None)
        .await
        .expect("save phase 2");

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(report.snapshots.len(), 2);
    assert!(report.skipped_phases.is_empty());
}

#[tokio::test]
async fn load_full_report_excludes_old_schema_rows() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Phase 1 with current schema
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");

    // Phase 2 with stale schema_version
    let state_json = serde_json::to_string(&state).expect("serialize");
    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json,
             token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(2i64)
    .bind("researcher_debate")
    .bind(&state_json)
    .bind(None::<&str>)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind("AAPL")
    .bind(999i64)
    .execute(&store.pool)
    .await
    .expect("insert stale");

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(report.snapshots.len(), 1, "should only return current-schema phases");
    assert_eq!(report.snapshots[0].phase_number, 1);
    assert!(report.skipped_phases.is_empty(), "stale rows are filtered, not skipped");
}

#[tokio::test]
async fn load_full_report_with_only_old_schema_rows_returns_empty_report() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // All phases with stale schema_version
    let state_json = serde_json::to_string(&state).expect("serialize");
    for phase_num in 1..=5i64 {
        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json,
                 token_usage_json, created_at, symbol, schema_version)
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

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert!(report.snapshots.is_empty(), "all-stale execution must look not-found");
}

#[tokio::test]
async fn load_full_report_tracks_phases_that_fail_deserialization() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    // Phase 1 normal
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");

    // Phase 2 with current schema_version but invalid JSON (corrupt-row scenario)
    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json,
             token_usage_json, created_at, symbol, schema_version)
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

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(report.snapshots.len(), 1, "should skip deserialization failure");
    assert_eq!(report.skipped_phases, vec![2], "corrupt phase number tracked for CLI surface");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p scorpio-core --features test-helpers -- load_full_report`
Expected: FAIL with "method `load_full_report` not found"

- [ ] **Step 3: Add `LoadedReportSnapshot` and `LoadedReport` types**

`load_full_report` returns a multi-row result that needs phase identity per row plus a record of any rows that were soft-skipped due to corruption. Rather than mutate the public `LoadedSnapshot` (a re-exported, `PartialEq`-deriving type with existing pattern-match callers), introduce sibling types:

```rust
/// Per-phase snapshot returned by `load_full_report`.
///
/// Distinct from `LoadedSnapshot` because the multi-row result needs to track
/// which phase each row corresponds to.
#[derive(Debug, Clone)]
pub struct LoadedReportSnapshot {
    pub state: TradingState,
    pub token_usage: Option<Vec<AgentTokenUsage>>,
    pub phase_number: i64,
}

/// Result of `load_full_report` — visible per-phase snapshots plus a list of
/// phase numbers that were soft-skipped due to deserialization failure.
///
/// The CLI surfaces `skipped_phases` as a stderr banner so corrupt rows are
/// visible to users instead of only appearing in `tracing::warn!` logs.
#[derive(Debug, Clone)]
pub struct LoadedReport {
    pub snapshots: Vec<LoadedReportSnapshot>,
    pub skipped_phases: Vec<i64>,
}
```

`LoadedSnapshot` is left untouched.

- [ ] **Step 4: Implement load_full_report**

Add this method to the `impl SnapshotStore` block:

```rust
/// Load all phase snapshots for a given execution ID, scoped to the active schema.
///
/// Returns visible snapshots ordered by phase_number ascending plus a list of
/// phase numbers that were soft-skipped due to deserialization failure. Rows
/// from older schema versions are filtered at the SQL boundary — they are
/// intentionally retired data, not "missing" data. An execution whose rows
/// are all stale will appear as not-found to the caller.
///
/// A failure of `token_usage_json` degrades only that phase's `token_usage` to
/// `None`; the snapshot is still returned (not added to `skipped_phases`).
pub async fn load_full_report(
    &self,
    execution_id: &str,
) -> Result<LoadedReport, TradingError> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT phase_number, trading_state_json, token_usage_json
         FROM phase_snapshots
         WHERE execution_id = ? AND schema_version = ?
         ORDER BY phase_number ASC",
    )
    .bind(execution_id)
    .bind(THESIS_MEMORY_SCHEMA_VERSION)
    .fetch_all(&self.pool)
    .await
    .with_context(|| format!("failed to load full report for execution_id={execution_id}"))
    .map_err(TradingError::Storage)?;

    let mut snapshots = Vec::with_capacity(rows.len());
    let mut skipped_phases = Vec::new();

    for (phase_number, state_json, usage_json) in rows {
        let state: TradingState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(_err) => {
                warn!(
                    execution_id,
                    phase_number,
                    error.kind = "deserialize",
                    "report snapshot failed to deserialize; skipping"
                );
                skipped_phases.push(phase_number);
                continue;
            }
        };

        let token_usage = usage_json.and_then(|json| {
            match serde_json::from_str::<Vec<AgentTokenUsage>>(&json) {
                Ok(u) => Some(u),
                Err(_err) => {
                    warn!(
                        execution_id,
                        phase_number,
                        error.kind = "deserialize",
                        "report token usage failed to deserialize; degrading to None"
                    );
                    None
                }
            }
        });

        snapshots.push(LoadedReportSnapshot { state, token_usage, phase_number });
    }

    Ok(LoadedReport { snapshots, skipped_phases })
}
```

- [ ] **Step 5: Add required import**

Ensure `warn` is imported at the top of `snapshot.rs`:

```rust
use tracing::{debug, warn};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p scorpio-core --features test-helpers -- report_queries`
Expected: All tests PASS

- [ ] **Step 7: Run full test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: All tests PASS

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/workflow/snapshot.rs crates/scorpio-core/src/workflow/snapshot/tests/report_queries.rs
git commit -m "feat: add load_full_report method to SnapshotStore"
```

---

## Chunk 2: scorpio-reporters helper

### Task 4: Add render_execution_list helper to scorpio-reporters

**Files:**
- Modify: `crates/scorpio-reporters/src/terminal/mod.rs` (the existing `terminal/` is a directory module containing `mod.rs`, `coverage.rs`, `final_report.rs`, `provenance.rs`, `valuation.rs`)

- [ ] **Step 1: Add the helper**

Expose a function symmetric to `render_final_report` for the list view. The helper renders only the table; the CLI caller decides what message to show for empty input.

```rust
use comfy_table::{Cell, Table};
use scorpio_core::workflow::snapshot::ExecutionSummary;

/// Render a list of execution summaries as a terminal table.
///
/// Always returns a comfy-table dump (header + zero or more rows). Empty-state
/// messaging is the CLI's responsibility — callers can branch on the input
/// slice before invoking this function.
pub fn render_execution_list(summaries: &[ExecutionSummary]) -> String {
    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("Execution ID"),
        Cell::new("Symbol"),
        Cell::new("Date"),
    ]);

    for summary in summaries {
        table.add_row(vec![
            Cell::new(&summary.execution_id),
            Cell::new(summary.symbol.as_deref().unwrap_or("—")),
            Cell::new(summary.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
        ]);
    }

    table.to_string()
}
```

The `Date` column is formatted via chrono's `format` rather than printing the raw RFC3339 string — gives a stable, human-friendly display regardless of how the row was originally stored.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-reporters`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-reporters/src/terminal.rs
git commit -m "feat: add render_execution_list helper to scorpio-reporters"
```

---

## Chunk 3: CLI Report Command

### Task 5: Add Report subcommand types to CLI

**Files:**
- Modify: `crates/scorpio-cli/src/cli/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/scorpio-cli/src/cli/mod.rs` (the existing test block already imports `clap::error::ErrorKind`):

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

        /// Output structured JSON instead of the terminal report.
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

### Task 6: Create report.rs with run_list, run_show, and ReportJson

**Files:**
- Create: `crates/scorpio-cli/src/cli/report.rs`

`run`, `run_list`, and `run_show` are all `async fn` and dispatched directly with `.await` from `main.rs`. No nested runtime or `spawn_blocking` — the function has zero sync deps.

- [ ] **Step 1: Create the file**

```rust
//! `scorpio report` subcommand handler.

use anyhow::Context;
use serde::{Deserialize, Serialize};

use scorpio_core::config::Config;
use scorpio_core::state::{AgentTokenUsage, TradingState};
use scorpio_core::workflow::snapshot::{SnapshotStore, THESIS_MEMORY_SCHEMA_VERSION};
use scorpio_reporters::terminal::{render_execution_list, render_final_report};

use super::{ReportArgs, ReportSubcommand};

const CONFIG_LOAD_MSG: &str =
    "✗ Failed to load configuration. Run `scorpio setup` if this is a fresh install.";

/// JSON payload emitted by `report show --json`.
///
/// Round-trippable: callers can deserialize back into this struct to drive
/// audit/replay tooling.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReportJson {
    pub execution_id: String,
    pub state: TradingState,
    pub token_usage: Option<Vec<AgentTokenUsage>>,
    /// Phase number of `state` (the highest visible phase for this execution).
    pub phase_number: i64,
    /// Total phases visible for this execution under the active schema.
    pub phases_present: usize,
    /// Whether this execution reached the final phase (FundManager).
    /// Canonical completion check — JSON consumers should branch on this rather
    /// than inspect `state` to decide whether the run is final.
    pub is_complete: bool,
    /// Schema version this payload was produced against.
    pub schema_version: i64,
}

/// Dispatch `scorpio report` subcommands.
pub async fn run(args: &ReportArgs) -> anyhow::Result<()> {
    match &args.subcommand {
        ReportSubcommand::List => run_list().await,
        ReportSubcommand::Show { execution_id, json } => run_show(execution_id, *json).await,
    }
}

/// Load only the snapshot DB path from config — report commands don't need API keys.
async fn open_store() -> anyhow::Result<SnapshotStore> {
    let cfg = Config::load().context(CONFIG_LOAD_MSG)?;
    SnapshotStore::from_config(&cfg)
        .await
        .map_err(anyhow::Error::from)
}

/// List all past analysis executions.
async fn run_list() -> anyhow::Result<()> {
    let store = open_store().await?;
    let listing = store.list_executions().await?;

    if listing.summaries.is_empty() {
        println!("No executions found.");
    } else {
        println!("{}", render_execution_list(&listing.summaries));
    }

    if listing.stale_count > 0 {
        eprintln!(
            "Note: {} run(s) are not displayed because they were created with an older schema. \
             Re-run the analysis to produce a new execution under schema version {}.",
            listing.stale_count, THESIS_MEMORY_SCHEMA_VERSION,
        );
    }

    Ok(())
}

/// Show the full report for a specific execution.
async fn run_show(execution_id: &str, json: bool) -> anyhow::Result<()> {
    let store = open_store().await?;
    let report = store.load_full_report(execution_id).await?;

    if report.snapshots.is_empty() {
        println!("No report found for execution ID: {execution_id}");
        if !report.skipped_phases.is_empty() {
            eprintln!(
                "Warning: {} phase(s) were unreadable (corrupt rows): {:?}",
                report.skipped_phases.len(),
                report.skipped_phases,
            );
        }
        return Ok(());
    }

    // Snapshots are ordered ASC by phase_number — the last is the highest visible phase.
    let phases_present = report.snapshots.len();
    let selected = report.snapshots.last().expect("non-empty vec has a last");
    let is_complete = selected.phase_number == 5;

    if json {
        let payload = ReportJson {
            execution_id: execution_id.to_string(),
            state: selected.state.clone(),
            token_usage: selected.token_usage.clone(),
            phase_number: selected.phase_number,
            phases_present,
            is_complete,
            schema_version: THESIS_MEMORY_SCHEMA_VERSION,
        };
        let out = serde_json::to_string_pretty(&payload)
            .context("failed to serialize ReportJson")?;
        println!("{out}");
    } else {
        if !is_complete {
            println!("(incomplete run — {phases_present} of 5 phases present)");
        }
        let rendered = render_final_report(&selected.state);
        println!("{rendered}");
    }

    if !report.skipped_phases.is_empty() {
        eprintln!(
            "Warning: {} phase(s) were unreadable (corrupt rows): {:?}",
            report.skipped_phases.len(),
            report.skipped_phases,
        );
    }

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p scorpio-cli`
Expected: Compiles without errors

- [ ] **Step 3: Commit**

```bash
git add crates/scorpio-cli/src/cli/report.rs
git commit -m "feat: add report.rs with async run, run_list, run_show, and ReportJson"
```

---

### Task 7: Add dispatch in main.rs

**Files:**
- Modify: `crates/scorpio-cli/src/main.rs`

- [ ] **Step 1: Add the dispatch arm**

In `main.rs`, add the `Commands::Report` arm to the match expression (after the `Commands::Upgrade` arm). Unlike `Setup` and `Analyze` (which need `spawn_blocking` to bridge sync helpers or spin up a multi-thread runtime), `report::run` is `async fn` with no sync deps and can be awaited directly:

```rust
Commands::Report(args) => scorpio_cli::cli::report::run(args).await,
```

- [ ] **Step 2: Suppress the post-run upgrade notice for `Report`**

`main.rs` has a guard `let is_upgrade = matches!(&cli.command, Commands::Upgrade);` that prevents the post-run "scorpio upgrade is available" notice for upgrade itself. `report` is a fast local query and should also skip the GitHub release check. Generalize the guard:

```rust
let skip_upgrade_notice = matches!(
    &cli.command,
    Commands::Upgrade | Commands::Report(_)
);
```

Update the corresponding `if !is_upgrade { ... }` predicate to use `skip_upgrade_notice`. Also verify `should_show_analyze_banner` does not match `Commands::Report` (it is currently scoped to `Commands::Analyze`, so this should be a no-op).

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p scorpio-cli`
Expected: Compiles without errors

- [ ] **Step 4: Run all CLI tests**

Run: `cargo test -p scorpio-cli`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-cli/src/main.rs
git commit -m "feat: add Report dispatch in main.rs and suppress upgrade notice for it"
```

---

### Task 8: End-to-end JSON round-trip integration test

**Files:**
- Create: `crates/scorpio-cli/tests/report_json_roundtrip.rs`

- [ ] **Step 1: Write the integration test**

This test populates a temp SQLite DB via `save_snapshot`, runs the CLI binary, captures stdout, and asserts the JSON deserializes back into `ReportJson`. It is the smallest end-to-end check that the `--json` contract holds.

```rust
//! End-to-end test: `scorpio report show --json` produces parseable JSON.

use std::path::PathBuf;
use std::process::Command;

use scorpio_cli::cli::report::ReportJson; // exposed via lib.rs `pub mod cli;`
use scorpio_core::state::TradingState;
use scorpio_core::workflow::snapshot::{SnapshotPhase, SnapshotStore};

#[tokio::test]
async fn report_show_json_round_trips() {
    let tmp_home = tempfile::tempdir().expect("temp home");
    let tmp_db = tempfile::NamedTempFile::new().expect("temp file");
    let db_path: PathBuf = tmp_db.path().to_path_buf();

    // Create store at the temp path and populate one execution.
    let store = SnapshotStore::new(Some(&db_path)).await.expect("open store");
    // Inline TradingState construction — `sample_state` is a private test helper
    // inside scorpio-core's snapshot tests module, not part of the public API.
    let state = TradingState::new("AAPL", "2026-01-15");
    let exec_id = state.execution_id.to_string();
    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save");

    // Drop the store handle so the binary can open the DB.
    drop(store);

    // Run the CLI binary with the temp DB.
    // HOME redirection prevents the test from inheriting the developer's
    // ~/.scorpio-analyst/config.toml — keeps the test hermetic.
    let bin = env!("CARGO_BIN_EXE_scorpio");
    let output = Command::new(bin)
        .args(["report", "show", &exec_id, "--json"])
        .env("HOME", tmp_home.path())
        .env("SCORPIO__STORAGE__SNAPSHOT_DB_PATH", &db_path)
        .output()
        .expect("run CLI");

    assert!(
        output.status.success(),
        "CLI exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: ReportJson = serde_json::from_slice(&output.stdout)
        .expect("output should round-trip through ReportJson");

    assert_eq!(parsed.execution_id, exec_id);
    assert_eq!(parsed.phase_number, 5);
    assert_eq!(parsed.phases_present, 1);
    assert!(parsed.is_complete, "phase 5 save should mark is_complete true");
}
```

The test uses `SnapshotStore::new(Some(&path))` (public API at `snapshot.rs:112`) directly — no synthesized `Config` needed. `HOME` is redirected to a temp dir to prevent the binary from reading the developer's `~/.scorpio-analyst/config.toml` and bleeding state into the test.

- [ ] **Step 2: Ensure `cli::report::ReportJson` is reachable from the integration test**

`scorpio-cli` already exposes `pub mod cli;` via `lib.rs`. Verify the `ReportJson` struct from Task 6 is `pub` and the integration test can import it as `scorpio_cli::cli::report::ReportJson`.

- [ ] **Step 3: Run the integration test**

Run: `cargo test -p scorpio-cli --test report_json_roundtrip`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-cli/tests/report_json_roundtrip.rs
git commit -m "test: end-to-end JSON round-trip for report show"
```

---

### Task 9: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: All tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt -- --check`
Expected: No formatting issues

- [ ] **Step 4: Smoke test (manual)**

```bash
cargo run -p scorpio-cli -- report list                      # no executions or table
cargo run -p scorpio-cli -- report show <some-execution-id>  # full report or "not found"
cargo run -p scorpio-cli -- report show <some-execution-id> --json | jq .
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test: verify report CLI integration tests pass"
```

---

## Chunk 4: Documentation

### Task 10: Update README.md

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add report commands to CLI usage section**

```markdown
### Report Commands

Query past analysis executions:

```bash
# List all past executions visible to the current binary
scorpio report list

# Show the full report for a specific execution
scorpio report show <EXECUTION_ID>

# Output structured JSON (round-trippable into ReportJson)
scorpio report show <EXECUTION_ID> --json
```

Notes:
- After a scorpio upgrade that bumps the snapshot schema, prior runs become
  invisible to these commands by design — re-run the analysis to produce a
  new execution under the current schema. `scorpio report list` will print a
  stderr banner indicating how many runs were retired so you know the DB is
  not actually empty.
- `scorpio report` does not require API keys — it reads only the local SQLite
  snapshot DB.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add report commands to README"
```
