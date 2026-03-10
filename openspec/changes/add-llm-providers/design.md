# Design for `add-llm-providers`

## Context

With the foundation layer complete (`core-types`, `config`, `error-handling`, `observability`, `rate-limiting`,
`testing-strategy`), the project needs a concrete LLM communication layer before any agent can be implemented. The PRD
mandates `rig-core` as the unified LLM abstraction across OpenAI, Anthropic, and Gemini providers, with a dual-tier
model routing strategy. This design captures the architectural decisions for the provider layer.

## Goals / Non-Goals

- **Goals**
    - Integrate `rig-core` and its OpenAI, Anthropic, and Gemini provider features.
    - Define a `ModelTier` enum that encodes the PRD's quick-thinking / deep-thinking routing strategy.
    - Build a provider factory that takes `LLMConfig` and `ApiConfig` and returns a ready-to-use `rig` completion model
      for a given tier.
    - Establish reusable agent builder helpers (system prompt, tools, structured output) that all downstream agent
      specs share.
    - Support both `prompt` and `chat` execution paths so downstream cyclic debate phases can reuse the same provider
      abstractions without bypassing retry, timeout, and accounting hooks.
    - Wrap `rig` `prompt`/`chat` calls with the foundation's `RetryPolicy` and `tokio::time::timeout`.
    - Distinguish transport/provider failures from structured-output failures by mapping provider errors into
      `TradingError::Rig` and schema/JSON extraction failures into `TradingError::SchemaViolation`.
- **Non-Goals**
    - Implementing the custom GitHub Copilot ACP provider (deferred to `add-copilot-provider`).
    - Implementing any specific agent logic, system prompts, or tool bindings (deferred to agent changes).
    - Defining `graph-flow` tasks or workflow edges.
    - Integrating vector stores or embeddings (deferred to `add-financial-data`).

## Architectural Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     src/providers/                          │
│                                                             │
│  mod.rs ─── ModelTier enum + re-exports                     │
│  factory.rs ── create_completion_model()                    │
│               build_agent()                                 │
│               prompt_with_retry()                           │
│                                                             │
│  (copilot.rs + acp.rs added later by add-copilot-provider)  │
└─────────────────────────────────────────────────────────────┘
          │ uses                       │ uses
          ▼                            ▼
   ┌───────────────┐            ┌────────────────┐
   │   Config      │            │  TradingError  │
   │  (LlmConfig,  │            │  ::Rig         │
   │   ApiConfig)  │            │  RetryPolicy   │
   └───────────────┘            └────────────────┘
```

### Provider Resolution Flow

1. Caller specifies a `ModelTier` (quick-thinking or deep-thinking).
2. The factory reads `LLMConfig.quick_thinking_provider` or `LLMConfig.deep_thinking_provider` based on
   `ModelTier` to determine which backend (OpenAI / Anthropic / Gemini).
3. The factory selects the model ID from `LLMConfig.quick_thinking_model` or `LLMConfig.deep_thinking_model`.
4. The factory resolves the provider credential from `ApiConfig`, constructs or reuses the corresponding `rig` client,
   and returns a shared completion-model handle for that provider/model combination.

### Provider Reuse And Registry

The provider layer avoids rebuilding network clients on every call.

1. Application startup validates provider names and model identifiers, normalizing them into provider descriptors.
2. The provider module maintains a reusable registry keyed by provider + model ID + tier.
3. Retry attempts reuse the same client/model handle rather than reconstructing clients, agents, or tool schemas.
4. Agent-specific builder helpers construct tool-enabled agents once per task/agent instance and reuse them for
   repeated prompt or chat calls.

### Prompt And Chat Support

The provider layer exposes helpers for both one-shot prompts and chat-history-based execution:

1. `prompt_with_retry` handles stateless request/response calls used by analysts and other single-shot tasks.
2. `chat_with_retry` accepts prior `rig::message::Message` history so debate-oriented agents can continue a structured
   exchange without rebuilding ad-hoc retry logic.
3. Both helpers enforce the same timeout, retry, and error-mapping rules so downstream callers receive a uniform
   failure contract regardless of invocation style.
4. The provider layer processes prompt text, chat history, and structured outputs in memory only for the duration of a
   request and does not persist or log raw prompt/history/response bodies.

### Dual-Tier Routing

| Tier          | Default Model   | Usage                                   |
|---------------|-----------------|-----------------------------------------|
| QuickThinking | gemini-2.5-fast | Analyst team (data extraction, summary) |
| DeepThinking  | gpt-5.4         | Researchers, Trader, Risk, Fund Manager |

The config is the single source of truth for model IDs — agents never hardcode model names.

### Retry and Timeout Wrapping

The `prompt_with_retry` helper:

1. Applies `tokio::time::timeout(agent_timeout_secs)` around each attempt while also respecting a total wall-clock
   request budget shared across retries.
2. On transient pre-tool transport errors (rate limit, timeout, temporary upstream unavailability), retries using
   `RetryPolicy::delay_for_attempt`.
3. On permanent errors (auth, configuration, unsupported provider/model, permission, schema violations), fails
   immediately without retry.
4. Does not retry any request after tool execution has started unless every tool in that call path is explicitly
   documented as read-only and idempotent.
5. Records per-attempt timing and provider/model metadata for the calling agent to feed into `TokenUsageTracker`.

### Agent Builder Pattern

A `build_agent` helper wraps `rig::AgentBuilder` to:

1. Set the system prompt (passed as `&str`).
2. Attach tool definitions (passed as a collection of code-defined `rig` tool objects).
3. Configure structured output extraction via `rig`'s JSON schema enforcement.
4. Return a configured agent object that downstream code calls with `prompt()` or `chat()`.

This helper is intentionally thin — it avoids coupling the provider layer to specific agent personas.

### Tool-Calling And Structured Output

The PRD requires tool execution and rigid JSON schemas to eliminate the telephone effect. The provider layer therefore
standardizes two rules for downstream agents:

1. Tools are declared through `rig`'s typed schema system (for example via the `#[tool]` macro or equivalent
   provider-owned typed helpers), not through free-text prompt conventions or runtime-supplied raw schema strings.
2. Structured outputs are decoded through provider-owned helpers that treat malformed JSON or schema mismatches as
   `TradingError::SchemaViolation`, separate from transport- or provider-level failures.
3. Schema-violation context is sanitized and truncated before it is surfaced so callers can distinguish malformed output
   without receiving raw prompt text, chat history, or unbounded model output.

This keeps downstream agent specs focused on domain prompts and tools rather than repetitive parsing logic.

## Key Decisions

- **Per-tier providers with per-tier model selection**: The factory resolves provider by tier
  (`quick_thinking_provider` for `QuickThinking`, `deep_thinking_provider` for `DeepThinking`) and routes to the
  corresponding tier model ID. This allows fast/deep tiers to use different backends without introducing per-agent
  override complexity.

- **Prompt/chat parity at the provider layer**: Because the PRD explicitly relies on both `prompt` and `chat` traits,
  the provider layer owns wrappers for both invocation styles. This avoids a future split where cyclic agents implement
  their own retry and timeout logic differently from single-shot agents.

- **Shared provider handles over per-call reconstruction**: the provider registry returns reusable handles so repeated
  fan-out calls and retry attempts reuse the same initialized client/model instead of paying connection setup costs on
  every call.

- **Separate schema vs provider failures**: The foundation already defines both `Rig` and `SchemaViolation` variants.
  The provider layer uses `TradingError::Rig` for provider construction, authentication, transport, or `rig` runtime
  failures, and uses `TradingError::SchemaViolation` when a completion cannot be decoded into the expected JSON schema.
  This preserves the PRD's requirement for rigid structured outputs while keeping remediation signals precise.

- **Sanitized provider error boundary**: provider-layer failures surface only sanitized metadata such as provider name,
  model ID, correlation-safe context, and a bounded error summary. Raw prompts, chat history, raw model outputs,
  credentials, and native `rig` error objects do not cross the provider boundary.

- **Retry at the provider layer, not the agent layer**: Centralizing retry in a shared `prompt_with_retry` function
  avoids duplicating backoff logic across 10+ agents. Agents call this helper and receive either a successful
  completion or a terminal `TradingError`.

## Trade-offs

- **Tier-level provider vs. per-agent provider**: Tier-level provider config allows quick/deep tiers to diverge while
  still avoiding the larger complexity of per-agent override matrices.
- **Reusable registry vs. simpler factory-only construction**: a registry adds a small amount of lifecycle management,
  but avoids avoidable client/model rebuild costs during concurrent fan-out and retry loops.
