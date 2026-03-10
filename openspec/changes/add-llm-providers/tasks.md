# Tasks for `add-llm-providers`

## 1. Dependency setup

- [x] 1.1 Add `rig-core` with OpenAI, Anthropic, and Gemini provider features to `Cargo.toml`.
- [x] 1.2 Verify the project compiles cleanly with the new dependencies (`cargo build`).

## 2. Model tier definition

- [x] 2.1 Define the `ModelTier` enum (`QuickThinking`, `DeepThinking`) in `src/providers/mod.rs` with
      `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq` derives.
- [x] 2.2 Implement a method on `ModelTier` that resolves the model ID string from `LlmConfig`.
- [x] 2.3 Validate tier routing so analyst-facing code resolves `QuickThinking` and researcher/trader/risk/manager
      code resolves `DeepThinking`.

## 3. Provider factory

- [x] 3.1 Implement `create_completion_model(tier, llm_config, api_config)` in `src/providers/factory.rs` that
      constructs a `rig` completion model based on the tier's provider (`quick_thinking_provider` /
      `deep_thinking_provider`) and model ID.
- [x] 3.2 Support `"openai"`, `"anthropic"`, and `"gemini"` provider backends, returning
      `TradingError::Config` for unknown providers or missing API keys.
- [x] 3.3 Re-export the factory function and `ModelTier` from `src/providers/mod.rs`.
- [x] 3.4 Normalize and validate provider/model selection before first live request execution.
- [x] 3.5 Reuse initialized provider clients and completion-model handles across repeated requests rather than
      reconstructing them per call.

## 4. Agent builder helper

- [x] 4.1 Implement `build_agent()` helper in `src/providers/factory.rs` that wraps `rig::AgentBuilder`
      with system prompt, tool attachment, and optional structured JSON output extraction.
- [x] 4.2 Ensure the helper supports both one-shot `prompt()` usage and history-aware `chat()` usage for downstream
      debate-style agents.
- [x] 4.3 Standardize typed tool registration through `rig` tool schemas so downstream agents can bind tools without
      custom parsing glue.
- [x] 4.4 Reject free-text or runtime-supplied raw tool manifests in favor of code-defined typed tool schemas only.

## 5. Retry-wrapped completion

- [x] 5.1 Implement `prompt_with_retry()` in `src/providers/factory.rs` that wraps a rig completion
      call with `RetryPolicy` exponential backoff and `tokio::time::timeout`.
- [x] 5.2 Map transient errors (rate limit, timeout) to retries and permanent errors to immediate
      `TradingError::Rig` failures.
- [x] 5.3 Implement `chat_with_retry()` with the same timeout, retry, and error-mapping behavior for agents that
      operate on prior `rig::message::Message` history.
- [x] 5.4 Enforce a total wall-clock request budget across retries so per-attempt timeouts cannot grow unbounded.
- [x] 5.5 Avoid retrying authentication failures, permission failures, unsupported provider/model selections, and schema
      violations.
- [x] 5.6 Avoid retrying requests after tool execution begins unless all tools in that request path are explicitly
      documented as read-only and idempotent.

## 6. Error mapping

- [x] 6.1 Ensure all `rig` errors are caught and converted to `TradingError::Rig` with provider name,
      model ID, and original error message context.
- [x] 6.2 Ensure malformed structured output or schema extraction failures are converted to
      `TradingError::SchemaViolation` rather than being collapsed into generic provider errors.
- [x] 6.3 Sanitize all surfaced provider and schema-failure context so secrets, auth headers, raw prompts, chat
      history, and raw model outputs do not escape the provider layer.

## 7. Testing

- [x] 7.1 Add unit tests for `ModelTier::model_id()` resolution from `LlmConfig`.
- [x] 7.2 Add table-driven unit tests for factory success paths across OpenAI, Anthropic, and Gemini for both tier
      routes.
- [x] 7.3 Add unit tests for factory error paths (unknown provider, missing OpenAI API key, missing Anthropic API key,
      missing Gemini API key, empty model ID, unsupported provider/model combination).
- [x] 7.4 Add tests covering `build_agent()` helper behavior, typed tool registration, and structured-output success
      extraction using deterministic mocked completions.
- [x] 7.5 Add tests proving `prompt_with_retry()` and `chat_with_retry()` share equivalent retry, timeout, and
      permanent-error handling semantics.
- [x] 7.6 Add an integration test verifying the retry helper respects backoff delays, total request budget, and timeout
      using a mock/stub completion model.
- [x] 7.7 Add tests covering chat-history retries, structured-output schema violation mapping, and non-retry of auth or
      schema failures.
- [x] 7.8 Add tests proving raw `rig` errors do not escape the provider boundary and that surfaced error context is
      sanitized.
- [x] 7.9 Run `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt -- --check` with no failures.
