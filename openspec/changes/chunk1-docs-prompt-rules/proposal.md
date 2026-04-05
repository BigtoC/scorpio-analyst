## Why

The project was directly inspired by Anthropic's financial-services-plugins architecture, which applies evidence discipline, provenance reporting, and event-driven analysis patterns to LLM-based financial tooling. As scorpio-analyst matures, its multi-agent pipeline faces the same failure mode that financial-services-plugins addresses: agents fabricate or pad outputs when runtime evidence is sparse, invent estimates not present in tool output, and conflate observed facts with interpretation. This erodes trust in the analysis and makes downstream audit difficult.

Chunk 1 is the foundational delivery slice. It adds the documentation credit, establishes global prompt rules in `docs/prompts.md`, introduces shared Rust prompt helpers in `src/agents/shared/prompt.rs`, and hardens all four analyst system prompts around evidence discipline. All subsequent chunks (typed evidence context, provenance fields, event-driven signals) build on this layer.

## What Changes

- **`README.md`**: Add a credit to Anthropic's financial-services-plugins in the project introduction, clarifying the architectural inspiration.
- **`docs/prompts.md`**: Extend the existing `## Global Prompt Rules` section with five new evidence-discipline rules: prefer authoritative runtime evidence over inference; return schema-compatible null/[]/sparse summaries rather than guessing when data is missing; distinguish observed facts from interpretation; lower confidence explicitly when evidence is missing or sparse; let Rust compute deterministic comparisons and use the model to interpret them.
- **`src/agents/shared/prompt.rs`**: Add three static rule helpers (`build_authoritative_source_prompt_rule`, `build_missing_data_prompt_rule`, `build_data_quality_prompt_rule`) and two dynamic context builders (`build_evidence_context`, `build_data_quality_context`) that read from `TradingState` and return compact JSON blocks or graceful fallback text — never panic on absent fields.
- **`src/agents/analyst/fundamental.rs`**, **`news.rs`**, **`sentiment.rs`**, **`technical.rs`**: Import and append the three shared rule helpers to each analyst system prompt; add explicit lines against inferring estimates, transcript commentary, or quarter labels unless the runtime provides them; require explicit sparse-evidence acknowledgement in `summary`; and require separation of observed facts from interpretation.

## Capabilities

### New Capabilities
- `evidence-prompt-rules`: Shared, testable prompt rule helpers for evidence discipline, missing-data handling, and data quality context — available to all agents via `src/agents/shared/prompt.rs`.

### Modified Capabilities
- `analyst-team`: All four analyst system prompts are hardened with evidence discipline rules sourced from the new shared helpers.

## Impact

- **Docs**: `README.md` gains one attribution sentence; `docs/prompts.md` gains five new global rules under the existing `## Global Prompt Rules` section.
- **Code**: `src/agents/shared/prompt.rs` gains five new pub(crate) functions; each of the four analyst `.rs` files gains an import and system prompt extension. No state schema changes, no new crate dependencies, no config changes.
- **Tests**: Unit tests added for all five new prompt helpers; string-contains tests added in each analyst file to verify the rules are present in the compiled system prompt.
- **Rollback**: Revert the five new functions in `prompt.rs`, the prompt extensions in the four analyst files, and the documentation additions. No state migration, no DB changes, no config changes required.

## Alternatives Considered

### Option: Inline rules per analyst (no shared helpers)
Add the evidence rules directly into each analyst's `const` system prompt string, without a shared helper module.

Pros: Zero new abstractions, minimal code surface, trivially reviewable.

Cons: Rules drift independently across the four analyst files and any future agents. Updating a rule requires touching every agent file. No testability isolation — the rules can only be verified by reading the full prompt string. Duplicates ~200 bytes of text four times, inflating token usage.

Why rejected: The shared helper approach costs almost nothing (five small functions) and gives us one source of truth, isolated unit tests per rule, and a stable import point for all future agents.

### Option: Encode rules as TOML/config and inject at runtime
Store the global prompt rules in `config.toml` or a sidecar `prompts.toml`, load them at startup, and inject them into agent prompts dynamically.

Pros: Rules can be tuned without recompiling; non-Rust operators can adjust them.

Why rejected: The project's current architecture embeds system prompts as `const &str` in Rust source. Config-driven rules would require a new config section, a loading path, propagation through `TradingState` or a separate context object, and error handling for missing config entries. The added complexity is disproportionate to the benefit for a project in early development. If operator-tunable rules become a requirement, a future change can introduce that path built on top of this foundation.

### Option: Defer to a later chunk with typed PromptContext struct
Skip standalone rule helpers now and wait until a future chunk introduces a full `PromptContext` or `EvidenceContext` typed struct, then express all rules through that struct's builder methods.

Pros: One coherent abstraction rather than incremental additions; avoids refactoring the helpers later.

Why rejected: The typed context struct depends on richer state fields (confidence scores, provenance metadata) that are not yet present in `TradingState`. Waiting blocks the immediate documentation and analyst-hardening goals. The simple static helpers in this chunk are directly compatible with any future typed context builder — they can be called from inside a builder method with no interface break.
