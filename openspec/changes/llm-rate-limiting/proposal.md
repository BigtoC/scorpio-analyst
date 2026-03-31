## Why

LLM API calls during fan-out phases (4 analysts in parallel, 3 risk agents in parallel) fire concurrently without proactive throttling. The system relies entirely on reactive 429 handling via retry/backoff, which wastes time and tokens on failed attempts. A `SharedRateLimiter` already exists and is wired into data clients (Finnhub, YFinance) but is never applied to LLM calls. As the system scales (more debate rounds, multi-cycle backtesting), unthrottled concurrent requests will increasingly hit provider rate limits.

## What Changes

- Add a `[rate_limits]` section to `config.toml` with per-provider RPM (requests per minute) settings for OpenAI, Anthropic, Gemini, and Copilot. Setting `0` disables limiting for that provider.
- **BREAKING**: Migrate `[api].finnhub_rate_limit` to `[rate_limits].finnhub_rps` and remove the old field.
- Extend `SharedRateLimiter` so it can be constructed from an exact `governor::Quota`, then create a `ProviderRateLimiters` registry that maps `ProviderId` to `SharedRateLimiter`, constructed from config using `governor::Quota::with_period` for exact RPM-to-request-spacing conversion.
- Embed an `Option<SharedRateLimiter>` in `CompletionModelHandle` and copy it into `LlmAgent` during agent construction so the limiter is available inside retry helpers.
- Insert `limiter.acquire().await` before each attempt inside the retry loops (`retry_prompt_budget_loop`, `chat_with_retry_budget` / `chat_with_retry_details_budget`, and the inline loop in `prompt_typed_with_retry`), using Option C semantics: acquire sits outside the per-attempt timeout but is bounded by the total budget, so LLM calls always get their full per-attempt timeout.
- Add `rate_limit_wait_ms` field to `AgentTokenUsage` to track time spent waiting for rate limit permits, surfaced in the per-agent usage output.
- Introduce a `RetryOutcome<T>` wrapper returned from retry functions to thread wait-time measurements back to callers alongside the response.

## Capabilities

### New Capabilities
- `llm-rate-limiting`: Per-provider proactive rate limiting for LLM API requests, configurable via `config.toml` RPM settings, with wait-time observability in `TokenUsageTracker`.

### Modified Capabilities
<!-- No existing specs to modify -->

## Impact

- **Config**: `config.toml` gains `[rate_limits]` section; `[api].finnhub_rate_limit` removed (breaking for existing config files).
- **Core types**: `CompletionModelHandle` gains a `rate_limiter` field; `AgentTokenUsage` gains `rate_limit_wait_ms`; retry functions return `RetryOutcome<T>` instead of bare `T`.
- **Call sites**: All agent call sites that invoke `prompt_with_retry_details` / `chat_with_retry_details` / `prompt_typed_with_retry` need to destructure `RetryOutcome` and pass `rate_limit_wait_ms` to `usage_from_response`. Handle-construction call sites that use `create_completion_model()` also need the new limiter registry parameter.
- **Dependencies**: No new crates (`governor` already in `Cargo.toml`).
- **Tests**: Test helpers across ~12 files that reference `finnhub_rate_limit: 30` need updating to the new config shape; `for_test()` constructors need to accept `None` for the limiter.
