# Change: Add GitHub Copilot Provider via ACP

## Why

The PRD mandates GitHub Copilot as one of the cognitive engines available to the multi-agent trading firm. Because
Copilot does not expose a public REST API for direct third-party orchestration, `rig-core` cannot support it natively.
The official Agent Client Protocol (ACP) provides a standardized, secure, and locally-hosted bridge to Copilot's
reasoning engine by spawning the Copilot CLI as an ACP server over stdio. This proposal adds a custom `rig` provider
that communicates with GitHub Copilot through ACP, enabling any agent in the trading pipeline to use Copilot as its
LLM backend by simply setting the provider configuration to `"copilot"`.

## What Changes

- Implement an ACP transport layer (`src/providers/acp.rs`) that spawns `copilot --acp --stdio` as a child process and
  communicates over NDJSON-encoded JSON-RPC 2.0 streams.
- Implement a custom Copilot provider layer (`src/providers/copilot.rs`) that satisfies `rig`'s `ProviderClient`,
  `CompletionClient`, and `CompletionModel` trait boundaries by translating `rig` completion requests into ACP
  `session/prompt` calls and mapping ACP responses back into `rig` completion results.
- Validate Copilot ACP connectivity during application startup whenever any active LLM tier is configured with
  `"copilot"`, failing fast if the CLI cannot be spawned, authenticated, or initialized.
- Register the `"copilot"` provider variant in the existing provider factory so agents can select Copilot through
  `LlmConfig.quick_thinking_provider` or `LlmConfig.deep_thinking_provider` without any code changes.
- Manage the ACP lifecycle (initialize, session/new, session/prompt, cleanup) transparently within the completion model
  implementation.
- Preserve compatibility with the shared provider-layer `prompt_with_retry` and `chat_with_retry` helpers, including
  history-aware requests used by debate-style agents.
- Record authoritative latency for Copilot-backed calls while documenting that ACP does not expose authoritative token
  usage; any future visible-text token estimate would be heuristic-only and out of MVP scope.
- Ensure concurrent callers cannot corrupt the shared ACP stdio connection by enforcing request/response isolation at
  the Copilot client boundary.

## Impact

- Affected specs: `copilot-provider` (new)
- Affected code: `src/providers/copilot.rs` (new), `src/providers/acp.rs` (new), minor factory registration addition
  in `src/providers/factory.rs`, and application startup wiring that performs Copilot ACP preflight when configured
- Dependencies: `add-project-foundation` (core types, error handling), `add-llm-providers` (provider factory,
  `CompletionModel` trait patterns, retry/timeout helpers)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`)
