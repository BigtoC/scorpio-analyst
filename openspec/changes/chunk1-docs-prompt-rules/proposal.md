## Why

The project is directly informed by Anthropic's financial-services-plugins architecture, especially its evidence discipline, provenance posture, and refusal to let models fabricate unsupported financial facts. Scorpio-Analyst faces the same failure mode: when the prompt contract is loose, analyst agents can pad summaries, invent quarter labels or estimates that never appeared in tool output, and blur facts with interpretation. That undermines downstream reasoning and operator trust.

Per `docs/architect-plan.md`, this work belongs to the architected `evidence-provenance` capability. Chunk 1 is only the first delivery slice of that capability. To stay aligned with the architect plan and avoid overlap with later chunks, this slice must stay constrained to:

- documentation alignment in `README.md` and `docs/prompts.md`
- shared static prompt-rule helpers in `src/agents/shared/prompt.rs`
- analyst prompt hardening in `src/agents/analyst/*`

The previous draft over-scoped Chunk 1 by claiming state-dependent prompt-context builders that overlap `chunk3-evidence-state-sync`, and by inventing a new capability name that does not exist in the architect plan. This review corrects that scope drift.

## What Changes

- **`README.md`**: Add a credit to Anthropic's financial-services-plugins in the project introduction, clarifying the architectural inspiration.
- **`docs/prompts.md`**: Tighten the existing `## Global Prompt Rules` section so it explicitly covers five evidence-discipline directives without duplicating guidance already present in the file: authoritative runtime evidence over inference; schema-compatible missing-data handling without padding; facts vs. interpretation; explicit confidence reduction when evidence is sparse or missing; and Rust-owned deterministic comparisons with model-owned interpretation.
- **`src/agents/shared/prompt.rs`**: Add three static rule helpers only: `build_authoritative_source_prompt_rule`, `build_missing_data_prompt_rule`, and `build_data_quality_prompt_rule`.
- **`src/agents/analyst/fundamental.rs`**, **`news.rs`**, **`sentiment.rs`**, **`technical.rs`**: Import and append the three shared rule helpers at the existing runtime prompt-construction site for each analyst; add explicit lines against inferring estimates, transcript commentary, or quarter labels unless runtime data provides them; require explicit sparse-evidence acknowledgement in `summary`; and require separation of observed facts from interpretation. If needed for testability, extract the current runtime prompt assembly into a small local render helper per file.

## Capabilities

### Architected Capability Slice

- `evidence-provenance`: This change delivers the documentation-and-analyst-prompt slice of the architected cross-cutting capability described in `docs/architect-plan.md`. It does **not** create a separate top-level capability ID such as `evidence-prompt-rules`.

### Explicitly Deferred To Later Chunks

- `DataEnrichmentConfig`, `ResolvedInstrument`, `ProviderCapabilities`, and `PreflightTask` remain in `chunk2-config-entity-preflight`.
- Typed evidence/provenance/coverage state and state-dependent prompt context builders remain in `chunk3-evidence-state-sync`.
- Final report coverage/provenance sections remain in `chunk4-report-verification`.

## Impact

- **Docs**: `README.md` gains one attribution sentence; `docs/prompts.md` is tightened so the global rules explicitly cover the five evidence-discipline directives without redundant duplicate bullets.
- **Code**: `src/agents/shared/prompt.rs` gains three new `pub(crate)` static helper functions only; each of the four analyst `.rs` files gains prompt-hardening changes at the runtime prompt-rendering site. No state schema changes, no new crate dependencies, no config changes, no provider API additions, and no workflow/report changes are part of this chunk.
- **Tests**: Unit tests added for the three static prompt helpers; rendered-prompt string-contains tests added in each analyst file to verify the shared rule text and analyst-specific unsupported-inference lines are present in the final system prompt used by `run()`.
- **Rollback**: Revert the three new functions in `src/agents/shared/prompt.rs`, the prompt-hardening changes in the four analyst files, and the documentation additions. No state migration, no DB changes, no config changes required.

## Cross-Owner Changes

This slice requires explicit cross-owner acknowledgement under the rules in [`docs/architect-plan.md#conflict-analysis`](../../../docs/architect-plan.md#conflict-analysis) and [`docs/architect-plan.md#module-ownership-map`](../../../docs/architect-plan.md#module-ownership-map).

- [`src/agents/shared/prompt.rs`](../../../src/agents/shared/prompt.rs) — owned by `add-project-foundation`. This file is the architected home for shared prompt-discipline helpers, so Chunk 1 must add the static evidence-rule helpers here rather than duplicating them in analyst modules.
- [`src/agents/analyst/fundamental.rs`](../../../src/agents/analyst/fundamental.rs) — owned by `add-analyst-team`. Chunk 1 hardens the actual runtime system prompt for the Fundamental Analyst in-place.
- [`src/agents/analyst/news.rs`](../../../src/agents/analyst/news.rs) — owned by `add-analyst-team`. Chunk 1 hardens the actual runtime system prompt for the News Analyst in-place.
- [`src/agents/analyst/sentiment.rs`](../../../src/agents/analyst/sentiment.rs) — owned by `add-analyst-team`. Chunk 1 hardens the actual runtime system prompt for the Sentiment Analyst in-place.
- [`src/agents/analyst/technical.rs`](../../../src/agents/analyst/technical.rs) — owned by `add-analyst-team`. Chunk 1 hardens the actual runtime system prompt for the Technical Analyst in-place.

`README.md` and `docs/prompts.md` are repository docs and are not assigned to a specific change owner in the module ownership map.

No cross-owner modifications to `src/config.rs`, `config.toml`, `src/state/*`, `src/data/*`, `src/workflow/*`, `src/report/*`, or `src/providers/*` are required in this chunk. If implementation needs those files, the work has drifted into Chunks 2-4 and the proposal must be re-scoped.

## Alternatives Considered

### Option: Inline rules per analyst (no shared helpers)

Add the evidence rules directly into each analyst's prompt text without a shared helper module.

Pros: Zero new abstractions, minimal code surface, trivially reviewable.

Cons: Rules drift independently across four analyst files and future agents. Updating a rule requires touching every agent file. Tests cannot verify a single shared source of truth.

Why rejected: The shared helper approach costs almost nothing and gives one source of truth, isolated unit tests per rule, and a stable import point for future prompt consumers.

### Option: Add state-dependent evidence/data-quality context builders in Chunk 1

Implement `build_evidence_context(state)` and `build_data_quality_context(state)` now, even though the typed evidence and coverage fields do not exist yet.

Pros: Matches the full Stage 1 architecture boundary in one chunk. Gives later chunks fewer moving parts.

Cons: Overlaps directly with `chunk3-evidence-state-sync`, which already owns the typed evidence/provenance/coverage state and downstream prompt-context injection. In the current codebase, `TradingState` does not yet contain `evidence_*`, `data_coverage`, or `provenance_summary`, so any Chunk 1 implementation would either be forced to use the wrong legacy fields or require premature state changes.

Why rejected: Chunk 1 should establish the human docs and shared static rules first. State-dependent context builders belong in the chunk that introduces the typed state they are meant to render.

### Option: Encode rules as TOML/config and inject at runtime

Store the global prompt rules in `config.toml` or a sidecar prompt config file, load them at startup, and inject them into agent prompts dynamically.

Pros: Rules can be tuned without recompiling.

Cons: The project currently embeds system prompts in Rust source. Config-driven rules would require new config surface area, loading paths, and error handling for missing config entries. That complexity is disproportionate for a small first slice.

Why rejected: Static shared helpers are the smallest correct implementation for the current architecture.
