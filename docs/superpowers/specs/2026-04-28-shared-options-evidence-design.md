# Design — shared options evidence for downstream agents

**Date:** 2026-04-28
**Author:** brainstorming session with BigtoC
**Status:** Draft — reviewed 2026-04-28

## Summary

Extend the existing equity-options integration from "Technical Analyst only" into a shared downstream input for the researcher, trader, risk, and fund-manager stages.

This is an expansion of the 2026-04-24 options design, not a reversal of its value judgment. The original scope was a complexity-management choice for v1: prove the new `OptionsProvider` and `GetOptionsSnapshot` path inside one agent before routing the data more broadly. Now that the provider contract exists and the Technical Analyst path is wired, the next step is to persist one Rust-owned options artifact in state so downstream agents can reason from the same substrate instead of relying on a model-copied string.

A simpler alternative — fixing `technical_analyst.md` to produce a real interpretation instead of copying raw tool JSON — was evaluated. That change alone improves `options_summary` quality but leaves shared options facts model-authored (not Rust-owned), making it impossible for downstream agents to reason from the same structured substrate. Both changes are needed; they are shipped together rather than staged because the Rust-owned field is what makes the interpretation trustworthy.

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

| Decision                 | Choice                                                                                  | Rationale                                                                                                                    |
|--------------------------|-----------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------|
| Shared-data seam         | Extend `TechnicalData`                                                                  | All downstream agents already receive the serialized technical payload; reuse the existing seam                              |
| Structured field         | Add `options_context: Option<TechnicalOptionsContext>`                                  | Preserve both successful `OptionsOutcome` data and explicit fetch-failure state                                              |
| Analyst interpretation   | Keep `options_summary: Option<String>`                                                  | Downstream agents need the technical desk's read, but it should be separate from raw facts                                   |
| Analyst output contract  | Split from persisted state                                                              | Prevent the LLM from being asked to author the Rust-owned structured options field                                           |
| Runtime ownership        | Prefetch once into an `OptionsToolContext` before the LLM turn; tool replays the result | Technical Analyst and downstream agents must reason from the same result                                                     |
| Failure code granularity | `FetchFailed { reason: String }` only                                                   | No downstream consumer branches on error category in this slice; add an enum variant when a consumer needs it                |
| Prompt rollout           | Update all downstream prompts with minimal generic options guidance                     | The point of this slice is shared usage, but prompt churn should stay narrow                                                 |
| Downstream serialization | Each call site serializes `state.technical_indicators()` inline                         | Without legacy suppression there is no new shared behavior to centralize; extract a helper if behavior diverges across roles |
| Legacy compatibility     | Old `options_summary` blobs are tolerated, not suppressed                               | Per-role prompt guidance already instructs agents to treat `options_summary` as supplemental                                 |

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

#[serde(tag = "status", rename_all = "snake_case")]
enum TechnicalOptionsContext {
    Available { outcome: OptionsOutcome },
    FetchFailed {
        // sanitized human-readable text only — no raw provider URLs,
        // embedded keys, or partial JSON
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

### OptionsToolContext

A runtime-only analysis-scoped struct that carries the prefetched result through the Technical Analyst turn. It is never serialized.

```rust
// crates/scorpio-core/src/agents/analyst/equity/technical.rs

/// Write-once analysis-scoped cache. TechnicalAnalyst::run() writes it before
/// the LLM turn; GetOptionsSnapshot::call() reads from it instead of fetching.
struct OptionsToolContext {
    outcome: OptionsOutcome,
}

impl OptionsToolContext {
    fn new(outcome: OptionsOutcome) -> Self { Self { outcome } }
    fn outcome(&self) -> &OptionsOutcome { &self.outcome }
}
```

`TechnicalData.options_context` is the persisted normalized form that downstream agents receive via `technical_report`. Both hold the same underlying `OptionsOutcome` value but serve different lifetimes: `OptionsToolContext` lives only for the analyst turn; `options_context` persists into the phase snapshot.

### TechnicalAnalystResponse → TechnicalData conversion

After inference, `TechnicalAnalyst::run()` applies a single merge step:

1. Parse the LLM's JSON response into `TechnicalAnalystResponse`.
2. Read the prefetched result from `OptionsToolContext`.
3. Construct `TechnicalData` by spreading `TechnicalAnalystResponse` fields and setting `options_context` from the prefetched `TechnicalOptionsContext`.
4. Enforce consistency: if `options_context` is not `Available { outcome: Snapshot(_) }`, clear any model-authored `options_summary` before persistence. This prevents hallucinated options interpretations from flowing downstream when the structured evidence does not support them.

This is the only point where model-authored and Rust-owned fields combine.

### Why `options_context` instead of `options_snapshot`

The shared field is named `options_context` because downstream agents need more than the happy path. It needs to carry:

- successful `OptionsOutcome` values, including non-snapshot outcomes such as `HistoricalRun` or `NoListedInstrument`
- explicit fetch-failure state when Yahoo could not be reached or the call otherwise failed

A historical backtest run or a no-listed-instrument symbol still carries useful state, and a fetch failure must stay distinguishable from those successful no-snapshot outcomes.

### Why keep `options_summary`

The Technical Analyst should keep emitting a short options interpretation because downstream agents benefit from knowing how the technical desk read the options tape. But the field should return to its intended meaning:

- `options_context` = Rust-owned structured facts and fetch status
- `options_summary` = model-authored interpretation of those facts (cleared by the conversion step when not warranted)

The raw tool JSON should no longer be stuffed into `options_summary`.

### Canonical naming

In code, the persisted payload is `TechnicalData` reached through `state.technical_indicators()`.

In prompts, that same serialized payload is injected as `{technical_report}`.

This document uses:

- `technical_indicators()` when describing Rust call sites
- `technical_report.options_context` and `technical_report.options_summary` when describing prompt behavior

### Prompt serialization seam

Each downstream prompt builder (researcher, risk, trader, fund-manager) continues to serialize `state.technical_indicators()` directly using the existing `sanitize_prompt_context(serde_json::to_string(...))` pattern. No new shared helper is introduced in this slice.

Token budget: `OptionsOutcome::Snapshot` carries chain data that is non-trivially sized. To avoid inflating the prompt budget for non-technical roles, serialize a projected subset of the snapshot for the `technical_report` injection — fields directly useful to downstream reasoning (IV regime, skew sign, ATM IV, NTM strike-interest summary) — rather than the full chain. The full chain is only needed during the Technical Analyst turn itself.

If the projection shape diverges across roles or additional normalization behavior accumulates, extract a shared helper at that point.

## Runtime flow

### Technical Analyst phase

1. `TechnicalAnalyst::run()` is the sole owner of this options prefetch flow.
2. Before the Technical Analyst turn begins, `TechnicalAnalyst::run()` performs one scoped options fetch using `YFinanceOptionsProvider`. This prefetch is unconditional across symbols, including those that will return `NoListedInstrument`; the cost is bounded by the provider's own timeout. Caching the `NoListedInstrument` outcome per symbol is a future optimization, not a prerequisite for this slice.
3. `TechnicalAnalyst::run()` stores the result in a new analysis-scoped `OptionsToolContext`.
4. When the prefetch succeeds, `TechnicalAnalyst::run()` binds `GetOptionsSnapshot` into the tool list. **`GetOptionsSnapshot::call()` must be refactored for this slice**: instead of invoking `provider.fetch_snapshot()`, it reads from the analysis-scoped `OptionsToolContext` when present. The model receives the same serialized outcome it would have seen from a live call.
5. When the prefetch fails, `GetOptionsSnapshot` is omitted for that run, and the prompt is rendered with a variant that omits all options-tool guidance and explicitly states that the live options provider was unavailable (see Prompt behavior below). The failure is captured in persisted `options_context`.
6. The model uses the replayed tool result for reasoning during the technical turn when the tool is present.
7. The model returns `TechnicalAnalystResponse`, which may include `options_summary`.
8. `TechnicalAnalyst::run()` applies the TechnicalAnalystResponse → TechnicalData conversion described above, enforcing the `options_summary` consistency rule.

### Why use an options context immediately

The purpose of this slice is to let downstream agents use the same options evidence the Technical Analyst used. A post-inference second fetch would weaken that guarantee, because the provider is live and today-scoped.

The `OhlcvToolContext` pattern is the closest analogue, but the ownership is inverted here by design. In `OhlcvToolContext`, `get_ohlcv` is the writer (it fetches live on first call), and downstream indicator tools are read-only consumers. Here, `TechnicalAnalyst::run()` prefetches before the LLM turn, then `GetOptionsSnapshot::call()` reads from context. Both patterns share the single-source-of-truth-per-cycle goal; the inversion is intentional because the shared-state goal requires the result to exist before inference begins.

The approved trade-off is:

- keep the current Technical Analyst tool path intact (with the `call()` refactor above)
- add a small `OptionsToolContext` now instead of accepting a second fetch
- defer any further generalization of the cache pattern beyond this local context

This is acceptable because:

- the extra local context is smaller than introducing a new top-level routing layer
- it preserves one source of truth for both tool output and persisted state

The prefetched options artifact is authoritative for the rest of that analysis cycle. No mid-cycle refresh is attempted. `OptionsToolContext` does not carry per-tool provenance timestamps in this slice; technical evidence keeps the existing cycle-level merge-time semantics for `fetched_at`.

### Failure behavior

- If the prefetch returns `Ok(outcome)`, persist `TechnicalOptionsContext::Available { outcome }`.
- If the prefetch returns `Err(err)`, persist `TechnicalOptionsContext::FetchFailed { reason }` where `reason` is a sanitized human-readable message (no raw URLs, embedded keys, or partial JSON). Error categorization into a stable code enum is deferred until a downstream consumer needs to branch on it.
- `GetOptionsSnapshot` is only bound when prefetch succeeded. A prefetch failure therefore cannot surface as a tool error that aborts the Technical Analyst turn.
- The Technical Analyst run remains fail-open unless the analyst output itself is invalid.
- Downstream prompts distinguish:
  - `technical_report.options_context.status == "available"`
  - `technical_report.options_context.outcome.kind == "snapshot"` for a live usable options snapshot
  - `technical_report.options_context.outcome.kind != "snapshot"` for successful no-snapshot outcomes such as `historical_run` or `no_listed_instrument`
  - `technical_report.options_context.status == "fetch_failed"`
  - `technical_report.options_context == null` for legacy or pre-options snapshots only

This keeps the shared state honest: successful no-snapshot outcomes stay explicit, and provider failures do not collapse back into `null`.

## Prompt behavior by role

**`OptionsOutcome` variant safety:** Whenever a new `OptionsOutcome` variant is added to the Rust enum, all downstream prompts that branch on `outcome.kind` must be updated. A dedicated prompt-bundle fixture test asserting the presence of every known variant name in each affected prompt guards against silent regression — analogous to the exhaustive `Role`→`PromptSlot` match in `workflow/topology.rs`.

### Technical Analyst

Update `technical_analyst.md` so that:

- `get_options_snapshot` remains a runtime tool when prefetch succeeded.
- `options_summary` becomes a concise options interpretation.
- The model stops copying raw tool JSON into `options_summary`.
- The prompt explains that downstream agents will receive both the interpreted `options_summary` and the persisted `options_context`.

**Prompt conditioning on prefetch outcome:** Prompt rendering branches on whether the prefetch succeeded. A `{options_tool_available}` template variable (or equivalent mechanism) is passed from `TechnicalAnalyst::run()` to the prompt builder:

- When `true`: include `get_options_snapshot` tool guidance and encourage (but do not require) `options_summary` when the tool returns a live snapshot.
- When `false`: render a variant section that omits all `get_options_snapshot` guidance and explicitly states the live options provider was unavailable for this run. Do not require or encourage `options_summary`.

This prevents the model from attempting to call a tool that is not bound for the run.

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

### Datasets field update

`AnalystSyncTask` should stop using `options_summary.is_some()` as the proxy for whether technical evidence included options data.

Instead:

- keep one Yahoo `EvidenceSource` for technical evidence in this slice
- use `datasets = ["ohlcv"]` when `options_context.is_none()`
- use `datasets = ["ohlcv", "options_context"]` when `options_context.is_some()`

### Provenance semantics

Keep `EvidenceSource.fetched_at` on its existing cycle-level merge-time semantics from `AnalystSyncTask`, not per-tool fetch timestamps. Keep `quality_flags` unchanged in this slice because the structured context itself now carries successful degraded outcomes versus fetch failures.

This design intentionally keeps explicit failure signaling in the deep technical payload, not in top-level evidence metadata. The evidence record remains provenance-first, while downstream prompts consume the detailed `options_context` directly.

This better reflects what the cycle actually persisted for downstream reasoning.

## Backward compatibility

- `TechnicalData.options_context` is additive and must use `#[serde(default)]`.
- Existing snapshot compatibility rules remain unchanged: no schema bump, no `deny_unknown_fields` on snapshotted state.
- Older snapshots deserialize with `options_context = None`.
- Existing `options_summary` snapshots that contain the legacy raw-JSON transport blob are tolerated as-is. Per-role downstream prompts instruct agents to treat `options_summary` as supplemental analyst interpretation, not authority — this is sufficient for graceful handling of old blobs without active suppression. If suppression proves necessary, the mechanism belongs in a `THESIS_MEMORY_SCHEMA_VERSION` bump that explicitly retires pre-cutoff rows, not in the hot serialization path.

**Consumer audit:** Before this slice ships, audit every reader of `TechnicalData.options_summary` (known: `AnalystSyncTask`) to confirm no other consumer parses it as JSON (audit reports, terminal reporters, thesis-memory readers, snapshot-driven analytics). The semantic change from "raw tool output" to "analyst interpretation" is safe only if no other path depends on the old format.

## Testing strategy

### State and serde

- Add a regression proving `TechnicalData` without `options_context` still deserializes cleanly.
- Extend snapshot-compatibility tests so old stored technical payloads load under the current schema version.
- Extend round-trip/property tests for the new optional field.
- Add a test asserting that the assembled `TechnicalData.options_context` carries the right `TechnicalOptionsContext` variant after the `TechnicalAnalystResponse` conversion step — covering both `Available` and `FetchFailed`.

### Technical Analyst runtime

- Add unit coverage for the new split between `TechnicalAnalystResponse` and persisted `TechnicalData`, including the merge step.
- Add focused tests proving `OptionsToolContext`, `GetOptionsSnapshot`, and persisted `TechnicalData.options_context` all reflect the same prefetched result.
- Add a fail-open regression proving an options prefetch error does not fail the entire technical pass, omits the tool for that run, and persists `FetchFailed { reason }`.
- Add a test proving the `options_summary` consistency rule: when `options_context` is not a live snapshot, any model-authored `options_summary` is cleared before persistence.
- Extend stale-state tests so reused `TradingState` clears both `options_summary` and `options_context`.
- Add a test for the `{options_tool_available}` prompt conditioning path — confirm that when prefetch fails, the rendered prompt contains no `get_options_snapshot` guidance.

### Evidence wiring

- Update `AnalystSyncTask` tests so `EvidenceSource.datasets` reflects `options_context.is_some()`.

### Prompt coverage

- Refresh prompt-bundle fixtures for the Technical Analyst and each downstream role whose prompt changes.
- Add a prompt-bundle fixture test asserting every known `OptionsOutcome` discriminant name appears in each prompt that branches on `outcome.kind`, so adding a new Rust variant requires updating the prompt before the test passes.

### Integration coverage

- Add a pipeline test proving a run with stubbed options data yields persisted `technical_indicators().options_context` that downstream stages can read.
- Add a pipeline test proving a run with no stubbed options data (prefetch failure path) yields `technical_indicators().options_context == FetchFailed` and that downstream stages receive a coherent `technical_report`.

## Alternatives considered

### 1. Prompt-only reuse

Leave `options_summary` as the only shared surface and just update downstream prompts.

Rejected because it keeps shared options facts model-authored instead of Rust-owned.

### 2. New top-level options state or routing path

Add a dedicated options branch on `TradingState` or route through `data/routing.rs::derivatives`.

Rejected for this slice because all downstream agents already consume `technical_report`, so a new route adds more churn than value.

### 3. Fix the prompt first, defer structured evidence

Fix `technical_analyst.md` so `options_summary` carries a real interpretation (not the JSON copy), update only trader and risk prompts to use that interpretation, and defer the structured-context work until usage data shows interpretation alone is insufficient.

Rejected because downstream agents cannot distinguish a well-written interpretation from a hallucination without Rust-owned structured ground truth. Both changes are needed; staging them adds a release cycle of ambiguous signals.

### 4. Post-inference second fetch

Prefetch nothing, let the tool fetch during the Technical Analyst turn, then fetch again after inference to populate persisted state.

Rejected because it breaks the core guarantee of this slice: the Technical Analyst and downstream agents should consume the same options artifact.

## Deferred follow-up

If the local `OptionsToolContext` pattern proves useful beyond this slice, a later cleanup may extract a shared cache helper alongside `OhlcvToolContext` instead of keeping two parallel bespoke contexts.

That follow-up is a cleanup, not a prerequisite for this design. **Consolidation trigger:** before a third tool context (e.g., `NewsToolContext`, `FundamentalsToolContext`) would otherwise be introduced, extract first. The three-context threshold is the prompt to extract.
