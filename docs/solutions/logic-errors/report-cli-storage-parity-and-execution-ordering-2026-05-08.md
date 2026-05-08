---
title: "Report CLI Storage Parity and Execution Ordering"
date: 2026-05-08
category: docs/solutions/logic-errors
module: cli/report + workflow/snapshot
problem_type: logic_error
component: tooling
symptoms:
  - "`scorpio report` could read a different snapshot DB contract than the rest of the runtime"
  - "A malformed `~/.scorpio-analyst/config.toml` was ignored whenever `SCORPIO__STORAGE__SNAPSHOT_DB_PATH` was set"
  - "`scorpio report list` could misorder executions created within the same second"
  - "`scorpio report list` silently stopped at 100 rows even though the final plan dropped that cap"
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - database
  - testing_framework
tags:
  - report-cli
  - snapshot-store
  - config-loading
  - malformed-config
  - timestamp-ordering
  - regression-tests
  - sqlite
---

# Report CLI Storage Parity and Execution Ordering

## Problem

The `scorpio report list/show` follow-up fixes introduced a second storage-loading path and changed the snapshot listing query, but two review-driven regressions remained: malformed user config could be bypassed whenever the snapshot DB path came from `SCORPIO__STORAGE__SNAPSHOT_DB_PATH`, and `report list` lost ordering fidelity by collapsing timestamps to whole seconds. The same slice also carried forward an older 100-row listing cap even though the final implementation plan had explicitly removed it.

## Symptoms

- `scorpio report` could succeed against an env-selected snapshot DB even while `~/.scorpio-analyst/config.toml` was syntactically broken.
- `scorpio report list` could show executions in the wrong order when two runs were saved within the same second.
- The list JSON/table output stopped at 100 executions and reported truncation, even though the final plan said to return all visible executions.
- The empty-visible-report failure path for corrupt rows was easy to regress because only the not-found and schema-mismatch branches were explicitly tested.

## What Didn't Work

- Fixing the "wrong DB" problem by adding `Config::load_storage()` but returning early on `SCORPIO__STORAGE__SNAPSHOT_DB_PATH`. That restored env overrides, but it skipped malformed-TOML detection entirely.
- Switching `list_executions()` to `MAX(unixepoch(created_at))` to sort mixed RFC3339 and legacy SQLite timestamps. That fixed string-ordering bugs across formats, but it also truncated RFC3339 sub-second precision.
- Keeping the draft-spec `LIMIT 101` / `truncated` behavior after the final plan had already dropped the 100-row cap.

## Solution

The fix kept storage-only loading narrow, restored syntax-failure reporting, and moved execution ordering back to full parsed timestamps instead of second-level buckets.

### 1. Preserve malformed-config detection before honoring the storage env override

`crates/scorpio-core/src/config.rs` now validates user-config TOML syntax first, then applies the explicit snapshot-path env override:

Before:

```rust
if let Ok(snapshot_db_path) = std::env::var("SCORPIO__STORAGE__SNAPSHOT_DB_PATH") {
    return Ok(StorageConfig { snapshot_db_path });
}

if let Ok(path) = crate::settings::user_config_path() {
    let _ = crate::settings::load_user_config_at(&path)?;
    builder = builder.add_source(config::File::from(path).required(false));
}
```

After:

```rust
if let Ok(path) = crate::settings::user_config_path() {
    let _ = crate::settings::load_user_config_at(&path)?;
    builder = builder.add_source(config::File::from(path).required(false));
}

if let Ok(snapshot_db_path) = std::env::var("SCORPIO__STORAGE__SNAPSHOT_DB_PATH") {
    return Ok(StorageConfig { snapshot_db_path });
}
```

This keeps the intended contract:

- malformed TOML still fails fast with the normal `failed to parse config file` error
- unrelated runtime keys are still ignored by the storage-only path
- `SCORPIO__STORAGE__SNAPSHOT_DB_PATH` still wins over the file for the actual DB path

### 2. Order execution listings by parsed timestamps, not `unixepoch()` buckets

`crates/scorpio-core/src/workflow/snapshot.rs` no longer uses:

```sql
SELECT execution_id, MAX(symbol) as symbol, MAX(unixepoch(created_at)) as latest_epoch
...
ORDER BY latest_epoch DESC
LIMIT 101
```

Instead, it loads visible rows, parses `created_at` with one helper that supports both persisted formats, keeps the latest valid timestamp per execution in Rust, and sorts the resulting `ExecutionSummary` values by full `DateTime<Utc>` precision.

That solved both ordering problems at once:

- mixed RFC3339 / `YYYY-MM-DD HH:MM:SS` rows still sort by real time
- same-second RFC3339 rows retain their sub-second ordering

The timestamp parser now accepts:

```rust
if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
    return Ok(parsed.with_timezone(&Utc));
}

for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(raw, format) {
        return Ok(parsed.and_utc());
    }
}
```

### 3. Remove the unintended 100-row cap

The final implementation plan explicitly dropped the `LIMIT 100` behavior, so `ExecutionListing` no longer carries `truncated`, `run_list()` no longer prints `(showing 100 most recent)`, and the report-query tests now assert that 101 inserted visible executions produce 101 listed results.

### 4. Lock down the edge cases with targeted regression tests

This pass added/updated tests for the exact review-driven failure modes:

- `load_storage_ignores_invalid_unrelated_runtime_fields`
- `load_storage_env_override_still_fails_on_malformed_user_config`
- `list_executions_preserves_subsecond_ordering_within_same_second`
- `list_executions_returns_all_visible_results_without_truncation`
- `report_lookup_error_surfaces_corrupt_only_reports`

These sit alongside the earlier report-query tests for mixed timestamp formats, invalid timestamps, stale rows, corrupt rows, JSON rendering, and malformed-config failure handling.

## Why This Works

The underlying problem was contract drift in two different places.

For config loading, the report commands needed a storage-only path so they would not be blocked by unrelated provider/model validation, but the first implementation accidentally changed syntax-failure behavior depending on whether the DB path came from env or file. Moving the env override behind the syntax check preserves the repo's "malformed user config is still an error" rule without re-coupling `report` to full runtime validation.

For execution ordering, `unixepoch(created_at)` looked attractive because it normalized mixed timestamp formats inside SQLite, but it threw away the sub-second precision that current snapshots actually persist. Parsing timestamps once at the query boundary and sorting typed `DateTime<Utc>` values keeps mixed-format compatibility and ordering fidelity at the same time.

Removing the 100-row cap was a contract correction: the older draft spec had that limit, but the final approved plan explicitly dropped it. Aligning the code with the final plan removed an unintended user-visible divergence.

## Prevention

- When splitting a read-only config path out of the main runtime loader, preserve the existing syntax/error boundary first, then narrow validation scope. Do not let env overrides bypass malformed-config detection unless that behavior is explicitly intended and documented.
- Treat approved plans as the source of truth over earlier draft specs. This fix existed because the code had carried forward an outdated `LIMIT 100` behavior after the final plan removed it.
- Do not sort persisted RFC3339 timestamps through second-level conversions unless second-level precision is actually the contract. If the storage format preserves sub-seconds, keep them through ordering.
- Add regression tests for both timestamp families when a SQLite query depends on date ordering: mixed persisted formats and same-second sub-second variants.
- For CLI error classification, add one test per user-visible branch. The corrupt-only `report show` branch was easy to miss until a dedicated test covered it.
- Re-run the repo verification sequence after cross-cutting CLI/config/query fixes. This fix passed:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast`
  - Final `nextest` result: `1692 passed`, `3 skipped`

## Related Issues

- Related learning: `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`
- Related learning: `docs/solutions/logic-errors/thesis-memory-deserialization-crash-on-stale-snapshot-2026-04-13.md`
- Reporter/JSON contract background: `docs/solutions/logic-errors/reporter-system-validation-and-safe-json-output-2026-04-23.md`
- Implementation plan: `docs/superpowers/plans/2026-05-07-query-analysis-report.md`
