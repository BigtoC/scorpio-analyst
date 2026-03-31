# `llm-providers` Capability

## MODIFIED Requirements

### Requirement: Provider Module Boundary

The provider factory and helpers MUST be implemented within `src/providers/mod.rs` and
`src/providers/factory.rs`. The module MUST re-export all public types needed by downstream agent
code from `src/providers/mod.rs`. The module MUST NOT modify foundation-owned files (`src/config.rs`,
`src/error.rs`, `src/state/*`).
This capability MUST remain limited to native `rig-core` providers for OpenAI, Anthropic, Gemini, and OpenRouter. It MUST NOT add
ACP transport, subprocess spawning, or GitHub Copilot-specific transport logic, which belong exclusively to
`add-copilot-provider`.

#### Scenario: Downstream Agent Import Path

When an agent change imports the provider factory, it uses `use scorpio_analyst::providers::{...}`
and receives `ModelTier`, the factory function, the agent builder helper, and the retry-wrapped
completion helper through a single module path.

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

#### Scenario: Switching To OpenRouter Provider

When the selected tier provider is set to `"openrouter"` in configuration, the same factory call returns an
OpenRouter-backed completion model using `openrouter_api_key`, with no code changes required in downstream agents.

#### Scenario: Unsupported Provider Fails Fast

When the selected provider string does not match a supported backend, the provider factory rejects configuration before
live completion execution begins and returns a typed configuration error instead of retrying a request.

### Requirement: Rig-Core Integration

The system MUST depend on `rig-core` with support for at least OpenAI, Anthropic, Gemini, and OpenRouter
provider features. Client initialization MUST use the `rig` crate's builder APIs and expose
completion models conforming to `rig`'s `CompletionModel` trait.
The provider layer SHOULD reuse initialized clients and model handles across repeated requests rather than rebuilding
them for every completion call.

#### Scenario: Initializing Multiple Providers

When the application starts, the provider factory is capable of constructing completion models for
any of the four supported HTTP backends (OpenAI, Anthropic, Gemini, OpenRouter) depending on the active
tier-level provider configuration values.
