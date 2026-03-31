## 0. Approval Gate

- [x] 0.1 Obtain approval for the cross-owner file changes listed in `proposal.md` before implementation begins

## 1. Configuration Layer

- [x] 1.1 Add `"openrouter"` to `deserialize_provider_name()` in `src/config.rs` — accept it as a valid provider name (case-insensitive, whitespace-trimmed) and update the error message listing supported providers
- [x] 1.2 Add `openrouter_api_key: Option<SecretString>` to `ApiConfig` in `src/config.rs` — add the field, update `Debug` impl to include it, add `secret_from_env("SCORPIO_OPENROUTER_API_KEY")` in `Config::load_from()`, and include it in the `has_key` validation check
- [x] 1.3 Add `openrouter_rpm: u32` to `RateLimitConfig` in `src/config.rs` — add the field with `#[serde(default = "default_openrouter_rpm")]` defaulting to 20, and update the `Default` impl

## 2. Provider Enum and Factory

- [x] 2.1 Add `ProviderId::OpenRouter` variant in `src/providers/mod.rs` — add the variant, `as_str()` returning `"openrouter"`, and `missing_key_hint()` returning `"SCORPIO_OPENROUTER_API_KEY"`
- [x] 2.2 Add `ProviderClient::OpenRouter` variant and `create_provider_client_for()` match arm in `src/providers/factory.rs` — import `rig::providers::openrouter`, add the variant wrapping `openrouter::Client`, construct it from `api_config.openrouter_api_key`
- [x] 2.3 Add `validate_provider_id()` match arm for `"openrouter"` in `src/providers/factory.rs` — map to `ProviderId::OpenRouter` and update the error message listing supported providers

## 3. Agent Enum Dispatch

- [x] 3.1 Add `OpenRouterModel` type alias and `LlmAgentInner::OpenRouter` variant in `src/providers/factory.rs` — add `type OpenRouterModel = rig::providers::openrouter::completion::CompletionModel` and the corresponding `LlmAgentInner` variant
- [x] 3.2 Add OpenRouter match arms to `LlmAgent` methods in `src/providers/factory.rs` — add the `OpenRouter` arm to `prompt()`, `prompt_details()`, `prompt_typed_details()`, `chat()`, and `chat_details()`
- [x] 3.3 Add OpenRouter match arm to `build_agent_inner()` in `src/providers/factory.rs` — follow the OpenAI/Gemini pattern (no `.max_tokens()` override)

## 4. Rate Limiting

- [x] 4.1 Add `ProviderId::OpenRouter` entry to `ProviderRateLimiters::from_config()` in `src/rate_limit.rs` — wire `cfg.openrouter_rpm` to `ProviderId::OpenRouter`

## 5. Config Files

- [x] 5.1 Add `openrouter_rpm = 20` to the `[rate_limits]` section in `config.toml`
- [x] 5.2 Add `SCORPIO_OPENROUTER_API_KEY=` to `.env.example`

## 6. Tests

- [x] 6.1 Update config tests in `src/config.rs` — add `"openrouter"` to `deserialize_provider_name_accepts_valid`, verify `openrouter_rpm` default in `load_from_defaults_only`, add `openrouter_api_key` assertion to `api_config_debug_redacts_secrets`
- [x] 6.2 Update rate limit tests in `src/rate_limit.rs` — add `ProviderId::OpenRouter` assertions to existing tests that verify limiter registration
- [x] 6.3 Update factory tests in `src/providers/factory.rs` — add OpenRouter to any existing provider validation tests, verify `validate_provider_id("openrouter")` returns `ProviderId::OpenRouter`
- [x] 6.4 Add a factory/config test proving OpenRouter free-model identifiers such as `qwen/qwen3.6-plus-preview:free` and `minimax/minimax-m2.5:free` are accepted unchanged

## 7. Verification

- [x] 7.1 Run `cargo fmt --check && cargo clippy -- -D warnings && cargo test` — verify no formatting issues, no clippy warnings, and all tests pass
