# Add LLM Providers

## Motivation

The foundation layer established core types, configuration, error handling, and observability — but the system cannot
yet communicate with any Large Language Model. Every downstream agent (analysts, researchers, trader, risk team, fund
manager) requires a provider abstraction to send prompts and receive structured completions. This proposal introduces a
unified LLM provider layer built on `rig-core`, implementing a dual-tier cognitive routing strategy (quick-thinking vs.
deep-thinking) and a provider factory that resolves the correct client from configuration. Once this layer exists, all
agent changes (I3) can develop in parallel against a stable provider interface.

## Goals

- Add `rig-core` (and its feature-gated provider sub-crates) to the project dependency tree.
- Define a `ModelTier` enum encoding the dual-tier cognitive routing strategy from the PRD.
- Implement a provider factory that constructs `rig` completion models from `Config` and `ApiConfig`.
- Support both stateless prompt execution and stateful chat-history execution so downstream debate and risk loops can
  reuse the same provider layer.
- Establish agent builder helper patterns (system prompt + tools + structured output extraction) that
  downstream agent specs can reuse directly.
- Standardize typed tool-calling through `rig`'s schema-driven tool interfaces so downstream agents do not invent
  ad-hoc tool protocols.
- Wrap `rig` completion calls with the retry/timeout policies defined in `error-handling`.
- Surface transport/provider failures through `TradingError::Rig` and structured-output failures through
  `TradingError::SchemaViolation` with proper context propagation.

## Scope

- **In Scope:** `rig-core` dependency, `ModelTier` enum, provider factory, prompt/chat-compatible agent builder helpers,
  typed tool-calling patterns, schema-enforced structured output handling, retry-wrapped completion calls, provider
  integration tests with mocked completions, `Cargo.toml` dependency additions.
- **Out of Scope / Deferred:** Custom GitHub Copilot ACP integration (`add-copilot-provider`), specific agent
  implementations (`add-analyst-team`, etc.), `graph-flow` task wiring (`add-graph-orchestration`), financial data
  clients, CLI/TUI/GUI behavior.

## Capabilities Changed

- **`llm-providers` (Added):** Specifies `rig-core` client initialization, dual-tier model routing, provider factory,
  prompt/chat agent builder patterns, typed tool-calling, schema-enforced completions, and retry-wrapped completion
  calls.

## Impact

- **Affected specs:** `llm-providers` (new)
- **Affected code:** `Cargo.toml`, `src/providers/mod.rs`, `src/providers/factory.rs`
- **Downstream benefit:** All 5 agent changes plus the orchestration layer can import provider construction and agent
  building utilities without reimplementing client initialization or retry logic. The `add-copilot-provider` change
  extends this layer by adding a custom provider in its own files (`src/providers/copilot.rs`,
  `src/providers/acp.rs`) without modifying the factory interface.
