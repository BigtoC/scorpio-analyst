# Design — shared options evidence for downstream agents

**Date:** 2026-04-28
**Author:** brainstorming session with BigtoC
**Status:** Draft — pending implementation plan

## Summary

Extend the existing equity-options integration from "Technical Analyst only" into a shared downstream input for the researcher, trader, risk, and fund-manager stages.

This is an expansion of the 2026-04-24 options design, not a reversal of its value judgment. The original scope was a complexity-management choice for v1: prove the new `OptionsProvider` and `GetOptionsSnapshot` path inside one agent before routing the data more broadly. Now that the provider contract exists and the Technical Analyst path is wired, the next step is to persist one Rust-owned options artifact in state so downstream agents can reason from the same substrate instead of relying on a model-copied string.

## Goals

- Preserve the existing Technical Analyst options workflow and keep `GetOptionsSnapshot` available during the analyst turn.
- Promote options data from a Technical-Analyst-local tool result into shared state consumed by downstream agents.
- Keep two distinct layers of value:
  - Rust-owned structured options facts and fetch status for shared reasoning.
  - Technical-Analyst-authored `options_summary` for concise interpretation.
- Reuse the current downstream prompt seam (`technical_report`) instead of introducing a new top-level routing system.
- Use a single options fetch/result per technical phase so the Technical Analyst tool and downstream state share the same artifact.
- Maintain snapshot compatibility with additive fields only.
- Stay fail-open when the live options fetch is unavailable. Intentional non-live skips remain represented as the existing successful `OptionsOutcome::HistoricalRun` path.

## Concrete downstream use cases

- Researcher debate: use options positioning and volatility regime as supporting or contrarian evidence instead of treating the Technical Analyst's prose as the only options input.
- Risk review: use explicit options context for post-event IV regime, degraded-chain conditions, and strike-interest context when evaluating proposed stops and confidence.
- Trader synthesis: use options context as supporting evidence behind valuation and debate consensus rather than as a hidden detail inside one analyst's string field.
- Fund manager review: remain a passive consumer, but see the same structured options context the earlier stages used.

## Non-goals

- No new top-level `TradingState` branch dedicated to options in this slice.
- No migration of options data through `data/routing.rs::derivatives` or the crypto-oriented `DerivativesProvider` placeholder.
- No new CLI report surface.
- No raw option-chain persistence. Downstream state shares the normalized options outcome plus explicit fetch-failure metadata, not raw chain data.
- No attempt to make the Technical Analyst's options interpretation authoritative. Downstream agents may use it, disagree with it, or ignore it when the structured evidence points elsewhere.

## Current-state observations

1. The live options provider already exists as `OptionsProvider::fetch_snapshot(...) -> Result<OptionsOutcome, TradingError>`.
2. The Technical Analyst already has access to `get_options_snapshot`.
3. `TechnicalData.options_summary` already flows to downstream agents today because researcher, trader, risk, and fund-manager prompts all serialize `technical_indicators()`.
4. The current Technical Analyst prompt uses `options_summary` as a transport hack: it tells the model to copy the raw tool JSON string into the field instead of writing an interpretation.
5. `TechnicalData` currently serves two roles at once:
  - the LLM output contract for the Technical Analyst
  - the persisted state contract shared with downstream agents

That last point is the main design constraint. If we add a Rust-owned structured options field directly to `TechnicalData` without changing the analyst-output path, the model would also be asked to generate that field.

## Design choices

| Decision | Choice | Rationale |
|---|---|---|
| Shared-data seam | Extend `TechnicalData` | All downstream agents already receive the serialized technical payload; reuse the existing seam |
| Structured field | Add `options_context: Option<TechnicalOptionsContext>` | Preserve both successful `OptionsOutcome` data and explicit fetch-failure state |
| Analyst interpretation | Keep `options_summary: Option<String>` | Downstream agents need the technical desk's read, but it should be separate from raw facts |
| Analyst output contract | Split from persisted state | Prevent the LLM from being asked to author the Rust-owned structured options field |
| Runtime ownership | Prefetch once into an `OptionsToolContext` shared by the tool and persisted state | Technical Analyst and downstream agents must reason from the same result |
| Prompt rollout | Update all downstream prompts with minimal generic options guidance | The point of this slice is shared usage, but prompt churn should stay narrow |
| Legacy compatibility | Normalize old `options_summary` transport blobs out of downstream prompt context | Old snapshots should not masquerade as fresh analyst interpretation |

## Architecture

### Data model

Keep `TechnicalData` as the persisted downstream-facing contract, but stop using it as the direct LLM output type.

```rust
// crates/scorpio-core/src/state/technical.rs

pub struct TechnicalData {
    // existing fields unchanged
    pub summary: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_summary: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options_context: Option<TechnicalOptionsContext>,
}

// crates/scorpio-core/src/agents/analyst/equity/technical.rs

struct TechnicalAnalystResponse {
    // same fields the model already owns today
    // includes options_summary, excludes options_context
}

#[serde(rename_all = "snake_case")]
enum TechnicalOptionsFetchFailureCode {
    ProviderError,
    Timeout,
    SchemaViolation,
}

#[serde(tag = "status", rename_all = "snake_case")]
enum TechnicalOptionsContext {
    Available { outcome: OptionsOutcome },
    FetchFailed {
        code: TechnicalOptionsFetchFailureCode,
        reason: String,
    },
}
```

`OptionsOutcome` is already the correct shared shape for this slice:

- `Snapshot(OptionsSnapshot)` when live normalized data exists.
- `NoListedInstrument` when the ticker has no listed options.
- `SparseChain` when options exist but the normalized NTM slice is not safe to use.
- `HistoricalRun` when the target date is not market-local today.
- `MissingSpot` when the underlying close is unavailable.

This preserves explicit absence semantics instead of collapsing everything into `None`.

### Why `options_context` instead of `options_snapshot`

The shared field is named `options_context` because downstream agents need more than the happy path. It needs to carry:

- successful `OptionsOutcome` values, including non-snapshot outcomes such as `HistoricalRun` or `NoListedInstrument`
- explicit fetch-failure state when Yahoo could not be reached or the call otherwise failed

A historical backtest run or a no-listed-instrument symbol still carries useful state, and a fetch failure must stay distinguishable from those successful no-snapshot outcomes.

### Why keep `options_summary`

The Technical Analyst should keep emitting a short options interpretation because downstream agents benefit from knowing how the technical desk read the options tape. But the field should return to its intended meaning:

- `options_context` = Rust-owned structured facts and fetch status
- `options_summary` = model-authored interpretation of those facts

The raw tool JSON should no longer be stuffed into `options_summary`.

### Canonical naming

In code, the persisted payload is `TechnicalData` reached through `state.technical_indicators()`.

In prompts, that same serialized payload is injected as `{technical_report}`.

This document uses:

- `technical_indicators()` when describing Rust call sites
- `technical_report.options_context` and `technical_report.options_summary` when describing prompt behavior

### Prompt serialization seam

Legacy suppression and prompt-shape normalization must happen in one shared serialization helper rather than ad hoc in each agent. The intended seam is a shared helper in `agents/shared/prompt.rs`, used by researcher, risk, trader, and fund-manager prompt builders, that:

- serializes `TechnicalData` for prompt injection
- suppresses legacy transport-style `options_summary` values when needed
- preserves the persisted state unchanged

This helper becomes the sole authoritative serializer for downstream technical prompt context. Researcher, risk, trader, and fund-manager prompt builders should all call it instead of directly serializing `state.technical_indicators()`.

## Runtime flow

### Technical Analyst phase

1. `TechnicalAnalyst::run()` is the sole owner of this options prefetch flow.
2. Before the Technical Analyst turn begins, `TechnicalAnalyst::run()` performs one scoped options fetch using `YFinanceOptionsProvider`.
3. `TechnicalAnalyst::run()` stores the result in a new analysis-scoped `OptionsToolContext`, including the fetch timestamp.
4. When the prefetch succeeds, `TechnicalAnalyst::run()` binds `GetOptionsSnapshot` into the tool list and the tool replays the already-fetched outcome instead of issuing a second live fetch.
5. When the prefetch fails, the run stays fail-open: `GetOptionsSnapshot` is omitted for that run and the failure is captured only in persisted `options_context`.
6. The model uses the replayed tool result for reasoning during the technical turn when the tool is present.
7. The model returns `TechnicalAnalystResponse`, which may include `options_summary`.
8. `TechnicalAnalyst::run()` constructs the persisted `TechnicalData` value from:
   - model-owned fields from `TechnicalAnalystResponse`
   - the prefetched result normalized into `TechnicalOptionsContext`

### Why use an options context immediately

The purpose of this slice is to let downstream agents use the same options evidence the Technical Analyst used. A post-inference second fetch would weaken that guarantee, because the provider is live and today-scoped.

The approved trade-off is:

- keep the current Technical Analyst tool path intact
- add a small `OptionsToolContext` now instead of accepting a second fetch
- defer any further generalization of the cache pattern beyond this local context

This is acceptable because:

- the repo already uses an analogous pattern for OHLCV via `OhlcvToolContext`
- the extra local context is smaller than introducing a new top-level routing layer
- it preserves one source of truth for both tool output and persisted state

The prefetched options artifact is authoritative for the rest of that analysis cycle. No mid-cycle refresh is attempted. `OptionsToolContext` may carry internal timing details for runtime use, but this slice does not upgrade `EvidenceSource.fetched_at` into a per-tool provenance timestamp. Technical evidence keeps the existing cycle-level merge-time semantics for `fetched_at`.

### Failure behavior

- If the prefetch returns `Ok(outcome)`, persist `TechnicalOptionsContext::Available { outcome }`.
- If the prefetch returns `Err(err)`, persist `TechnicalOptionsContext::FetchFailed { code, reason }`, where `code` is a stable enum and `reason` is sanitized human-readable detail.
- `GetOptionsSnapshot` is only bound when prefetch succeeded. A prefetch failure therefore cannot surface as a tool error that aborts the Technical Analyst turn.
- The Technical Analyst run remains fail-open unless the analyst output itself is invalid.
- Downstream prompts distinguish:
  - `technical_report.options_context.status == "available"`
  - `technical_report.options_context.outcome.kind == "snapshot"` for a live usable options snapshot
  - `technical_report.options_context.outcome.kind != "snapshot"` for successful no-snapshot outcomes such as `historical_run` or `no_listed_instrument`
  - `technical_report.options_context.status == "fetch_failed"`
  - `technical_report.options_context == null` for legacy or pre-options snapshots only

This keeps the shared state honest: successful no-snapshot outcomes stay explicit, and provider failures do not collapse back into `null`.

The `TradingError -> TechnicalOptionsFetchFailureCode` mapping for this slice is:

- `TradingError::NetworkTimeout` -> `Timeout`
- `TradingError::SchemaViolation { .. }` -> `SchemaViolation`
- all other options-prefetch errors on this path, including provider/configuration failures -> `ProviderError`

## Prompt behavior by role

### Technical Analyst

Update `technical_analyst.md` so that:

- `get_options_snapshot` remains a runtime tool.
- `options_summary` becomes a concise options interpretation.
- The model stops copying raw tool JSON into `options_summary`.
- The prompt explains that downstream agents will receive both the interpreted `options_summary` and the persisted `options_context`.

The Technical Analyst should be encouraged, not hard-required, to populate `options_summary` when `get_options_snapshot` returns a live snapshot. Do not make the run brittle by rejecting otherwise valid technical output solely because `options_summary` is absent.

When the options prefetch failed and the tool is omitted for that run, the prompt should not require or encourage `options_summary`.

For successful non-snapshot outcomes such as `historical_run`, `no_listed_instrument`, `sparse_chain`, or `missing_spot`, the prompt should allow but not require a brief explicit-absence or degraded-conditions note in `options_summary`. It should not pressure the model to invent directional options analysis in those cases.

### Researchers

Update the bullish, bearish, and moderator prompts to explicitly consider `technical_report.options_context` and `technical_report.options_summary` when present.

Keep the guidance generic in this slice:

- inspect `technical_report.options_context.outcome.kind`, not just the outer `status`
- use the structured options context only when that inner `kind` justifies the claim being made
- do not invent options claims when the context is `fetch_failed`, `null`, or a non-snapshot outcome that does not justify the claim being made
- treat `options_summary` as supplemental analyst interpretation, not authority

### Risk agents

Update all risk prompts to explicitly consume options context from `technical_report`.

Keep the guidance generic in this slice:

- inspect `technical_report.options_context.outcome.kind`, not just the outer `status`
- use options context when it is available and relevant
- calibrate caution when the options context reports degraded conditions rather than a live snapshot
- treat `options_summary` as supplemental analyst interpretation, not authority

### Trader

Update `trader.md` so the trader explicitly uses options context as supporting evidence, while keeping deterministic valuation and debate consensus as the primary decision anchors.

The trader guidance should also branch on `technical_report.options_context.outcome.kind`, so a successful no-snapshot outcome is treated as explicit absence rather than positive options signal.

### Fund Manager

Keep the fund manager as a passive consumer, but add a small instruction acknowledging that enriched technical data may include structured options context and a technical-desk options interpretation.

## Evidence and provenance

`AnalystSyncTask` should stop using `options_summary.is_some()` as the proxy for whether technical evidence included options data.

Instead:

- keep one Yahoo `EvidenceSource` for technical evidence in this slice
- use `datasets = ["ohlcv"]` when `options_context.is_none()`
- use `datasets = ["ohlcv", "options_context"]` when `options_context.is_some()`
- keep `EvidenceSource.fetched_at` on its existing cycle-level merge-time semantics from `AnalystSyncTask`, not per-tool fetch timestamps
- keep `quality_flags` unchanged in this slice because the structured context itself now carries successful degraded outcomes versus fetch failures

This design intentionally keeps explicit failure signaling in the deep technical payload, not in top-level evidence metadata. The evidence record remains provenance-first, while downstream prompts consume the detailed `options_context` directly.

This better reflects what the cycle actually persisted for downstream reasoning.

## Backward compatibility

- `TechnicalData.options_context` is additive and must use `#[serde(default)]`.
- Existing snapshot compatibility rules remain unchanged: no schema bump, no `deny_unknown_fields` on snapshotted state.
- Older snapshots deserialize with `options_context = None`.
- Existing `options_summary` snapshots remain shape-compatible, but not all old values should be treated as analyst interpretation. Older runs may still contain the legacy raw-JSON transport blob.
- Prompt rendering must therefore normalize legacy state in the shared prompt-serialization helper only; persisted snapshots stay unchanged.
- Legacy detection is deterministic, not heuristic: when `options_context` is `None` and `options_summary` parses as a JSON object with a top-level `kind` matching a legacy `OptionsOutcome` discriminant (`snapshot`, `no_listed_instrument`, `sparse_chain`, `historical_run`, `missing_spot`), downstream prompts suppress that `options_summary` instead of treating it as fresh analyst interpretation.

## Testing strategy

### State and serde

- Add a regression proving `TechnicalData` without `options_context` still deserializes cleanly.
- Extend snapshot-compatibility tests so old stored technical payloads load under the current schema version.
- Extend round-trip/property tests for the new optional field.
- Add a regression for legacy raw-JSON `options_summary` normalization when `options_context` is absent.

### Technical Analyst runtime

- Add unit coverage for the new split between `TechnicalAnalystResponse` and persisted `TechnicalData`.
- Add focused tests proving `OptionsToolContext`, `GetOptionsSnapshot`, and persisted `TechnicalData.options_context` all reflect the same prefetched result.
- Add a fail-open regression proving an options prefetch error does not fail the entire technical pass, omits the tool for that run, and persists `FetchFailed { code, reason }`.
- Extend stale-state tests so reused `TradingState` clears both `options_summary` and `options_context`.

### Evidence wiring

- Update `AnalystSyncTask` tests so `EvidenceSource.datasets` reflects `options_context.is_some()`.

### Prompt coverage

- Refresh prompt-bundle fixtures for the Technical Analyst and every downstream role whose prompt changes.
- Add targeted prompt tests where needed to prove the new guidance mentions `technical_report.options_context` and `technical_report.options_summary` explicitly.
- Add prompt-level coverage proving legacy raw-JSON `options_summary` is suppressed when `options_context` is absent.
- Add targeted prompt serialization tests for the shared helper so researcher, risk, trader, and fund-manager roles all get the same normalized technical payload.

### Integration coverage

- Add a pipeline test proving a run with stubbed options data yields persisted `technical_indicators().options_context` that downstream stages can read.

## Alternatives considered

### 1. Prompt-only reuse

Leave `options_summary` as the only shared surface and just update downstream prompts.

Rejected because it keeps shared options facts model-authored instead of Rust-owned.

### 2. New top-level options state or routing path

Add a dedicated options branch on `TradingState` or route through `data/routing.rs::derivatives`.

Rejected for this slice because all downstream agents already consume `technical_report`, so a new route adds more churn than value.

### 3. Post-inference second fetch

Prefetch nothing, let the tool fetch during the Technical Analyst turn, then fetch again after inference to populate persisted state.

Rejected because it breaks the core guarantee of this slice: the Technical Analyst and downstream agents should consume the same options artifact.

## Deferred follow-up

If the local `OptionsToolContext` pattern proves useful beyond this slice, a later cleanup may extract a shared cache helper alongside `OhlcvToolContext` instead of keeping two parallel bespoke contexts.

That follow-up is a cleanup, not a prerequisite for this design.
