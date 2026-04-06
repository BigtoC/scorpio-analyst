## 1. Documentation Updates

- [x] 1.1 Add a credit sentence to `README.md` in the project introduction section acknowledging Anthropic's financial-services-plugins architecture as an inspiration for the evidence discipline and provenance-reporting patterns in this project.
- [x] 1.2 Update the `## Global Prompt Rules` section in `docs/prompts.md` so it explicitly covers these five evidence-discipline directives:
  - authoritative runtime evidence over inference or memory
  - schema-compatible missing-data handling without guessing or padding
  - observed facts versus interpretation
  - explicit confidence reduction when evidence is sparse or missing
  - Rust-owned deterministic comparisons with model-owned interpretation

  While editing, do **not** duplicate the existing missing-data or facts-vs-interpretation bullets verbatim; tighten or extend the existing wording instead.
- [x] 1.3 Verify the additions with: `rg -n "financial-services-plugins|authoritative runtime evidence|lower confidence|Let Rust compute" README.md docs/prompts.md`
- [x] 1.4 Commit: `git add README.md docs/prompts.md && git commit -m "docs: credit financial-services-plugins inspiration and harden prompt rules"`

## 2. Shared Static Prompt Helpers

- [x] 2.1 Add `build_authoritative_source_prompt_rule() -> &'static str` to `src/agents/shared/prompt.rs`. Return a terse imperative rule: prefer authoritative runtime evidence and never infer estimates, transcript commentary, or quarter labels unless the runtime provides them.
- [x] 2.2 Add `build_missing_data_prompt_rule() -> &'static str` to `src/agents/shared/prompt.rs`. Return a terse imperative rule: when evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims; return `null` or `[]` for missing structured fields.
- [x] 2.3 Add `build_data_quality_prompt_rule() -> &'static str` to `src/agents/shared/prompt.rs`. Return a terse imperative rule: separate observed facts (tool output) from interpretation (your reasoning); do not present interpretation as established fact.
- [x] 2.4 Do **not** add `build_evidence_context(...)` or `build_data_quality_context(...)` in this chunk. Those state-dependent helpers are deferred to `chunk3-evidence-state-sync`.
- [x] 2.5 Add unit tests in `src/agents/shared/prompt.rs` under `#[cfg(test)]`:
  - `test_authoritative_source_rule_mentions_runtime_evidence`
  - `test_missing_data_rule_mentions_null_or_empty`
  - `test_data_quality_rule_mentions_facts_and_interpretation`

  Each test should assert a small, stable phrase rather than only checking for non-empty output.
- [x] 2.6 Run `cargo test --lib agents::shared::prompt -- --nocapture` and confirm the helper tests pass.
- [x] 2.7 Commit: `git add src/agents/shared/prompt.rs && git commit -m "feat: add shared static prompt helpers for evidence discipline"`

## 3. Analyst Prompt Hardening

- [x] 3.1 In `src/agents/analyst/fundamental.rs`, import `build_authoritative_source_prompt_rule`, `build_missing_data_prompt_rule`, and `build_data_quality_prompt_rule` from `crate::agents::shared::prompt`. Extend the runtime-rendered system prompt by appending the text of all three rule helpers, plus explicit lines:
  - "Do not infer estimates, transcript commentary, or quarter labels unless the runtime provides them."
  - "If evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims."
  - "Separate observed facts from interpretation."

  Use runtime string composition that matches the current code shape (for example, a small `build_*_system_prompt(symbol, target_date) -> String` helper reused by `run()` and tests). Do **not** rely on `concat!` with function calls.
- [x] 3.2 In `src/agents/analyst/news.rs`, apply the same import and rendered-prompt extension as 3.1.
- [x] 3.3 In `src/agents/analyst/sentiment.rs`, apply the same import and rendered-prompt extension as 3.1.
- [x] 3.4 In `src/agents/analyst/technical.rs`, apply the same import and rendered-prompt extension as 3.1.
- [x] 3.5 Add string-contains unit tests in each of the four analyst files under `#[cfg(test)]` that assert against the rendered final system prompt, not only the raw `const` template:
  - Assert that the rendered prompt contains the text returned by `build_authoritative_source_prompt_rule()`.
  - Assert that the rendered prompt contains the text returned by `build_missing_data_prompt_rule()`.
  - Assert that the rendered prompt contains the text returned by `build_data_quality_prompt_rule()`.
  - Assert that the rendered prompt contains `"Do not infer estimates"`.
  - Assert that the rendered prompt contains `"sparse or missing"`.
  - Assert that the rendered prompt contains `"Separate observed facts"`.
- [x] 3.6 Run `cargo test --lib agents::analyst -- --nocapture` and confirm the prompt-hardening tests pass.
- [x] 3.7 Commit: `git add src/agents/analyst/fundamental.rs src/agents/analyst/news.rs src/agents/analyst/sentiment.rs src/agents/analyst/technical.rs && git commit -m "feat: harden analyst prompts around missing and unsupported evidence"`

## 4. Verification

- [x] 4.1 Run `cargo fmt -- --check` and fix any formatting issues.
- [x] 4.2 Run `cargo clippy --all-targets -- -D warnings` and resolve all warnings.
- [x] 4.3 Run `cargo nextest run --all-features --locked` and confirm all tests pass, including the new helper tests and rendered-prompt tests.
- [x] 4.4 Run `cargo build` and confirm clean compilation after the required CI checks pass.
- [x] 4.5 Run `openspec validate chunk1-docs-prompt-rules --strict` and confirm the change remains valid.
