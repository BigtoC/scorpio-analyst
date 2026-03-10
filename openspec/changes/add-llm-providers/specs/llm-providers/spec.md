# `llm-providers` Capability

## ADDED Requirements

### Requirement: Dual-Tier Model Routing

The system **MUST** define a `ModelTier` enum with variants `QuickThinking` and `DeepThinking` to encode the PRD's
dual-tier cognitive routing strategy. Agents MUST resolve their model selection through this enum rather than hardcoding
model identifiers.

- `QuickThinking` MUST be the tier used by the full Analyst Team.
- `DeepThinking` MUST be the tier used by the Researcher Team, Trader, Risk Team, and Fund Manager.

#### Scenario: Analyst Selects Quick-Thinking Model

When an analyst agent requests a completion model for the `QuickThinking` tier, the provider layer
returns a model configured with the `quick_thinking_model` identifier from `LlmConfig` (e.g.,
`gemini-2.5-fast`).

#### Scenario: Deep-Thinking Researcher Resolution

When a researcher agent requests a model for the `DeepThinking` tier, the provider layer returns a
model configured with the `deep_thinking_model` identifier from `LlmConfig` (e.g., `gpt-5.4`).

#### Scenario: Trader And Risk Agents Resolve Deep-Thinking Tier

When the Trader, any Risk agent, or the Fund Manager requests a completion model, the provider layer
resolves the `DeepThinking` tier rather than using analyst-tier routing.

### Requirement: Provider Factory Construction

The system MUST expose a provider factory function that accepts a `ModelTier` and configuration references (`LLMConfig`
and `ApiConfig`) and returns a reusable completion-model handle ready for prompt execution.
The factory MUST resolve the backend provider from `LLMConfig.quick_thinking_provider` or
`LLMConfig.deep_thinking_provider` according to the requested `ModelTier`, then inject the corresponding API key from
`ApiConfig`.
The provider layer MUST validate provider names and model identifiers before first request execution and fail fast with
a configuration error on unsupported or missing values.

#### Scenario: Building An OpenAI Completion Model

When the selected tier provider is `"openai"` and a valid `openai_api_key` is present, the factory constructs an
OpenAI-backed completion model for the requested tier. If the API key is missing, the factory returns a
`TradingError::Config` indicating the absent credential.

#### Scenario: Switching To Anthropic Provider

When the selected tier provider is set to `"anthropic"` in configuration, the same factory call returns an
Anthropic-backed completion model using `anthropic_api_key`, with no code changes required in downstream agents.

#### Scenario: Switching To Gemini Provider

When the selected tier provider is set to `"gemini"` in configuration, the same factory call returns a Gemini-backed
completion model using `gemini_api_key`, with no code changes required in downstream agents.

#### Scenario: Unsupported Provider Fails Fast

When the selected provider string does not match a supported backend, the provider factory rejects configuration before
live completion execution begins and returns a typed configuration error instead of retrying a request.

### Requirement: Rig-Core Integration

The system MUST depend on `rig-core` with support for at least OpenAI, Anthropic, and Gemini
provider features. Client initialization MUST use the `rig` crate's builder APIs and expose
completion models conforming to `rig`'s `CompletionModel` trait.
The provider layer SHOULD reuse initialized clients and model handles across repeated requests rather than rebuilding
them for every completion call.

#### Scenario: Initializing Multiple Providers

When the application starts, the provider factory is capable of constructing completion models for
any of the three supported backends (OpenAI, Anthropic, Gemini) depending on the active
tier-level provider configuration values.

### Requirement: Agent Builder Helper

The system MUST provide a reusable agent builder helper that wraps `rig::AgentBuilder` to configure
a system prompt, attach tool definitions, and optionally enforce structured JSON output extraction.
Downstream agent specs MUST use this helper rather than constructing agents from scratch.
The helper MUST accept only code-defined typed tool objects or provider-owned typed tool helpers, not free-text tool
manifests or runtime-supplied raw schema strings.

#### Scenario: Constructing A Tool-Equipped Agent

When a downstream agent change creates a new tool-equipped agent, it supplies a system prompt string
and a set of tool definitions to the agent builder helper, receiving a configured agent instance
without directly depending on `rig::AgentBuilder` initialization details.

#### Scenario: Free-Text Tool Manifest Is Rejected

When a caller attempts to register a tool through a raw prompt string or untyped dynamic manifest, the provider layer
rejects that registration path and requires a typed `rig` tool schema instead.

### Requirement: Prompt And Chat Invocation Support

The provider layer MUST support both one-shot prompt execution and history-aware chat execution
through shared helper functions. Both invocation styles MUST reuse the same timeout, retry, and
error-mapping policies so downstream agents do not implement divergent completion logic.
The provider layer MUST NOT persist or log raw prompts, chat history, or raw response bodies as part of these helper
functions.

#### Scenario: Debate Agent Uses Chat History

When a downstream debate-oriented agent supplies prior `rig::message::Message` history to continue
an exchange, the provider layer executes the chat request through the same retry and timeout rules
used for one-shot prompt execution.

#### Scenario: Analyst Uses Single Prompt Helper

When a downstream analyst performs a one-shot completion without prior history, the provider layer
executes the request through the prompt helper without requiring the analyst to build a chat session.

### Requirement: Typed Tool And Structured Output Enforcement

The provider layer MUST standardize typed tool registration and schema-enforced structured outputs
through `rig` APIs. Tool-enabled agents MUST bind tools through `rig`'s schema system rather than
free-text prompt conventions, and structured completion parsing MUST validate the returned payload
against the expected schema before data leaves the provider layer.

#### Scenario: Agent Binds Tools Through Rig Schema

When a downstream agent attaches a financial data tool, the provider layer registers that tool using
`rig`'s typed tool interface so the LLM receives a machine-readable tool schema instead of an
informal prompt description.

#### Scenario: Structured Completion Matches Expected Schema

When an agent expects a JSON object matching a Rust output type, the provider layer validates the
completion against the configured schema before returning the parsed value to the caller.

### Requirement: Retry-Wrapped Completions

The system MUST provide a `prompt_with_retry` helper that wraps `rig` completion calls with the
foundation's `RetryPolicy` exponential backoff and `tokio::time::timeout` using the configured
`agent_timeout_secs`. Transient pre-tool failures (rate limits, timeouts, temporary provider unavailability) MUST be
retried up to `RetryPolicy.max_retries` times. Permanent failures MUST fail immediately.
The helper MUST enforce a total wall-clock request budget across all retry attempts so retries cannot exceed the
provider layer's configured runtime budget indefinitely.
The helper MUST NOT retry authentication failures, configuration failures, permission failures, unsupported provider or
model selections, or schema violations.
The provider layer MUST NOT retry a request after tool execution has started unless every tool in that request path is
explicitly documented as read-only and idempotent.

#### Scenario: Transient Rate Limit Triggers Backoff

When a completion call returns a rate-limit error on the first attempt, the helper waits for the
`RetryPolicy` delay, retries the call, and succeeds on the second attempt without surfacing an
error to the calling agent.

#### Scenario: Timeout Exhaustion Aborts

When every retry attempt exceeds `agent_timeout_secs`, the helper returns a
`TradingError::NetworkTimeout` with the elapsed duration and a descriptive message.

#### Scenario: Authentication Failure Fails Without Retry

When the configured provider rejects a request due to invalid credentials, the provider layer fails immediately with a
typed provider error and does not consume retry attempts.

### Requirement: Error Mapping

Provider construction, transport, authentication, and other `rig-core` runtime failures MUST be
mapped into `TradingError::Rig` with sufficient context (provider name, model ID, original error
message) for debugging. Structured-output decoding failures MUST be mapped into
`TradingError::SchemaViolation`. The system MUST NOT expose raw `rig` error types to callers
outside the provider module.
Any provider or schema failure surfaced by the provider layer MUST be sanitized and bounded. Errors MUST NOT include API
keys, authorization headers, raw prompts, chat history, raw model output bodies, or unsanitized native SDK payloads.
Safe context MAY include provider name, model ID, a bounded error summary, and correlation-safe request metadata.

#### Scenario: Rig Deserialization Failure

When the LLM returns malformed JSON that `rig` cannot deserialize, the provider layer catches the
error and returns `TradingError::SchemaViolation` containing the model ID and parse context,
allowing the caller to distinguish malformed structured output from transport-level provider
failures.

#### Scenario: Provider Authentication Failure

When the configured provider rejects a request due to an invalid API key, the provider layer returns
`TradingError::Rig` containing the provider identity, model ID, and the original authentication
failure message.

### Requirement: Provider Module Boundary

The provider factory and helpers MUST be implemented within `src/providers/mod.rs` and
`src/providers/factory.rs`. The module MUST re-export all public types needed by downstream agent
code from `src/providers/mod.rs`. The module MUST NOT modify foundation-owned files (`src/config.rs`,
`src/error.rs`, `src/state/*`).
This capability MUST remain limited to native `rig-core` providers for OpenAI, Anthropic, and Gemini. It MUST NOT add
ACP transport, subprocess spawning, or GitHub Copilot-specific transport logic, which belong exclusively to
`add-copilot-provider`.

#### Scenario: Downstream Agent Import Path

When an agent change imports the provider factory, it uses `use scorpio_analyst::providers::{...}`
and receives `ModelTier`, the factory function, the agent builder helper, and the retry-wrapped
completion helper through a single module path.
