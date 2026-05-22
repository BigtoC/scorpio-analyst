---
title: ETF Review Fixes for Historical Inputs, Snapshot Schema, and Handoff Boundaries
date: 2026-05-22
category: docs/solutions/logic-errors
module: workflow/tasks/analyst + workflow/snapshot + data/sec_edgar + workflow/tasks/preflight
problem_type: logic_error
component: assistant
symptoms:
  - "Historical ETF runs could reuse live quote, holdings, and OHLCV inputs instead of degrading cleanly for past target dates."
  - "`ScenarioValuation::Etf` expanded persisted snapshot shape without advancing `THESIS_MEMORY_SCHEMA_VERSION`, leaving v3 readers exposed to incompatible `etf` enum tags."
  - "`lookup_cik()` bypassed the shared EDGAR circuit breaker and kept issuing requests after repeated failures."
  - "`PreflightTask::with_runtime_policy_and_routing()` reopened a public runtime override path outside the sealed handoff contract."
root_cause: logic_error
resolution_type: code_fix
severity: high
tags:
  - etf
  - historical-data
  - snapshot
  - schema-versioning
  - circuit-breaker
  - sec-edgar
  - preflight
  - handoff
---

# ETF Review Fixes for Historical Inputs, Snapshot Schema, and Handoff Boundaries

## Problem

The ETF baseline rollout was functionally close, but review surfaced four contract
violations that would have made the feature drift from the rest of the runtime:
historical ETF runs still pulled present-day market inputs, ETF snapshot shape
changed without a schema-version bump, the SEC EDGAR CIK lookup ignored the
shared circuit breaker, and preflight still exposed a second public override path
for runtime routing metadata.

## Symptoms

- Backtests or historical ETF analyses could mix a past `target_date` with live
  quote, benchmark, dividend-yield, and N-PORT inputs.
- ETF-bearing snapshots serialized successfully under the new binary, but a
  pre-ETF reader would fail on the unknown `etf` enum variant while still seeing
  schema version `3`.
- Repeated `company_tickers.json` failures did not stop subsequent `lookup_cik()`
  requests, even after other EDGAR fetches would have opened the breaker.
- Tests could still inject routing fallback reasons through
  `PreflightTask::with_runtime_policy_and_routing()` instead of the sealed
  `handoff` path used by production.

## What Didn't Work

- Relying on existing ETF live-data helpers was not enough. Those helpers are
  intentionally now-based, so leaving them in place for historical runs would
  have made backtests look precise while silently mixing time horizons.
- Treating the new `ScenarioValuation::Etf` variant as additive would have been
  wrong. `#[serde(default)]` protects missing fields, not unknown closed-enum
  variants.
- The earlier ETF preflight work had already converged on a sealed typed handoff
  for runtime-policy overrides (session history). Reintroducing a constructor
  seam for routing fallback data would have created two production authority
  paths again.

## Solution

### 1. Skip live ETF fetches for historical target dates

`crates/scorpio-core/src/workflow/tasks/analyst.rs`

- Added `target_date: &str` to `fetch_valuation_inputs(...)`.
- For `PackId::EtfBaseline`, compare `target_date` with today's UTC date string.
- If the run is historical, return early after fetching only `profile`, leaving
  ETF-only live fields (`etf_quote`, `etf_holdings`, `etf_ohlcv`, benchmark
  OHLCV, distribution yield) as `None`.
- Threaded `state.target_date` through the production call site and updated the
  existing ETF-pack test to pass today's date explicitly.
- Added a red/green regression test:
  `etf_baseline_historical_target_date_skips_live_etf_fetches`.

### 2. Bump snapshot schema for ETF-bearing persisted state

`crates/scorpio-core/src/workflow/snapshot/thesis.rs`

- Bumped `THESIS_MEMORY_SCHEMA_VERSION` from `3` to `4`.
- Updated the doc comment to record the new breaking change: the closed serde
  enum `ScenarioValuation` now includes `Etf(EtfValuation)`, which older
  binaries cannot deserialize.

`crates/scorpio-core/tests/state_roundtrip.rs`

- Added `etf_variant_requires_snapshot_schema_version_above_v3`.
- The test proves the break in both directions:
  - a legacy pre-ETF decoder rejects the serialized `etf` variant tag,
  - a current `SnapshotStore` write persists a schema version greater than `3`.

`crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`

- Updated stale version-language in the compatibility tests so the comments match
  the active same-version-only contract.

### 3. Put `lookup_cik()` behind the EDGAR circuit breaker

`crates/scorpio-core/src/data/sec_edgar/mod.rs`

- Added an open-breaker fast exit to the uncached `lookup_cik()` path.
- Recorded breaker failures for transport, non-200, and parse failures.
- Recorded breaker success on a successful `company_tickers.json` parse.
- Left the in-memory cache fast path unchanged.
- Added the red/green regression test
  `lookup_cik_skips_request_when_circuit_open` and verified it alongside the
  existing `fetch_recent_filings_skips_request_when_circuit_open` test.

### 4. Restore a single runtime handoff boundary into preflight

`crates/scorpio-core/src/workflow/tasks/preflight.rs`

- Rewrote `preflight_records_routing_fallback_reason_in_state_and_context` to
  seed the override through `handoff::put_into_context(...)`, matching the
  production path.
- Removed `PreflightTask::with_runtime_policy_and_routing()` entirely.
- Kept `with_runtime_policy(...)` for tests that need a plain fixed runtime
  policy without an override payload.

## Why This Works

- Historical ETF runs now fail soft in the same way other historical-only
  enrichments do: they preserve asset-shape detection from `profile`, but avoid
  pretending that present-day ETF market structure is valid for a past run.
- Snapshot compatibility is explicit again. Older binaries skip newer rows by
  schema version instead of stumbling into an `unknown variant: etf` decode
  failure.
- All outbound SEC EDGAR paths now participate in the same breaker policy, so a
  degraded upstream cannot keep one uncapped endpoint hot while the others back
  off.
- Preflight regains its single-authority runtime hydration boundary. Session
  history shows this was already a deliberate design decision during the earlier
  ETF routing handoff work (session history), so removing the extra constructor
  keeps the implementation aligned with that contract.

## Prevention

- When a data source is intrinsically live-only, gate it on `target_date` before
  wiring it into historical or replayable analysis paths.
- Treat new closed-enum variants on persisted state as schema-breaking changes.
  `#[serde(default)]` is not a substitute for a schema-version bump.
- If an HTTP client owns a shared circuit breaker, every network path in that
  client should either honor it or explicitly document why it is exempt.
- Keep runtime-policy and fallback-reason overrides sealed behind
  `workflow/tasks/handoff.rs`; do not add alternate production injection paths
  through public task constructors.
- Use focused red tests for review findings before fixing them. The four tests
  added in this slice made each contract break concrete before code changed.

## Related Issues

- `docs/solutions/logic-errors/etf-runtime-policy-preseed-preflight-contract-2026-05-22.md`
  — earlier ETF routing fix that established the sealed typed handoff boundary.
- `docs/solutions/logic-errors/thesis-memory-deserialization-crash-on-stale-snapshot-2026-04-13.md`
  — earlier snapshot-compatibility guidance explaining why incompatible
  `TradingState` changes must advance `THESIS_MEMORY_SCHEMA_VERSION`.
- Session history: the ETF handoff design review and implementation on
  `feature/enhance-etf-analysis` had already decided that preflight should be
  the sole production writer for runtime routing surfaces (session history).

## Verification

- Targeted red/green tests:
  - `cargo nextest run -p scorpio-core etf_baseline_historical_target_date_skips_live_etf_fetches etf_baseline_fetch_skips_equity_statement_fanout --no-fail-fast`
  - `cargo nextest run -p scorpio-core etf_variant_requires_snapshot_schema_version_above_v3 --no-fail-fast`
  - `cargo nextest run -p scorpio-core lookup_cik_skips_request_when_circuit_open fetch_recent_filings_skips_request_when_circuit_open --no-fail-fast`
  - `cargo nextest run -p scorpio-core preflight_records_routing_fallback_reason_in_state_and_context preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed --no-fail-fast`
- Full repo verification passed after the fixes:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast`
