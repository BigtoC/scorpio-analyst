# `config` Capability

## ADDED Requirements

### Requirement: Structured Config Domains

System configurations MUST map onto strictly grouped struct boundaries including `Config`, `LLMConfig`, `TradingConfig`,
and `ApiConfig`, ensuring isolated access domains.

- `LLMConfig` MUST include stable fields for analyst model selection, researcher model selection, `max_debate_rounds`,
  `max_risk_rounds`, and `analyst_timeout_secs`.
- `ApiConfig` MUST include provider credentials and provider quota inputs needed by downstream clients and rate-limiters.
- `TradingConfig` MAY own non-secret trading defaults needed by downstream execution layers.

#### Scenario: Sub-System Instantiation

When initializing the provider factory, only the `LLMConfig` slice is passed downstream, explicitly isolating it from
unrelated trading defaults while still exposing model names, round limits, and analyst timeout settings.

### Requirement: 3-Tier Multi-Layer Configuration Pipeline

Configuration loading MUST execute through an explicitly prioritized resolution tree backed by foundation-owned artifacts:

1. `config.toml` (base layer checked into repository control defining safe defaults)
2. `.env` via `dotenvy` (local override, generally untracked locally)
3. Environment variables (top-priority overrides)

The foundation MUST define both a checked-in `config.toml` and a redacted `.env.example` describing the required secret
inputs.

#### Scenario: Local Developer Booting

The operator launches the application initially; it gathers defaults like debate round limits and timeout values from
`config.toml`, locates API credentials from the local `.env`, and allows environment variables to override both when
present.

### Requirement: Strong Credential Isolation

All secret-bearing configuration values within the foundation MUST be wrapped using `secrecy::SecretString` to prevent
accidental leakage in debug output, memory handling, and tracing.

#### Scenario: Tracing System Debug Output

When a developer prints a debug tree containing the overarching configuration block, the inner strings belonging to API
keys render as redacted rather than exposing the underlying secret values.

### Requirement: Structural Startup Validation

Before startup completes, aggregated configuration MUST run validation that fails fast on missing required settings or
invalid baseline values needed by the primary workflow.

Validation at this stage MUST cover structural presence and value ranges for required foundation fields, but MUST NOT
require live provider connectivity checks that belong to later changes.

#### Scenario: Incomplete Runtime Execution

When an environment starts without a required LLM model field or provider credential, validation detects the omission
immediately after config aggregation and exits with an instructive configuration error instead of waiting for a later
workflow crash.
