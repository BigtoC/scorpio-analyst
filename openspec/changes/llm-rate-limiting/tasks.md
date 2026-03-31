## 1. Configuration

- [x] 1.1 Add `RateLimitConfig` struct to `src/config.rs` with per-provider RPM fields (`openai_rpm`, `anthropic_rpm`, `gemini_rpm`, `copilot_rpm`) and `finnhub_rps`, all with serde defaults. Add `rate_limits: RateLimitConfig` to the top-level `Config` struct and mark the field with `#[serde(default)]` (or equivalent `Default` support) so config loading succeeds when the entire `[rate_limits]` section is absent.
- [x] 1.2 Remove `finnhub_rate_limit` from `ApiConfig` in `src/config.rs` and its default function. Update the `Debug` impl for `ApiConfig` accordingly.
- [x] 1.3 Update `config.toml` to replace `[api].finnhub_rate_limit` with a new `[rate_limits]` section containing all provider RPM defaults and `finnhub_rps = 30`.
- [x] 1.4 Update config loading tests in `src/config.rs` to validate the new `RateLimitConfig` fields and removal of `finnhub_rate_limit`.

## 2. Rate Limiter Registry

- [x] 2.1 Extend `SharedRateLimiter` in `src/rate_limit.rs` with a quota-aware constructor (or equivalent exact-period constructor) so callers can build a limiter from `Quota::with_period(...)` without duplicating `SharedRateLimiter` internals. Preserve the existing per-second constructor for current Finnhub call sites and tests where appropriate.
- [x] 2.2 Add `ProviderRateLimiters` struct to `src/rate_limit.rs`: a wrapper around `HashMap<ProviderId, SharedRateLimiter>` with a `from_config(cfg: &RateLimitConfig)` constructor that creates limiters using `Quota::with_period(Duration::from_secs(60) / rpm)` for each provider where `rpm > 0`, and a `get(id: ProviderId) -> Option<&SharedRateLimiter>` accessor.
- [x] 2.3 Add a `from_config` constructor helper for the Finnhub limiter that reads `finnhub_rps` from `RateLimitConfig` (replacing the inline construction in `main.rs`).
- [x] 2.4 Add unit tests for `ProviderRateLimiters`: construction with mixed RPM values (some zero, some positive), `get()` returns `None` for disabled providers, `get()` returns `Some` for enabled providers, and zero-RPM providers are absent from the map.

## 3. CompletionModelHandle Integration

- [x] 3.1 Add `rate_limiter: Option<SharedRateLimiter>` field to `CompletionModelHandle` in `src/providers/factory.rs`. Add a `rate_limiter()` accessor method. Update the `for_test()` constructor to set `rate_limiter: None`.
- [x] 3.2 Update `create_completion_model()` signature to accept `&ProviderRateLimiters` and attach the matching limiter from the registry to the handle.
- [x] 3.3 Add `rate_limiter: Option<SharedRateLimiter>` to `LlmAgent` and copy it from `CompletionModelHandle` inside `build_agent()` / `build_agent_with_tools()`. Add a `rate_limiter()` accessor on `LlmAgent` for the retry helpers.
- [x] 3.4 Update all `create_completion_model()` call sites, including `main.rs`, `preflight_configured_providers()`, one-shot constructors in `trader` / `fund_manager`, and affected tests/helpers, to pass the registry.

## 4. Retry Loop Changes

- [x] 4.1 Define `RetryOutcome<T>` struct in `src/providers/factory.rs` with fields `result: T` and `rate_limit_wait_ms: u64`.
- [x] 4.2 Update `retry_prompt_budget_loop` to: (a) call `limiter.acquire().await` bounded by remaining budget before each attempt, (b) measure acquire duration, (c) accumulate wait time across retries, (d) return `RetryOutcome<R>` instead of `R`.
- [x] 4.3 Apply the same changes to both chat retry loops (`chat_with_retry_budget` and `chat_with_retry_details_budget`) — acquire before each attempt, measure, accumulate, return `RetryOutcome`.
- [x] 4.4 Apply the same changes to `prompt_typed_with_retry` — acquire before each attempt, measure, accumulate, return `RetryOutcome`.
- [x] 4.5 Update all public retry function signatures (`prompt_with_retry`, `prompt_with_retry_details`, `chat_with_retry`, `chat_with_retry_details`, `prompt_typed_with_retry`) to return `RetryOutcome<T>`.

## 5. Observability

- [x] 5.1 Add `rate_limit_wait_ms: u64` field to `AgentTokenUsage` in `src/state/token_usage.rs` with `#[serde(default)]` for backward-compatible deserialization.
- [x] 5.2 Update all 4 `usage_from_response()` functions (in `src/agents/analyst/common.rs`, `src/agents/researcher/common.rs`, `src/agents/risk/common.rs`, `src/agents/fund_manager/usage.rs`) to accept a `rate_limit_wait_ms: u64` parameter and set the field.
- [x] 5.3 Update `AgentTokenUsage::unavailable()` to set `rate_limit_wait_ms: 0`.

## 6. Agent Call Site Updates

- [x] 6.1 Update the 4 analyst agent modules (`fundamental.rs`, `sentiment.rs`, `news.rs`, `technical.rs`) to destructure `RetryOutcome` from the retry call and pass `rate_limit_wait_ms` to `usage_from_response()`.
- [x] 6.2 Update the 2 researcher agents (`bullish.rs`, `bearish.rs`) and researcher moderator (`moderator.rs`) to destructure `RetryOutcome` and pass wait time through.
- [x] 6.3 Update the 3 risk agents (`aggressive.rs`, `conservative.rs`, `neutral.rs`) and risk moderator (`moderator.rs`) to destructure `RetryOutcome` and pass wait time through.
- [x] 6.4 Update the trader agent (`src/agents/trader/mod.rs`) and fund manager agent (`src/agents/fund_manager/agent.rs`) to destructure `RetryOutcome` and pass wait time through.

## 7. Handle Construction Wiring

- [x] 7.1 Update `src/main.rs` to construct `ProviderRateLimiters` from `cfg.rate_limits` and pass it to `create_completion_model()` calls. Construct the Finnhub limiter from `cfg.rate_limits.finnhub_rps` instead of `cfg.api.finnhub_rate_limit`.
- [x] 7.2 Update `preflight_configured_providers()` and any helper APIs in `src/providers/factory.rs` that create completion handles so they also accept and use `&ProviderRateLimiters`.
- [x] 7.3 Update one-shot handle construction in agents such as trader and fund manager, plus any shared pipeline wiring, to use the new config/registry shape.

## 8. Test Fixups

- [x] 8.1 Update all test helper `ApiConfig` construction sites (~12 files) to remove `finnhub_rate_limit: 30` and add appropriate `RateLimitConfig` where needed.
- [x] 8.2 Update `usage_from_response` tests in analyst/researcher/risk/fund_manager common modules to pass `rate_limit_wait_ms` and assert the new field.
- [x] 8.3 Update factory tests and helper call sites affected by the `create_completion_model(..., &ProviderRateLimiters)` signature change, including preflight tests and any agent tests that construct handles directly.
- [x] 8.4 Update retry-loop tests in `src/providers/factory.rs` to validate that `RetryOutcome` is returned with correct `rate_limit_wait_ms` values for prompt, chat, chat-details, and typed-prompt paths.

## 9. Verification

- [x] 9.1 Run `cargo fmt -- --check` and fix any formatting issues.
- [x] 9.2 Run `cargo clippy` and resolve all warnings.
- [x] 9.3 Run `cargo test` and ensure all tests pass.
- [x] 9.4 Run `cargo build` and confirm clean compilation.
