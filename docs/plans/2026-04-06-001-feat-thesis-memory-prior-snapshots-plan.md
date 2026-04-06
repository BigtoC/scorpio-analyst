---
title: feat: Add thesis memory from prior snapshots
type: feat
status: active
date: 2026-04-06
deepened: 2026-04-06
---

# Add Thesis Memory From Prior Snapshots

## Overview

Add a thesis-memory milestone that reuses the existing snapshot store to load the latest compatible prior thesis for the same symbol, injects that memory into downstream prompts, and persists the current run's final thesis into the phase-5 snapshot for future runs.

This plan is a follow-on to the evidence/provenance foundation work described in `docs/superpowers/plans/2026-04-05-evidence-provenance-foundation.md` and implements Milestone 5 from `docs/superpowers/specs/2026-04-05-financial-services-plugins-inspired-architecture-design.md`.

This plan is intentionally sequenced **after** the evidence/provenance foundation. It is not directly implementable against the current repo head until the prerequisite seams land: preflight startup wiring, entity resolution, and the shared thesis/context scaffolding.

## Problem Frame

The current pipeline reasons only from the current run's analyst, debate, risk, and final-decision state. The architecture spec already reserves a thesis-memory seam through `past_memory_str`, `KEY_PREVIOUS_THESIS`, `src/state/thesis.rs`, and `build_thesis_memory_context(state)`, but none of that behavior exists yet. As a result, each run starts from zero even when the repo already persists full `TradingState` snapshots after every phase.

The goal of this milestone is to turn those snapshots into a bounded memory source without adding a second persistence system or changing the five-phase pipeline. The implementation needs to stay safe for backtesting and replay workloads, which means it cannot leak future knowledge from later target dates into earlier runs.

## Requirements Trace

- R1. Add typed thesis-memory state under `src/state/thesis.rs` and thread it through `TradingState`.
- R2. Load prior thesis memory from the existing snapshot store using a single canonical symbol authority plus prior compatible snapshot lineage.
- R3. Missing prior thesis must degrade to `None` and allow the run to continue.
- R4. Stale, future-dated, or structurally incompatible thesis memory must be dropped, logged, and treated as absent.
- R5. Snapshot runtime/query failures remain hard `TradingError::Storage` failures.
- R6. Add `build_thesis_memory_context(state)` and wire it into researcher, risk, trader, fund-manager, and shared moderator prompt paths through the existing `past_memory_str` seam.
- R7. Persist the current run's authoritative thesis into the final phase snapshot so a later run can reuse it.
- R8. Preserve the current five-phase workflow and avoid introducing a separate thesis database in this slice.

## Scope Boundaries

- No scenario valuation, peer/comps analysis, concrete enrichment providers, or analysis-pack extraction.
- No new user-facing CLI/TUI/GUI surface beyond the fact that `TradingState` serialization will now include thesis memory.
- No dedicated thesis store or new configuration flags in the first slice.
- No additional LLM call purely to synthesize thesis memory; the memory record should be distilled from the current run's existing typed outputs.
- No change to the business workflow order; the plan keeps the same five execution phases.

## Context & Research

### Relevant Code and Patterns

- `src/workflow/snapshot.rs` persists full `TradingState` JSON snapshots and treats runtime save/load failures as `TradingError::Storage`.
- `migrations/0001_create_phase_snapshots.sql` shows the current snapshot table shape: `execution_id`, `phase_number`, `phase_name`, `trading_state_json`, `token_usage_json`, `created_at`.
- `src/workflow/context_bridge.rs` is the established pattern for extending `TradingState` and additional context keys together.
- `src/workflow/tasks/common.rs` is the central home for workflow context keys.
- `src/agents/shared/prompt.rs` is the established place for sanitize/redact/bounded prompt-context helpers.
- `src/agents/researcher/common.rs`, `src/agents/risk/common.rs`, `src/agents/trader/mod.rs`, and `src/agents/fund_manager/prompt.rs` already contain a `{past_memory_str}` placeholder but currently replace it with an empty string.
- `src/workflow/tasks/trading.rs` is the authoritative final-phase save path today because `FundManagerTask` writes the phase-5 snapshot.

### Institutional Learnings

- There are no relevant `docs/solutions/` entries yet for historical-memory or snapshot-derived prompt context.
- `docs/superpowers/plans/2026-04-05-evidence-provenance-foundation.md` is the best local pattern for extending state, the context bridge, snapshots, and prompt helpers together.

### External References

- None. Local repo patterns and the existing architecture spec are sufficient for this plan.

## Key Technical Decisions

- **Use a compact typed thesis record, not raw historical snapshots.**
  Rationale: downstream prompts need a small, bounded memory block, not full prior `TradingState` blobs. The thesis record should carry only the normalized symbol, prior target date, source execution metadata, final decision/action, consensus snapshot, trader rationale, final rationale, and an explicit `schema_version`.

- **Use the canonical preflight-resolved symbol as the lookup and persistence authority for thesis memory.**
  Rationale: the repo already distinguishes between raw requested symbol input and downstream canonicalization. Thesis reuse will fragment across casing and alias variants if it keys off the raw `TradingState.asset_symbol` string. The implementation should read the canonical symbol from the existing preflight/entity-resolution seam when available and persist that same canonical symbol inside `ThesisMemory`.

- **Canonical symbol resolution must happen before current-run startup provider fetches.**
  Rationale: `TradingPipeline::run_analysis_cycle` currently performs some startup data fetches before graph tasks run. Thesis lookup, startup provider fetches, and downstream graph execution must all use the same canonical symbol authority. This slice should not allow thesis memory to key off one symbol form while current-price/news/VIX fetches still use another.

- **Store thesis memory on `TradingState` as a single optional field named for the current memory payload.**
  Rationale: this keeps the state surface small. Early in a run, the field holds prior memory loaded from snapshots. After the Fund Manager decision, the field is overwritten with the current run's canonical thesis immediately before saving the phase-5 snapshot. No downstream consumer needs both previous and current thesis simultaneously.

- **Treat phase 5 (`FundManager`) as the only authoritative thesis writer and only query prior phase-5 snapshots for reuse.**
  Rationale: only the final phase has the complete trade proposal, risk discussion, and execution decision. Restricting lookup to phase 5 avoids unstable memory from partial or aborted runs.

- **Reuse the existing `phase_snapshots` table and query the stored JSON rather than adding a new thesis table or indexed metadata columns in this slice.**
  Rationale: the existing storage boundary is already approved in the architecture spec. JSON-field lookup keeps the first slice minimal. If this query path becomes hot or too slow, a later plan can promote `asset_symbol` / `target_date` to indexed snapshot metadata columns.

- **Define “compatible prior thesis” as: same canonical symbol, different `execution_id`, phase 5 snapshot, prior `target_date <= current target_date`, deserializable current `TradingState`, and supported `ThesisMemory.schema_version`.**
  Rationale: this prevents future-data leakage during backtests and avoids loading partial or incompatible memory.

- **Make candidate selection deterministic for the current storage shape.**
  Rationale: `phase_snapshots` stores JSON blobs plus `created_at`, not indexed symbol/date columns. The lookup algorithm should deserialize candidate `TradingState` payloads, keep only compatible phase-5 rows, order them by parsed `target_date` descending and then `created_at` descending, and choose the first eligible row. Malformed or unparseable target dates should be treated as incompatible and skipped with a log.

- **Keep failure policy asymmetric: storage/query failures are fatal; semantic absence is soft.**
  Rationale: this matches the existing snapshot-store contract. Missing thesis memory, unsupported schema versions, and future-dated or absent prior runs degrade to `None`; actual database/runtime failures should still fail closed.

- **`TradingState.thesis_memory` is authoritative; `KEY_PREVIOUS_THESIS` is a derived context mirror.**
  Rationale: prompt builders already consume `TradingState`, while `KEY_PREVIOUS_THESIS` is retained only because the upstream architecture spec already reserves that workflow-context seam. The plan should preserve one source of truth. If both are present and disagree, treat that as orchestration corruption rather than allowing silent drift.

- **Update final in-memory/context state and the phase-5 snapshot from the same post-capture `TradingState`.**
  Rationale: `FundManagerTask` currently saves workflow state before saving the snapshot. Thesis capture must happen before both save steps so the final returned `TradingState` and the persisted snapshot stay aligned.

- **Malformed candidate snapshot rows remain hard storage failures; semantic thesis incompatibility degrades to `None`.**
  Rationale: the existing `SnapshotStore` contract already treats `TradingState` decode failures as `TradingError::Storage`. This slice should preserve that boundary. The soft-degrade path applies only after a candidate row is successfully decoded into `TradingState` and the thesis payload is then found to be unsupported, future-dated, or otherwise semantically ineligible.

## Open Questions

### Resolved During Planning

- **Which phase should write the authoritative thesis for future runs?**
  Fund Manager / phase 5. It has the fullest context and already owns the final snapshot write.

- **Should incomplete prior runs count as thesis-memory sources?**
  No. Only prior phase-5 snapshots are eligible in this slice.

- **What makes a prior thesis stale or ineligible?**
  No age-based expiry in the first slice. Ineligibility is compatibility-based: wrong symbol, unsupported schema version, future-dated relative to the current run, or structurally incompatible payload.

- **Should this slice add snapshot metadata columns or a dedicated thesis table?**
  No. Reuse the current snapshot table and query JSON fields first.

- **How should snapshot lookup failures behave?**
  Database/query/load failures remain hard `TradingError::Storage` failures. Only semantic absence degrades to `None`.

- **Should moderators also receive thesis memory context?**
  Yes. The researcher and risk moderator prompt paths already share constructors/templates that expose the same `past_memory_str` seam. This slice should keep moderator behavior aligned with the rest of the downstream prompt stack.

### Deferred to Implementation

- **Exact helper names for lookup/capture functions.**
  The plan fixes the boundaries and responsibilities, but the final helper names should follow the concrete code shape after touching `src/workflow/snapshot.rs` and `src/state/thesis.rs`.

- **Whether JSON-query performance warrants a later metadata/index migration.**
  This should be revisited only if real usage shows the snapshot lookup path becoming a bottleneck.

- **Whether a later milestone needs separate `previous_thesis` and `current_thesis` fields.**
  The first slice should stay minimal with one field; split fields only if future compare/diff behavior justifies the added complexity.

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```mermaid
flowchart TB
    A[New analysis cycle] --> B[Pipeline entry / Preflight]
    B --> C[SnapshotStore thesis lookup\nby canonical symbol + target_date + phase 5]
    C -->|No compatible prior thesis| D[thesis_memory = None\nKEY_PREVIOUS_THESIS = null]
    C -->|Compatible prior thesis| E[thesis_memory = prior thesis\nKEY_PREVIOUS_THESIS = serialized thesis]
    D --> F[Researcher / Risk / Trader / Fund Manager\nuse build_thesis_memory_context(state)]
    E --> F
    F --> G[Fund Manager final decision]
    G --> H[Capture current canonical thesis\nfrom final typed state]
    H --> I[Overwrite state.thesis_memory]
    I --> J[Save final workflow state]
    J --> K[Save phase-5 snapshot]
```

## Alternative Approaches Considered

- **Dedicated thesis table or store**: rejected for this slice because the architecture spec explicitly prefers the existing snapshot store first, and this work does not need a second persistence system yet.
- **Persist and query earlier phase snapshots**: rejected because earlier phases do not contain the final trade direction and decision context.
- **Add a new LLM call to summarize prior runs into memory**: rejected because the milestone can derive memory from the current run's typed outputs without increasing model cost or latency.

## Dependencies / Prerequisites

- `docs/superpowers/plans/2026-04-05-evidence-provenance-foundation.md` must land first.
- In particular, this plan assumes the codebase has the Stage-1-style seams for `src/workflow/tasks/preflight.rs`, `src/agents/shared/prompt.rs`, entity resolution, and `KEY_PREVIOUS_THESIS` integration points. If those seams are absent when implementation starts, stop and land the prerequisite work first rather than inventing an alternate initialization path ad hoc.

## Implementation Units

- [ ] **Unit 1: Add typed thesis-memory state and lifecycle rules**

**Goal:** Define the thesis-memory payload and thread it through shared state with explicit compatibility and lifecycle semantics.

**Requirements:** R1, R3, R4, R7

**Dependencies:** Evidence/provenance foundation scaffolding must be present or landed first.

**Files:**
- Create: `src/state/thesis.rs`
- Modify: `src/state/mod.rs`
- Modify: `src/state/trading_state.rs`
- Test: `src/state/thesis.rs`
- Test: `tests/state_roundtrip.rs`

**Approach:**
- Add a compact `ThesisMemory` type with explicit source metadata and `schema_version`.
- Persist the canonical symbol inside the memory payload rather than relying on the raw request symbol.
- Extend `TradingState` with `thesis_memory: Option<ThesisMemory>` and initialize it to `None` in `TradingState::new`.
- Keep the payload bounded and typed; do not duplicate the entire prior `TradingState`.
- Document the lifecycle clearly: prior thesis loaded at pipeline entry, overwritten with current canonical thesis before phase-5 snapshot save.

**Patterns to follow:**
- `src/state/trading_state.rs`
- Existing derive/serde patterns across `src/state/*.rs`

**Test scenarios:**
- Happy path: serializing and deserializing a populated `ThesisMemory` preserves symbol, source metadata, and final decision/action fields.
- Edge case: `TradingState::new` initializes `thesis_memory` to `None`.
- Edge case: deserializing older `TradingState` JSON that lacks `thesis_memory` still succeeds and yields `None`.
- Edge case: thesis memory preserves the canonical symbol even when the current run started from a differently cased or aliased raw symbol.
- Error path: compatibility helpers reject unsupported `schema_version` values without panicking.

**Verification:**
- State round-trip tests prove the new payload is additive and backward-compatible with snapshots that predate thesis memory.

- [ ] **Unit 2: Extend the snapshot store with prior-thesis lookup**

**Goal:** Reuse the existing snapshot table to retrieve the latest compatible prior thesis for the same symbol without changing the storage backend.

**Requirements:** R2, R3, R4, R5, R8

**Dependencies:** Unit 1

**Files:**
- Modify: `src/workflow/snapshot.rs`
- Test: `src/workflow/snapshot.rs`

**Approach:**
- Add a lookup API that searches only phase-5 snapshots and excludes the current `execution_id`.
- Match on canonical symbol and restrict candidate rows to `target_date <= current target_date` so backtests cannot see future memory.
- Parse candidate and current target dates explicitly; malformed dates are incompatible and must be skipped with a log.
- Extract the stored `TradingState` from the candidate row, then read and validate `thesis_memory` from it.
- Apply deterministic tie-breaking: latest compatible parsed `target_date`, then latest `created_at`.
- Return `Ok(None)` for missing thesis, unsupported schema versions, or incompatible/future-dated candidates; return `TradingError::Storage` for candidate-row query failures and full `TradingState` decode/runtime failures.

**Execution note:** Start with failing lookup tests that cover same-symbol reuse, current-execution exclusion, and future-date rejection before changing the query path.

**Patterns to follow:**
- `src/workflow/snapshot.rs`
- `migrations/0001_create_phase_snapshots.sql`

**Test scenarios:**
- Happy path: the lookup returns the latest eligible phase-5 thesis for the same symbol.
- Edge case: the lookup ignores snapshots from the current `execution_id`.
- Edge case: when two eligible rows share the same target date, the lookup prefers the most recent `created_at` row.
- Edge case: the lookup ignores a later `target_date` snapshot when the current run targets an earlier date.
- Edge case: malformed candidate or current target dates are treated as incompatible rather than causing a panic.
- Edge case: the lookup returns `None` when the candidate snapshot has no `thesis_memory` field.
- Edge case: the lookup returns `None` and logs when the candidate thesis has an unsupported schema version.
- Error path: closed-pool or malformed-snapshot failures still surface as `TradingError::Storage`.

**Verification:**
- Snapshot-store tests prove semantic absence degrades to `None` while runtime persistence failures remain fatal.

- [ ] **Unit 3: Load prior thesis at pipeline entry and expose it through context**

**Goal:** Make thesis memory available to the current run before any downstream prompt-building begins.

**Requirements:** R2, R3, R5, R6

**Dependencies:** Unit 2 and the evidence/provenance preflight scaffolding

**Files:**
- Modify: `src/workflow/tasks/common.rs`
- Modify: `src/workflow/tasks/mod.rs`
- Modify: `src/workflow/tasks/preflight.rs`
- Modify: `src/workflow/context_bridge.rs`
- Modify: `src/workflow/pipeline.rs`
- Test: `src/workflow/context_bridge.rs`
- Test: `src/workflow/tasks/preflight_tests.rs`

**Approach:**
- Introduce `KEY_PREVIOUS_THESIS` in the central task-key registry.
- Extend pipeline-entry/preflight logic to resolve the canonical lookup symbol, call the snapshot-store lookup API, and write the result to both `TradingState.thesis_memory` and `KEY_PREVIOUS_THESIS`.
- Move canonical symbol resolution and thesis lookup ahead of any startup provider fetches that currently run before graph execution so the same symbol authority is used end-to-end.
- Store missing prior memory as explicit `null`, not as a missing context key.
- Define `KEY_PREVIOUS_THESIS` as a derived mirror of `TradingState.thesis_memory`, with an explicit parity rule: disagreement between the two is orchestration corruption.
- If the preflight scaffold is not yet present at implementation time, stop and land the foundation plan first rather than silently moving this logic into an alternate startup seam.
- Preserve the repo's existing invariant that missing required post-preflight keys are orchestration corruption.

**Patterns to follow:**
- `src/workflow/tasks/common.rs`
- `src/workflow/context_bridge.rs`
- `src/workflow/pipeline.rs`

**Test scenarios:**
- Happy path: pipeline entry/preflight loads compatible prior thesis and stores it in both state and context.
- Edge case: no eligible prior thesis writes `null` to the context key and leaves `state.thesis_memory` as `None`.
- Edge case: the state field and context key remain byte-for-byte equivalent for the same thesis payload.
- Edge case: the current run does not observe its own in-progress snapshots.
- Error path: snapshot lookup failure aborts preflight/pipeline entry with the expected storage error path.
- Integration: context round-trip preserves loaded thesis memory across phase boundaries.

**Verification:**
- Preflight/context tests prove thesis memory is available before researcher/trader/risk/fund-manager prompt construction begins.

- [ ] **Unit 4: Add the shared thesis-memory prompt helper and wire all downstream consumers**

**Goal:** Replace the current empty `past_memory_str` seam with a bounded, sanitized thesis-memory block.

**Requirements:** R6

**Dependencies:** Unit 3

**Files:**
- Modify: `src/agents/shared/prompt.rs`
- Modify: `src/agents/researcher/common.rs`
- Modify: `src/agents/risk/common.rs`
- Modify: `src/agents/trader/mod.rs`
- Modify: `src/agents/fund_manager/prompt.rs`
- Modify: `docs/prompts.md`
- Test: `src/agents/shared/prompt.rs`
- Test: `src/agents/researcher/common.rs`
- Test: `src/agents/risk/common.rs`
- Test: `src/agents/trader/tests.rs`
- Test: `src/agents/fund_manager/tests.rs`

**Approach:**
- Add `build_thesis_memory_context(state)` to the shared prompt helper module using the existing sanitize/redact/truncate utilities.
- Render a compact untrusted-context block when thesis memory is present and an explicit fallback string when it is absent.
- Replace the empty-string `{past_memory_str}` substitutions in researcher, risk, trader, fund-manager, and any shared moderator prompt construction paths with the shared helper output.
- Update `docs/prompts.md` so the prompt contract documents what `past_memory_str` contains and how agents should treat it.

**Execution note:** Add prompt-rendering coverage before replacing the current empty-string prompt seam so each consumer's behavior is pinned down first.

**Patterns to follow:**
- `src/agents/shared/prompt.rs`
- `src/agents/researcher/common.rs`
- `src/agents/trader/tests.rs`
- `src/agents/fund_manager/tests.rs`

**Test scenarios:**
- Happy path: populated thesis memory renders a bounded prompt-safe block containing prior date, final action/decision, and thesis summary fields.
- Edge case: `None` renders explicit fallback text rather than an empty string.
- Edge case: oversized prior rationale or summary is truncated to prompt bounds and remains sanitized.
- Error path: prompt helper serialization failure degrades to a compact unavailable/fallback block rather than panicking.
- Integration: researcher, risk, trader, fund-manager, and moderator-adjacent prompt tests all prove `{past_memory_str}` is no longer replaced with `""`.

**Verification:**
- Prompt tests prove memory is included when present, omitted safely when absent, and never expands into raw unsanitized historical blobs.

- [ ] **Unit 5: Capture and persist the current run's authoritative thesis**

**Goal:** Ensure every completed run leaves behind a reusable phase-5 thesis record for the next compatible run.

**Requirements:** R5, R7, R8

**Dependencies:** Units 1-4

**Files:**
- Modify: `src/state/thesis.rs`
- Modify: `src/workflow/tasks/trading.rs`
- Modify: `src/workflow/tasks/tests.rs`
- Test: `src/workflow/tasks/tests.rs`
- Test: `tests/workflow_pipeline_e2e.rs`
- Test: `tests/support/workflow_pipeline_e2e_support.rs`

**Approach:**
- Add a helper that distills the current run's final thesis from existing typed outputs (`consensus_summary`, `trader_proposal`, `final_execution_status`, and available risk summary context) without a second LLM call.
- Call that helper in the Fund Manager task after the final decision is available and before both `save_state(...)` and the phase-5 snapshot save.
- Overwrite `state.thesis_memory` with the new authoritative thesis before both persistence steps so the final returned workflow state and the phase-5 snapshot stay aligned.
- Keep earlier phase snapshots untouched; the reuse path should read only phase-5 snapshots.

**Execution note:** Start with a failing multi-run integration test: first run writes a phase-5 thesis, second later run loads it for the same symbol, and an earlier target-date run does not.

**Patterns to follow:**
- `src/workflow/tasks/trading.rs`
- `src/workflow/tasks/tests.rs`
- `tests/workflow_pipeline_e2e.rs`

**Test scenarios:**
- Happy path: a completed phase-5 snapshot contains the newly captured thesis memory.
- Happy path: the final in-memory/context `TradingState` and the loaded phase-5 snapshot contain the same thesis-memory payload.
- Edge case: a rejected trade still captures a valid thesis record for future runs.
- Edge case: missing consensus or sparse risk context still produces a sparse-but-valid thesis payload rather than blocking snapshot save.
- Error path: snapshot save failure after thesis capture still fails the task and does not silently continue.
- Integration: a second run for the same symbol and later target date loads the prior run's phase-5 thesis into prompt context.

**Verification:**
- End-to-end workflow tests prove the thesis-memory loop closes: one run writes the authoritative thesis, a later compatible run reads it.

## System-Wide Impact

- **Interaction graph:** pipeline entry / preflight -> canonical symbol resolution -> snapshot-store lookup -> `TradingState.thesis_memory` + derived `KEY_PREVIOUS_THESIS` -> researcher/risk/trader/fund-manager/moderator prompt helpers -> Fund Manager final decision -> thesis capture -> final workflow-state save -> phase-5 snapshot save.
- **Error propagation:** snapshot-store query/load/save failures continue to surface as `TradingError::Storage`; missing, incompatible, or future-dated thesis memory is normalized to `None`.
- **State lifecycle risks:** `thesis_memory` changes role over the life of a run (prior memory at startup, current canonical memory at phase 5). Tests must prove the overwrite happens only after all prompt consumers have finished using the prior memory and that final returned state matches the saved phase-5 snapshot.
- **Graph/bootstrap impact:** this work depends on the startup/preflight seam introduced by the evidence/provenance foundation. If that seam is absent, implementation must stop and land the prerequisite rather than hiding thesis lookup inside an ad hoc alternate entry path.
- **Symbol authority:** canonical symbol resolution must happen before startup provider fetches and remain the single lookup/persistence authority for thesis memory, so historical lookup, provider fetches, and downstream prompt context cannot diverge on ticker identity.
- **API surface parity:** CLI JSON output will inherit the new field through serialized `TradingState`, but this plan does not add new commands or change command contracts.
- **Integration coverage:** same-symbol multi-run lookup, current-execution exclusion, backtest time-travel safety, prompt fallback behavior, and final-phase persistence all need cross-layer coverage.
- **Unchanged invariants:** the five-phase workflow remains intact; no new LLM calls are added; the existing snapshot store stays the persistence boundary; no dedicated thesis database or new runtime config is introduced.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Snapshot JSON lookup becomes slow as history grows | Restrict lookup to phase-5 rows, order by prior `target_date` and `created_at`, and defer indexed metadata columns until performance data justifies them |
| Backtests accidentally read future thesis memory | Filter candidates to `prior target_date <= current target_date`, parse dates explicitly, treat malformed dates as incompatible, and add explicit earlier/later-run integration coverage |
| Thesis reuse fragments across raw-symbol variants | Use the canonical preflight-resolved symbol as the lookup/persistence authority and store it inside `ThesisMemory` |
| The single `thesis_memory` field is overwritten too early | Capture current thesis only after the Fund Manager decision and immediately before both `save_state` and phase-5 snapshot save |
| The evidence/provenance foundation seams are still missing when implementation starts | Treat the foundation plan as a hard prerequisite and stop if `preflight`, shared prompt helpers, or task-key scaffolding are absent |
| Incompatible thesis schema breaks older snapshots | Keep the payload additive, include `schema_version`, and degrade unsupported schemas to `None` with a log rather than hard-failing the run |

## Documentation / Operational Notes

- Update `docs/prompts.md` to describe the `past_memory_str` contract and its fallback semantics.
- No new environment variables or config flags are required in this slice.
- If implementation reveals JSON-query performance pain, capture that as a separate follow-on plan rather than expanding this milestone into a snapshot-schema redesign.

## Sources & References

- Milestone source: `docs/superpowers/specs/2026-04-05-financial-services-plugins-inspired-architecture-design.md`
- Stage-1 prerequisite: `docs/superpowers/plans/2026-04-05-evidence-provenance-foundation.md`
- Related code: `src/workflow/snapshot.rs`
- Related code: `src/workflow/context_bridge.rs`
- Related code: `src/workflow/tasks/common.rs`
- Related code: `src/workflow/tasks/trading.rs`
- Related code: `src/agents/shared/prompt.rs`
- Related code: `src/agents/researcher/common.rs`
- Related code: `src/agents/risk/common.rs`
- Related code: `src/agents/trader/mod.rs`
- Related code: `src/agents/fund_manager/prompt.rs`
- Related storage schema: `migrations/0001_create_phase_snapshots.sql`
