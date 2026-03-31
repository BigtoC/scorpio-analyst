# `llm-rate-limiting` Capability

## MODIFIED Requirements

### Requirement: Per-provider RPM configuration
The system SHALL support configuring LLM API rate limits per provider (OpenAI, Anthropic, Gemini, Copilot, and OpenRouter) as requests-per-minute (RPM) values in a `[rate_limits]` section of `config.toml`. The `Config` loader SHALL treat the entire `rate_limits` section as optional and SHALL populate it from typed defaults when the section is absent. The system SHALL also support overriding these values via environment variables using the `SCORPIO__RATE_LIMITS__` prefix.

#### Scenario: All providers configured with positive RPM
- **GIVEN** `config.toml` contains `[rate_limits]` with `openai_rpm = 500`, `anthropic_rpm = 5`, `gemini_rpm = 500`, `copilot_rpm = 0`, and `openrouter_rpm = 20`
- **WHEN** the application loads configuration
- **THEN** the system SHALL create a `SharedRateLimiter` for each provider with a positive RPM value using request spacing derived from the configured RPM

#### Scenario: Provider RPM set to zero disables limiting
- **GIVEN** `config.toml` contains `openrouter_rpm = 0`
- **WHEN** the application loads configuration
- **THEN** the system SHALL NOT create a rate limiter for the OpenRouter provider, and LLM calls to OpenRouter SHALL proceed without proactive throttling

#### Scenario: Default values used when section is absent
- **GIVEN** `config.toml` does not contain a `[rate_limits]` section
- **WHEN** the application loads configuration
- **THEN** the system SHALL use typed default RPM values of `openai_rpm = 500`, `anthropic_rpm = 500`, `gemini_rpm = 500`, `copilot_rpm = 0`, and `openrouter_rpm = 20`

#### Scenario: Environment variable override
- **GIVEN** `config.toml` contains `openrouter_rpm = 20` and environment variable `SCORPIO__RATE_LIMITS__OPENROUTER_RPM=40` is set
- **WHEN** the application loads configuration
- **THEN** the system SHALL use `40` as the OpenRouter RPM value
