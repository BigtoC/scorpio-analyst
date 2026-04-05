## 1. Documentation Updates

- [ ] 1.1 Add a credit sentence to `README.md` in the project introduction section acknowledging the Anthropic financial-services-plugins architecture as an inspiration for the evidence discipline and provenance-reporting patterns in this project.
- [ ] 1.2 Add five new rules to the `## Global Prompt Rules` section in `docs/prompts.md`, after the existing seven rules:
  - "Prefer authoritative runtime evidence over inference or memory. Only report what the bound tools actually returned for this run."
  - "If required data is missing, return schema-compatible `null`, `[]`, or sparse summaries instead of guessing or padding."
  - "Distinguish observed facts from interpretation. Tool output is fact; your reasoning about that output is interpretation."
  - "Missing or sparse evidence must lower confidence explicitly. Do not claim high confidence when key inputs are absent."
  - "Let Rust compute deterministic comparisons (indicator thresholds, ratio checks). Use the model to interpret those computed results."
- [ ] 1.3 Verify the additions with: `rg -n "financial-services-plugins|authoritative runtime evidence|Let Rust compute" README.md docs/prompts.md`
- [ ] 1.4 Commit: `git add README.md docs/prompts.md && git commit -m "docs: credit financial-services-plugins inspiration and harden prompt rules"`

## 2. Shared Prompt Helpers

- [ ] 2.1 Add `build_authoritative_source_prompt_rule() -> &'static str` to `src/agents/shared/prompt.rs`. Return a terse imperative rule: prefer authoritative runtime evidence and never infer estimates, transcript commentary, or quarter labels unless the runtime provides them.
- [ ] 2.2 Add `build_missing_data_prompt_rule() -> &'static str` to `src/agents/shared/prompt.rs`. Return a terse imperative rule: when evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims; return `null` or `[]` for missing structured fields.
- [ ] 2.3 Add `build_data_quality_prompt_rule() -> &'static str` to `src/agents/shared/prompt.rs`. Return a terse imperative rule: separate observed facts (tool output) from interpretation (your reasoning); do not present interpretation as established fact.
- [ ] 2.4 Add `build_evidence_context(state: &TradingState) -> String` to `src/agents/shared/prompt.rs`. The function must:
  - Import `crate::state::TradingState`.
  - Serialize each of `state.fundamental_metrics`, `state.technical_indicators`, `state.market_sentiment`, `state.macro_news` via `serialize_prompt_value` (already in the module).
  - Format a compact multi-line block with a `[evidence_context]` header and labeled fields.
  - Never panic on `None` fields — each absent field serializes as `null`.
  - Apply `sanitize_prompt_context` to the full output.
- [ ] 2.5 Add `build_data_quality_context(state: &TradingState) -> String` to `src/agents/shared/prompt.rs`. The function must:
  - Compute `required_inputs`: always the list `["fundamental_metrics", "technical_indicators", "market_sentiment", "macro_news"]`.
  - Compute `missing_inputs`: those field names from `required_inputs` whose corresponding `state` field is `None`.
  - Set `providers_used`: the string `"runtime"` (provenance details are deferred to a later chunk).
  - Format a compact multi-line block with a `[data_quality]` header.
  - Apply `sanitize_prompt_context` to the full output.
- [ ] 2.6 Add unit tests in `src/agents/shared/prompt.rs` under `#[cfg(test)]`:
  - `test_authoritative_source_rule_is_not_empty`: assert `!build_authoritative_source_prompt_rule().is_empty()`.
  - `test_missing_data_rule_is_not_empty`: assert `!build_missing_data_prompt_rule().is_empty()`.
  - `test_data_quality_rule_is_not_empty`: assert `!build_data_quality_prompt_rule().is_empty()`.
  - `test_build_evidence_context_with_empty_state`: construct a `TradingState::new("AAPL", "2026-04-05")` with no phase data set, call `build_evidence_context`, assert the result contains `"null"` (because all optional fields are `None`) and contains `"[evidence_context]"`.
  - `test_build_data_quality_context_all_missing`: same empty state, call `build_data_quality_context`, assert the result contains `"fundamental_metrics"` in the missing list and contains `"[data_quality]"`.
  - `test_build_data_quality_context_none_missing`: set all four analyst fields to non-None values on a `TradingState`, call `build_data_quality_context`, assert `missing_inputs` is empty or absent in the output.
- [ ] 2.7 Commit: `git add src/agents/shared/prompt.rs && git commit -m "feat: add shared prompt helpers for evidence and data quality"`

## 3. Analyst Prompt Hardening

- [ ] 3.1 In `src/agents/analyst/fundamental.rs`, import `build_authoritative_source_prompt_rule`, `build_missing_data_prompt_rule`, `build_data_quality_prompt_rule` from `crate::agents::shared::prompt`. Extend `FUNDAMENTAL_SYSTEM_PROMPT` by appending — via `concat!` or a composed constant — the text of all three rule helpers, plus explicit lines:
  - "Do not infer estimates, transcript commentary, or quarter labels unless the runtime provides them."
  - "If evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims."
  - "Separate observed facts from interpretation."
- [ ] 3.2 In `src/agents/analyst/news.rs`, apply the same import and prompt extension as 3.1.
- [ ] 3.3 In `src/agents/analyst/sentiment.rs`, apply the same import and prompt extension as 3.1.
- [ ] 3.4 In `src/agents/analyst/technical.rs`, apply the same import and prompt extension as 3.1.
- [ ] 3.5 Add string-contains unit tests in each of the four analyst files under `#[cfg(test)]`:
  - Assert that the composed system prompt string contains the text returned by `build_authoritative_source_prompt_rule()`.
  - Assert that the composed system prompt string contains the text returned by `build_missing_data_prompt_rule()`.
  - Assert that the composed system prompt string contains the text returned by `build_data_quality_prompt_rule()`.
  - Assert that the composed system prompt string contains `"Do not infer estimates"`.
  - Assert that the composed system prompt string contains `"sparse or missing"`.
  - Assert that the composed system prompt string contains `"Separate observed facts"`.
- [ ] 3.6 Commit: `git add src/agents/analyst/fundamental.rs src/agents/analyst/news.rs src/agents/analyst/sentiment.rs src/agents/analyst/technical.rs && git commit -m "feat: harden analyst prompts around missing and unsupported evidence"`

## 4. Verification

- [ ] 4.1 Run `cargo fmt -- --check` and fix any formatting issues.
- [ ] 4.2 Run `cargo clippy --all-targets -- -D warnings` and resolve all warnings.
- [ ] 4.3 Run `cargo test` and confirm all tests pass, including the new unit tests from tasks 2.6 and 3.5.
- [ ] 4.4 Run `cargo build` and confirm clean compilation.
