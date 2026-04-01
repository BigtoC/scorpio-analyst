# End-to-end Analyst Hardening Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the four phase-1 analysts complete reliably under OpenRouter by adding a provider-aware text fallback, fixing empty-object tool argument handling, hardening technical OHLCV reuse, and tightening the analyst prompts.

**Architecture:** Keep the current analyst structure and add the smallest shared helper in `src/agents/analyst/common.rs` so each analyst can choose typed output for supported providers and a text-to-JSON fallback for `ProviderId::OpenRouter`. Keep tool and context fixes local to `src/data/finnhub.rs` and `src/data/yfinance.rs`, and use existing validation functions plus existing `deny_unknown_fields` state types as the schema gate.

**Tech Stack:** Rust 2024, `tokio`, `rig-core`, `serde`, `serde_json`, `schemars`, `finnhub`, `yfinance-rs`, `tracing`

---

## File Map

- `docs/superpowers/specs/2026-04-01-end-to-end-analyst-hardening-design.md` - approved source of truth for this plan
- `src/agents/analyst/common.rs` - shared analyst inference helper, fallback JSON parsing helpers, shared usage extraction integration
- `src/agents/analyst/fundamental.rs` - route fundamental analyst through the shared helper and tighten output contract wording
- `src/agents/analyst/news.rs` - route news analyst through the shared helper and state exact tool argument shapes in prompts
- `src/agents/analyst/sentiment.rs` - route sentiment analyst through the shared helper and state exact `get_news` argument shape in prompts
- `src/agents/analyst/technical.rs` - route technical analyst through the shared helper and state one-time `get_ohlcv` guidance in prompts
- `src/providers/factory/agent.rs` - expose minimal provider-aware agent plumbing for fallback selection and a `max_turns`-aware text prompt path
- `src/providers/factory/agent_test_support.rs` - test-only provider-aware mock factory and counters for text-turn, one-shot prompt, and typed-path assertions
- `src/providers/factory/text_retry.rs` - add the tool-enabled text retry helper and its focused tests without growing `retry.rs`
- `src/providers/factory/retry.rs` - expose only the shared retry internals needed by `text_retry.rs` and keep existing typed retry behavior unchanged
- `src/providers/factory/mod.rs` - export the new retry helper if the shared analyst helper needs it through the facade
- `src/data/finnhub.rs` - add strict empty-object args type and use it for `get_market_news` and `get_economic_indicators`
- `src/data/yfinance.rs` - make identical scoped `get_ohlcv` reuse cached candles while preserving scope validation failures
- `src/state/fundamental.rs` - verify no production change needed; already has `deny_unknown_fields`
- `src/state/news.rs` - verify no production change needed; already has `deny_unknown_fields`
- `src/state/sentiment.rs` - verify no production change needed; already has `deny_unknown_fields`
- `src/state/technical.rs` - verify no production change needed; already has `deny_unknown_fields`

## Constraints

- Preserve typed prompt behavior for non-OpenRouter providers.
- Use an explicit `ProviderId::OpenRouter` check; do not generalize capability detection in this pass.
- Follow TDD strictly: failing test, verify failure, minimal implementation, verify pass.
- Keep schema failures terminal for the fallback path; do not add retries for local JSON parse or semantic validation failures.
- Keep `get_ohlcv` scope validation strict; only identical scoped repeats become idempotent.
- Prefer additive shared helper changes in `common.rs` over new abstraction layers.
- Keep `src/providers/factory/agent.rs` production edits minimal: only `provider_id()` and `prompt_text_details(prompt, max_turns)`. Put new mock-only seams in `src/providers/factory/agent_test_support.rs` behind `#[cfg(test)]`.

## Chunk 1: Provider-Aware Analyst Inference

### Task 1: Add a tool-enabled text retry helper for analyst fallback

**Files:**
- Modify: `src/providers/factory/agent.rs`
- Create: `src/providers/factory/agent_test_support.rs`
- Create: `src/providers/factory/text_retry.rs`
- Modify: `src/providers/factory/retry.rs`
- Modify: `src/providers/factory/mod.rs`
- Test: `src/providers/factory/text_retry.rs`

- [ ] **Step 1: Write the failing retry-helper tests**

Add tests with these exact names to `src/providers/factory/text_retry.rs`:

```rust
#[tokio::test]
async fn prompt_text_with_retry_returns_usage_from_prompt_details() {}

#[tokio::test]
async fn prompt_text_with_retry_retries_transient_prompt_errors() {}

#[tokio::test]
async fn prompt_text_with_retry_times_out_with_text_prompt_operation_name() {}

#[tokio::test]
async fn prompt_text_with_retry_preserves_max_turns_for_tool_enabled_requests() {}

#[tokio::test]
async fn prompt_text_with_retry_uses_text_turn_agent_path_not_one_shot_prompt_details() {}
```

- [ ] **Step 2: Run the new retry-helper tests and verify they fail**

Run:

```bash
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_returns_usage_from_prompt_details -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_retries_transient_prompt_errors -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_times_out_with_text_prompt_operation_name -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_preserves_max_turns_for_tool_enabled_requests -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_uses_text_turn_agent_path_not_one_shot_prompt_details -- --nocapture
```

Expected: FAIL because `prompt_text_with_retry` does not exist yet.

- [ ] **Step 3: Implement the smallest text retry helper**

Add a helper in `src/providers/factory/text_retry.rs` with this shape:

```rust
pub async fn prompt_text_with_retry(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
    max_turns: usize,
) -> Result<RetryOutcome<PromptResponse>, TradingError>
```

Implementation notes:

- follow the structure of `prompt_typed_with_retry(...)` so `max_turns` is preserved for the fallback path
- keep the response type as `PromptResponse` so callers have both `output` text and `usage`
- call the same tool-enabled underlying agent path used by the current typed analyst flow, but return final text plus usage details
- add the minimal `LlmAgent` API needed in `src/providers/factory/agent.rs`:
  - `prompt_text_details(prompt, max_turns)` for the production path
- put all new mock-only seams in `src/providers/factory/agent_test_support.rs`:
  - `mock_llm_agent_with_provider(...)`
  - `prompt_attempts()`
  - `text_turn_attempts()`
  - reuse or re-export `typed_attempts()` there for tests
- keep the new production logic and tests in `text_retry.rs`; only lift the smallest shared retry internals out of `retry.rs` via `pub(super)` helpers if `text_retry.rs` needs them
- only add the public wrapper and any minimal export needed in `src/providers/factory/mod.rs`

- [ ] **Step 4: Re-run the retry-helper tests and existing retry regression tests**

Run:

```bash
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_returns_usage_from_prompt_details -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_retries_transient_prompt_errors -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_times_out_with_text_prompt_operation_name -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_preserves_max_turns_for_tool_enabled_requests -- --nocapture
cargo test providers::factory::text_retry::tests::prompt_text_with_retry_uses_text_turn_agent_path_not_one_shot_prompt_details -- --nocapture
cargo test providers::factory::retry::tests::prompt_typed_with_retry_public_entrypoint_does_not_retry_schema_violations -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit the retry-helper foundation**

```bash
git add src/providers/factory/agent.rs src/providers/factory/agent_test_support.rs src/providers/factory/text_retry.rs src/providers/factory/retry.rs src/providers/factory/mod.rs
git commit -m "feat(factory): add text retry helper for analyst fallback"
```

### Task 2: Add a shared analyst inference helper with OpenRouter fallback

**Files:**
- Modify: `src/providers/factory/agent.rs`
- Modify: `src/providers/factory/agent_test_support.rs`
- Modify: `src/agents/analyst/common.rs`
- Test: `src/agents/analyst/common.rs`

- [ ] **Step 1: Write the failing shared-helper tests**

Add tests with these exact names to `src/agents/analyst/common.rs`:

```rust
#[tokio::test]
async fn run_analyst_inference_uses_typed_path_for_non_openrouter() {}

#[tokio::test]
async fn run_analyst_inference_uses_text_fallback_for_openrouter() {}

#[tokio::test]
async fn run_analyst_inference_returns_schema_violation_for_invalid_fallback_json() {}

#[tokio::test]
async fn run_analyst_inference_preserves_usage_from_fallback_response() {}

#[tokio::test]
async fn run_analyst_inference_returns_terminal_schema_violation_for_semantically_invalid_fallback_output() {}
```

- [ ] **Step 2: Run the shared-helper tests and verify they fail**

Run:

```bash
cargo test agents::analyst::common::tests::run_analyst_inference_uses_typed_path_for_non_openrouter -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_uses_text_fallback_for_openrouter -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_returns_schema_violation_for_invalid_fallback_json -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_preserves_usage_from_fallback_response -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_returns_terminal_schema_violation_for_semantically_invalid_fallback_output -- --nocapture
```

Expected: FAIL because the shared helper does not exist yet.

- [ ] **Step 3: Implement the shared helper in `common.rs` with this exact contract**

Add a focused helper in `src/agents/analyst/common.rs` with this exact shape:

```rust
pub(super) struct AnalystInferenceOutcome<T> {
    pub output: T,
    pub usage: rig::completion::Usage,
    pub rate_limit_wait_ms: u64,
}

pub(super) async fn run_analyst_inference<T, Parse, Validate>(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    retry_policy: &RetryPolicy,
    max_turns: usize,
    parse: Parse,
    validate: Validate,
) -> Result<AnalystInferenceOutcome<T>, TradingError>
where
    Parse: Fn(&str) -> Result<T, TradingError>,
    Validate: Fn(&T) -> Result<(), TradingError>,
```

Implementation notes:

- add a minimal `LlmAgent::provider_id() -> ProviderId` accessor in `src/providers/factory/agent.rs`
- keep `src/providers/factory/agent.rs` production-only in this task except for the accessor and `prompt_text_details(...)`
- add one explicit test seam in `src/providers/factory/agent_test_support.rs` and use it consistently everywhere in this plan:
  - `mock_llm_agent_with_provider(provider_id, model_id, prompt_results, chat_results)`
  - `prompt_attempts()` on the mock-backed `LlmAgent`
  - reuse or re-export `typed_attempts()` for typed-route assertions
  - add mock-only `text_turn_attempts()` for the new `prompt_text_details(..., max_turns)` path
- branch on `agent.provider_id() == ProviderId::OpenRouter`
- OpenRouter path:
  - call `prompt_text_with_retry(..., max_turns)`
  - parse `response.output` with the supplied parse hook
  - run the validate hook
  - if parsing or validation fails, return `TradingError::SchemaViolation` immediately without retry
  - return `AnalystInferenceOutcome { output, usage, rate_limit_wait_ms }`
- non-OpenRouter path:
  - keep `prompt_typed_with_retry::<T>(..., max_turns)`
  - run the validate hook on `response.output`
  - if validation fails, return `TradingError::SchemaViolation` immediately without retry
  - return `AnalystInferenceOutcome { output, usage, rate_limit_wait_ms }`
- use the existing state structs' `deny_unknown_fields`; do not change the state files unless tests prove a gap
- make invalid fallback JSON and semantically invalid fallback output terminal by returning `TradingError::SchemaViolation` directly

- [ ] **Step 4: Re-run the shared-helper tests and existing `common.rs` tests**

Run:

```bash
cargo test agents::analyst::common::tests::run_analyst_inference_uses_typed_path_for_non_openrouter -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_uses_text_fallback_for_openrouter -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_returns_schema_violation_for_invalid_fallback_json -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_preserves_usage_from_fallback_response -- --nocapture
cargo test agents::analyst::common::tests::run_analyst_inference_returns_terminal_schema_violation_for_semantically_invalid_fallback_output -- --nocapture
cargo test agents::analyst::common::tests::usage_from_response_marks_available_when_total_nonzero -- --nocapture
cargo test providers::factory::agent::tests::build_agent_creates_openrouter_agent -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit the shared analyst helper**

```bash
git add src/providers/factory/agent.rs src/providers/factory/agent_test_support.rs src/agents/analyst/common.rs
git commit -m "feat(analyst): add provider-aware analyst inference helper"
```

### Task 3: Migrate the Fundamental Analyst onto the shared helper

**Files:**
- Modify: `src/agents/analyst/fundamental.rs`
- Test: `src/agents/analyst/fundamental.rs`

- [ ] **Step 1: Write the failing Fundamental Analyst tests**

Add tests with these exact names:

```rust
#[test]
fn fundamental_prompt_requires_exactly_one_json_object_response() {}

#[test]
fn parse_fundamental_rejects_unknown_fields() {}

#[tokio::test]
async fn fundamental_run_uses_shared_inference_helper_for_openrouter() {}
```

Use the exact seam introduced in Task 2:

- construct the analyst with `mock_llm_agent_with_provider(ProviderId::OpenRouter, ...)`
- assert `typed_attempts() == 0`
- assert `text_turn_attempts() == 1`
- assert `prompt_attempts() == 0` so the test proves the fallback did not use one-shot `prompt_details(...)`

Prompt-contract assertions must target `FUNDAMENTAL_SYSTEM_PROMPT` and include these exact required substrings:

- `exactly one JSON object`
- `no prose`
- `no markdown fences`

- [ ] **Step 2: Run the new Fundamental Analyst tests and verify they fail**

Run:

```bash
cargo test agents::analyst::fundamental::tests::fundamental_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::fundamental::tests::parse_fundamental_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::fundamental::tests::fundamental_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: FAIL because the module still uses the old direct typed path and does not yet enforce the new fallback output contract.

- [ ] **Step 3: Implement the smallest Fundamental Analyst migration**

In `src/agents/analyst/fundamental.rs`:

- replace direct `prompt_typed_with_retry::<FundamentalData>(...)` usage with `run_analyst_inference(...)`
- keep `validate_fundamental(...)`
- expose or add `parse_fundamental(...)` and pass it into the shared helper
- require exactly one JSON object and no prose or markdown fences in the prompt
- keep `usage_from_response(...)` unchanged at the analyst layer

- [ ] **Step 4: Re-run the Fundamental Analyst tests**

Run:

```bash
cargo test agents::analyst::fundamental::tests::fundamental_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::fundamental::tests::parse_fundamental_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::fundamental::tests::fundamental_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: PASS.

### Task 4: Migrate the News Analyst onto the shared helper

**Files:**
- Modify: `src/agents/analyst/news.rs`
- Test: `src/agents/analyst/news.rs`

- [ ] **Step 1: Write the failing News Analyst tests**

Add tests with these exact names:

```rust
#[test]
fn news_prompt_states_exact_tool_argument_shapes() {}

#[test]
fn news_prompt_requires_exactly_one_json_object_response() {}

#[test]
fn parse_news_rejects_unknown_fields() {}

#[tokio::test]
async fn news_run_uses_shared_inference_helper_for_openrouter() {}
```

Prompt-contract assertions must target `NEWS_SYSTEM_PROMPT` and include these exact required substrings:

- `get_news requires {"symbol":"<ticker>"}`
- `get_market_news takes {}`
- `get_economic_indicators takes {}`
- `exactly one JSON object`
- `no prose`
- `no markdown fences`

- [ ] **Step 2: Run the new News Analyst tests and verify they fail**

Run:

```bash
cargo test agents::analyst::news::tests::news_prompt_states_exact_tool_argument_shapes -- --nocapture
cargo test agents::analyst::news::tests::news_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::news::tests::parse_news_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::news::tests::news_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: FAIL because the prompt and public `run()` path have not yet been hardened.

- [ ] **Step 3: Implement the smallest News Analyst migration**

In `src/agents/analyst/news.rs`:

- replace direct `prompt_typed_with_retry::<NewsData>(...)` usage with `run_analyst_inference(...)`
- keep `validate_news(...)`
- expose or add `parse_news(...)` and pass it into the shared helper
- mention `get_news` requires `{"symbol":"<ticker>"}`
- mention `get_market_news` takes `{}`
- mention `get_economic_indicators` takes `{}`
- require exactly one JSON object and no prose or markdown fences
- keep `usage_from_response(...)` unchanged at the analyst layer

- [ ] **Step 4: Re-run the News Analyst tests**

Run:

```bash
cargo test agents::analyst::news::tests::news_prompt_states_exact_tool_argument_shapes -- --nocapture
cargo test agents::analyst::news::tests::news_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::news::tests::parse_news_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::news::tests::news_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: PASS.

### Task 5: Migrate the Sentiment Analyst onto the shared helper

**Files:**
- Modify: `src/agents/analyst/sentiment.rs`
- Test: `src/agents/analyst/sentiment.rs`

- [ ] **Step 1: Write the failing Sentiment Analyst tests**

Add tests with these exact names:

```rust
#[test]
fn sentiment_prompt_states_get_news_argument_shape() {}

#[test]
fn sentiment_prompt_requires_exactly_one_json_object_response() {}

#[test]
fn parse_sentiment_rejects_unknown_fields() {}

#[tokio::test]
async fn sentiment_run_uses_shared_inference_helper_for_openrouter() {}
```

Prompt-contract assertions must target `SENTIMENT_SYSTEM_PROMPT` and include these exact required substrings:

- `get_news requires {"symbol":"<ticker>"}`
- `exactly one JSON object`
- `no prose`
- `no markdown fences`

- [ ] **Step 2: Run the new Sentiment Analyst tests and verify they fail**

Run:

```bash
cargo test agents::analyst::sentiment::tests::sentiment_prompt_states_get_news_argument_shape -- --nocapture
cargo test agents::analyst::sentiment::tests::sentiment_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::sentiment::tests::parse_sentiment_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::sentiment::tests::sentiment_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: FAIL because the prompt and public `run()` path have not yet been hardened.

- [ ] **Step 3: Implement the smallest Sentiment Analyst migration**

In `src/agents/analyst/sentiment.rs`:

- replace direct `prompt_typed_with_retry::<SentimentData>(...)` usage with `run_analyst_inference(...)`
- keep `validate_sentiment(...)`
- expose or add `parse_sentiment(...)` and pass it into the shared helper
- mention `get_news` requires `{"symbol":"<ticker>"}`
- require exactly one JSON object and no prose or markdown fences
- keep `usage_from_response(...)` unchanged at the analyst layer

- [ ] **Step 4: Re-run the Sentiment Analyst tests**

Run:

```bash
cargo test agents::analyst::sentiment::tests::sentiment_prompt_states_get_news_argument_shape -- --nocapture
cargo test agents::analyst::sentiment::tests::sentiment_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::sentiment::tests::parse_sentiment_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::sentiment::tests::sentiment_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: PASS.

### Task 6: Migrate the Technical Analyst onto the shared helper

**Files:**
- Modify: `src/agents/analyst/technical.rs`
- Test: `src/agents/analyst/technical.rs`

- [ ] **Step 1: Write the failing Technical Analyst tests**

Add tests with these exact names:

```rust
#[test]
fn technical_prompt_limits_get_ohlcv_to_one_call() {}

#[test]
fn technical_prompt_requires_exactly_one_json_object_response() {}

#[test]
fn parse_technical_rejects_unknown_fields() {}

#[tokio::test]
async fn technical_run_uses_shared_inference_helper_for_openrouter() {}
```

Prompt-contract assertions must target `TECHNICAL_SYSTEM_PROMPT` and include these exact required substrings:

- `get_ohlcv`
- `called at most once`
- `indicator tools`
- `exactly one JSON object`
- `no prose`
- `no markdown fences`

- [ ] **Step 2: Run the new Technical Analyst tests and verify they fail**

Run:

```bash
cargo test agents::analyst::technical::tests::technical_prompt_limits_get_ohlcv_to_one_call -- --nocapture
cargo test agents::analyst::technical::tests::technical_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::technical::tests::parse_technical_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::technical::tests::technical_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: FAIL because the prompt and public `run()` path have not yet been hardened.

- [ ] **Step 3: Implement the smallest Technical Analyst migration**

In `src/agents/analyst/technical.rs`:

- replace direct `prompt_typed_with_retry::<TechnicalData>(...)` usage with `run_analyst_inference(...)`
- keep `validate_technical(...)`
- expose or add `parse_technical(...)` and pass it into the shared helper
- mention `get_ohlcv` is scoped to the provided symbol/dates
- mention it must be called at most once
- mention indicator tools should be used after the first fetch
- require exactly one JSON object and no prose or markdown fences
- keep `usage_from_response(...)` unchanged at the analyst layer

- [ ] **Step 4: Re-run the Technical Analyst tests**

Run:

```bash
cargo test agents::analyst::technical::tests::technical_prompt_limits_get_ohlcv_to_one_call -- --nocapture
cargo test agents::analyst::technical::tests::technical_prompt_requires_exactly_one_json_object_response -- --nocapture
cargo test agents::analyst::technical::tests::parse_technical_rejects_unknown_fields -- --nocapture
cargo test agents::analyst::technical::tests::technical_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: PASS.

### Task 7: Run chunk-level analyst routing verification and commit

**Files:**
- Modify: `src/agents/analyst/fundamental.rs`
- Modify: `src/agents/analyst/news.rs`
- Modify: `src/agents/analyst/sentiment.rs`
- Modify: `src/agents/analyst/technical.rs`
- Test: existing analyst and workflow tests only

- [ ] **Step 1: Re-run the focused analyst regression sweep**

Run:

```bash
cargo test providers::factory::text_retry::tests:: -- --nocapture
cargo test agents::analyst::fundamental::tests::fundamental_run_uses_shared_inference_helper_for_openrouter -- --nocapture
cargo test agents::analyst::news::tests::news_run_uses_shared_inference_helper_for_openrouter -- --nocapture
cargo test agents::analyst::sentiment::tests::sentiment_run_uses_shared_inference_helper_for_openrouter -- --nocapture
cargo test agents::analyst::technical::tests::technical_run_uses_shared_inference_helper_for_openrouter -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run the exact higher-level analyst workflow verification**

Run:

```bash
cargo test agents::analyst::tests::all_four_succeed_populates_all_state_fields -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Commit the analyst migration and prompt hardening**

```bash
git add src/agents/analyst/common.rs src/agents/analyst/fundamental.rs src/agents/analyst/news.rs src/agents/analyst/sentiment.rs src/agents/analyst/technical.rs
git commit -m "fix(analyst): harden OpenRouter fallback and prompt contracts"
```

## Chunk 2: Tool-Argument And OHLCV Hardening

### Task 8: Make empty-object Finnhub tools accept `{}` and reject extra keys

**Files:**
- Modify: `src/data/finnhub.rs`
- Test: `src/data/finnhub.rs`

Containment note:

- keep this change in `src/data/finnhub.rs` because the repo already colocates Finnhub tool arg types, tool implementations, and tool tests there
- keep the new `EmptyObjectArgs` type and new regressions tightly grouped with the existing macro-tool definitions rather than expanding responsibilities elsewhere
- keep the new tests adjacent to the existing macro-tool tests near `GetMarketNews` and `GetEconomicIndicators`; do not add unrelated Finnhub refactors in this task

- [ ] **Step 1: Write the failing Finnhub tool-args tests**

Add tests with these exact names to `src/data/finnhub.rs`:

```rust
#[test]
fn empty_object_args_accepts_empty_json_object() {}

#[test]
fn empty_object_args_rejects_unexpected_properties() {}

#[tokio::test]
async fn get_market_news_accepts_empty_object_args_at_tool_boundary() {}

#[tokio::test]
async fn get_economic_indicators_accepts_empty_object_args_at_tool_boundary() {}

#[tokio::test]
async fn get_market_news_definition_advertises_empty_object_schema() {}

#[tokio::test]
async fn get_economic_indicators_definition_advertises_empty_object_schema() {}
```

Use these exact test bodies or equivalent assertions:

```rust
#[test]
fn empty_object_args_accepts_empty_json_object() {
    let parsed: EmptyObjectArgs = serde_json::from_str("{}").expect("{} should deserialize");
    assert_eq!(parsed, EmptyObjectArgs {});
}

#[test]
fn empty_object_args_rejects_unexpected_properties() {
    let err = serde_json::from_str::<EmptyObjectArgs>(r#"{"unexpected":1}"#).unwrap_err();
    assert!(err.to_string().contains("unknown field"));
}

#[tokio::test]
async fn get_market_news_accepts_empty_object_args_at_tool_boundary() {
    let tool = GetMarketNews { client: None };
    let result = tool.call(EmptyObjectArgs {}).await;
    assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
}

#[tokio::test]
async fn get_economic_indicators_accepts_empty_object_args_at_tool_boundary() {
    let tool = GetEconomicIndicators { client: None };
    let result = tool.call(EmptyObjectArgs {}).await;
    assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
}
```

Definition-test assertions must check:

- `def.name` matches the tool name
- `def.parameters["type"] == "object"`
- `def.parameters["properties"]` is empty
- `def.parameters["additionalProperties"] == false`

- [ ] **Step 2: Run the Finnhub tool-args tests and verify they fail**

Run:

```bash
cargo test data::finnhub::tests::empty_object_args_accepts_empty_json_object -- --nocapture
cargo test data::finnhub::tests::empty_object_args_rejects_unexpected_properties -- --nocapture
cargo test data::finnhub::tests::get_market_news_accepts_empty_object_args_at_tool_boundary -- --nocapture
cargo test data::finnhub::tests::get_economic_indicators_accepts_empty_object_args_at_tool_boundary -- --nocapture
cargo test data::finnhub::tests::get_market_news_definition_advertises_empty_object_schema -- --nocapture
cargo test data::finnhub::tests::get_economic_indicators_definition_advertises_empty_object_schema -- --nocapture
```

Expected: FAIL because `EmptyObjectArgs` does not exist yet.

- [ ] **Step 3: Implement the smallest strict empty-object args type**

In `src/data/finnhub.rs`:

- add `EmptyObjectArgs` near the other tool arg types
- derive exactly `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`, and `Eq`
- add `#[serde(deny_unknown_fields)]`
- switch `GetMarketNews::Args` and `GetEconomicIndicators::Args` from `()` to `EmptyObjectArgs`
- keep the published tool schema as the existing empty object with `additionalProperties: false`

- [ ] **Step 4: Re-run the Finnhub tests and existing tool tests**

Run:

```bash
cargo test data::finnhub::tests::empty_object_args_accepts_empty_json_object -- --nocapture
cargo test data::finnhub::tests::empty_object_args_rejects_unexpected_properties -- --nocapture
cargo test data::finnhub::tests::get_market_news_accepts_empty_object_args_at_tool_boundary -- --nocapture
cargo test data::finnhub::tests::get_economic_indicators_accepts_empty_object_args_at_tool_boundary -- --nocapture
cargo test data::finnhub::tests::get_market_news_definition_advertises_empty_object_schema -- --nocapture
cargo test data::finnhub::tests::get_economic_indicators_definition_advertises_empty_object_schema -- --nocapture
cargo test data::finnhub::tests::tool_call_without_client_returns_config_error -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit the empty-object args fix**

```bash
git add src/data/finnhub.rs
git commit -m "fix(finnhub): accept strict empty-object macro tool args"
```

### Task 9: Make scoped duplicate `get_ohlcv` calls idempotent but keep scope failures strict

**Files:**
- Modify: `src/data/yfinance.rs`
- Test: `src/data/yfinance.rs`

- [ ] **Step 1: Write the failing OHLCV reuse tests**

Add tests with these exact names to `src/data/yfinance.rs`:

```rust
#[tokio::test]
async fn get_ohlcv_returns_cached_candles_when_context_is_already_populated() {}

#[tokio::test]
async fn get_ohlcv_still_rejects_mismatched_scoped_args_after_context_is_populated() {}
```

Use these exact setup rules:

- pre-populate `OhlcvToolContext` with one candle via `ctx.store(vec![sample_candle()]).await`
- for the idempotent case, construct `GetOhlcv` with:
  - `allowed_symbol: Some("AAPL".to_owned())`
  - `allowed_start: Some("2024-01-01".to_owned())`
  - `allowed_end: Some("2024-01-31".to_owned())`
  - `context: Some(ctx.clone())`
  - `client: None`
- call `.call(OhlcvArgs { symbol: "AAPL".to_owned(), start: "2024-01-01".to_owned(), end: "2024-01-31".to_owned() })`
- assert it returns `Ok(candles)` equal to the pre-populated cached candle vector and does not require a client
- for the mismatched case, keep the same populated context but call with one mismatched field, such as `end: "2024-02-01".to_owned()`
- assert it returns `TradingError::SchemaViolation` mentioning the scoped end-date mismatch

- [ ] **Step 2: Run the new OHLCV tests and verify they fail**

Run:

```bash
cargo test data::yfinance::tests::get_ohlcv_returns_cached_candles_when_context_is_already_populated -- --nocapture
cargo test data::yfinance::tests::get_ohlcv_still_rejects_mismatched_scoped_args_after_context_is_populated -- --nocapture
```

Expected: FAIL because duplicate context reuse is not implemented yet.

- [ ] **Step 3: Implement the smallest idempotent reuse change**

In `src/data/yfinance.rs`:

- add a context read path that can detect existing candles before a second store
- keep `GetOhlcv::validate_scope(&args)` first
- if the scoped args are valid and cached candles already exist, return them directly
- otherwise perform the normal fetch and first store path
- do not relax `OhlcvToolContext::store(...)`; keep write-once semantics for direct store callers unless tests require a different boundary

- [ ] **Step 4: Re-run the OHLCV tests and nearby existing tests**

Run:

```bash
cargo test data::yfinance::tests::get_ohlcv_returns_cached_candles_when_context_is_already_populated -- --nocapture
cargo test data::yfinance::tests::get_ohlcv_still_rejects_mismatched_scoped_args_after_context_is_populated -- --nocapture
cargo test data::yfinance::tests::ohlcv_context_store_write_once_rejects_second_write -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit the OHLCV reuse hardening**

```bash
git add src/data/yfinance.rs
git commit -m "fix(yfinance): reuse scoped ohlcv context for duplicate calls"
```

### Task 10: Run final targeted verification for the full analyst hardening slice

**Files:**
- Modify: none unless verification exposes gaps
- Test: existing tests only

Sequencing note:

- run this task only after Chunk 1 and Chunk 2 changes are both landed locally, because it verifies the full analyst-hardening slice end to end

- [ ] **Step 1: Run the focused unit test sweep**

Run:

```bash
cargo test agents::analyst::common::tests:: -- --nocapture
cargo test agents::analyst::fundamental::tests:: -- --nocapture
cargo test agents::analyst::news::tests:: -- --nocapture
cargo test agents::analyst::sentiment::tests:: -- --nocapture
cargo test agents::analyst::technical::tests:: -- --nocapture
cargo test data::finnhub::tests:: -- --nocapture
cargo test data::yfinance::tests:: -- --nocapture
cargo test providers::factory::text_retry::tests:: -- --nocapture
cargo test providers::factory::retry::tests:: -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run the exact higher-level analyst workflow verification**

Run:

```bash
cargo test agents::analyst::tests::all_four_succeed_populates_all_state_fields -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: If practical, run the real failing scenario equivalent**

Run only if the required OpenRouter environment is available locally:

- `SCORPIO_OPENROUTER_API_KEY` is set
- the active config selects `quick_thinking_provider = "openrouter"`
- the active config selects an OpenRouter quick-thinking model such as the previously used free-tier model

```bash
cargo run -- analyze AAPL
```

Expected success indicators:

- the analyst fanout completes instead of aborting with all four analysts failed
- logs do not contain the previous empty-object tool-argument failures for `get_market_news` or `get_economic_indicators`
- logs do not contain the previous duplicate `get_ohlcv may only be called once per analysis cycle` failure

- [ ] **Step 4: Commit any final verification-only prompt or test adjustments**

```bash
git add src/agents/analyst/common.rs src/agents/analyst/fundamental.rs src/agents/analyst/news.rs src/agents/analyst/sentiment.rs src/agents/analyst/technical.rs src/data/finnhub.rs src/data/yfinance.rs src/providers/factory/agent.rs src/providers/factory/agent_test_support.rs src/providers/factory/text_retry.rs src/providers/factory/retry.rs src/providers/factory/mod.rs
git commit -m "test: verify analyst hardening regressions"
```

Only do this commit if verification required code changes after Task 9.
