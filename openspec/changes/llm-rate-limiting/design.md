## Context

The system currently has two layers of LLM error handling but no proactive throttling for LLM API calls:

1. **Proactive rate limiting** exists for data clients (Finnhub, YFinance) via `SharedRateLimiter` backed by `governor` — the limiter is injected at construction and `acquire().await` is called before every HTTP request.
2. **Reactive retry** exists for LLM calls via `retry_prompt_budget_loop` in `src/providers/factory.rs` — 429 errors are detected by string-matching in `is_transient_error()` and retried with exponential backoff.

The gap: LLM calls during fan-out phases (4 analysts, 3 risk agents) fire concurrently without proactive throttling. The existing `SharedRateLimiter` type is fully implemented but never instantiated for LLM providers.

Constraints:
- `rig`'s `CompletionError` flattens HTTP status codes and headers into strings — `Retry-After` headers are not accessible. This rules out header-based adaptive rate adjustment at this layer.
- `governor::RateLimiter` does not support dynamic quota changes after construction. True adaptive throttling would require `ArcSwap` or a semaphore-based approach (deferred).
- The system uses 4 distinct LLM providers (OpenAI, Anthropic, Gemini, Copilot), each with independent rate limits.

## Goals / Non-Goals

**Goals:**
- Proactively throttle LLM API requests per provider using configurable RPM values in `config.toml`.
- Ensure rate limiter waiting does not reduce the per-attempt timeout available for actual LLM calls (Option C semantics).
- Surface rate-limit wait time in `AgentTokenUsage` so operators can identify throttling bottlenecks.
- Migrate the existing `finnhub_rate_limit` into a unified `[rate_limits]` config section.

**Non-Goals:**
- Adaptive rate adjustment based on 429 frequency or `Retry-After` headers (deferred to a future change).
- Per-model rate limiting (providers rate-limit per organization, not per model).
- Dynamic quota modification at runtime (governor limitation — would require `ArcSwap` or different backend).
- Token-based rate limiting (TPM) — only request-based (RPM) for V1.

## Decisions

### 1. Per-provider keying with `HashMap<ProviderId, SharedRateLimiter>`

**Decision**: One `SharedRateLimiter` per provider, keyed by `ProviderId` enum.

**Rationale**: LLM providers enforce rate limits per API key / organization, not per model. A single OpenAI key shared by `gpt-4o-mini` (analysts) and `o3` (researchers) hits one combined limit. Per-model keying would add complexity without matching real-world enforcement.

**Alternatives considered**:
- *Global single limiter*: Too coarse — penalizes Anthropic calls for OpenAI congestion.
- *Per-model limiter*: Over-granular — providers don't enforce per-model.

### 2. RPM in config, `Quota::with_period` for exact spacing

**Decision**: Config uses RPM (requests per minute) matching provider documentation. Internally converted using `governor::Quota::with_period(Duration::from_secs(60) / rpm)` for exact per-request spacing with no integer division loss.

**Rationale**: Users think in RPM (it's what provider dashboards show). `Quota::per_second(rpm / 60)` loses precision for sub-60 RPM values (e.g., RPM=30 → RPS=0 with integer division). `with_period` is exact.

**Alternatives considered**:
- *RPS in config*: Maps directly to `Quota::per_second` but unfamiliar to users — providers document limits in RPM.

### 3. `0` means disabled

**Decision**: A provider RPM of `0` means no rate limiter is created for that provider. The `ProviderRateLimiters::get()` method returns `None`, and retry loops skip the acquire step.

**Rationale**: Copilot ACP has no documented rate limits. Defaulting to `0` (disabled) avoids unnecessary throttling while still allowing users to opt in if they discover limits empirically.

### 4. Limiter embedded in `CompletionModelHandle` and copied into `LlmAgent`

**Decision**: `CompletionModelHandle` gains an `Option<SharedRateLimiter>` field. The limiter is attached during `create_completion_model()` by looking up the provider ID in the `ProviderRateLimiters` map. `build_agent()` / `build_agent_with_tools()` then copy that `Option<SharedRateLimiter>` into `LlmAgent` so retry helpers can access it directly.

**Rationale**: The limiter is a property of "talking to this provider," which is already what `CompletionModelHandle` represents. But the retry helpers operate on `&LlmAgent`, and `LlmAgent` does not retain a reference to the original handle. Copying the cheap `Arc`-backed limiter into `LlmAgent` preserves the current ownership model while keeping retry helper signatures unchanged.

**Alternatives considered**:
- *Pass limiter as extra param to retry functions*: Explicit, but requires changing 14+ call sites and threading it through every agent method.
- *Embed in `ProviderClient` enum*: Cleanest architecturally but requires reworking the enum dispatch layer.

### 5. Exact RPM spacing requires a quota-aware limiter constructor

**Decision**: Extend `SharedRateLimiter` with a constructor that accepts a precomputed `governor::Quota` (or an equivalent period-based constructor), while retaining the existing per-second constructor for Finnhub and existing tests.

**Rationale**: The current `SharedRateLimiter::new(label, per_second)` only supports `Quota::per_second`. The new provider RPM registry needs exact spacing from `Quota::with_period(Duration::from_secs(60) / rpm)`, so the rate-limit module must expose a way to build limiters from that quota instead of forcing the registry to duplicate `SharedRateLimiter` internals.

### 6. Option C: Acquire outside timeout, deducted from budget

**Decision**: In the retry loop, `limiter.acquire()` is called before each attempt but outside the `tokio::time::timeout` that wraps the LLM call. The acquire is itself bounded by `tokio::time::timeout(remaining_budget, ...)` so it can't block forever.

```
let remaining = total_budget - elapsed;
match tokio::time::timeout(remaining, limiter.acquire()).await { ... }
// LLM call gets its full per-attempt timeout
let attempt_timeout = timeout.min(total_budget - elapsed);
tokio::time::timeout(attempt_timeout, call_fn()).await
```

**Rationale**: The LLM call always gets its full configured per-attempt timeout. The limiter wait is bounded by the total budget and can't silently steal time from the call. At typical concurrency (4 agents, 500 RPM), worst-case wait is ~360ms against a 30s timeout — negligible. But at aggressive configs (1 RPM, 4 agents), Option A would steal 75% of the timeout.

**Alternatives considered**:
- *Option A (inside timeout)*: Simpler, but squeezes LLM call time under contention.
- *Option B (outside, unbounded)*: LLM gets full time, but total budget accounting becomes inaccurate.

### 7. `RetryOutcome<T>` wrapper for wait-time observability

**Decision**: Retry functions return `RetryOutcome<T>` containing both the result and accumulated `rate_limit_wait_ms`. The `_details` variants return `RetryOutcome<PromptResponse>`. Callers destructure and pass `rate_limit_wait_ms` to `usage_from_response()`.

**Rationale**: Wait time is measured inside the retry loop (accumulated across retries). It needs to reach `AgentTokenUsage` which is constructed by the caller. A wrapper type is the simplest way to thread this out without adding shared mutable state.

### 8. `AgentTokenUsage` gains `rate_limit_wait_ms` field

**Decision**: Per-agent `rate_limit_wait_ms: u64` field on `AgentTokenUsage`, representing total milliseconds spent in `limiter.acquire()` across all retry attempts for that agent call.

**Rationale**: Per-agent granularity reveals which agents in a fan-out are waiting longest (the last-queued analyst will always wait more). Phase-level aggregation would lose this diagnostic value.

## Risks / Trade-offs

- **[Stale defaults]** → Config defaults may not match actual provider tier limits. Mitigation: document that users should check their provider dashboard and adjust RPM values. Conservative defaults err on the side of throttling too much rather than too little.
- **[Integer RPM floor]** → `Quota::with_period(Duration::from_secs(60) / rpm)` panics if `rpm == 0`. Mitigation: skip limiter construction when `rpm == 0` before any division.
- **[Breaking config migration]** → Moving `finnhub_rate_limit` to `[rate_limits]` breaks existing config files. Mitigation: project is pre-1.0, change is documented as breaking in proposal.
- **[Governor not dynamic]** → Cannot lower rate at runtime when hitting 429s frequently. Mitigation: acceptable for V1 — reactive retry/backoff handles overflow. Dynamic adjustment is a future enhancement.
- **[Serialization compatibility]** → Adding `rate_limit_wait_ms` to `AgentTokenUsage` changes the serialized shape (used in snapshot DB). Mitigation: use `#[serde(default)]` so existing snapshots deserialize with `rate_limit_wait_ms: 0`.
