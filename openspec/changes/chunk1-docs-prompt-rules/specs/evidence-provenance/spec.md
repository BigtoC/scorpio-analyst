# `evidence-provenance` Capability

## ADDED Requirements

### Requirement: Evidence Discipline Documentation Alignment

The repository MUST document Anthropic's financial-services-plugins as an architectural inspiration alongside TradingAgents, and the human-readable prompt reference in `docs/prompts.md` MUST codify the Stage 1 evidence-discipline posture.

The `## Global Prompt Rules` section in `docs/prompts.md` MUST explicitly cover all of the following directives:

- authoritative runtime evidence over inference or memory
- schema-compatible missing-data handling without guessing or padding
- observed facts versus interpretation
- explicit confidence reduction when evidence is sparse or missing
- Rust-owned deterministic comparisons with model-owned interpretation

The documentation MAY tighten or merge existing bullets to express those directives, but it MUST NOT duplicate equivalent rules with conflicting wording.

#### Scenario: README Credits Both Inspirations

- **WHEN** a developer reads the project introduction in `README.md`
- **THEN** it names both TradingAgents and Anthropic's financial-services-plugins as architectural inspirations

#### Scenario: Global Prompt Rules Stay Non-Duplicative

- **WHEN** `docs/prompts.md` is updated for Stage 1 evidence discipline
- **THEN** the five directives above are present in `## Global Prompt Rules`, and the file does not introduce redundant duplicate bullets for missing-data handling or facts-versus-interpretation guidance

### Requirement: Shared Static Evidence-Discipline Prompt Rules

`src/agents/shared/prompt.rs` MUST provide three reusable static helper functions:

- `build_authoritative_source_prompt_rule() -> &'static str`
- `build_missing_data_prompt_rule() -> &'static str`
- `build_data_quality_prompt_rule() -> &'static str`

These helpers MUST centralize the Stage 1 evidence-discipline language used by analyst prompts so the rule text remains single-source and unit-testable.

This Chunk 1 slice MUST NOT introduce state-dependent `build_evidence_context(...)` or `build_data_quality_context(...)` helpers. Those are deferred until the typed evidence and coverage state exists.

#### Scenario: Analyst Prompt Uses Shared Rule Helpers

- **WHEN** an analyst prompt needs evidence-discipline instructions
- **THEN** it imports the three shared helper functions from `src/agents/shared/prompt.rs` instead of duplicating equivalent rule text inline in multiple files

#### Scenario: State-Dependent Context Builders Remain Deferred

- **WHEN** Chunk 1 is implemented
- **THEN** `src/agents/shared/prompt.rs` does not add helper functions that depend on `TradingState`, typed evidence fields, or coverage/provenance reporting fields

### Requirement: Analyst Prompt Evidence Hardening

The four analyst modules in `src/agents/analyst/` MUST append the shared evidence-discipline rules to their system prompts at the existing runtime prompt-construction site used before `build_agent_with_tools(...)` is called.

Each hardened analyst prompt MUST:

- forbid inferring unsupported runtime data such as estimates, transcript commentary, or quarter labels unless the runtime explicitly supplies them
- require the `summary` field to acknowledge sparse or missing evidence instead of padding weak claims
- separate observed facts from interpretation

The implementation MUST be compatible with the current code shape where analyst prompts begin as `const &str` templates and are rendered at runtime via placeholder replacement. It MUST NOT rely on `concat!` with function calls.

Prompt-rendering tests MUST assert against the rendered final system prompt string used by the agent.

#### Scenario: Rendered Analyst Prompt Includes Shared Evidence Rules

- **WHEN** any analyst constructs its system prompt for a run
- **THEN** the rendered prompt includes the three shared rule-helper strings plus the analyst-specific unsupported-inference and sparse-evidence instructions before the agent is built

#### Scenario: Prompt Composition Matches Current Runtime Structure

- **WHEN** the analyst prompts are hardened in this slice
- **THEN** the implementation uses runtime string composition compatible with the existing `.replace(...)` prompt-rendering flow, rather than invalid const-only composition based on helper-function calls
