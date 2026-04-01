## Context

The system currently supports four LLM providers: OpenAI, Anthropic, Gemini (all HTTP API-key clients via rig-core), and Copilot (custom ACP transport). The provider architecture uses enum dispatch across `ProviderId`, `ProviderClient`, and `LlmAgentInner` to route completion requests to the correct backend. Each new provider requires adding a variant to these enums and wiring match arms through ~10 call sites in the factory module.

rig-core 0.32 (our current dependency) already ships `rig::providers::openrouter` as a built-in module with `Client`, `CompletionModel`, and `Usage` types. OpenRouter is an OpenAI-compatible API proxy aggregating 300+ models behind a single API key, including free-tier models at 20 RPM.

## Goals / Non-Goals

**Goals:**
- Add OpenRouter as a first-class provider following the exact same enum-dispatch pattern used by OpenAI/Anthropic/Gemini (standard HTTP API key client, no custom transport).
- Enable free-tier model usage for development and testing via dual-tier routing (e.g., `quick_thinking_provider = "openrouter"` with a free model, `deep_thinking_provider = "anthropic"` with a paid model).
- Provide appropriate rate limiting defaults for OpenRouter's free tier (20 RPM).

**Non-Goals:**
- OpenRouter-specific advanced features (provider preferences, quantization controls, max price routing) — these can be added later if needed.
- Per-model rate limiting — the current per-provider RPM design is retained. If both tiers use OpenRouter, they share the same 20 RPM limiter. A per-model rate limiting redesign would affect all providers and is out of scope.
- Upgrading rig-core beyond 0.32 — the OpenRouter module is fully available in the current version.
- Embedding model support via OpenRouter — only completion models are wired.

## Decisions

### Decision 1: Standard enum-dispatch variant (not a "generic OpenAI-compatible" shim)

**Choice**: Add `ProviderId::OpenRouter` as a dedicated variant with its own `ProviderClient::OpenRouter(openrouter::Client)` and `LlmAgentInner::OpenRouter(Agent<OpenRouterModel>)`.

**Alternative considered**: Treat OpenRouter as a "custom base URL" on the existing OpenAI client. Rejected because rig-core already provides a dedicated OpenRouter module with its own `CompletionModel`, `Usage` struct, and `OpenRouterExt` builder. Using the dedicated module gives us proper type safety, OpenRouter-specific error messages, and future extensibility (provider preferences, etc.) without fighting the type system.

**Rationale**: Follows the established pattern set by all four existing providers. Each match site gains one arm — mechanical and low-risk.

### Decision 2: Single `openrouter_rpm` rate limit field defaulting to 20

**Choice**: Add `openrouter_rpm: u32` to `RateLimitConfig` with a default of 20 (matching the documented free-tier limit).

**Alternative considered**: Per-model rate limiting. Rejected as over-engineering — it would require restructuring `ProviderRateLimiters` from `HashMap<ProviderId, _>` to a model-aware lookup, affecting all providers. The 20 RPM default is safe for all OpenRouter models (paid models have higher limits and will simply not saturate the limiter).

### Decision 3: `SCORPIO_OPENROUTER_API_KEY` env var with `SecretString` handling

**Choice**: Follow the existing pattern — `ApiConfig.openrouter_api_key: Option<SecretString>`, loaded via `secret_from_env("SCORPIO_OPENROUTER_API_KEY")`, redacted in `Debug` impl.

**Rationale**: Exact consistency with the OpenAI/Anthropic/Gemini key handling. No deviation.

### Decision 4: Type alias `OpenRouterModel = rig::providers::openrouter::completion::CompletionModel`

**Choice**: Add a type alias in `factory.rs` alongside the existing `OpenAIModel`, `AnthropicModel`, `GeminiModel` aliases, then use it for the `LlmAgentInner::OpenRouter` variant.

**Rationale**: Matches the established pattern exactly. The rig openrouter module exposes `completion::CompletionModel` at the same path convention as other providers.

### Decision 5: Reuse the existing shared token-usage path

**Choice**: Do not introduce any OpenRouter-specific token tracking abstraction. OpenRouter completions continue to flow through the existing `rig::completion::Usage` handling already consumed by the analyst, researcher, trader, risk, and fund-manager usage helpers.

**Rationale**: Token accounting is a shared cross-capability behavior owned by existing agent/state code, not by the provider factory itself. Because rig-core's OpenRouter module returns the same usage shape already handled by the system, the provider change only needs to preserve that path rather than add new state types or tracking logic.

## Risks / Trade-offs

**[Risk] Free-tier models may have lower quality or inconsistent structured output compliance** → Mitigation: This is a user configuration choice, not a system risk. The existing `SchemaViolation` error handling and retry logic apply identically. Users can switch to paid OpenRouter models or other providers at any time via config.

**[Risk] OpenRouter rate limits vary by model and account tier** → Mitigation: The 20 RPM default is the lowest common denominator (free tier). Users with paid accounts can increase `openrouter_rpm` via config or env override. No code change needed.

**[Risk] rig-core's openrouter module may have undiscovered quirks at 0.32** → Mitigation: Low probability — the module has a full `CompletionModel` impl, `Usage` struct, and follows the same pattern as all other rig providers. Integration tests with a real OpenRouter key will surface any issues early.

**[Trade-off] No Anthropic-style `max_tokens` override for OpenRouter** → The `build_agent_inner` function sets `.max_tokens(4096)` only for the Anthropic variant (required by Anthropic's API). OpenRouter does not require this. The OpenRouter match arm in `build_agent_inner` will follow the OpenAI/Gemini pattern (no `max_tokens` call).

## Migration Plan

- Obtain approval for the cross-owner file edits listed in `proposal.md` before implementation begins.
- Implement the change as additive registration only: add the provider enum/client/match arms, add the API key and RPM config surfaces, wire the shared rate limiter registry, and update any shared test fixtures that manually construct `ApiConfig` literals.
- Verify that existing provider selections remain unchanged when `openrouter` is not configured.
- Rollback is trivial: revert the additive OpenRouter registration changes and remove any `openrouter_*` configuration entries. No state or data migration is involved.

## Open Questions

- None at the proposal stage. The OpenRouter client exists in rig-core 0.32, the free-tier RPM is fixed at 20 for this change, and the requested models are represented as ordinary string model IDs.
