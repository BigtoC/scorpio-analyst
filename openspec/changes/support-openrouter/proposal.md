## Why

The current provider set (OpenAI, Anthropic, Gemini, Copilot) requires paid API keys for all HTTP-based LLM backends. OpenRouter aggregates 300+ models behind a single API key, including free-tier models (e.g., `qwen/qwen3.6-plus-preview:free`, `minimax/minimax-m2.5:free` at 20 RPM) that enable zero-cost development, testing, and demonstration runs. Adding OpenRouter as a first-class provider lets users pair free quick-thinking models with paid deep-thinking models via the existing dual-tier routing — no code changes to downstream agents.

## What Changes

- Add `ProviderId::OpenRouter` variant to the provider enum with full enum dispatch across all match sites (factory construction, agent building, prompt/chat/typed-prompt invocation).
- Add `ProviderClient::OpenRouter(openrouter::Client)` and `LlmAgentInner::OpenRouter(Agent<OpenRouterModel>)` variants using rig-core 0.32's built-in `rig::providers::openrouter` module — no custom transport code.
- Accept `"openrouter"` in config deserialization (`deserialize_provider_name`) and provider validation (`validate_provider_id`).
- Add `SCORPIO_OPENROUTER_API_KEY` environment variable and `openrouter_api_key` field to `ApiConfig`.
- Add `openrouter_rpm` to `RateLimitConfig` with a default of 20 RPM (free-tier limit).
- Extend the shared `llm-rate-limiting` contract so OpenRouter participates in the existing per-provider RPM configuration and limiter registry.
- Update `config.toml` and `.env.example` with the new provider's defaults.

## Capabilities

### New Capabilities
- `openrouter-provider`: Integration of OpenRouter as a first-class LLM provider via rig-core's built-in `rig::providers::openrouter` module, including factory registration, agent building, rate limiting, and configuration.

### Modified Capabilities
- `llm-providers`: The "Provider Module Boundary" requirement currently scopes native rig-core providers to OpenAI, Anthropic, and Gemini. This change expands that set to include OpenRouter. The "Provider Factory Construction" and "Rig-Core Integration" requirements gain an additional supported backend. No other requirement semantics change.
- `llm-rate-limiting`: The per-provider RPM configuration currently enumerates OpenAI, Anthropic, Gemini, and Copilot. This change expands that shared contract to include `openrouter_rpm` with a default of 20 RPM and corresponding limiter registration.

## Cross-Owner Changes

This change requires approved cross-owner edits before implementation begins.

- `src/providers/mod.rs` (owner: `add-project-foundation` skeleton): add `ProviderId::OpenRouter` so the shared provider enum can route OpenRouter handles.
- `src/providers/factory.rs` (owner: `add-llm-providers`): add the native OpenRouter client/model/dispatch arms to the shared provider factory.
- `src/config.rs` (owner: `add-project-foundation` / `config` capability): add `openrouter_api_key`, accept `"openrouter"` as a valid provider name, and include the key in startup validation.
- `src/rate_limit.rs` (owner: `add-project-foundation` and shared `llm-rate-limiting` behavior): register `ProviderId::OpenRouter` in `ProviderRateLimiters::from_config()`.
- `config.toml` (owner: `add-project-foundation`): document the checked-in `openrouter_rpm = 20` default in `[rate_limits]`.
- `.env.example` (owner: `add-project-foundation`): document `SCORPIO_OPENROUTER_API_KEY` for operators.

## Impact

- **Code**: `src/providers/mod.rs`, `src/providers/factory.rs`, `src/config.rs`, `src/rate_limit.rs` — purely additive match arms and struct fields. No changes to existing provider logic, agent behavior, or state management.
- **Cross-owner approval required**: Implementation touches files owned by `add-project-foundation`, `add-llm-providers`, and the shared `llm-rate-limiting` change. Those edits are limited to additive provider registration and configuration surfaces.
- **Dependencies**: None — `rig::providers::openrouter` is already available in rig-core 0.32 with no feature flag required.
- **Configuration**: New env var `SCORPIO_OPENROUTER_API_KEY`, new config field `openrouter_rpm`. Existing configs remain valid without modification.
- **Tests**: Existing provider tests need updated assertions to include the new variant. New unit tests for OpenRouter factory construction and config validation.
