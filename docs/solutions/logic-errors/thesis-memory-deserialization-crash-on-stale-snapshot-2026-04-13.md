---
title: Thesis Memory Lookup Crashes Pipeline on Stale Snapshot Deserialization
date: "2026-04-13"
category: logic-errors
module: workflow/snapshot
problem_type: runtime_error
component: database
severity: high
symptoms:
  - "Pipeline aborts at preflight with: thesis memory lookup failed: storage error: failed to deserialize TradingState during prior-thesis lookup for symbol=QQQ schema_version=1"
  - "Any symbol with a snapshot written by a prior struct layout hits the crash on the next run"
  - "THESIS_MEMORY_SCHEMA_VERSION version guard does not catch the incompatibility because it was never bumped"
root_cause: logic_error
resolution_type: code_fix
tags:
  - serde
  - schema-versioning
  - trading-state
  - snapshot
  - deserialization
  - thesis-memory
  - preflight
  - schema-evolution
---

# Thesis Memory Lookup Crashes Pipeline on Stale Snapshot Deserialization

## Problem

`load_prior_thesis_for_symbol()` treated every `serde_json::from_str` failure as a fatal
`TradingError::Storage`, crashing the entire pipeline when a stored `TradingState` snapshot
could not be deserialized with the current struct. A stale QQQ snapshot written during an
intermediate commit of the analysis-packs feature branch (where `RuntimePolicy` changed shape
across several iterations) triggered a complete preflight abort on the next run.

## Symptoms

- Pipeline aborts in the `preflight` phase with no partial results:
  ```
  ERROR scorpio_analyst::workflow::pipeline::runtime: cycle failed, symbol: QQQ,
  error: graph-flow error in phase 'preflight' task 'preflight': Task execution failed:
  PreflightTask: thesis memory lookup failed: storage error: failed to deserialize
  TradingState during prior-thesis lookup for symbol=QQQ schema_version=1
  ```
- Affects every symbol that has a snapshot written against a prior (now-incompatible) struct layout.
- The version guard (`THESIS_MEMORY_SCHEMA_VERSION`) does not catch the issue because it was
  never bumped when the nested struct changed shape — so the snapshot passes the version filter
  and then fails at deserialization.

## What Didn't Work

Fix was applied directly once the root cause was identified. As an immediate recovery step the
`phase_snapshots` table was truncated to remove incompatible rows — valid short-term relief, but
not a sustainable mitigation for production data.

## Solution

**`src/workflow/snapshot/thesis.rs`** — change deserialization from hard-fail to warn-and-skip:

Before:
```rust
let state: TradingState = serde_json::from_str(&state_json)
    .with_context(|| {
        format!(
            "failed to deserialize TradingState during prior-thesis lookup \
             for symbol={symbol} schema_version={schema_version}"
        )
    })
    .map_err(TradingError::Storage)?;
```

After:
```rust
let state: TradingState = match serde_json::from_str(&state_json) {
    Ok(s) => s,
    Err(err) => {
        warn!(
            symbol,
            schema_version,
            %err,
            "prior-thesis snapshot failed to deserialize (schema evolution); skipping"
        );
        continue;
    }
};
```

Also update the doc comment: remove "Malformed payloads for otherwise-supported rows are treated
as hard storage failures" and replace with "Rows that fail deserialization due to schema evolution
are skipped with a warning."

**`src/workflow/snapshot/tests/thesis_compat.rs`** — rename test and invert assertion:

Before:
```rust
async fn load_prior_thesis_returns_storage_error_for_malformed_supported_payload() {
    // ...
    assert!(matches!(result, Err(TradingError::Storage(_))));
}
```

After:
```rust
async fn load_prior_thesis_skips_undeserializable_payload_and_returns_none() {
    // ...
    assert!(
        matches!(result, Ok(None)),
        "undeserializable row should be skipped, not hard-failed: {result:?}"
    );
}
```

Also remove the now-unused `use crate::error::TradingError;` import.

## Why This Works

Thesis memory is a best-effort feature: if a prior run's snapshot cannot be read, the pipeline
should continue without a thesis rather than abort. The original `.map_err(TradingError::Storage)?`
propagated any deserialization failure as a fatal error — correct for corruption of critical data,
but overly strict for an advisory historical lookup. The `continue` inside the row-iteration loop
skips the unreadable row and moves on; if no readable row exists, the function returns `Ok(None)`,
which the caller already handles as "no prior thesis found." The `warn!` log preserves full
observability without sacrificing availability.

The version guard (`THESIS_MEMORY_SCHEMA_VERSION`) was designed to prevent this scenario, but it
relies on developers remembering to bump it. Because the constant was not incremented when
`RuntimePolicy` changed shape during iterative feature development, the incompatible snapshots
passed the version filter and then failed at deserialization. The graceful-skip fix makes the
system resilient to this oversight category going forward.

## Prevention

1. **`#[serde(default)]` on every new `TradingState` field — required, not optional.**
   Additive changes (new fields) always deserialize safely from old snapshots when the field
   carries a `Default` impl. Missing this annotation makes all existing snapshots unreadable:

   ```rust
   // ✅ old snapshots deserialize successfully; new field defaults to None
   #[serde(default)]
   pub analysis_runtime_policy: Option<RuntimePolicy>,

   // ❌ old snapshots missing this field fail to deserialize
   pub analysis_runtime_policy: Option<RuntimePolicy>,
   ```

2. **Bump `THESIS_MEMORY_SCHEMA_VERSION` on any rename, removal, or type change.**
   `#[serde(default)]` covers additive changes. Non-additive changes (rename, remove, type change)
   are breaking — bump the constant so the lookup skips all prior snapshots rather than attempting
   to decode them:

   ```rust
   // src/workflow/snapshot/thesis.rs
   const THESIS_MEMORY_SCHEMA_VERSION: i64 = 2; // bump on every breaking TradingState change
   ```

3. **Add a compat test for each schema evolution.**
   `thesis_compat.rs` already verifies that an undeserializable row produces `Ok(None)`. Extend
   the pattern: when adding a new `TradingState` field, add a test that deserializes a JSON blob
   missing that field and confirms `Ok(Some(_))` — this verifies `#[serde(default)]` is in place
   before the code ships.

4. **Both `CLAUDE.md` and `AGENTS.md` document these rules** under the "TradingState schema
   evolution" bullet in Key Design Decisions / Architecture gotchas respectively. Code review
   should reference those sections when `TradingState` is touched.

## Related Issues

- `docs/solutions/logic-errors/thesis-memory-untrusted-context-boundary-2026-04-09.md` — same
  module, different failure mode (prompt-trust injection vs. deserialization crash). The prevention
  sections are complementary: that doc covers treating persisted model output as untrusted text;
  this doc covers treating persisted struct JSON as potentially schema-incompatible.
- `docs/solutions/logic-errors/stale-trading-state-evidence-and-unavailable-data-quality-fallbacks-2026-04-07.md`
  — complementary prevention guidance: "when adding a new per-cycle `TradingState` field, update
  `reset_cycle_outputs()`" pairs with "when adding a new `TradingState` field, annotate
  `#[serde(default)]`".
