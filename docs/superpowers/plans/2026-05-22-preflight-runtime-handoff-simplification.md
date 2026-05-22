# Preflight Runtime Handoff Simplification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the two-key JSON override transport between `run_analysis_cycle` and `PreflightTask` into a single private typed handoff carried through one sealed submodule, then inline one-use ceremony and replace combinator-heavy override resolution with direct branching.

**Architecture:** A new sealed submodule `crates/scorpio-core/src/workflow/tasks/handoff.rs` owns the override type, its single context key, and read/write accessor functions. The module itself is declared `pub(in crate::workflow) mod handoff;`, so workflow-internal callers can name `crate::workflow::tasks::handoff` directly without re-exporting it outside the workflow tree. The struct itself stays `pub(super)`; the accessor functions are `pub(in crate::workflow)` and take/return primitives (`RuntimePolicy`, `Option<String>`) so no caller outside the submodule names the type. `run_analysis_cycle` writes via `put_into_context(...)`; `PreflightTask` reads via `try_load_from_context(...)` which preserves the existing fail-loud `TaskExecutionFailed` contract on malformed payloads (the typed `Context::get::<T>` path is explicitly avoided because it returns `None` on deserialize mismatch, silently downgrading to the constructor-derived fallback). This stays out of `context_bridge` because that module owns `TradingState` serialization, while this handoff is a preflight-only orchestration override; a dedicated submodule is the smallest place to keep the key, payload shape, and fail-loud accessors together. Invariant: the handoff always carries a `RuntimePolicy`; `routing_fallback_reason` is optional metadata attached to that policy, not an independently overridable value.

**Tech Stack:** Rust 1.93+ (edition 2024), `graph-flow` 0.5, `serde`/`serde_json`, `tokio` 1, `async-trait`, `cargo nextest`. Existing structs `RuntimePolicy` (from `crate::analysis_packs`) and `graph_flow::Context` are unchanged.

**Shipping order:** Two atomic commits. Commit 1 lands the typed handoff and removes the old keys while preserving the existing override-resolution shape. Commit 2 lands the readability cleanup (inline helper, direct branching). Each commit must independently pass the merge gates so a regression in either is revertible without dragging the other.

---

## File Structure

| File                                                   | Action | Responsibility                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
|--------------------------------------------------------|--------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/workflow/tasks/handoff.rs`    | Create | Private `RuntimePreflightOverride` struct, single context key constant, and `pub(in crate::workflow)` accessor functions for write/read.                                                                                                                                                                                                                                                                                                                                                                                                  |
| `crates/scorpio-core/src/workflow/tasks/mod.rs`        | Modify | Declare `pub(in crate::workflow) mod handoff;`; remove the `pub(crate) use common::{KEY_ROUTING_FALLBACK_REASON_OVERRIDE, KEY_RUNTIME_POLICY_OVERRIDE};` re-export.                                                                                                                                                                                                                                                                                                                                                                       |
| `crates/scorpio-core/src/workflow/tasks/common.rs`     | Modify | Remove `KEY_RUNTIME_POLICY_OVERRIDE` and `KEY_ROUTING_FALLBACK_REASON_OVERRIDE` constants (lines 71-76).                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `crates/scorpio-core/src/workflow/pipeline/runtime.rs` | Modify | (Commit 1) Replace the two-key write block (lines 527-551) with one `handoff::put_into_context(...)` call. (Commit 2) Inline `classify_runtime_pack_selection` into `run_analysis_cycle`; the standalone function (lines 53-62) is deleted.                                                                                                                                                                                                                                                                                               |
| `crates/scorpio-core/src/workflow/tasks/preflight.rs`  | Modify | (Commit 1) Replace `runtime_policy_override(...)` and `routing_fallback_reason_override(...)` helpers (lines 375-400) with one call to `handoff::try_load_from_context(...)`; keep the fallback resolution in its existing combinator shape so the transport swap stays isolated. Update the existing test `preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed` to write the single new key. (Commit 2) Replace the combinator-heavy resolution with `if let Some((policy, reason)) = ...` direct branching. |

No changes to `pipeline/tests.rs` or `tests/workflow_pipeline_structure.rs` are expected — those tests do not reference the override keys directly (`grep` confirmed), and `run_analysis_cycle_routes_baseline_pipeline_to_etf_pack_per_run` remains the load-bearing end-to-end proof that `run_analysis_cycle` and `PreflightTask` stay wired together through the new handoff. If `cargo nextest` surfaces a failure there, treat it as a real regression rather than a mechanical update.

---

## Commit 1 — Typed handoff replaces the two-key transport

### Task 1: Create `handoff.rs` with the override type, key, and accessors (TDD)

**Files:**
- Create: `crates/scorpio-core/src/workflow/tasks/handoff.rs`

- [ ] **Step 1: Declare the new module so its tests are reachable**

In `crates/scorpio-core/src/workflow/tasks/mod.rs`, immediately after line 10 (`pub mod preflight;`), add:

```rust
pub(in crate::workflow) mod handoff;
```

Do NOT add a wider re-export — the submodule stays sealed outside `crate::workflow`, and workflow-internal consumers reach into it via its `pub(in crate::workflow)` accessor functions rather than `tasks::*`.

- [ ] **Step 2: Write the failing tests first**

Create `crates/scorpio-core/src/workflow/tasks/handoff.rs` with ONLY the test module populated (no implementation yet). The tests cover four required behaviors: round-trip via accessors, absent-key returns `Ok(None)`, malformed payload returns `TaskExecutionFailed`, and absent fallback reason round-trips as `None`. Together they codify the invariant that the handoff always carries a `RuntimePolicy`; the fallback reason is optional metadata, not an independent override path.

```rust
//! Private typed handoff between `run_analysis_cycle` and `PreflightTask`.
//!
//! Replaces the prior two-key JSON override transport
//! (`KEY_RUNTIME_POLICY_OVERRIDE` + `KEY_ROUTING_FALLBACK_REASON_OVERRIDE`)
//! with one sealed context value. The struct is `pub(super)` and the
//! accessor functions are `pub(in crate::workflow)`, so no caller outside
//! the workflow module tree names the type.
//!
//! Read path is intentionally string-based + `serde_json::from_str` (not the
//! typed `Context::get::<T>`) because graph-flow's typed `get` returns `None`
//! on any deserialize mismatch, which would silently downgrade to the
//! constructor-derived fallback. The string + explicit parse preserves the
//! fail-loud `TaskExecutionFailed` contract that the old two-key code
//! provided.

use graph_flow::Context;
use serde::{Deserialize, Serialize};

use crate::analysis_packs::RuntimePolicy;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct RuntimePreflightOverride {
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
}

pub(in crate::workflow) const KEY_RUNTIME_PREFLIGHT_OVERRIDE: &str =
    "runtime_preflight_override";

pub(in crate::workflow) async fn put_into_context(
    context: &Context,
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
) -> graph_flow::Result<()> {
    // Implementation in Step 4.
    let _ = (context, runtime_policy, routing_fallback_reason);
    unimplemented!("put_into_context not yet implemented")
}

pub(in crate::workflow) async fn try_load_from_context(
    context: &Context,
) -> graph_flow::Result<Option<(RuntimePolicy, Option<String>)>> {
    // Implementation in Step 4.
    let _ = context;
    unimplemented!("try_load_from_context not yet implemented")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis_packs::{PackId, resolve_pack, resolve_runtime_policy_for_manifest};

    fn baseline_policy() -> RuntimePolicy {
        resolve_runtime_policy_for_manifest(&resolve_pack(PackId::Baseline))
            .expect("baseline pack must resolve")
    }

    #[tokio::test]
    async fn roundtrip_preserves_policy_and_reason() {
        let context = Context::new();
        let policy = baseline_policy();
        put_into_context(&context, policy.clone(), Some("profile_lookup_unavailable".to_owned()))
            .await
            .expect("write");
        let (loaded_policy, loaded_reason) = try_load_from_context(&context)
            .await
            .expect("read")
            .expect("override present");
        assert_eq!(loaded_policy, policy);
        assert_eq!(loaded_reason.as_deref(), Some("profile_lookup_unavailable"));
    }

    #[tokio::test]
    async fn absent_key_returns_ok_none() {
        let context = Context::new();
        let outcome = try_load_from_context(&context).await.expect("read");
        assert!(outcome.is_none());
    }

    #[tokio::test]
    async fn malformed_payload_returns_task_execution_failed() {
        let context = Context::new();
        context
            .set(KEY_RUNTIME_PREFLIGHT_OVERRIDE, "{not valid json".to_owned())
            .await;
        let err = try_load_from_context(&context)
            .await
            .expect_err("malformed override must surface as TaskExecutionFailed");
        match err {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert!(
                    message.contains("runtime preflight override"),
                    "error message should identify the override subsystem: {message}"
                );
            }
            other => panic!("expected TaskExecutionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn absent_reason_roundtrips_as_none() {
        let context = Context::new();
        put_into_context(&context, baseline_policy(), None)
            .await
            .expect("write");
        let (_, reason) = try_load_from_context(&context)
            .await
            .expect("read")
            .expect("override present");
        assert!(reason.is_none());
    }
}
```

- [ ] **Step 3: Run the failing tests to confirm they compile and fail for the right reason**

Run: `cargo nextest run -p scorpio-core workflow::tasks::handoff --no-fail-fast`
Expected: each of the four tests panics on the `unimplemented!()` macro inside `put_into_context` or `try_load_from_context`. No compile errors. If you see "module `handoff` not declared," return to Step 1.

- [ ] **Step 4: Implement the accessors**

In `crates/scorpio-core/src/workflow/tasks/handoff.rs`, replace the two `unimplemented!()` function bodies with the real implementations. The whole file becomes:

```rust
//! Private typed handoff between `run_analysis_cycle` and `PreflightTask`.
//!
//! Replaces the prior two-key JSON override transport
//! (`KEY_RUNTIME_POLICY_OVERRIDE` + `KEY_ROUTING_FALLBACK_REASON_OVERRIDE`)
//! with one sealed context value. The struct is `pub(super)` and the
//! accessor functions are `pub(in crate::workflow)`, so no caller outside
//! the workflow module tree names the type.
//!
//! Read path is intentionally string-based + `serde_json::from_str` (not the
//! typed `Context::get::<T>`) because graph-flow's typed `get` returns `None`
//! on any deserialize mismatch, which would silently downgrade to the
//! constructor-derived fallback. The string + explicit parse preserves the
//! fail-loud `TaskExecutionFailed` contract that the old two-key code
//! provided.

use graph_flow::Context;
use serde::{Deserialize, Serialize};

use crate::analysis_packs::RuntimePolicy;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct RuntimePreflightOverride {
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
}

pub(in crate::workflow) const KEY_RUNTIME_PREFLIGHT_OVERRIDE: &str =
    "runtime_preflight_override";

pub(in crate::workflow) async fn put_into_context(
    context: &Context,
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
) -> graph_flow::Result<()> {
    let payload = RuntimePreflightOverride {
        runtime_policy,
        routing_fallback_reason,
    };
    let json = serde_json::to_string(&payload).map_err(|err| {
        graph_flow::GraphError::TaskExecutionFailed(format!(
            "orchestration corruption: runtime preflight override serialization failed: {err}"
        ))
    })?;
    context.set(KEY_RUNTIME_PREFLIGHT_OVERRIDE, json).await;
    Ok(())
}

pub(in crate::workflow) async fn try_load_from_context(
    context: &Context,
) -> graph_flow::Result<Option<(RuntimePolicy, Option<String>)>> {
    let raw: Option<String> = context.get(KEY_RUNTIME_PREFLIGHT_OVERRIDE).await;
    let Some(json) = raw else {
        return Ok(None);
    };
    let payload: RuntimePreflightOverride = serde_json::from_str(&json).map_err(|err| {
        graph_flow::GraphError::TaskExecutionFailed(format!(
            "PreflightTask: orchestration corruption: runtime preflight override deserialization failed: {err}"
        ))
    })?;
    Ok(Some((payload.runtime_policy, payload.routing_fallback_reason)))
}

#[cfg(test)]
mod tests {
    // Same test module as Step 2 — leave unchanged.
}
```

(Leave the `mod tests { … }` block from Step 2 intact; only the two function bodies change.)

- [ ] **Step 5: Run the tests; they must all pass**

Run: `cargo nextest run -p scorpio-core workflow::tasks::handoff --no-fail-fast`
Expected: 4 tests passed, 0 failed.

- [ ] **Step 6: Confirm clippy is clean on the new file**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings`
Expected: no warnings or errors. If unused-import lints fire on `KEY_RUNTIME_POLICY_OVERRIDE` / `KEY_ROUTING_FALLBACK_REASON_OVERRIDE` in other files, leave them — they get removed in Task 4.

---

### Task 2: Switch `run_analysis_cycle` to write via the new accessor

**Files:**
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs:34-39` (use clause), `:527-551` (override write block)

- [ ] **Step 1: Update the use clause**

In `crates/scorpio-core/src/workflow/pipeline/runtime.rs`, the existing import block at lines 30-40 imports `KEY_ROUTING_FALLBACK_REASON_OVERRIDE, KEY_RUNTIME_POLICY_OVERRIDE` from `workflow::tasks`. Remove those two names from the import. The block becomes:

```rust
    workflow::{
        RuntimePackSelection, SnapshotStore, classify_runtime_pack,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        tasks::{
            FundamentalAnalystTask, KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_NEWS,
            KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_RISK_ROUND,
            KEY_TRANSCRIPT_FETCH_STATUS, NewsAnalystTask, SentimentAnalystTask,
            TechnicalAnalystTask,
        },
    },
```

Add a sibling use for the handoff accessors below the existing `tasks::` import block:

```rust
use crate::workflow::tasks::handoff;
```

Place it with the other `use` statements at the top of the file. This is the only supported access path for workflow-internal callers; do not add a re-export or alternate call-site path.

- [ ] **Step 2: Replace the two-key write block**

Currently `crates/scorpio-core/src/workflow/pipeline/runtime.rs:527-551` reads:

```rust
    let runtime_policy_json =
        serde_json::to_string(&runtime_policy).map_err(|error| TradingError::GraphFlow {
            phase: "init".into(),
            task: "serialize_runtime_policy_override".into(),
            cause: error.to_string(),
        })?;
    session
        .context
        .set(KEY_RUNTIME_POLICY_OVERRIDE, runtime_policy_json)
        .await;
    let routing_fallback_reason_json =
        serde_json::to_string(&routing_fallback_reason).map_err(|error| {
            TradingError::GraphFlow {
                phase: "init".into(),
                task: "serialize_routing_fallback_reason_override".into(),
                cause: error.to_string(),
            }
        })?;
    session
        .context
        .set(
            KEY_ROUTING_FALLBACK_REASON_OVERRIDE,
            routing_fallback_reason_json,
        )
        .await;
```

Replace the entire block with:

```rust
    handoff::put_into_context(
        &session.context,
        runtime_policy.clone(),
        routing_fallback_reason.clone(),
    )
    .await
    .map_err(|error| TradingError::GraphFlow {
        phase: "init".into(),
        task: "serialize_runtime_preflight_override".into(),
        cause: error.to_string(),
    })?;
```

`runtime_policy` and `routing_fallback_reason` are still consumed later in the function (they feed `runtime_policy.enrichment_intent` reads), so the `.clone()` is required. If `runtime_policy` is owned and not consumed downstream, drop the clone — verify by re-reading the function body once after the edit.

- [ ] **Step 3: Build the crate to confirm the wiring compiles**

Run: `cargo build -p scorpio-core`
Expected: clean build. If the compiler complains about an unused import for `KEY_ROUTING_FALLBACK_REASON_OVERRIDE` or `KEY_RUNTIME_POLICY_OVERRIDE`, you missed Step 1 — go back and remove them from the use clause.

- [ ] **Step 4: Re-run handoff tests only**

Run: `cargo nextest run -p scorpio-core workflow::tasks::handoff --no-fail-fast`
Expected: handoff tests still pass. Do not treat pipeline-routing coverage as meaningful yet — `PreflightTask` still reads the old keys until Task 3 lands.

---

### Task 3: Switch `PreflightTask` to read via the new accessor

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/preflight.rs:38-39` (use clause), `:229-241` (override resolution), `:375-400` (delete old helpers)

- [ ] **Step 1: Update the use clause**

In `crates/scorpio-core/src/workflow/tasks/preflight.rs`, lines 37-40 currently read:

```rust
use crate::workflow::tasks::common::{
    KEY_ROUTING_FALLBACK_REASON, KEY_ROUTING_FALLBACK_REASON_OVERRIDE, KEY_ROUTING_FLAGS,
    KEY_RUNTIME_PACK_ROUTE, KEY_RUNTIME_POLICY, KEY_RUNTIME_POLICY_OVERRIDE,
    KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT, KEY_PROVIDER_CAPABILITIES,
};
```

(Exact field order may differ; preserve the file's existing alphabetical layout.) Remove `KEY_ROUTING_FALLBACK_REASON_OVERRIDE` and `KEY_RUNTIME_POLICY_OVERRIDE`. Add a sibling use:

```rust
use crate::workflow::tasks::handoff;
```

- [ ] **Step 2: Replace the override-resolution block**

Currently `preflight.rs:229-241` reads:

```rust
        let runtime_policy = runtime_policy_override(&context)
            .await?
            .map(Ok)
            .unwrap_or_else(|| {
                self.runtime_policy.clone().map_err(|e| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "PreflightTask: pack resolution failed: {e}"
                    ))
                })
            })?;
        let routing_fallback_reason = routing_fallback_reason_override(&context)
            .await?
            .or_else(|| self.routing_fallback_reason.clone());
```

Replace the block with a single call to the accessor. (Direct-branching rewrite for readability is Commit 2's job — for Commit 1, keep the combinator style so the diff is purely a transport substitution.)

```rust
        let override_payload = handoff::try_load_from_context(&context).await?;
        let runtime_policy = override_payload
            .as_ref()
            .map(|(policy, _)| policy.clone())
            .map(Ok)
            .unwrap_or_else(|| {
                self.runtime_policy.clone().map_err(|e| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "PreflightTask: pack resolution failed: {e}"
                    ))
                })
            })?;
        let routing_fallback_reason = override_payload
            .and_then(|(_, reason)| reason)
            .or_else(|| self.routing_fallback_reason.clone());
```

This keeps Commit 1 as a pure transport substitution. Commit 2 will spend the readability budget on rewriting the same logic into direct branching once the handoff migration is already green.

- [ ] **Step 3: Delete the now-unused helper functions**

`preflight.rs:375-400` contains `runtime_policy_override(...)` and `routing_fallback_reason_override(...)`. Delete both functions in their entirety. The file's `mod tests` block at line 417 stays.

- [ ] **Step 4: Build to confirm everything wires**

Run: `cargo build -p scorpio-core`
Expected: clean build, no unused-function warnings, no missing-symbol errors.

---

### Task 4: Update the existing override-hydration test to write the single new key

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/preflight.rs:1031-1078` (the `preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed` test) and the test module's use clause around line 440.

- [ ] **Step 1: Update the test module's use clause**

`preflight.rs:440-442` currently imports `KEY_ROUTING_FALLBACK_REASON_OVERRIDE` and `KEY_RUNTIME_POLICY_OVERRIDE`:

```rust
    use crate::workflow::tasks::common::{
        KEY_ROUTING_FALLBACK_REASON_OVERRIDE, KEY_RUNTIME_POLICY, KEY_RUNTIME_POLICY_OVERRIDE,
    };
```

Replace with:

```rust
    use crate::workflow::tasks::common::KEY_RUNTIME_POLICY;
    use crate::workflow::tasks::handoff;
```

- [ ] **Step 2: Rewrite the test body to use the new accessor**

The current test body (`preflight.rs:1031-1078`) writes two JSON strings under the old keys. Replace lines 1043-1050 (the `runtime_policy_json` / `fallback_json` build + `ctx.set` calls) with one call to the accessor:

```rust
        handoff::put_into_context(
            &ctx,
            runtime_policy.clone(),
            Some("profile_lookup_unavailable".to_owned()),
        )
        .await
        .expect("override write");
```

Drop the now-unused `runtime_policy_json` and `fallback_json` locals. The rest of the test (TradingState setup, PreflightTask construction, post-run assertions) stays identical.

- [ ] **Step 3: Run the regression tests**

Run: `cargo nextest run -p scorpio-core preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed run_analysis_cycle_routes_baseline_pipeline_to_etf_pack_per_run --no-fail-fast`
Expected: 2 tests passed. The first preserves the preflight hydration contract; the second is the load-bearing producer-to-consumer regression proving `run_analysis_cycle` writes what `PreflightTask` reads through the new handoff after legacy-key removal.

---

### Task 5: Remove the old override key constants and re-export

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/common.rs:71-76` (delete two constants)
- Modify: `crates/scorpio-core/src/workflow/tasks/mod.rs:35` (delete the `pub(crate) use` line)

- [ ] **Step 1: Delete the constants in `common.rs`**

`common.rs:71` and `:76` currently define:

```rust
pub(crate) const KEY_RUNTIME_POLICY_OVERRIDE: &str = "runtime_policy_override";
// (blank line and comment may exist between them)
pub(crate) const KEY_ROUTING_FALLBACK_REASON_OVERRIDE: &str = "routing.fallback_reason_override";
```

Delete both lines and any leading doc-comments specific to them. Leave `KEY_RUNTIME_POLICY` (line 66) and `KEY_ROUTING_FLAGS` (line 85) intact — they are the *public* post-preflight keys, not the override keys.

- [ ] **Step 2: Delete the re-export in `mod.rs`**

`tasks/mod.rs:35` currently reads:

```rust
pub(crate) use common::{KEY_ROUTING_FALLBACK_REASON_OVERRIDE, KEY_RUNTIME_POLICY_OVERRIDE};
```

Delete that line entirely.

- [ ] **Step 3: Build and verify no stragglers**

Run: `cargo build -p scorpio-core --all-targets`
Expected: clean build. Any "cannot find value `KEY_*_OVERRIDE`" error indicates a missed call site — fix it before continuing.

Run: `grep -rn "KEY_RUNTIME_POLICY_OVERRIDE\|KEY_ROUTING_FALLBACK_REASON_OVERRIDE" crates/`
Expected: zero matches. If any remain, delete them.

---

### Task 6: Run the full merge-gate verification and commit Commit 1

**Files:** none modified in this task — verification only.

- [ ] **Step 1: Format check**

Run: `cargo fmt -- --check`
Expected: no output, exit code 0. If format violations exist, run `cargo fmt` and re-check.

- [ ] **Step 2: Clippy with workspace-wide deny-warnings**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings or errors.

- [ ] **Step 3: Full test suite via nextest**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: all tests pass. Pay special attention to:
- `preflight_hydrates_runtime_surfaces_from_context_override_without_state_preseed` (the load-bearing override regression — touched by Task 4)
- `activation_path_audit_*` (authority-boundary audits — must still pass without modification)
- `run_analysis_cycle_routes_baseline_pipeline_to_etf_pack_per_run` and `run_analysis_cycle_preserves_from_pack_fixed_manifest_over_runtime_etf_route` (routing regressions — must still pass)
- `workflow::tasks::handoff::tests::*` (the four new tests from Task 1)

- [ ] **Step 4: Examples still build**

Run: `cargo build -p scorpio-core --examples`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/workflow/tasks/handoff.rs \
        crates/scorpio-core/src/workflow/tasks/mod.rs \
        crates/scorpio-core/src/workflow/tasks/common.rs \
        crates/scorpio-core/src/workflow/tasks/preflight.rs \
        crates/scorpio-core/src/workflow/pipeline/runtime.rs
git commit -m "$(cat <<'EOF'
refactor(workflow): collapse two-key preflight override into typed handoff

Replace KEY_RUNTIME_POLICY_OVERRIDE + KEY_ROUTING_FALLBACK_REASON_OVERRIDE
with one private sealed submodule (workflow::tasks::handoff) that owns the
override type, its single context key, and accessor functions taking
primitives. Preserve the fail-loud TaskExecutionFailed contract on malformed
payloads via explicit serde_json::from_str (not typed Context::get) so a
serialization regression in run_analysis_cycle cannot silently downgrade to
the constructor-derived fallback. No user-visible behavior changes.
EOF
)"
```

After the commit lands, run `git status` to confirm a clean working tree. If anything other than the planned files appears in the diff, investigate before proceeding to Commit 2.

---

## Commit 2 — Readability cleanup

### Task 7: Inline `classify_runtime_pack_selection` into `run_analysis_cycle`

**Files:**
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs:53-62` (delete the standalone function), `:344-360` (inline at the single call site)

- [ ] **Step 1: Identify the call site**

`run_analysis_cycle` at `runtime.rs:344-360` currently calls `classify_runtime_pack_selection`:

```rust
    let (runtime_policy, routing_fallback_reason) = match pipeline.runtime_policy.clone() {
        Some(policy) => (policy, None),
        None => {
            let selection =
                classify_runtime_pack_selection(&pipeline.yfinance, &initial_state.asset_symbol)
                    .await;
            let pack_id = selection.pack_id();
            let policy =
                resolve_runtime_policy_for_manifest(&resolve_pack(pack_id)).map_err(|cause| {
                    TradingError::Config(anyhow::anyhow!(
                        "analysis pack resolution failed for '{}': {cause}",
                        pack_id.as_str()
                    ))
                })?;
            (policy, selection.fallback_reason().map(str::to_owned))
        }
    };
```

The standalone function `classify_runtime_pack_selection` at `runtime.rs:53-62`:

```rust
async fn classify_runtime_pack_selection(
    yfinance: &YFinanceClient,
    symbol: &str,
) -> RuntimePackSelection {
    let profile = yfinance.get_profile(symbol).await;
    let fund_info = profile
        .as_ref()
        .and_then(|profile| crate::data::yfinance::etf::fund_info_from_profile(symbol, profile));
    classify_runtime_pack(profile.as_ref(), fund_info.as_ref())
}
```

It has exactly one call site — confirmed by `grep -rn 'classify_runtime_pack_selection' crates/` returning only the two lines above.

- [ ] **Step 2: Inline the body and delete the standalone function**

Replace the `match` arm body at `runtime.rs:347-358` with the inlined classification:

```rust
        None => {
            let symbol = &initial_state.asset_symbol;
            let profile = pipeline.yfinance.get_profile(symbol).await;
            let fund_info = profile.as_ref().and_then(|profile| {
                crate::data::yfinance::etf::fund_info_from_profile(symbol, profile)
            });
            let selection = classify_runtime_pack(profile.as_ref(), fund_info.as_ref());
            let pack_id = selection.pack_id();
            let policy = resolve_runtime_policy_for_manifest(&resolve_pack(pack_id)).map_err(
                |cause| {
                    TradingError::Config(anyhow::anyhow!(
                        "analysis pack resolution failed for '{}': {cause}",
                        pack_id.as_str()
                    ))
                },
            )?;
            (policy, selection.fallback_reason().map(str::to_owned))
        }
```

Then delete the standalone `classify_runtime_pack_selection` function at `runtime.rs:53-62` in its entirety. Also delete the `use` line for `YFinanceClient` if it becomes unused — `cargo build` will surface this.

- [ ] **Step 3: Build and lint**

Run: `cargo build -p scorpio-core --all-targets && cargo clippy -p scorpio-core --all-targets -- -D warnings`
Expected: clean build, no clippy warnings. If unused-import warnings appear for `YFinanceClient`, remove the import.

- [ ] **Step 4: Run pipeline tests**

Run: `cargo nextest run -p scorpio-core workflow::pipeline --no-fail-fast`
Expected: all tests pass. The inlining is behavior-preserving; failures here indicate a transcription error.

---

### Task 8: Rewrite the override-resolution block to direct branching, then verify

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/preflight.rs:229-241` (rewrite override resolution)

Commit 1 intentionally kept the fallback resolution in its original combinator shape so the transport swap stayed isolated. Commit 2 now spends the readability budget on rewriting that same block to direct branching:

```rust
        let (runtime_policy, routing_fallback_reason) =
            if let Some((policy, reason)) = handoff::try_load_from_context(&context).await? {
                (policy, reason)
            } else {
                let policy = self.runtime_policy.clone().map_err(|e| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "PreflightTask: pack resolution failed: {e}"
                    ))
                })?;
                (policy, self.routing_fallback_reason.clone())
            };
```

- [ ] **Step 1: Replace the combinator chain with direct branching**

Apply the rewrite above in `preflight.rs:229-241`. Keep the error message and fallback semantics identical; only the control-flow shape changes in Commit 2.

- [ ] **Step 2: Re-read the resolved block to confirm no combinator-heavy code remains**

Open `preflight.rs:229-260` and read top-to-bottom. Verify:
- No `.map(Ok).unwrap_or_else(...)` chains
- No nested `.and_then(...)` or `.or_else(...)` on the override resolution
- The intent — "use override if present; otherwise use constructor fallback" — reads as a single `if let` / `else` branch

If anything combinator-heavy remains (e.g., the `routing_fallback_reason` still uses `.or_else(|| self.routing_fallback_reason.clone())` from the pre-Commit-1 shape), rewrite it inline now to match the `if let` form above.

- [ ] **Step 3: Format and lint**

Run: `cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

---

### Task 9: Run the full merge-gate verification and commit Commit 2

**Files:** none modified in this task — verification + commit only.

- [ ] **Step 1: Format check**

Run: `cargo fmt -- --check`
Expected: no output.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Full test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: all tests pass. The same load-bearing tests as Task 6 Step 3 must still pass.

- [ ] **Step 4: Examples build**

Run: `cargo build -p scorpio-core --examples`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/workflow/pipeline/runtime.rs \
        crates/scorpio-core/src/workflow/tasks/preflight.rs
git commit -m "$(cat <<'EOF'
refactor(workflow): inline runtime pack helper and preflight branching

Inline the single-call-site classify_runtime_pack_selection helper into
run_analysis_cycle and rewrite the PreflightTask override-resolution block
from combinators to direct branching. No user-visible behavior changes.
EOF
)"
```

- [ ] **Step 6: Confirm clean working tree**

Run: `git status`
Expected: nothing to commit, working tree clean. If `git diff main...HEAD` shows any unintended changes (other than the two commits described in this plan), investigate before declaring done.

---

## Post-Implementation Verification Checklist

Before declaring the slice ready for review, manually confirm the design invariants:

- [ ] **Invariant 1 — pre-graph TradingState is untouched.**
  `grep -n "state.analysis_pack_name\|state.analysis_runtime_policy\|state.etf_routing_fallback_reason" crates/scorpio-core/src/workflow/pipeline/runtime.rs`
  Expected: zero matches — `run_analysis_cycle` must not write any of those fields. (PreflightTask remains the sole writer.)

- [ ] **Invariant 3 — PreflightTask is still the sole production writer of those surfaces.**
  `grep -rn "state.analysis_pack_name = \|state.analysis_runtime_policy = \|state.etf_routing_fallback_reason = " crates/scorpio-core/src/`
  Expected: only matches inside `crates/scorpio-core/src/workflow/tasks/preflight.rs` (and possibly test helpers under `#[cfg(test)]` or `feature = "test-helpers"` — those are not "production" writers and are acceptable).

- [ ] **Invariant — old keys are gone.**
  `grep -rn "KEY_RUNTIME_POLICY_OVERRIDE\|KEY_ROUTING_FALLBACK_REASON_OVERRIDE\|runtime_policy_override\b\|routing_fallback_reason_override\b" crates/`
  Expected: zero matches.

- [ ] **Invariant — handoff submodule is sealed.**
  `grep -rn "RuntimePreflightOverride" crates/`
  Expected: matches only inside `crates/scorpio-core/src/workflow/tasks/handoff.rs`. The struct must not leak outside its submodule.

- [ ] **Invariant — fail-loud preserved.**
  Re-run `cargo nextest run -p scorpio-core workflow::tasks::handoff::tests::malformed_payload_returns_task_execution_failed`
  Expected: 1 test passed. This is the canary that catches future regressions if a refactor swaps the string transport for a typed `Context::get::<T>` read.

---

## Out-of-Scope Reminders

These items appear in the design's Non-Goals or Follow-up sections and MUST NOT be implemented in this slice:

- Do not pre-seed runtime surfaces on `TradingState` before graph execution.
- Do not change `TradingPipeline::from_pack(...)` fixed-pack semantics.
- Do not relocate ETF benchmark normalization — it is deferred to a separate follow-up plan, which has its own `FundInfo` consumer-inventory gate before the relocation lands.
- Do not change user-visible routing behavior or final report behavior.

If you find yourself touching `analyst.rs`, `derive_runtime_valuation`, or `ValuationInputs` while executing this plan, stop and verify it is genuinely required by one of the tasks above. The benchmark normalization work is a different commit on a different plan.
