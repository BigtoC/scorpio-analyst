# `openrouter-provider` Capability

## ADDED Requirements

### Requirement: OpenRouter Provider Factory Registration

The provider factory MUST accept `"openrouter"` as a valid provider name in `LlmConfig.quick_thinking_provider` or `LlmConfig.deep_thinking_provider`. When `"openrouter"` is selected for a tier, the factory MUST construct and return an `openrouter::Client`-backed completion model using rig-core 0.32's built-in `rig::providers::openrouter` module. The factory MUST require a valid `SCORPIO_OPENROUTER_API_KEY` and return `TradingError::Config` if the key is missing.

#### Scenario: Selecting OpenRouter As Quick-Thinking Provider

- **WHEN** `LlmConfig.quick_thinking_provider` is set to `"openrouter"` and a valid `openrouter_api_key` is present
- **THEN** the factory constructs an OpenRouter-backed completion model for the quick-thinking tier, and downstream agents execute completions through the OpenRouter API without code changes

#### Scenario: Missing OpenRouter API Key Fails Fast

- **WHEN** `LlmConfig.quick_thinking_provider` is set to `"openrouter"` and `SCORPIO_OPENROUTER_API_KEY` is not set
- **THEN** the factory returns `TradingError::Config` indicating the absent credential with a hint to set `SCORPIO_OPENROUTER_API_KEY`

#### Scenario: OpenRouter With Deep-Thinking Tier

- **WHEN** `LlmConfig.deep_thinking_provider` is set to `"openrouter"` and a valid `openrouter_api_key` is present
- **THEN** the factory constructs an OpenRouter-backed completion model for the deep-thinking tier

### Requirement: OpenRouter Enum Dispatch

The system MUST add an `OpenRouter` variant to `ProviderId`, `ProviderClient`, and `LlmAgentInner` enums. All existing match sites (prompt, prompt_details, prompt_typed_details, chat, chat_details, build_agent_inner) MUST include the `OpenRouter` arm. The OpenRouter variant MUST follow the same dispatch pattern as OpenAI and Gemini (no provider-specific overrides like Anthropic's `max_tokens`).

#### Scenario: OpenRouter Agent Executes Prompt

- **WHEN** an agent backed by the OpenRouter provider calls `prompt("analyze this stock")`
- **THEN** the `LlmAgent::prompt` match dispatches to the `LlmAgentInner::OpenRouter` arm and executes the completion through rig's OpenRouter client

#### Scenario: OpenRouter Agent Executes Chat With History

- **WHEN** a debate-oriented agent backed by OpenRouter calls `chat_details` with prior message history
- **THEN** the provider dispatches through the OpenRouter arm and returns a `PromptResponse` with usage details, identical to how other providers handle chat history

#### Scenario: OpenRouter Typed Prompt With Schema Enforcement

- **WHEN** an agent backed by OpenRouter calls `prompt_typed_details::<TradeProposal>`
- **THEN** the provider dispatches through the OpenRouter arm, and schema validation failures are mapped to `TradingError::SchemaViolation` by the existing error mapping logic

### Requirement: OpenRouter Configuration Validation

The config deserialization layer MUST accept `"openrouter"` (case-insensitive, whitespace-trimmed) as a valid provider name. The `validate_provider_id` function MUST map `"openrouter"` to `ProviderId::OpenRouter`. Unknown provider names MUST continue to be rejected with an error listing all supported providers including `"openrouter"`.

#### Scenario: Config Accepts OpenRouter Provider Name

- **WHEN** `config.toml` contains `quick_thinking_provider = "openrouter"`
- **THEN** the config deserializes successfully with the normalized value `"openrouter"`

#### Scenario: Case-Insensitive Provider Name

- **WHEN** `config.toml` contains `quick_thinking_provider = "  OpenRouter  "`
- **THEN** the config deserializes successfully with the normalized value `"openrouter"`

#### Scenario: Unknown Provider Error Lists OpenRouter

- **WHEN** `config.toml` contains `quick_thinking_provider = "invalid"`
- **THEN** the deserialization error message lists `openrouter` among the supported providers

### Requirement: OpenRouter API Key Management

The `ApiConfig` struct MUST include an `openrouter_api_key: Option<SecretString>` field loaded from the `SCORPIO_OPENROUTER_API_KEY` environment variable. The key MUST be stored as `secrecy::SecretString`, excluded from `Debug` output (displayed as `[REDACTED]` when present or `<not set>` when absent), and never logged. The `Config::validate()` method MUST include the OpenRouter key in the `has_key` check that warns when no LLM provider key is configured.

#### Scenario: OpenRouter Key Loaded From Environment

- **WHEN** `SCORPIO_OPENROUTER_API_KEY` is set in the environment
- **THEN** `ApiConfig.openrouter_api_key` contains a `Some(SecretString)` wrapping the value

#### Scenario: OpenRouter Key Redacted In Debug Output

- **WHEN** an `ApiConfig` with a present `openrouter_api_key` is formatted via `Debug`
- **THEN** the output shows `openrouter_api_key: [REDACTED]` and does not contain the actual key value

#### Scenario: No Warning When Only OpenRouter Key Present

- **WHEN** only `SCORPIO_OPENROUTER_API_KEY` is set (no OpenAI, Anthropic, or Gemini keys)
- **THEN** the `Config::validate()` method does not emit the "no LLM provider API key found" warning

### Requirement: OpenRouter Rate Limiting

The `RateLimitConfig` MUST include an `openrouter_rpm: u32` field with a default of 20 (matching OpenRouter's free-tier rate limit). The `ProviderRateLimiters::from_config()` MUST register a limiter for `ProviderId::OpenRouter` when `openrouter_rpm > 0`. Setting `openrouter_rpm = 0` MUST disable rate limiting for the OpenRouter provider.

#### Scenario: Default Rate Limit Is 20 RPM

- **WHEN** no `openrouter_rpm` is specified in config
- **THEN** the rate limiter for OpenRouter permits 20 requests per minute

#### Scenario: Rate Limiting Disabled When Zero

- **WHEN** `openrouter_rpm = 0` is set in config
- **THEN** no rate limiter is registered for `ProviderId::OpenRouter`

#### Scenario: Custom Rate Limit Via Config Override

- **WHEN** `openrouter_rpm = 100` is set in config (paid account)
- **THEN** the rate limiter for OpenRouter permits 100 requests per minute

### Requirement: OpenRouter Token Usage Reporting

The OpenRouter provider MUST report completion metadata to the `TokenUsageTracker` via the same interface used by other providers. Because rig-core's OpenRouter module exposes a `Usage` struct with token count fields, the OpenRouter provider MUST report authoritative provider-reported token counts (prompt, completion, total) when the API response includes them. The `token_counts_available` flag MUST be set to `true` when at least one token count is non-zero, following the same logic used for OpenAI, Anthropic, and Gemini.

#### Scenario: Token Usage Recorded With Counts

- **WHEN** a completion call through the OpenRouter provider succeeds and the API returns token counts
- **THEN** the `AgentTokenUsage` record contains valid `prompt_tokens`, `completion_tokens`, `total_tokens`, and `latency_ms` values with `token_counts_available` set to `true`

#### Scenario: Token Usage With Zero Counts

- **WHEN** a completion call through OpenRouter succeeds but the API returns zero token counts
- **THEN** the `AgentTokenUsage` record has `token_counts_available` set to `false`, consistent with how other providers handle zero-count responses
