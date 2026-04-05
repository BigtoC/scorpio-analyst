## Context

The system's five-phase pipeline already enforces structured JSON outputs via serde, but it does not yet enforce evidence discipline at the analyst prompt layer. Analysts receive tool bindings and are expected to return data drawn from tool output, but nothing in the runtime prompt contract explicitly:

1. prohibits inferring estimates, quarter labels, or transcript commentary not present in tool output
2. requires sparse or missing evidence to lower confidence explicitly in `summary`
3. requires separation of observed facts from model interpretation
4. provides a shared Rust API for reusable evidence-discipline prompt fragments

The `src/agents/shared/prompt.rs` module already provides sanitization helpers (`sanitize_symbol_for_prompt`, `sanitize_prompt_context`, `serialize_prompt_value`) but has no shared evidence-discipline rule helpers.

`docs/prompts.md` already contains a `## Global Prompt Rules` section with eight rules. Two of the desired evidence-discipline directives already exist in partial form:

- preserve missing data honestly
- distinguish observed facts from interpretation

This chunk therefore needs to tighten and extend that section, not blindly append duplicated bullets with slightly different wording.

`README.md` currently mentions TradingAgents but not Anthropic's financial-services-plugins, even though the architect plan and Stage 1 design explicitly cite it as an inspiration.

The current analyst modules build their final system prompts at runtime by taking a `const &str` template and calling `.replace("{ticker}", ...)` / `.replace("{current_date}", ...)` inside `run()`. There is no existing rendered-prompt helper. This matters because the earlier draft suggested `concat!`-style composition with function-returned strings, but `concat!` only accepts string literals and cannot call helper functions.

## Constraints

- No new crate dependencies.
- No `TradingState` schema changes.
- No config changes.
- New shared helper functions follow the existing `pub(crate)` visibility convention in `src/agents/shared/prompt.rs`.
- Chunk 1 must not introduce state-dependent `build_evidence_context(...)` or `build_data_quality_context(...)` helpers; those belong to `chunk3-evidence-state-sync`, which introduces the typed evidence and coverage fields they should render.
- Cross-owner edits are limited to `src/agents/shared/prompt.rs` and the four analyst modules. If the work needs config, state, workflow, provider, or report changes, that is scope drift into later chunks.

## Goals / Non-Goals

**Goals:**

- Credit Anthropic's financial-services-plugins inspiration in `README.md`.
- Update `docs/prompts.md` so the `## Global Prompt Rules` section explicitly covers the five evidence-discipline directives without duplicating existing rules unnecessarily.
- Add three static rule helpers to `src/agents/shared/prompt.rs` that return `&'static str` rule text.
- Extend the four analyst prompts to include the three shared rule helpers.
- Add unit tests for the three new helpers and rendered-prompt string-contains tests in each analyst file.

**Non-Goals:**

- Changing `TradingState` fields or adding provenance metadata fields.
- Introducing state-dependent `build_evidence_context(...)` or `build_data_quality_context(...)` helpers.
- Injecting evidence/data-quality context into researcher, trader, risk, or fund-manager prompts.
- Config, workflow, provider, report, or `TradingState` changes.

## Decisions

### 1. Static rule helpers return `&'static str`

**Decision**: The three rule helpers (`build_authoritative_source_prompt_rule`, `build_missing_data_prompt_rule`, `build_data_quality_prompt_rule`) return `&'static str` pointing to compile-time constant strings.

**Rationale**: The rules are invariant and do not depend on runtime state or configuration. `&'static str` avoids allocation and matches the existing style of `UNTRUSTED_CONTEXT_NOTICE` in the same module.

**Alternatives considered**:

- *Return `String`*: requires allocation with no benefit for invariant text.
- *Return `Cow<'static, str>`*: adds complexity with no payoff for purely static content.

### 2. `docs/prompts.md` is tightened, not duplicated

**Decision**: The `## Global Prompt Rules` section is updated so it explicitly covers all five evidence-discipline directives, but existing bullets are edited or strengthened where that is cleaner than adding duplicate bullets.

**Rationale**: The file already contains missing-data and facts-vs-interpretation guidance. Repeating those lines with slightly different wording would make the docs noisier and could create drift about which bullet is authoritative.

### 3. Analyst prompts are hardened at runtime prompt assembly

**Decision**: Each analyst file appends the shared rule-helper strings at the runtime prompt-rendering site used in `run()`. If helpful for testability, the file may extract the existing `.replace(...)` chain into a small `build_*_system_prompt(...) -> String` helper reused by both `run()` and tests.

**Rationale**: This matches the real code shape in `src/agents/analyst/*.rs`. It avoids the invalid `concat!`-with-function-call approach and keeps the change minimal: prompt construction still happens where it already happens.

**Alternatives considered**:

- *Inline everything into the raw `const &str` template*: duplicates shared rule text four times.
- *Introduce a large cross-agent prompt builder abstraction now*: too much scope for a docs-and-prompts slice.

### 4. Chunk 1 does not add state-dependent prompt-context builders

**Decision**: `build_evidence_context(state)` and `build_data_quality_context(state)` are not part of Chunk 1. They remain owned by `chunk3-evidence-state-sync`.

**Rationale**: Those helpers are only useful once the typed evidence and coverage fields exist. Adding them now would either force premature `TradingState` changes or bind them to the wrong legacy fields and then rewrite them again in Chunk 3.

### 5. Prompt tests assert on the rendered prompt, not only the raw template

**Decision**: Each analyst file adds or updates tests so they assert against the final rendered system prompt string used by the agent, including the shared helper text and the analyst-specific unsupported-inference lines.

**Rationale**: If hardening is appended during runtime prompt assembly, tests that only inspect the raw `const &str` template can miss regressions.

## Risks / Trade-offs

- **[Prompt length increase]** Each analyst prompt grows by a few lines of rule text. The token cost is small relative to the benefit of reducing unsupported claims.
- **[Rule phrasing may need tuning]** Initial phrasing is a best-effort starting point; specific models may respond better to different wording. Mitigation: keep the rules centralized in `src/agents/shared/prompt.rs`.
- **[Test brittleness]** Rendered-prompt string-contains tests will fail if someone rewrites prompt assembly and forgets to include the rules. This is the intended regression guard. Mitigation: assert on short, stable phrases instead of entire prompts.
- **[Scope drift into later chunks]** It is easy to start adding state-dependent prompt context, config, or workflow changes while working in these files. Mitigation: this chunk explicitly forbids those changes and records them as deferred work in Chunks 2-4.

## Migration Plan

No migration required. This change is purely additive:

1. Update documentation in `README.md` and `docs/prompts.md`.
2. Add three static helper functions to `src/agents/shared/prompt.rs`.
3. Extend analyst prompt rendering with append-only hardening text and rendered-prompt tests.
4. Roll back by reverting the touched docs and prompt files. No config, state, workflow, or storage migration is involved.

## Open Questions

None at proposal stage. The remaining deferred question is sequencing, not behavior: dynamic evidence/data-quality context helpers are intentionally left to `chunk3-evidence-state-sync`.
