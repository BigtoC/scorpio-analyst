## Context

The system's five-phase pipeline currently enforces structured JSON outputs via serde but does not enforce evidence discipline at the prompt layer. Analysts receive tool bindings and are expected to return data drawn from tool output, but nothing in the system prompt explicitly:

1. Prohibits inferring estimates, quarter labels, or transcript commentary not present in tool output.
2. Requires sparse or missing evidence to lower confidence explicitly.
3. Requires separation of observed facts from model interpretation.
4. Provides a shared Rust API for building prompt fragments programmatically.

The `src/agents/shared/prompt.rs` module already provides sanitization helpers (`sanitize_symbol_for_prompt`, `sanitize_prompt_context`, `serialize_prompt_value`) but has no rule-building or context-building functions.

`docs/prompts.md` already has a `## Global Prompt Rules` section with seven rules. Five new rules covering evidence discipline need to be added there.

`README.md` has no mention of the Anthropic financial-services-plugins inspiration that motivates this evidence discipline posture.

Constraints:
- No new crate dependencies.
- No `TradingState` schema changes.
- No config changes.
- Prompt helpers must not panic when optional `TradingState` fields are absent — all `Option<T>` fields must yield graceful fallback text.
- New functions follow the existing `pub(crate)` visibility convention in `prompt.rs`.
- Analyst system prompts are `const &str` values; the shared rule helpers return `&'static str` and are appended at compile time or concatenated via `format!` / `concat!` in the calling module.

## Goals / Non-Goals

**Goals:**
- Credit Anthropic's financial-services-plugins inspiration in `README.md`.
- Add five evidence-discipline rules to `docs/prompts.md` under `## Global Prompt Rules`.
- Add three static rule helpers to `src/agents/shared/prompt.rs` that return `&'static str` rule text.
- Add two dynamic context builders to `src/agents/shared/prompt.rs` that accept `&TradingState` and return a `String`.
- Extend the four analyst system prompts to include the three shared rule helpers.
- Add unit tests for all five new functions and string-contains tests in each analyst file.

**Non-Goals:**
- Changing `TradingState` fields or adding provenance metadata fields (deferred to a later chunk).
- Introducing a typed `EvidenceContext` or `PromptContext` struct (deferred).
- Injecting `build_evidence_context` / `build_data_quality_context` output into analyst prompts at this stage — those helpers are written and tested now but wired into downstream agents in a later chunk.
- Changing researcher, trader, risk, or fund-manager prompts (scope limited to analyst tier in this chunk).

## Decisions

### 1. Static rule helpers return `&'static str`

**Decision**: The three rule helpers (`build_authoritative_source_prompt_rule`, `build_missing_data_prompt_rule`, `build_data_quality_prompt_rule`) return `&'static str` pointing to compile-time constant strings.

**Rationale**: The rules are invariant — they do not depend on runtime state or configuration. `&'static str` avoids allocation, is zero-cost to concatenate into analyst `const` prompts at compile time using `concat!`, and is consistent with how the existing `UNTRUSTED_CONTEXT_NOTICE` constant works in the same module.

**Alternatives considered**:
- *Return `String`*: Requires allocation on every call. No benefit when content is invariant.
- *Return `Cow<'static, str>`*: Adds complexity with no payoff for purely static content.

### 2. Dynamic context builders return `String` and never panic

**Decision**: `build_evidence_context(state: &TradingState) -> String` and `build_data_quality_context(state: &TradingState) -> String` return `String` and handle every `Option<T>` field with a `None` fallback (serialize as `null` or the string `"unavailable"`).

**Rationale**: `TradingState` fields are populated incrementally. When a builder is called early in the pipeline or in a test with partial state, absent fields must not panic or return an error. Returning `String` is consistent with `sanitize_prompt_context`, which already returns `String`.

**Implementation**: Use `serialize_prompt_value` (already in `prompt.rs`) for each `Option<T>` field, which calls `serde_json::to_string(&value).unwrap_or_else(|_| "null".to_owned())` and sanitizes the result. The two context builders produce multi-line compact JSON-like blocks (not raw JSON) bounded by `MAX_PROMPT_CONTEXT_CHARS` via `sanitize_prompt_context`.

### 3. Analyst prompts append rule helpers via `concat!` or const composition

**Decision**: Each analyst `const SYSTEM_PROMPT: &str` is extended by appending the three rule helper outputs as additional instruction lines at the end of the prompt. Where the prompt is a `const &str`, the helpers (which return `&'static str`) can be concatenated at compile time with `concat!`. Where the prompt uses `format!` for runtime substitution, the helper text is inserted before the `format!` call or embedded as a constant-suffix pattern.

**Rationale**: Keeps the rule text single-source (in `prompt.rs`) while preserving the existing `const &str` pattern in each analyst file. No runtime allocation for the rule text itself.

**Alternatives considered**:
- *Inline the rule text directly in each analyst prompt*: Creates four copies of identical text that drift independently. Rejected (see proposal).

### 4. Five new global prompt rules are added to `docs/prompts.md` — not copied from `context`/`rules`

**Decision**: The five rules are written once in `docs/prompts.md` as the authoritative human-readable reference. The Rust helpers in `prompt.rs` contain their own terse, model-facing phrasing of the same rules (not a copy of the doc text).

**Rationale**: The doc is for human reviewers and future prompt engineers. The Rust helpers are for the model. The phrasing can differ in register and length — the doc explains intent; the helper is compact and imperative.

### 5. `build_data_quality_context` computes `missing_inputs` by inspecting `Option<T>` fields

**Decision**: The function checks `state.fundamental_metrics.is_none()`, `state.technical_indicators.is_none()`, `state.market_sentiment.is_none()`, `state.macro_news.is_none()` and builds a `missing_inputs` list accordingly. `providers_used` is a placeholder `"runtime"` string at this stage (detailed provenance tracking is deferred to a later chunk).

**Rationale**: The missing-inputs list gives the model clear signal about which evidence buckets are absent without requiring any new state fields. The placeholder `providers_used` value is honest and avoids inventing provenance data before the infrastructure exists.

## Risks / Trade-offs

- **[Prompt length increase]** Each analyst prompt grows by ~3–5 lines of rule text. At typical gpt-4o-mini token costs this adds ~60–80 tokens per analyst call (4 calls per run), a negligible cost given the benefit of reduced hallucination. Mitigation: rule helper text is intentionally terse.
- **[Rule phrasing may need tuning]** Initial phrasing is a best-effort starting point; specific models may respond better to different wording. Mitigation: rules are in one place (`prompt.rs`) so a single edit propagates to all analysts. Tuning is a future task.
- **[Test brittleness]** String-contains tests for analyst prompts will fail if someone rewrites the system prompt and forgets to include the rules. This is the intended behavior — the test is a regression guard. Mitigation: keep the test assertion strings short (a few words) rather than matching the full rule text.
- **[Context builder output may be large]** `build_evidence_context` serializes up to four fully-populated analyst structs. Mitigation: `sanitize_prompt_context` already applies `MAX_PROMPT_CONTEXT_CHARS` truncation, which is the existing guard for all prompt context in the system.

## Migration Plan

No migration required. This change is purely additive:
1. Add documentation (no compilation dependency).
2. Add functions to `src/agents/shared/prompt.rs` (new exports, no changed signatures).
3. Extend analyst system prompt constants (appended text; existing tests that assert on prompt content may need to be updated if they do exact-match, but the project currently has no such tests).
4. Rollback: revert additions in all touched files. No database, config, or state migration needed.

## Open Questions

None at proposal stage. Phrasing of individual rule helper strings may be refined during implementation based on model-specific testing.
