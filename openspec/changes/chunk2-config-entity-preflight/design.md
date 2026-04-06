## Context

The current system has a gap between config-load time and analyst-run time: the ticker string goes from
`config.toml` → `TradingState::new()` → each data client with no canonical form and no single point of
validation beyond non-empty. The architecture spec introduces `ResolvedInstrument` and `PreflightTask` to close
this gap.

Existing code that must not change:
- `src/data/symbol.rs` — `validate_symbol` is the authoritative format checker; this function is called by all
  three data clients today. Chunk 2 delegates to it, not the reverse.
- `src/state/trading_state.rs` — `TradingState` struct gains no new fields in Chunk 2. The canonical symbol is
  available as `state.asset_symbol` (already stored); the resolved instrument record lives in workflow context, not
  in `TradingState`.
- `src/workflow/pipeline.rs` — the five existing phases are unchanged. The only pipeline change is inserting
  `PreflightTask` before the existing `analyst_fanout` node and updating the graph start task / session bootstrap to
  begin at `preflight`.

Constraints:
- No new crate dependencies.
- No `TradingState` schema changes.
- `DataEnrichmentConfig` defaults must leave the current runtime behavior unchanged: all three enrichment
  categories are `false` by default, so existing runs with no `[enrichment]` section in `config.toml` continue
  to work identically.
- `Config::validate()` can only reuse `validate_symbol` if the `symbol` module is visible outside `src/data/mod.rs`;
  the design therefore requires a minimal visibility widening on `src/data/mod.rs` (for example `pub(crate) mod symbol;`).
- All new context keys must use the `present-with-null` placeholder semantics defined in the spec: the key is
  always written by `PreflightTask` even when the payload is `null`.
- `PreflightTask` must hard-fail on symbol format violations and context write failures; it must not silently
  continue with a bad symbol.
- `ProviderCapabilities` in Stage 1 is config-derived only — no runtime API call is made to discover capabilities.
- The adapter contract slice is part of Chunk 2 in the Stage 1 plan: `TranscriptEvidence`, `ConsensusEvidence`,
  `EventNewsEvidence`, and their corresponding provider traits are declared now as seams only, while concrete
  providers remain deferred.

## Goals / Non-Goals

**Goals:**
- Add `DataEnrichmentConfig` to `src/config.rs` and `config.toml`, fully env-overridable via `SCORPIO__ENRICHMENT__*`.
- Strengthen `Config::validate()` to enforce symbol format using `validate_symbol`.
- Add `src/data/entity.rs` with `ResolvedInstrument` and `resolve_symbol` (Stage 1: uppercase normalization only,
  no live metadata fetch).
- Add `src/data/adapters/mod.rs` with `ProviderCapabilities` and a `from_config` constructor.
- Add `src/data/adapters/transcripts.rs`, `estimates.rs`, and `events.rs` with the Stage 1 evidence payload structs
  and provider traits.
- Extend the existing `src/workflow/tasks/common.rs` with all Stage 1 preflight context key constants.
- Add `src/workflow/tasks/preflight.rs` with `PreflightTask` implementing the current `graph_flow::Task` trait.
- Wire `PreflightTask` into `src/workflow/pipeline.rs` as the first node.
- Unit tests for all new functions and types; integration test for `PreflightTask` context writes; pipeline-structure
  test updates for the new start task.

**Non-Goals:**
- Live metadata enrichment for `ResolvedInstrument` fields (`issuer_name`, `exchange`, `instrument_type`) —
  all remain `None` in Stage 1.
- Concrete implementations of `TranscriptProvider`, `EstimatesProvider`, `EventNewsProvider` traits — deferred
  to Milestone 7.
- Changes to analyst system prompts — covered in Chunk 1.
- Changes to `TradingState` or evidence/provenance state fields — covered in Chunk 3.

## Decisions

### 1. `DataEnrichmentConfig` is added to `Config` with `#[serde(default)]`

**Decision**: Add `DataEnrichmentConfig` as a new optional top-level section in `Config`:

```rust
#[serde(default)]
pub enrichment: DataEnrichmentConfig,
```

with a `Default` impl returning `enable_transcripts: false`, `enable_consensus_estimates: false`,
`enable_event_news: false`, `max_evidence_age_hours: 48`.

**Rationale**: The `#[serde(default)]` pattern is already used for `api`, `providers`, `storage`, and
`rate_limits`. Adding `enrichment` the same way means that existing `config.toml` files without an `[enrichment]`
section continue to deserialize correctly — no breaking change for existing users. The default values (all
enrichment disabled) preserve current runtime behavior exactly. `max_evidence_age_hours = 48` is the spec default.

**Alternatives considered**:
- *Add `enable_*` fields directly to `TradingConfig`*: Would place enrichment flags alongside `asset_symbol` and
  backtest dates, mixing concerns. The enrichment section is a distinct config domain. Rejected.

### 2. `Config::validate()` calls `validate_symbol` from `src/data/symbol.rs`

**Decision**: Extend `Config::validate()` to call `crate::data::symbol::validate_symbol(&self.trading.asset_symbol)`
and map any `TradingError::SchemaViolation` to an `anyhow::Error` via `.map_err(|e| anyhow::anyhow!("{e}"))?`.

Because `src/config.rs` lives outside the `data` module, `src/data/mod.rs` must widen the `symbol` module visibility
just enough for this shared validator to be reused (`pub(crate) mod symbol;` or an equivalent crate-local re-export).

**Rationale**: `validate_symbol` is already the authoritative format checker used by all three data clients. Using
it from `Config::validate()` means format enforcement is consistent everywhere and there is one source of truth for
the accepted symbol grammar. The failure happens at startup, before any LLM or API client is constructed — this is
the desired early-exit behavior.

**Alternatives considered**:
- *Duplicate the validation logic inline in `validate()`*: Creates two independent definitions of valid symbol
  format that can drift. Rejected.

### 3. `ResolvedInstrument` is a plain struct; `resolve_symbol` is a synchronous free function

**Decision**: `ResolvedInstrument` derives `Debug`, `Clone`, `PartialEq`, `Serialize`, `Deserialize` (required for
context serialization). `resolve_symbol(symbol: &str) -> Result<ResolvedInstrument, TradingError>` is a free
function that: (1) calls `validate_symbol(symbol)` to reject invalid formats, (2) uppercase-normalizes the trimmed
symbol, (3) returns a `ResolvedInstrument` with `canonical_symbol` set and all metadata fields as `None`/`vec![]`.

**Rationale**: Stage 1 entity resolution is purely local (no API call), so a synchronous function is correct — no
`async fn` is needed. The struct is minimal: fields are exactly those specified in the architecture design doc.
`Serialize` + `Deserialize` are required because the struct must be written to and read from `graph_flow::Context`
as a JSON string.

**Alternatives considered**:
- *Make `resolve_symbol` async to leave room for future live-metadata fetch*: Adds complexity now with no payoff.
  When live metadata is added in a later milestone, the call site in `PreflightTask` (which is already async) can
  be changed to `await` without a signature breaking change in the callers. Rejected.

### 4. `ProviderCapabilities` is config-derived; `from_config` is infallible

**Decision**: `ProviderCapabilities::from_config(cfg: &DataEnrichmentConfig) -> ProviderCapabilities` maps the
three `enable_*` booleans directly to the three capability fields. No error return — reading from a fully-loaded
`DataEnrichmentConfig` cannot fail.

**Rationale**: The spec is explicit: "capability discovery itself cannot fail in the first slice because it is
config-derived only." An infallible constructor is the correct representation of that invariant.

### 5. Stage 1 adapter contracts are declared now, concrete providers later

**Decision**: Chunk 2 also declares the Stage 1 adapter contract seam in `src/data/adapters/`:

- `transcripts.rs` — `TranscriptEvidence`, `TranscriptProvider`
- `estimates.rs` — `ConsensusEvidence`, `EstimatesProvider`
- `events.rs` — `EventNewsEvidence`, `EventNewsProvider`

No concrete provider implementations are added in this chunk.

**Rationale**: The architect plan and Stage 1 implementation plan place these contracts in the same runtime-foundation
slice as `ProviderCapabilities` and `PreflightTask`. `PreflightTask` seeds typed JSON `null` placeholders for these
payloads, so the corresponding types must already exist even though live provider wiring is deferred.

### 6. All six Stage 1 context keys are added to the existing `src/workflow/tasks/common.rs`

**Decision**: Define:

```rust
pub const KEY_RESOLVED_INSTRUMENT: &str = "resolved_instrument";
pub const KEY_PROVIDER_CAPABILITIES: &str = "provider_capabilities";
pub const KEY_REQUIRED_COVERAGE_INPUTS: &str = "required_coverage_inputs";
pub const KEY_CACHED_TRANSCRIPT: &str = "cached_transcript";
pub const KEY_CACHED_CONSENSUS: &str = "cached_consensus";
pub const KEY_CACHED_EVENT_FEED: &str = "cached_event_feed";
```

**Rationale**: String constants in a shared `common.rs` module prevent key typos across producers and consumers. The
`common.rs` pattern is already in use in this repository. Adding the preflight keys to the existing shared constants
module is smaller and more accurate than creating a second `common.rs`. All six keys are written by `PreflightTask`
on every run; downstream tasks that need to read them import the constants from `common`. `KEY_PREVIOUS_THESIS` is
explicitly excluded (deferred to Milestone 5 per spec).

### 7. `PreflightTask` reads from runtime state, canonicalizes it, and fails hard on symbol errors

**Decision**: `PreflightTask` reads the input symbol from the runtime `TradingState` loaded from workflow context, not
from `Config.trading.asset_symbol`. If `resolve_symbol` returns `Err`, `PreflightTask` propagates the error
immediately — the graph execution halts with a descriptive error before any analyst task is spawned. After a
successful resolve, it writes the canonical uppercase symbol back into `TradingState.asset_symbol` and writes all six
context keys unconditionally. `KEY_CACHED_*` keys are written as typed JSON `null` payloads (not absent keys).

**Rationale**: The spec mandates "fail closed on invalid symbol input or orchestration corruption" and "missing
`KEY_CACHED_*` after `PreflightTask` is orchestration corruption, not normal absence." Writing all six keys
unconditionally ensures downstream tasks can always `expect` a key to be present and treat a missing key as a
programming error rather than a valid data-absence case. Reading from `TradingState` keeps preflight aligned with the
actual runtime request being executed, which may differ from `Config.trading.asset_symbol` in tests or future callers.
Writing the canonical symbol back into `TradingState` ensures pre- and post-preflight consumers see a single
authoritative value rather than raw mixed-case input.

### 8. `PreflightTask` is inserted as the first node with a direct edge to `analyst_fanout`

**Decision**: In `src/workflow/pipeline.rs`, add `PreflightTask` as the initial node (replacing the implicit start
that went directly to `analyst_fanout`). Add an edge: `PreflightTask` → `analyst_fanout`. Update the graph start task,
task-id constants/tests, and `Session::new_from_task(...)` bootstrap so execution begins at `preflight`. All
subsequent edges are unchanged.

**Rationale**: The spec defines the graph change as:

```
preflight -> analyst_fanout -> analyst_sync -> ...
```

This is a minimal topology change — one new node and one new edge. Keeping `analyst_fanout` as the next node
preserves the Phase 1 fan-out semantics unchanged.

## Risks / Trade-offs

- **[Startup failure on previously accepted tickers]** Existing users with a lowercase or malformed
  `asset_symbol` in `config.toml` will get a startup error after this change. Mitigation: `validate_symbol`
  accepts lowercase, hyphens, dots, underscores, and the `^` index prefix — the rule is broad; most real tickers
  are valid. Uppercase normalization is done at the entity-resolution step, not at validation time, so
  `"nvda"` passes validation and resolves to `"NVDA"`. Truly invalid values (spaces, semicolons, empty) correctly
  fail fast.
- **[Context key coupling]** Any task added after `PreflightTask` that reads a Stage 1 context key will fail at
  runtime if `PreflightTask` is removed or bypassed. Mitigation: the keys are constants in `common.rs`; tests
  verify that `PreflightTask` writes all six. The only way to break this is to remove `PreflightTask` from the
  pipeline, which would be caught by integration tests.
- **[Config/test fixture spillover]** Adding `Config.enrichment` is an additive schema change, but all manual
  `Config { ... }` literals must grow the new field. Mitigation: explicitly track the shared test/support files that
  construct `Config` literals so implementers do not stop at `src/config.rs`.
- **[`ResolvedInstrument` metadata fields always `None` in Stage 1]** Agents cannot yet use `issuer_name`,
  `exchange`, or `instrument_type` from the resolved instrument. Mitigation: the fields exist in the struct and
  are serialized to context; when Stage 2 enrichment is added, the fields will be populated without any
  breaking change to downstream consumers.

## Migration Plan

No database migration and no state schema migration required.

1. Merge `DataEnrichmentConfig` into `src/config.rs` and `config.toml`. Existing configs without `[enrichment]`
   continue to work — the section is optional with all-false defaults.
2. `Config::validate()` now rejects format-invalid symbols. Update `config.toml` if `asset_symbol` is lowercase
   (e.g., change `"nvda"` to `"NVDA"`) — or leave lowercase: validation accepts it, resolution uppercases it.
3. Add the adapter seam files and `PreflightTask`, then update the existing pipeline start task and shared test/support
   fixtures that construct `Config` literals or assert task order.
4. Rollback: remove the new files and revert the additive edits in config/data/workflow/test-support files. No
   downstream consumers exist yet for the new context keys (Chunk 3 adds them).

## Open Questions

None at proposal stage. The spec is explicit on Stage 1 entity-resolution policy, context-key semantics, and
`PreflightTask` responsibilities. Metadata enrichment (filling in `issuer_name` etc.) is explicitly deferred.
