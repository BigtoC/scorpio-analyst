## ADDED Requirements

### Requirement: Per-provider RPM configuration
The system SHALL support configuring LLM API rate limits per provider (OpenAI, Anthropic, Gemini, Copilot) as requests-per-minute (RPM) values in a `[rate_limits]` section of `config.toml`. The `Config` loader SHALL treat the entire `rate_limits` section as optional and SHALL populate it from typed defaults when the section is absent. The system SHALL also support overriding these values via environment variables using the `SCORPIO__RATE_LIMITS__` prefix.

#### Scenario: All providers configured with positive RPM
- **GIVEN** `config.toml` contains `[rate_limits]` with `openai_rpm = 500`, `anthropic_rpm = 300`, `gemini_rpm = 600`, `copilot_rpm = 180`
- **WHEN** the application loads configuration
- **THEN** the system SHALL create a `SharedRateLimiter` for each provider with request spacing derived from the configured RPM

#### Scenario: Provider RPM set to zero disables limiting
- **GIVEN** `config.toml` contains `copilot_rpm = 0`
- **WHEN** the application loads configuration
- **THEN** the system SHALL NOT create a rate limiter for the Copilot provider, and LLM calls to Copilot SHALL proceed without proactive throttling

#### Scenario: Default values used when section is absent
- **GIVEN** `config.toml` does not contain a `[rate_limits]` section
- **WHEN** the application loads configuration
- **THEN** the system SHALL use default RPM values: `openai_rpm = 500`, `anthropic_rpm = 300`, `gemini_rpm = 600`, `copilot_rpm = 0`

#### Scenario: Environment variable override
- **GIVEN** `config.toml` contains `openai_rpm = 500` and environment variable `SCORPIO__RATE_LIMITS__OPENAI_RPM=1000` is set
- **WHEN** the application loads configuration
- **THEN** the system SHALL use `1000` as the OpenAI RPM value

### Requirement: Finnhub rate limit migration
The system SHALL accept the Finnhub data API rate limit in the `[rate_limits]` section as `finnhub_rps`. The previous `[api].finnhub_rate_limit` field SHALL be removed.

#### Scenario: Finnhub rate limit in new location
- **GIVEN** `config.toml` contains `[rate_limits]` with `finnhub_rps = 30`
- **WHEN** the application loads configuration
- **THEN** the system SHALL create a `SharedRateLimiter` for Finnhub with 30 requests per second

#### Scenario: Default Finnhub rate limit
- **GIVEN** `config.toml` does not specify `finnhub_rps`
- **WHEN** the application loads configuration
- **THEN** the system SHALL default to `finnhub_rps = 30`

### Requirement: Proactive throttling before LLM calls
The system SHALL acquire a rate-limit permit from the provider's `SharedRateLimiter` before each LLM API call attempt within the retry loop. The acquire step SHALL apply to all public retry helpers that issue LLM requests, including `prompt_with_retry`, `prompt_with_retry_details`, `chat_with_retry`, `chat_with_retry_details`, and `prompt_typed_with_retry`.

#### Scenario: Rate limiter acquired before each attempt
- **GIVEN** OpenAI is configured with `openai_rpm = 60` (1 request per second)
- **WHEN** two agents concurrently call `prompt_with_retry` targeting OpenAI
- **THEN** the second call SHALL wait approximately 1 second for a permit before its LLM request fires

#### Scenario: Rate limiter acquired before retry attempts
- **GIVEN** OpenAI is configured with a rate limiter
- **WHEN** an LLM call fails with a transient error and the retry loop retries
- **THEN** the retry attempt SHALL also acquire a permit before firing the next request

#### Scenario: No limiter configured for provider
- **GIVEN** Copilot is configured with `copilot_rpm = 0` (disabled)
- **WHEN** an agent calls `prompt_with_retry` targeting Copilot
- **THEN** the retry loop SHALL skip the acquire step and proceed directly to the LLM call

### Requirement: Acquire does not reduce per-attempt timeout
The rate-limit acquire step SHALL NOT reduce the per-attempt timeout available for the actual LLM call. The acquire step SHALL be bounded by the remaining total budget, not by the per-attempt timeout.

#### Scenario: LLM call gets full per-attempt timeout
- **GIVEN** per-attempt timeout is 30 seconds and the rate limiter blocks for 2 seconds
- **WHEN** the acquire completes and the LLM call begins
- **THEN** the LLM call SHALL have the full 30-second per-attempt timeout (or the remaining total budget, whichever is less)

#### Scenario: Budget exhausted during acquire
- **GIVEN** the total retry budget has 1 second remaining and the rate limiter would need to wait 5 seconds
- **WHEN** the acquire step times out against the remaining budget
- **THEN** the system SHALL return a `NetworkTimeout` error indicating the retry budget was exhausted

### Requirement: Rate-limit wait time in AgentTokenUsage
`AgentTokenUsage` SHALL include a `rate_limit_wait_ms` field recording the total milliseconds the agent spent waiting for rate-limit permits across all retry attempts for that invocation.

#### Scenario: Wait time recorded for throttled agent
- **GIVEN** a rate limiter that causes an agent to wait 500ms total across its attempts
- **WHEN** the agent completes successfully
- **THEN** the resulting `AgentTokenUsage` SHALL have `rate_limit_wait_ms` equal to approximately 500

#### Scenario: Zero wait time when limiter disabled
- **GIVEN** the provider's rate limiter is disabled (`rpm = 0`)
- **WHEN** the agent completes successfully
- **THEN** the resulting `AgentTokenUsage` SHALL have `rate_limit_wait_ms` equal to 0

#### Scenario: Backward-compatible deserialization
- **GIVEN** a serialized `AgentTokenUsage` from a previous version without `rate_limit_wait_ms`
- **WHEN** the system deserializes this record
- **THEN** `rate_limit_wait_ms` SHALL default to 0

### Requirement: RPM to request spacing conversion
The system SHALL convert RPM values to exact per-request spacing using `governor::Quota::with_period(Duration::from_secs(60) / rpm)`, avoiding integer division loss from an intermediate RPS conversion.

#### Scenario: Exact spacing for 500 RPM
- **GIVEN** `openai_rpm = 500`
- **WHEN** the rate limiter is constructed
- **THEN** the limiter SHALL enforce a minimum spacing of 120ms between requests (60000ms / 500)

#### Scenario: Exact spacing for 30 RPM
- **GIVEN** a provider configured with `rpm = 30`
- **WHEN** the rate limiter is constructed
- **THEN** the limiter SHALL enforce a minimum spacing of 2000ms between requests (60000ms / 30)

### Requirement: Rate limiter propagation via CompletionModelHandle
`CompletionModelHandle` SHALL carry an `Option<SharedRateLimiter>` that is attached during `create_completion_model()` by looking up the provider in the `ProviderRateLimiters` registry. When an `LlmAgent` is built from the handle, the agent SHALL retain access to that same limiter so retry helpers can acquire permits without reconstructing limiter state.

#### Scenario: Handle carries limiter for configured provider
- **GIVEN** OpenAI is configured with `openai_rpm = 500`
- **WHEN** `create_completion_model(QuickThinking, ...)` is called for OpenAI
- **THEN** the returned `CompletionModelHandle` SHALL contain a `Some(SharedRateLimiter)` with the correct quota

#### Scenario: Handle carries no limiter for disabled provider
- **GIVEN** Copilot is configured with `copilot_rpm = 0`
- **WHEN** `create_completion_model(DeepThinking, ...)` is called for Copilot
- **THEN** the returned `CompletionModelHandle` SHALL contain `None` for the rate limiter
