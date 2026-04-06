## Why

Chunk 1 established evidence discipline at the prompt layer. Chunk 2 lays the infrastructure beneath it: before any
analyst can run, the system needs to know _what_ it is analyzing and _what_ enrichment capabilities the runtime
has enabled. Currently both questions are answered implicitly — the ticker string is taken verbatim from
`config.toml`, there is no canonical instrument record, and there are no capability flags for the enrichment
categories described in the architecture spec (`enable_transcripts`, `enable_consensus_estimates`,
`enable_event_news`). The result is:

- A misconfigured or ambiguously formatted ticker (`nvda`, `NVDA`, ` NVDA `) silently produces different query strings
  across the four data clients (Finnhub, FRED, yfinance), leading to partial or mismatched API responses.
- There is no single authoritative instrument object that agents can reference; each provider re-parses the raw symbol
  string independently, which is the "telephone effect" at the infrastructure layer.
- The enrichment config section (`DataEnrichmentConfig`) described in the architecture design spec does not exist in
  `src/config.rs`, so enrichment feature flags cannot be set via `config.toml` or environment variables.
- `Config::validate()` checks only that `asset_symbol` is non-empty; it does not enforce symbol format, which means
  invalid tickers like `"DROP;TABLE"` pass config validation and reach the API clients.
- There is no `PreflightTask` in the graph. The pipeline currently starts directly at the analyst fan-out, so there is
  no place to perform symbol canonicalization, write the resolved instrument to workflow context, or seed enrichment
  cache placeholder keys before Phase 1 begins.

Chunk 2 closes these gaps with a focused, additive scope: add `DataEnrichmentConfig` to `src/config.rs`, add
`src/data/entity.rs` with `ResolvedInstrument` and `resolve_symbol`, upgrade `Config::validate()` to enforce symbol
format, add the Stage 1 enrichment adapter contracts under `src/data/adapters/`, and wire in a new `PreflightTask`
as the first graph node in `src/workflow/pipeline.rs`. Chunks 3 and 4 (evidence/provenance state and report
sections) build directly on the `ResolvedInstrument`, adapter contracts, and context keys established here.

## What Changes

- **`src/config.rs`**: Add `DataEnrichmentConfig` struct (`enable_transcripts`, `enable_consensus_estimates`,
  `enable_event_news`, `max_evidence_age_hours`) with `#[serde(default)]` on `Config.enrichment`. Extend
  `Config::validate()` to call `validate_symbol` from `src/data/symbol.rs` on `trading.asset_symbol`, failing fast
  on format-invalid tickers before any LLM or API client is constructed. Add `default_max_evidence_age_hours()` returning `48`.
- **`config.toml`**: Add `[enrichment]` section with all four keys at their default values (`false`, `false`,
  `false`, `48`).
- **`src/data/entity.rs`** (new file): `ResolvedInstrument` struct and `resolve_symbol(symbol: &str) ->
  Result<ResolvedInstrument, TradingError>` function. Delegates format validation to `validate_symbol` in
  `src/data/symbol.rs`; canonicalizes to uppercase; leaves `issuer_name`, `exchange`, `instrument_type`, `aliases`
  as `None`/empty in Stage 1.
- **`src/data/adapters/mod.rs`**, **`transcripts.rs`**, **`estimates.rs`**, **`events.rs`** (new files):
  `ProviderCapabilities` plus Stage 1 trait contracts and evidence payload structs for transcripts, consensus
  estimates, and event news. In this slice they are type/trait seams only; no concrete providers are wired.
- **`src/data/mod.rs`**: Export the new `entity` and `adapters` modules, and widen `symbol` module visibility just
  enough for `Config::validate()` to reuse the shared validator.
- **`src/workflow/tasks/preflight.rs`** (new file): `PreflightTask` implementing `graph_flow::Task`. Responsibilities:
  validate and canonicalize the runtime symbol from `TradingState`, write the canonical symbol back into
  `TradingState.asset_symbol`, write `KEY_RESOLVED_INSTRUMENT` and
  `KEY_PROVIDER_CAPABILITIES` to workflow context, write `KEY_REQUIRED_COVERAGE_INPUTS` as
  `["fundamentals", "sentiment", "news", "technical"]`, seed `KEY_CACHED_TRANSCRIPT`, `KEY_CACHED_CONSENSUS`,
  `KEY_CACHED_EVENT_FEED` with typed JSON `null` placeholders. Hard-fails on invalid symbol or context corruption.
- **`src/workflow/tasks/common.rs`** (modify existing file): Add all Stage 1 preflight context key constants
  (`KEY_RESOLVED_INSTRUMENT`, `KEY_PROVIDER_CAPABILITIES`, `KEY_REQUIRED_COVERAGE_INPUTS`, `KEY_CACHED_TRANSCRIPT`,
  `KEY_CACHED_CONSENSUS`, `KEY_CACHED_EVENT_FEED`) alongside the existing task constants.
- **`src/workflow/tasks/mod.rs`** (modify existing file): Export `preflight` and the new preflight-related constants
  from `common`.
- **`src/workflow/pipeline.rs`**: Insert `PreflightTask` as the first node in the graph, with an edge to the existing
  `analyst_fanout` node. Update the start task and session bootstrap to begin at `preflight` instead of
  `analyst_fanout`.
- **`openspec/changes/chunk2-config-entity-preflight/specs/evidence-provenance/spec.md`** (new file): Add the
  missing OpenSpec delta for Chunk 2's runtime preflight/entity-resolution slice of the `evidence-provenance`
  capability.
- **`openspec/changes/chunk2-config-entity-preflight/specs/graph-orchestration/spec.md`** (new file): Add the
  missing OpenSpec delta updating the pipeline topology and start task to account for `PreflightTask`.

## Capabilities

### Modified Capabilities
- `evidence-provenance`: This chunk delivers the runtime preflight/entity-resolution slice of the architected
  cross-cutting capability: config-driven enrichment flags, canonical instrument resolution, Stage 1 adapter
  contracts, and preflight-seeded evidence context keys.
- `graph-orchestration`: The pipeline gains a `PreflightTask` start node before the analyst fan-out; the graph start
  task and entry sequencing change accordingly.

## Impact

- **Config**: `src/config.rs` gains `DataEnrichmentConfig` and one new `Config` field; `config.toml` gains an
  `[enrichment]` section. No existing config field names change.
- **Code**: New files under `src/data/` (`entity.rs`, `adapters/mod.rs`, `adapters/transcripts.rs`,
  `adapters/estimates.rs`, `adapters/events.rs`); one new file under `src/workflow/tasks/` (`preflight.rs`);
  additive modifications to `src/config.rs`, `src/data/mod.rs`, `src/workflow/tasks/common.rs`,
  `src/workflow/tasks/mod.rs`, `src/workflow/pipeline.rs`, and selected shared test/support files that construct
  `Config` literals or assert the pipeline start task. No new `TradingState` fields, no new crate dependencies, and
  no agent prompt changes in this chunk.
- **Tests**: Unit tests added for `DataEnrichmentConfig` deserialization and env overrides; unit tests for
  `resolve_symbol` (valid tickers, invalid tickers, case normalization); unit tests for `ProviderCapabilities`
  plus the Stage 1 adapter contract structs; integration tests for `PreflightTask` writing all six context keys;
  pipeline-structure tests updated for the new `preflight` start node; shared `Config { ... }` test fixtures updated
  for the new `enrichment` field.
- **Rollback**: Remove `DataEnrichmentConfig` from `src/config.rs` and the `[enrichment]` block from `config.toml`;
  delete `src/data/entity.rs`, `src/data/adapters/`, and `src/workflow/tasks/preflight.rs`; revert the additive edits
  in `src/data/mod.rs`, `src/workflow/tasks/common.rs`, `src/workflow/tasks/mod.rs`, `src/workflow/pipeline.rs`, and
  the affected shared test/support files. No database migration, no state schema change, no agent prompt change
  required.

## Cross-Owner Changes

This change requires approved cross-owner edits before implementation begins.

- [`src/config.rs`](../../../src/config.rs) — owner: `add-project-foundation`. Adds `DataEnrichmentConfig` and extends
  `Config` validation.
- [`config.toml`](../../../config.toml) — owner: `add-project-foundation`. Adds the checked-in `[enrichment]` defaults.
- [`src/data/mod.rs`](../../../src/data/mod.rs) — owner: `add-project-foundation` skeleton. Exports `entity` and
  `adapters`, and widens `symbol` module visibility enough for shared validation reuse.
- [`src/workflow/tasks/common.rs`](../../../src/workflow/tasks/common.rs) — owner: `add-graph-orchestration`. Adds
  preflight context-key constants to an already-existing shared task-constants module.
- [`src/workflow/tasks/mod.rs`](../../../src/workflow/tasks/mod.rs) — owner: `add-graph-orchestration`. Exports the
  new `preflight` task module and preflight-related constants.
- [`src/workflow/pipeline.rs`](../../../src/workflow/pipeline.rs) — owner: `add-graph-orchestration`. Inserts the new
  `PreflightTask`, changes the start task from `analyst_fanout` to `preflight`, and updates session bootstrap.
- [`tests/support/workflow_pipeline_make_pipeline.rs`](../../../tests/support/workflow_pipeline_make_pipeline.rs) —
  shared test-support owner. Manual `Config { ... }` construction must grow the new `enrichment` field.
- [`tests/support/workflow_observability_pipeline_support.rs`](../../../tests/support/workflow_observability_pipeline_support.rs)
  — shared test-support owner. Manual `Config { ... }` construction must grow the new `enrichment` field.
- [`tests/workflow_pipeline_structure.rs`](../../../tests/workflow_pipeline_structure.rs) — shared workflow test owner.
  Assertions about the graph start task and task list must be updated for the new `preflight` node.
- [`src/agents/trader/tests.rs`](../../../src/agents/trader/tests.rs) and
  [`src/agents/fund_manager/tests.rs`](../../../src/agents/fund_manager/tests.rs) — current direct `Config { ... }`
  literals will also need the new `enrichment` field once `Config` changes.

## Alternatives Considered

### Option: Inline symbol canonicalization into each data client instead of a shared entity module
Keep the existing per-client `validate_symbol` call pattern and add uppercase normalization inside `FinnhubClient`,
`FredClient`, and `YFinanceClient` individually rather than introducing `src/data/entity.rs`.

Pros: Zero new abstractions. Each client already calls `validate_symbol` independently; adding `.to_uppercase()` to
each call site is a one-line change per client. No new module, no new type, no workflow-context write.

Cons: The canonical instrument record is never written to a single authoritative location. Future agents (Chunk 3
evidence records, Chunk 4 report sections) that need to reference the canonical symbol must re-derive it from raw
state rather than reading from a typed `ResolvedInstrument`. Three independent normalization sites drift independently.
The `PreflightTask` has no typed instrument to write to context — it would have to write a raw string, weakening the
context contract.

Why rejected: The architecture spec explicitly defines `ResolvedInstrument` as the canonical instrument record and
mandates `KEY_RESOLVED_INSTRUMENT` in workflow context. A shared `resolve_symbol` function is the minimal correct
implementation of that contract. The overhead is one 30-line file and one struct; the payoff is a single source of
truth for all downstream consumers.

### Option: Add symbol validation to `Config::load_from` instead of `Config::validate`
Move the `validate_symbol` call into the deserialization path using a custom `#[serde(deserialize_with)]` on
`trading.asset_symbol`, similar to how `deserialize_provider_name` works for LLM provider names.

Pros: Validation runs at the earliest possible point (deserialization), before any other config field is accessed.
Consistent with how provider name validation is already done.

Cons: `validate_symbol` lives in `src/data/symbol.rs`, which is in a different crate layer than the config
deserializer. Pulling it into a serde deserialize hook would create a cross-layer dependency inside the
deserialization closure, making the deserialization error message less clear (serde wraps it in a generic
`DeserializationError`). The existing `Config::validate()` method is already the designated location for
domain-level validation that goes beyond pure syntax — the symbol format check belongs there. Testing is also simpler:
the existing `env_override_uses_double_underscore_separator` and `load_from_*` tests do not need to change.

Why rejected: `Config::validate()` is the right place for this check. The serde approach would require coupling
`src/config.rs` directly to `src/data/symbol.rs` inside a type-erased serde callback, adding complexity without
meaningfully improving the failure point (both `Config::validate()` and a serde hook run before any provider client
is constructed).

### Option: Defer `PreflightTask` and write resolved instrument directly from the analyst fan-out task
Instead of adding a new graph node, have the existing analyst fan-out task call `resolve_symbol` at the start of Phase
1 and write the resulting `ResolvedInstrument` to context before spawning analyst workers.

Pros: No new graph node, no pipeline topology change. Fewer files to add. The analyst fan-out task already has access
to `TradingState` and the `graph_flow::Context`.

Cons: The analyst fan-out task is responsible for spawning four concurrent analyst workers. Adding preflight
responsibilities (symbol resolution, capability flag derivation, cache-key seeding) to a task that already manages
concurrency makes it harder to reason about failure modes. If symbol resolution fails after worker spawning begins,
partial work may have already been dispatched. The architecture spec explicitly defines `PreflightTask` as a distinct
first graph node for this reason — it must run to completion before any analyst work starts.

Why rejected: The spec defines `PreflightTask` as the correct abstraction for pre-analysis validation. The graph
topology change is minimal (one new node, one new edge) and the resulting pipeline is more modular: preflight
failures produce a clean early exit without touching the analyst machinery at all.
