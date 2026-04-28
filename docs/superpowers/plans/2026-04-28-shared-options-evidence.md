# Shared Options Evidence For Downstream Agents Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist one Rust-owned options artifact from the Technical Analyst phase and make downstream agents explicitly use it without breaking snapshot compatibility, fail-open behavior, or the existing technical-report seam.

**Architecture:** Extend `TechnicalData` with additive `options_context`, split the Technical Analyst's LLM output type from the persisted technical state, and prefetch options once per technical run into a runtime-only context that `GetOptionsSnapshot` replays. Keep options routed through the existing `technical_report` payload, update downstream prompt markdown to reason over `technical_report.options_context`, and keep technical evidence provenance on the current cycle-level merge-time semantics.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `schemars`, `rig`, `yfinance-rs`, `cargo nextest`, `cargo fmt`, `cargo clippy`.

---

**Worktree:** Create and execute from a fresh dedicated worktree/branch such as `feature/shared-options-evidence`. Confirm with `git worktree list` first. Do not implement this on the current `feature/upgrade-rig-to-0.35.0` branch.

## Guardrails

### Scope and ownership

- Keep options under the existing technical seam. Do **not** add a new top-level `TradingState` branch and do **not** route this work through `data/routing.rs::derivatives` or `data/traits/derivatives.rs`.
- Keep the single-source-of-truth guarantee: one options fetch per technical run, no post-inference second fetch.
- Keep runtime ownership in `TechnicalAnalyst::run()`. If `OptionsToolContext` must move out of `technical.rs` to avoid a `data -> agents` dependency, keep it crate-private and local to this flow; do **not** broaden it into a generally re-exported abstraction.
- Keep `FetchFailed { reason: String }` only. Do **not** add a failure-code enum in this slice.

### Snapshot and compatibility

- `TechnicalData.options_context` is additive. Keep `THESIS_MEMORY_SCHEMA_VERSION` unchanged.
- `TechnicalData.options_summary` changes meaning from "raw copied tool JSON" to "technical-desk interpretation". The approved spec explicitly tolerates old blobs as-is; do **not** add legacy suppression logic or a helper layer in this slice.
- Before shipping, audit all `options_summary` readers to confirm no consumer parses it as JSON.

### Prompt and tool behavior

- Keep the existing per-role `technical_report` seam, but implement the spec's compact projection at each role-local serializer seam. Do **not** introduce a new shared prompt-serialization helper in this slice unless duplication becomes unavoidable during implementation.
- `GetOptionsSnapshot` must preserve its current JSON contract when it is bound: same `kind` discriminant behavior, same injected `reason` field on non-`Snapshot` outcomes.
- When the options prefetch fails, omit the tool for that run and condition the Technical Analyst prompt so it does not mention or expect `get_options_snapshot`.
- Authoritative rule for this slice: only live `Snapshot` outcomes may persist a non-`None` `options_summary`. For successful non-snapshot outcomes, the model may reason about explicit absence during the turn, but the persisted `options_summary` must be cleared before storage.

### Evidence and provenance

- Keep one Yahoo `EvidenceSource` for technical evidence.
- Keep `EvidenceSource.fetched_at` on the existing cycle-level merge-time semantics from `AnalystSyncTask`; do **not** invent per-tool timestamp transport.
- Switch technical datasets from `options_snapshot`/`options_summary` proxying to `options_context` presence.

### Verification

- CI uses `cargo nextest`, not `cargo test`.
- Before marking implementation complete, run:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast`

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Modify | `crates/scorpio-core/src/state/technical.rs` | Add `TechnicalOptionsContext` and additive `TechnicalData.options_context` |
| Modify | `crates/scorpio-core/src/agents/analyst/equity/technical.rs` | Split `TechnicalAnalystResponse` from persisted `TechnicalData`, prefetch options once, condition tool binding and prompt text, merge model-owned + Rust-owned fields |
| Modify | `crates/scorpio-core/src/data/yfinance/options.rs` | Add runtime-only `OptionsToolContext`, extract shared tool JSON serializer, replay prefetched outcomes in `GetOptionsSnapshot::call()` |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` | Replace raw-JSON-copy guidance with interpretation guidance and add tool-availability placeholder/note |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/bullish_researcher.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/bearish_researcher.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/debate_moderator.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/aggressive_risk.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/conservative_risk.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/neutral_risk.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/risk_moderator.md` | Add generic `technical_report.options_context` usage guidance |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/trader.md` | Add explicit supporting-evidence usage of `technical_report.options_context` |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md` | Add passive-consumer acknowledgement of `technical_report.options_context` |
| Modify | `crates/scorpio-core/src/workflow/tasks/analyst.rs` | Switch technical evidence dataset gating from `options_summary` to `options_context` |
| Modify | `crates/scorpio-core/src/workflow/pipeline/tests.rs` | Extend stale-state tests and add pipeline coverage for persisted `options_context` |
| Modify | `crates/scorpio-core/src/workflow/tasks/tests.rs` | Update technical evidence dataset tests to use `options_context` |
| Modify | `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs` | Prove additive `options_context` deserializes on the current schema version |
| Modify | `crates/scorpio-core/tests/state_roundtrip.rs` | Extend proptest generators and round-trip coverage for `options_context` |
| Modify | `crates/scorpio-core/src/testing/prompt_render.rs` | Update `sample_technical_data()` so prompt fixtures exercise `options_context` |
| Modify | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` | Add variant-name guard and regenerate affected fixtures |
| Modify | `crates/scorpio-core/tests/options_outcome_smoke.rs` | Keep smoke coverage on the serialized `get_options_snapshot` JSON contract after replay wiring |
| Modify | `crates/scorpio-core/src/agents/researcher/common.rs` | Add tests proving researcher analyst context includes serialized `options_context` |
| Modify | `crates/scorpio-core/src/agents/risk/common.rs` | Add tests proving risk analyst context includes serialized `options_context` |
| Modify | `crates/scorpio-core/src/agents/trader/prompt.rs` | Project compact `technical_report.options_context` for trader prompts and keep tests aligned |
| Modify | `crates/scorpio-core/src/agents/fund_manager/prompt.rs` | Project compact technical data for fund-manager prompt user context |
| Modify | `crates/scorpio-core/src/agents/analyst/mod.rs` | Update `TechnicalData` literals to include `options_context: None` |
| Modify | `crates/scorpio-core/src/agents/trader/tests.rs` | Update `TechnicalData` literals to include `options_context: None` |
| Modify | `crates/scorpio-core/src/agents/fund_manager/tests.rs` | Update `TechnicalData` literals to include `options_context: None` |
| Modify | `crates/scorpio-core/src/agents/fund_manager/prompt.rs` | Update `TechnicalData` test literals to include `options_context: None` |
| Modify | `crates/scorpio-core/src/indicators/batch.rs` | Update `TechnicalData` literal to include `options_context: None` |
| Modify | `crates/scorpio-core/src/workflow/tasks/test_helpers.rs` | Update `TechnicalData` literal to include `options_context: None` |
| Modify | `crates/scorpio-core/tests/workflow_pipeline_structure.rs` | Update `TechnicalData` literal to include `options_context: None` |
| Modify | `crates/scorpio-core/tests/support/workflow_observability_task_support.rs` | Update `TechnicalData` literal to include `options_context: None` |

## Literal Update Surfaces

- `TechnicalData` constructor sites that must add `options_context: None` or a concrete test value:
  - `crates/scorpio-core/src/agents/analyst/equity/technical.rs`
  - `crates/scorpio-core/src/agents/analyst/mod.rs`
  - `crates/scorpio-core/src/agents/trader/tests.rs`
  - `crates/scorpio-core/src/agents/fund_manager/tests.rs`
  - `crates/scorpio-core/src/agents/fund_manager/prompt.rs`
  - `crates/scorpio-core/src/indicators/batch.rs`
  - `crates/scorpio-core/src/testing/prompt_render.rs`
  - `crates/scorpio-core/src/workflow/pipeline/tests.rs`
  - `crates/scorpio-core/src/workflow/tasks/test_helpers.rs`
  - `crates/scorpio-core/src/workflow/tasks/tests.rs`
  - `crates/scorpio-core/tests/state_roundtrip.rs`
  - `crates/scorpio-core/tests/workflow_pipeline_structure.rs`
  - `crates/scorpio-core/tests/support/workflow_observability_task_support.rs`

- Before the Task 1 green slice, confirm you did not miss a constructor:

```bash
rg -n "options_summary:" crates/scorpio-core crates/scorpio-cli crates/scorpio-reporters
```

## Chunk 1: State Surface And Single-Fetch Runtime Contract

### Task 1: Audit `options_summary` consumers and add snapshot-safe `options_context`

**Files:**
- Modify: `crates/scorpio-core/src/state/technical.rs`
- Modify: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/tests.rs`
- Modify: `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`
- Modify: `crates/scorpio-core/tests/state_roundtrip.rs`
- Modify: all `TechnicalData` literal sites listed above

- [ ] **Step 1: Audit all `options_summary` readers before changing semantics**

Run:

```bash
rg -n "options_summary" crates/scorpio-core crates/scorpio-cli crates/scorpio-reporters
```

Expected: plain presence checks, serialization, reporting display, or test fixtures only. Record the audited hits, especially any CLI/reporting, snapshot, or thesis-memory readers. If any reader parses `options_summary` as JSON or depends on its old raw-tool-output format, stop and amend this plan before implementing further steps.

- [ ] **Step 2: Add the failing serde + stale-state regressions**

In `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, add:

```rust
#[test]
fn technical_data_missing_options_context_defaults_to_none() {
    let json = r#"{
        "rsi": 55.0,
        "macd": null,
        "atr": null,
        "sma_20": null,
        "sma_50": null,
        "ema_12": null,
        "ema_26": null,
        "bollinger_upper": null,
        "bollinger_lower": null,
        "support_level": null,
        "resistance_level": null,
        "volume_avg": null,
        "summary": "legacy technical payload",
        "options_summary": null
    }"#;

    let data: TechnicalData = serde_json::from_str(json).expect("legacy payload should deserialize");
    assert!(data.options_context.is_none());
}
```

In `crates/scorpio-core/src/workflow/pipeline/tests.rs`, add:

```rust
#[test]
fn clear_equity_resets_options_context_unit() {
    let mut state = TradingState::new("AAPL", "2026-01-01");
    state.set_technical_indicators(TechnicalData {
        // ... existing fields ...
        summary: "stale".to_owned(),
        options_summary: Some("stale interpretation".to_owned()),
        options_context: Some(TechnicalOptionsContext::FetchFailed {
            reason: "stale provider failure".to_owned(),
        }),
    });

    state.clear_equity();
    assert!(state.technical_indicators().is_none());
}
```

In `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`, add:

```rust
#[tokio::test]
async fn additive_options_context_field_does_not_require_schema_bump() {
    // Write a current-version snapshot row, strip `options_context` from stored JSON,
    // and prove the loader still returns the thesis.
}
```

- [ ] **Step 3: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(technical_data_missing_options_context_defaults_to_none) | test(clear_equity_resets_options_context_unit) | test(additive_options_context_field_does_not_require_schema_bump) | binary(state_roundtrip)'
```

Expected: FAIL because `TechnicalData` does not yet include `options_context`.

- [ ] **Step 4: Add `TechnicalOptionsContext` and `TechnicalData.options_context`**

In `crates/scorpio-core/src/state/technical.rs`, add the additive persisted state shape:

```rust
use crate::data::OptionsOutcome;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TechnicalOptionsContext {
    Available { outcome: OptionsOutcome },
    FetchFailed {
        #[serde(default)]
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TechnicalData {
    // existing fields...
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_context: Option<TechnicalOptionsContext>,
}
```

Keep the new field additive-only. Do not touch `THESIS_MEMORY_SCHEMA_VERSION`.

- [ ] **Step 5: Update every `TechnicalData` constructor and generator**

At each literal site, add `options_context: None` unless the test is explicitly exercising shared options behavior.

Update `crates/scorpio-core/tests/state_roundtrip.rs::arb_technical_data()` to generate an optional `TechnicalOptionsContext` value alongside `options_summary`.

- [ ] **Step 6: Re-run the focused green slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(technical_data_missing_options_context_defaults_to_none) | test(clear_equity_resets_options_context_unit) | test(additive_options_context_field_does_not_require_schema_bump) | binary(state_roundtrip)'
```

Expected: PASS.

- [ ] **Step 7: Commit the additive technical-state foundation**

Run:

```bash
git add crates/scorpio-core/src/state/technical.rs \
        crates/scorpio-core/src/agents/analyst/equity/technical.rs \
        crates/scorpio-core/src/workflow/pipeline/tests.rs \
        crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs \
        crates/scorpio-core/tests/state_roundtrip.rs \
        crates/scorpio-core/src/agents/analyst/mod.rs \
        crates/scorpio-core/src/agents/trader/tests.rs \
        crates/scorpio-core/src/agents/fund_manager/tests.rs \
        crates/scorpio-core/src/agents/fund_manager/prompt.rs \
        crates/scorpio-core/src/indicators/batch.rs \
        crates/scorpio-core/src/testing/prompt_render.rs \
        crates/scorpio-core/src/workflow/tasks/test_helpers.rs \
        crates/scorpio-core/src/workflow/tasks/tests.rs \
        crates/scorpio-core/tests/workflow_pipeline_structure.rs \
        crates/scorpio-core/tests/support/workflow_observability_task_support.rs && \
git commit -m "feat(core): add persisted technical options context"
```

### Task 2: Split `TechnicalAnalystResponse` from persisted `TechnicalData`

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`

- [ ] **Step 1: Add the failing response/merge regressions**

In `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, add:

```rust
#[test]
fn parse_technical_response_accepts_options_summary_without_options_context() {
    let json = r#"{
        "rsi": 52.0,
        "macd": null,
        "atr": null,
        "sma_20": null,
        "sma_50": null,
        "ema_12": null,
        "ema_26": null,
        "bollinger_upper": null,
        "bollinger_lower": null,
        "support_level": null,
        "resistance_level": null,
        "volume_avg": null,
        "summary": "Moderate bullish trend.",
        "options_summary": "Near-term IV remains elevated into earnings."
    }"#;

    let data = parse_technical_response(json).expect("response should parse");
    assert_eq!(data.options_summary.as_deref(), Some("Near-term IV remains elevated into earnings."));
}

#[test]
fn assemble_technical_data_keeps_options_summary_for_live_snapshot() {
    let response = sample_technical_response_with_options_summary();
    let data = assemble_technical_data(
        response,
        Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(sample_options_snapshot()),
        }),
    );
    assert!(data.options_summary.is_some());
}

#[test]
fn assemble_technical_data_clears_options_summary_for_non_snapshot_outcome() {
    let response = sample_technical_response_with_options_summary();
    let data = assemble_technical_data(
        response,
        Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::HistoricalRun,
        }),
    );
    assert!(data.options_summary.is_none());
}
```

- [ ] **Step 2: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(parse_technical_response_accepts_options_summary_without_options_context) | test(assemble_technical_data_keeps_options_summary_for_live_snapshot) | test(assemble_technical_data_clears_options_summary_for_non_snapshot_outcome)'
```

Expected: FAIL because `TechnicalAnalystResponse`, `parse_technical_response`, and `assemble_technical_data` do not exist yet.

- [ ] **Step 3: Introduce `TechnicalAnalystResponse` and the merge helper**

In `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, replace direct LLM parsing into `TechnicalData` with:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct TechnicalAnalystResponse {
    rsi: Option<f64>,
    macd: Option<MacdValues>,
    atr: Option<f64>,
    sma_20: Option<f64>,
    sma_50: Option<f64>,
    ema_12: Option<f64>,
    ema_26: Option<f64>,
    bollinger_upper: Option<f64>,
    bollinger_lower: Option<f64>,
    support_level: Option<f64>,
    resistance_level: Option<f64>,
    volume_avg: Option<f64>,
    summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    options_summary: Option<String>,
}

fn assemble_technical_data(
    response: TechnicalAnalystResponse,
    options_context: Option<TechnicalOptionsContext>,
) -> TechnicalData {
    let keep_options_summary = matches!(
        options_context,
        Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(_),
        })
    );

    TechnicalData {
        // copy scalar fields from response...
        options_summary: keep_options_summary.then_some(response.options_summary).flatten(),
        options_context,
    }
}
```

Move the current parse/validate logic to `TechnicalAnalystResponse`, not `TechnicalData`.

- [ ] **Step 4: Keep the existing MACD validation behavior intact**

Update the current `parse_technical(...)` / `validate_technical(...)` tests to target the new response parser/validator so scalar-MACD and summary-shape protections remain identical after the split.

- [ ] **Step 5: Re-run the focused green slice**

Run the command from Step 2 again.

Expected: PASS.

- [ ] **Step 6: Commit the analyst-response split**

Run:

```bash
git add crates/scorpio-core/src/agents/analyst/equity/technical.rs && \
git commit -m "refactor(core): split technical analyst response from state"
```

### Task 3: Add `OptionsToolContext` and replay prefetched tool results

**Files:**
- Modify: `crates/scorpio-core/src/data/yfinance/options.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/mod.rs` if a crate-private re-export is the narrowest practical import seam for `technical.rs`

- [ ] **Step 1: Add the failing context/replay regressions**

In `crates/scorpio-core/src/data/yfinance/options.rs`, add:

```rust
#[tokio::test]
async fn options_tool_context_loads_prefetched_outcome() {
    let ctx = OptionsToolContext::new();
    ctx.store(OptionsOutcome::HistoricalRun).await.expect("store once");
    assert_eq!(*ctx.load().await.expect("load stored outcome"), OptionsOutcome::HistoricalRun);
}

#[tokio::test]
async fn get_options_snapshot_replays_prefetched_snapshot_without_refetch() {
    let ctx = OptionsToolContext::new();
    ctx.store(OptionsOutcome::Snapshot(sample_snapshot())).await.unwrap();

    let tool = GetOptionsSnapshot::scoped_prefetched("AAPL", today_eastern(), ctx.clone());
    let result = rig::tool::Tool::call(&tool, OptionsSnapshotArgs {
        symbol: "AAPL".to_owned(),
        target_date: today_eastern(),
    }).await.expect("prefetched replay should succeed");

    assert_eq!(result["kind"], "snapshot");
}

#[tokio::test]
async fn get_options_snapshot_replays_prefetched_historical_run_with_reason() {
    let ctx = OptionsToolContext::new();
    ctx.store(OptionsOutcome::HistoricalRun).await.unwrap();

    let tool = GetOptionsSnapshot::scoped_prefetched("AAPL", yesterday_eastern(), ctx.clone());
    let result = rig::tool::Tool::call(&tool, OptionsSnapshotArgs {
        symbol: "AAPL".to_owned(),
        target_date: yesterday_eastern(),
    }).await.expect("prefetched replay should succeed");

    assert_eq!(result["kind"], "historical_run");
    assert!(result.get("reason").is_some());
}
```

- [ ] **Step 2: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(options_tool_context_loads_prefetched_outcome) | test(get_options_snapshot_replays_prefetched_snapshot_without_refetch) | test(get_options_snapshot_replays_prefetched_historical_run_with_reason)'
```

Expected: FAIL because `OptionsToolContext`, `scoped_prefetched`, and replay logic do not exist yet.

- [ ] **Step 3: Add `OptionsToolContext` and extract shared tool JSON serialization**

In `crates/scorpio-core/src/data/yfinance/options.rs`, add a write-once runtime cache:

```rust
#[derive(Debug, Clone, Default)]
pub struct OptionsToolContext {
    outcome: Arc<RwLock<Option<Arc<OptionsOutcome>>>>,
}

impl OptionsToolContext {
    pub fn new() -> Self { Self::default() }

    pub async fn store(&self, outcome: OptionsOutcome) -> Result<(), TradingError> {
        // write-once semantics, mirroring OhlcvToolContext
    }

    pub async fn load(&self) -> Result<Arc<OptionsOutcome>, TradingError> {
        // fail if the context is empty
    }
}

fn serialize_options_outcome_for_tool(outcome: &OptionsOutcome) -> Result<serde_json::Value, TradingError> {
    // move the existing reason-injection logic here
}
```

Mirror `OhlcvToolContext` semantics: write once, cheap `Arc` clone on read, and a schema-violation error when the context is empty.

- [ ] **Step 4: Update `GetOptionsSnapshot` to prefer context replay**

Add an optional `context: Option<OptionsToolContext>` field and a new constructor:

```rust
pub fn scoped_prefetched(
    symbol: impl Into<String>,
    target_date: impl Into<String>,
    context: OptionsToolContext,
) -> Self
```

Keep the existing provider-backed constructor intact for direct/live use. In `call()`, check `context` first, fall back to `provider.fetch_snapshot()` only when no prefetched context is present, and route both paths through `serialize_options_outcome_for_tool()`.

- [ ] **Step 5: Preserve the current serialized tool contract in the existing smoke test**

Update `crates/scorpio-core/tests/options_outcome_smoke.rs` only if needed so it still exercises the provider-backed `GetOptionsSnapshot::scoped(...)` path and verifies:

- `Snapshot` output has no injected `reason`
- every non-`Snapshot` outcome still carries a human-readable `reason`
- the existing `kind` discriminants stay unchanged

This slice must not silently change the external tool JSON contract while adding context replay.

- [ ] **Step 6: Keep the context local to this flow**

Do not re-export `OptionsToolContext` through `data/mod.rs`. Keep it crate-private and import it through the narrowest seam needed by `technical.rs`.

- [ ] **Step 7: Re-run the focused green slice**

Run the command from Step 2 again.

Expected: PASS.

- [ ] **Step 8: Re-run the serialized-tool smoke test**

Run:

```bash
cargo nextest run -p scorpio-core --test options_outcome_smoke --features test-helpers
```

Expected: PASS.

- [ ] **Step 9: Commit the replayable options-tool context**

Run:

```bash
git add crates/scorpio-core/src/data/yfinance/options.rs \
        crates/scorpio-core/src/data/yfinance/mod.rs && \
git commit -m "feat(core): replay prefetched options outcomes in technical tools"
```

## Chunk 2: Technical Runtime Wiring And Downstream Prompt Rollout

### Task 4: Prefetch options once in `TechnicalAnalyst::run()` and condition the technical prompt

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`
- Modify: `crates/scorpio-core/src/testing/prompt_render.rs`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md`
- Modify: `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt`

- [ ] **Step 1: Extract a small test seam before writing runtime regressions**

Because `TechnicalAnalyst::run()` currently constructs a real tool-enabled agent and there is no existing way to feed it a mocked successful LLM response, first extract a small helper in `technical.rs` that owns:

- prefetch outcome classification (`Available` vs `FetchFailed`)
- tool-availability decision
- concrete `GetOptionsSnapshot::scoped_prefetched(...)` construction when applicable
- sanitized `FetchFailed.reason`

Keep the helper local to this module and have `run()` call it.

- [ ] **Step 2: Add the failing prompt-conditioning and helper-level runtime regressions**

In `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, add:

```rust
#[test]
fn build_technical_system_prompt_includes_options_guidance_when_tool_available() {
    let policy = resolve_runtime_policy("baseline").unwrap();
    let prompt = build_technical_system_prompt("AAPL", "2026-01-01", &policy, true);
    assert!(prompt.contains("get_options_snapshot"));
}

#[test]
fn build_technical_system_prompt_omits_options_guidance_when_tool_unavailable() {
    let policy = resolve_runtime_policy("baseline").unwrap();
    let prompt = build_technical_system_prompt("AAPL", "2026-01-01", &policy, false);
    assert!(!prompt.contains("call once with the same symbol and date"));
    assert!(prompt.contains("live options provider was unavailable"));
}

#[tokio::test]
async fn prepare_options_runtime_persists_fetch_failed_context_and_omits_tool() {
    // feed the helper an Err(...) prefetch result
    // assert options_context == FetchFailed { reason }, tool_available == false, and no tool is returned
}

#[tokio::test]
async fn prepare_options_runtime_keeps_tool_available_for_historical_run() {
    // feed the helper Ok(OptionsOutcome::HistoricalRun)
    // assert tool_available == true, the prefetched tool is returned, and calling it replays historical_run
}

#[test]
fn assemble_technical_data_clears_options_summary_when_outcome_is_not_snapshot() {
    // prefetch HistoricalRun, return an LLM response that tries to set options_summary,
    // assert assemble_technical_data clears it before persistence
}
```

- [ ] **Step 3: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_technical_system_prompt_includes_options_guidance_when_tool_available) | test(build_technical_system_prompt_omits_options_guidance_when_tool_unavailable) | test(prepare_options_runtime_persists_fetch_failed_context_and_omits_tool) | test(prepare_options_runtime_keeps_tool_available_for_historical_run) | test(assemble_technical_data_clears_options_summary_when_outcome_is_not_snapshot)'
```

Expected: FAIL because the helper seam does not exist yet, successful non-snapshot outcomes are not yet kept on the tool-available path, and `build_technical_system_prompt(...)` does not yet accept a tool-availability flag.

- [ ] **Step 4: Make the technical prompt conditional on tool availability**

Because the current implementation renders `system_prompt` in `TechnicalAnalyst::new()`, but tool availability is only known after the prefetch inside `run()`, make this refactor explicit before wiring behavior:

- either store `RuntimePolicy` on `TechnicalAnalyst` and render the system prompt inside `run()` after prefetch
- or replace the stored `system_prompt: String` field with enough prompt inputs to render on demand in `run()`

Do not guess tool availability in `new()`.

Change `build_technical_system_prompt(...)` to accept an `options_tool_available: bool` flag and use a placeholder in `technical_analyst.md`, for example:

```rust
pub(crate) fn build_technical_system_prompt(
    symbol: &str,
    target_date: &str,
    policy: &RuntimePolicy,
    options_tool_available: bool,
) -> String {
    let tool_note = if options_tool_available {
        "- `get_options_snapshot` — call once with the same symbol and date; only valid for today's US/Eastern date"
    } else {
        "- Live options provider unavailable for this run. Do not mention `get_options_snapshot`, and do not emit `options_summary`."
    };

    render_analyst_system_prompt(...).replace("{options_tool_note}", tool_note)
}
```

- [ ] **Step 5: Use the helper seam to prefetch once, bind tools conditionally, and assemble persisted `TechnicalData`**

Inside `TechnicalAnalyst::run()`:

```rust
let prefetched = options_provider.fetch_snapshot(&self.typed_symbol, &self.target_date).await;

let prepared = prepare_options_runtime(
    prefetched,
    &self.symbol,
    &self.target_date,
)?;

let system_prompt = build_technical_system_prompt(..., prepared.options_tool_available);
let outcome = run_analyst_inference::<TechnicalAnalystResponse, _, _>(...);
let technical = assemble_technical_data(outcome.output, prepared.options_context);
```

Do **not** return a tool error on prefetch failure. Omit the tool and keep the run fail-open.
Use `crate::providers::factory::sanitize_error_summary` (already exported) so persisted `FetchFailed.reason` stays human-readable and secret-safe.
Successful non-snapshot outcomes such as `HistoricalRun` must still take the tool-available path; only true prefetch errors should take the unavailable-provider branch.

- [ ] **Step 6: Run the prompt gate before updating fixtures**

Run:

```bash
cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
```

Expected: FAIL because `technical_analyst.md` changed intentionally.

- [ ] **Step 7: Regenerate only the technical fixture for this chunk**

Run:

```bash
UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
git status --short crates/scorpio-core/tests/fixtures/prompt_bundle/
```

Expected: only `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt` changed in this step. If any downstream fixture changed already, stop and inspect prompt drift before proceeding.

- [ ] **Step 8: Re-run the focused green slice and prompt gate**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_technical_system_prompt_includes_options_guidance_when_tool_available) | test(build_technical_system_prompt_omits_options_guidance_when_tool_unavailable) | test(prepare_options_runtime_persists_fetch_failed_context_and_omits_tool) | test(prepare_options_runtime_keeps_tool_available_for_historical_run) | test(assemble_technical_data_clears_options_summary_when_outcome_is_not_snapshot)'
cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
```

Expected: PASS.

- [ ] **Step 9: Commit the single-fetch technical runtime and prompt together**

Run:

```bash
git add crates/scorpio-core/src/agents/analyst/equity/technical.rs \
        crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md \
        crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt \
        crates/scorpio-core/src/testing/prompt_render.rs && \
git commit -m "feat(core): prefetch and persist technical options context"
```

### Task 5: Project compact `technical_report.options_context` at each downstream serializer seam

**Files:**
- Modify: `crates/scorpio-core/src/agents/researcher/common.rs`
- Modify: `crates/scorpio-core/src/agents/risk/common.rs`
- Modify: `crates/scorpio-core/src/agents/trader/prompt.rs`
- Modify: `crates/scorpio-core/src/agents/fund_manager/prompt.rs`

- [ ] **Step 1: Add the failing projection regressions at each downstream seam**

Use a technical fixture whose `options_context` contains a non-empty `near_term_strikes` array so the tests can detect accidental full-shape serialization.
Also add one legacy compatibility fixture where `options_context = None` and `options_summary` contains the old raw-JSON blob string, then assert the downstream serializer surfaces still render coherent prompt context without treating that blob as authoritative structured state.

Add focused tests proving the downstream serializers:

- still include `options_context`
- include the compact fields needed for reasoning (`status`, `kind`, `atm_iv`, `put_call_volume_ratio`, `put_call_oi_ratio`, `max_pain_strike`, `near_term_expiration`)
- do **not** dump the raw `near_term_strikes` array verbatim into downstream prompts

- [ ] **Step 2: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(researcher_analyst_context_projects_options_context) | test(risk_analyst_context_projects_options_context) | test(trader_prompt_context_projects_options_context) | test(fund_manager_prompt_projects_options_context)'
```

Expected: FAIL because the downstream serializers still serialize `state.technical_indicators()` directly.

- [ ] **Step 3: Implement compact role-local projection**

At each serializer seam, keep the existing `technical_report` injection path but replace direct serialization of `state.technical_indicators()` with a compact projected value that preserves:

- `summary`
- `options_summary`
- `options_context.status`
- `options_context.outcome.kind`
- snapshot-only fields directly useful to downstream reasoning: `atm_iv`, `put_call_volume_ratio`, `put_call_oi_ratio`, `max_pain_strike`, `near_term_expiration`, and a small strike-interest summary derived from `near_term_strikes`

Do not include the raw `near_term_strikes` array in downstream prompts. Keep the full `OptionsOutcome` only in persisted `TechnicalData` and in the Technical Analyst tool/runtime path.

- [ ] **Step 4: Re-run the focused green slice**

Run the command from Step 2 again.

Expected: PASS.

- [ ] **Step 5: Commit the downstream projection seam**

Run:

```bash
git add crates/scorpio-core/src/agents/researcher/common.rs \
        crates/scorpio-core/src/agents/risk/common.rs \
        crates/scorpio-core/src/agents/trader/prompt.rs \
        crates/scorpio-core/src/agents/fund_manager/prompt.rs && \
git commit -m "feat(core): project shared options context for downstream prompts"
```

### Task 6: Teach downstream prompts to use `technical_report.options_context`

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/bullish_researcher.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/bearish_researcher.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/debate_moderator.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/aggressive_risk.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/conservative_risk.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/neutral_risk.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/risk_moderator.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/trader.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md`
- Modify: `crates/scorpio-core/src/testing/prompt_render.rs`
- Modify: `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`
- Modify: affected fixtures under `crates/scorpio-core/tests/fixtures/prompt_bundle/`

- [ ] **Step 1: Add the failing downstream prompt/context regressions**

In `crates/scorpio-core/src/agents/researcher/common.rs`, add or update:

```rust
#[test]
fn researcher_analyst_context_includes_options_context() {
    let mut state = TradingState::new("AAPL", "2026-01-15");
    state.set_technical_indicators(sample_technical_with_options_context_for_projection_tests());
    let rendered = build_analyst_context(&state);
    assert!(rendered.contains("options_context"));
    assert!(rendered.contains("snapshot"));
}
```

In `crates/scorpio-core/src/agents/risk/common.rs`, add the same style test:

```rust
#[test]
fn risk_analyst_context_includes_options_context() { /* same assertion style */ }
```

In `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`, add:

```rust
#[test]
fn options_outcome_variants_are_named_in_all_branching_prompts() {
    let required_tokens = [
        "snapshot",
        "no_listed_instrument",
        "sparse_chain",
        "historical_run",
        "missing_spot",
        "fetch_failed",
    ];

    for role in [
        Role::BullishResearcher,
        Role::BearishResearcher,
        Role::DebateModerator,
        Role::Trader,
        Role::AggressiveRisk,
        Role::ConservativeRisk,
        Role::NeutralRisk,
        Role::RiskModerator,
    ] {
        let rendered = render_baseline_prompt_for_role(role, PromptRenderScenario::AllInputsPresent);
        for token in required_tokens {
            assert!(rendered.contains(token), "{role:?} prompt must mention {token}");
        }
    }
}
```

Keep `FundManager` out of this exhaustive-variant guard unless the prompt is intentionally changed to branch on `outcome.kind`. The approved spec only requires passive acknowledgement there.
Strengthen this regression beyond token presence where practical: for the branching roles, also assert the prompt tells the agent to inspect `technical_report.options_context.outcome.kind` and to treat `options_summary` as supplemental rather than authoritative.

- [ ] **Step 2: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(researcher_analyst_context_includes_options_context) | test(risk_analyst_context_includes_options_context) | test(options_outcome_variants_are_named_in_all_branching_prompts)'
```

Expected: FAIL because downstream prompt markdown and fixture guard do not yet mention the options variants.

- [ ] **Step 3: Update the downstream prompt markdown with minimal generic guidance**

For each downstream prompt file, add small explicit instructions that:

- reference `technical_report.options_context`
- tell the agent to branch on `technical_report.options_context.outcome.kind`
- treat `technical_report.options_context.status == "fetch_failed"` and `null` as explicit missing options context
- treat `options_summary` as supplemental interpretation, not authority

Apply the `outcome.kind` branching requirement to the researcher, risk, and trader prompts. Keep the fund-manager prompt narrower: acknowledge that `technical_report` may include structured options context and an analyst interpretation, but do not require exhaustive branching language there.

Keep the guidance generic. Do **not** add new role-specific heuristics beyond the approved design.

- [ ] **Step 4: Make the prompt fixtures exercise `options_context`**

In `crates/scorpio-core/src/testing/prompt_render.rs::sample_technical_data()`, replace the all-`None` options fields with a representative fixture value, for example:

```rust
options_summary: Some("Near-term IV remains elevated, but the front-month term structure is orderly.".to_owned()),
options_context: Some(TechnicalOptionsContext::Available {
    outcome: OptionsOutcome::Snapshot(OptionsSnapshot {
        spot_price: 180.0,
        atm_iv: 0.28,
        iv_term_structure: vec![IvTermPoint { expiration: FIXTURE_DATE.to_owned(), atm_iv: 0.28 }],
        put_call_volume_ratio: 1.1,
        put_call_oi_ratio: 1.0,
        max_pain_strike: 180.0,
        near_term_expiration: FIXTURE_DATE.to_owned(),
        near_term_strikes: vec![],
    }),
}),
```

This is for prompt-fixture coverage only. Keep it compact and deterministic.

- [ ] **Step 5: Run the prompt gate without fixture updates**

Run:

```bash
cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
```

Expected: FAIL because the downstream prompt markdown changed intentionally.

- [ ] **Step 6: Regenerate fixtures and assert the diff stays scoped**

Run:

```bash
UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
git status --short crates/scorpio-core/tests/fixtures/prompt_bundle/
```

Expected changed fixtures:

- `bullish_researcher.txt`
- `bearish_researcher.txt`
- `debate_moderator.txt`
- `aggressive_risk.txt`
- `conservative_risk.txt`
- `neutral_risk.txt`
- `risk_moderator.txt`
- `trader.txt`
- `fund_manager.txt`
- `user/trader_all_inputs_present_user.txt`
- `user/trader_zero_debate_user.txt`
- `user/trader_missing_analyst_data_user.txt`
- `user/fund_manager_all_inputs_present_user.txt`
- `user/fund_manager_zero_risk_user.txt`
- `user/fund_manager_missing_analyst_data_user.txt`

If any other fixture changes, inspect the prompt-render surface before committing.

- [ ] **Step 7: Re-run the focused green slice and prompt gate**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(researcher_analyst_context_includes_options_context) | test(risk_analyst_context_includes_options_context) | test(options_outcome_variants_are_named_in_all_branching_prompts)'
cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
```

Expected: PASS.

- [ ] **Step 8: Commit the downstream prompt rollout**

Run:

```bash
git add crates/scorpio-core/src/analysis_packs/equity/prompts/bullish_researcher.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/bearish_researcher.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/debate_moderator.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/aggressive_risk.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/conservative_risk.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/neutral_risk.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/risk_moderator.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/trader.md \
        crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md \
        crates/scorpio-core/src/agents/researcher/common.rs \
        crates/scorpio-core/src/agents/risk/common.rs \
        crates/scorpio-core/src/testing/prompt_render.rs \
        crates/scorpio-core/tests/prompt_bundle_regression_gate.rs \
        crates/scorpio-core/tests/fixtures/prompt_bundle && \
git commit -m "feat(prompts): teach downstream agents to use shared options context"
```

### Task 7: Switch technical evidence datasets to `options_context`

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/tests.rs`

- [ ] **Step 1: Add the failing technical-evidence regression**

In `crates/scorpio-core/src/workflow/tasks/tests.rs`, replace the existing options-summary dataset test with:

```rust
#[tokio::test]
async fn technical_evidence_includes_options_context_dataset_when_options_context_present() {
    // Case 1: options_context present -> datasets = ["ohlcv", "options_context"]
    // Case 2: options_context absent  -> datasets = ["ohlcv"]
}
```

Populate `TechnicalData.options_context` with a real `TechnicalOptionsContext::Available { outcome: OptionsOutcome::HistoricalRun }` or `Snapshot(_)` value. Do not use `options_summary` as the gate.

- [ ] **Step 2: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(technical_evidence_includes_options_context_dataset_when_options_context_present)'
```

Expected: FAIL because `AnalystSyncTask` still pushes `options_snapshot` when `options_summary.is_some()`.

- [ ] **Step 3: Update `AnalystSyncTask` dataset gating**

In `crates/scorpio-core/src/workflow/tasks/analyst.rs`, change the technical source list to:

```rust
let mut datasets = vec!["ohlcv".to_owned()];
if data.options_context.is_some() {
    datasets.push("options_context".to_owned());
}
```

Keep a single Yahoo `EvidenceSource` and keep `fetched_at` on its current merge-time semantics.

- [ ] **Step 4: Re-run the focused green slice**

Run the command from Step 2 again.

Expected: PASS.

- [ ] **Step 5: Commit the technical evidence update**

Run:

```bash
git add crates/scorpio-core/src/workflow/tasks/analyst.rs \
        crates/scorpio-core/src/workflow/tasks/tests.rs && \
git commit -m "feat(core): track options context in technical evidence"
```

## Chunk 3: Integration Coverage, Verification, And Execution Handoff

### Task 8: Add pipeline coverage for shared options context

**Files:**
- Modify: `crates/scorpio-core/src/workflow/pipeline/tests.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/test_helpers.rs`

- [ ] **Step 1: Add the failing pipeline regressions**

Before writing the pipeline assertions, add a small test-only seam to `crates/scorpio-core/src/workflow/tasks/test_helpers.rs` so the stubbed technical analyst child can accept caller-supplied `TechnicalData` instead of always using the baked-in neutral fixture.

In `crates/scorpio-core/src/workflow/pipeline/tests.rs`, add:

```rust
#[tokio::test]
async fn run_analysis_cycle_preserves_options_context_in_technical_state() {
    // Stub the technical analyst child result with TechnicalData {
    //   options_context: Some(TechnicalOptionsContext::Available { outcome: OptionsOutcome::Snapshot(...) }),
    //   options_summary: Some("..."),
    // }
    // Run the pipeline and assert final_state.technical_indicators().unwrap().options_context is Some(...).
    // Then build the trader prompt context and assert the rendered prompt contains "options_context".
}

#[tokio::test]
async fn run_analysis_cycle_preserves_fetch_failed_options_context_and_coherent_prompt() {
    // Stub the technical analyst child result with TechnicalData {
    //   options_context: Some(TechnicalOptionsContext::FetchFailed { reason: "timeout".to_owned() }),
    //   options_summary: None,
    // }
    // Run the pipeline and assert final_state carries FetchFailed and downstream prompt rendering succeeds.
}
```

Use the existing pipeline stub helpers instead of invoking a live LLM path here. The technical-runtime prefetch behavior is already covered by Task 4 unit tests.

- [ ] **Step 2: Run the focused red slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(run_analysis_cycle_preserves_options_context_in_technical_state) | test(run_analysis_cycle_preserves_fetch_failed_options_context_and_coherent_prompt)'
```

Expected: FAIL because the new `options_context` assertions and prompt expectations are not yet implemented.

- [ ] **Step 3: Implement the minimal pipeline assertions and supporting test data**

Use the existing stubbed fan-out path, seed `TechnicalData.options_context` directly in the technical child result, and assert:

- `final_state.technical_indicators().unwrap().options_context` survives the cycle
- the downstream prompt built from `final_state` contains serialized `options_context`
- `FetchFailed { reason }` does not break downstream prompt construction

Prefer asserting downstream prompt rendering through the existing helper surfaces already used by fixture tests, such as `build_trader_prompt_context_for_test` or `render_prompt_output_for_role`, instead of inventing a new ad hoc serializer in the test.

- [ ] **Step 4: Add one single-fetch guarantee regression at the helper seam**

In `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, add one test around the extracted prefetch/assembly helpers proving:

- the prefetched options outcome is the one bound into `GetOptionsSnapshot`
- the same prefetched outcome is what becomes persisted `TechnicalData.options_context`
- no second provider fetch is required once the prefetched outcome exists

Keep this test at the helper seam if mocking full `TechnicalAnalyst::run()` would require disproportionate refactoring.

- [ ] **Step 5: Re-run the focused green slice**

Run the command from Step 2 again.

Expected: PASS.

- [ ] **Step 6: Commit the integration coverage**

Run:

```bash
git add crates/scorpio-core/src/workflow/pipeline/tests.rs \
        crates/scorpio-core/src/workflow/tasks/test_helpers.rs \
        crates/scorpio-core/src/agents/analyst/equity/technical.rs && \
git commit -m "test(core): add shared options context pipeline coverage"
```

### Task 9: Run final verification and hand off execution cleanly

**Files:**
- No planned file edits in this task unless verification exposes a real bug.

- [ ] **Step 1: Re-run a focused confidence slice across the new surfaces**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(technical_data_missing_options_context_defaults_to_none) | test(assemble_technical_data_keeps_options_summary_for_live_snapshot) | test(assemble_technical_data_clears_options_summary_for_non_snapshot_outcome) | test(get_options_snapshot_replays_prefetched_snapshot_without_refetch) | test(prepare_options_runtime_persists_fetch_failed_context_and_omits_tool) | test(prepare_options_runtime_keeps_tool_available_for_historical_run) | test(build_technical_system_prompt_omits_options_guidance_when_tool_unavailable) | test(researcher_analyst_context_includes_options_context) | test(risk_analyst_context_includes_options_context) | test(technical_evidence_includes_options_context_dataset_when_options_context_present) | test(run_analysis_cycle_preserves_options_context_in_technical_state) | test(run_analysis_cycle_preserves_fetch_failed_options_context_and_coherent_prompt) | binary(state_roundtrip)'
cargo nextest run -p scorpio-core --test options_outcome_smoke --features test-helpers
```

Expected: PASS.

- [ ] **Step 2: Re-run the prompt bundle regression gate**

Run:

```bash
cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
```

Expected: PASS.

- [ ] **Step 3: Run formatting exactly as CI does**

Run:

```bash
cargo fmt -- --check
```

Expected: PASS.

- [ ] **Step 4: Run clippy exactly as CI does**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Run nextest exactly as CI does**

Run:

```bash
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

Expected: PASS.

- [ ] **Step 6: Inspect the final worktree state**

Run:

```bash
git status --short
```

Expected: only the planned shared-options-evidence changes remain.

- [ ] **Step 7: Create one small follow-up commit only if verification forced code changes**

If Steps 2-5 required additional fixes, stage only those fixes and create one small follow-up commit. Otherwise keep the branch as the task-by-task commit stack above.

- [ ] **Step 8: Execute with fresh subagents, one task at a time**

Use `superpowers:subagent-driven-development` from the dedicated `feature/shared-options-evidence` worktree. Execute one task per fresh subagent, keep the focused test commands and commit boundaries above, and do not stop before Steps 2-5 are green.
