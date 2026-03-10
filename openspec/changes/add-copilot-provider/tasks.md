# Tasks for `add-copilot-provider`

## Prerequisites

- [x] `add-project-foundation` is complete (core types, error handling, config, module stubs)
- [x] `add-llm-providers` is complete (provider factory, CompletionModel patterns, retry helpers)

## 1. ACP Transport Layer (`src/providers/acp.rs`)

- [ ] 1.1 Define JSON-RPC 2.0 message types (`JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcNotification`) as
      `serde::Serialize` / `serde::Deserialize` structs with appropriate `#[serde]` attributes for the `jsonrpc`,
      `id`, `method`, `params`, `result`, and `error` fields
- [ ] 1.2 Define ACP-specific parameter and result types: `InitializeRequest`/`Response`,
      `NewSessionRequest`/`Response`, `PromptRequest`/`Response`, `SessionUpdate` notification variants
      (`agent_message_chunk`, `tool_call`, `plan`), `RequestPermissionRequest`/`Response`, `ContentBlock::Text`,
      `StopReason` enum
- [ ] 1.3 Implement `AcpTransport` struct wrapping `tokio::process::ChildStdin` (write) and a
      `tokio::io::BufReader<ChildStdout>` (read) with a `next_id: u64` counter for request ID sequencing
- [ ] 1.4 Implement `AcpTransport::send_request()` — serialize a `JsonRpcRequest` as a single JSON line + `\n` to
      stdin, flush, and return the assigned request ID
- [ ] 1.5 Implement `AcpTransport::read_message()` — read one NDJSON line from stdout, deserialize as either a
      `JsonRpcResponse` or `JsonRpcNotification`, return a discriminated enum
- [ ] 1.6 Implement typed ACP method helpers: `send_initialize()`, `send_session_new()`, `send_session_prompt()`,
      `send_permission_response()` — each constructs the appropriate params and calls `send_request()`
- [ ] 1.7 Write unit tests for JSON-RPC serialization/deserialization round-trips using known ACP message fixtures
- [ ] 1.8 Write unit tests for NDJSON framing (multi-line, interleaved notifications and responses)

## 2. Copilot Client and Process Management (`src/providers/copilot.rs`)

- [ ] 2.1 Implement `CopilotClient` struct holding `Option<AcpTransport>` (lazy init),
      `Option<tokio::process::Child>` (process handle), and the configured executable path
- [ ] 2.2 Implement `CopilotClient::ensure_initialized()` — if no transport exists, spawn `copilot --acp --stdio`
      via `tokio::process::Command` with stdin/stdout/stderr piped, create `AcpTransport`, send `initialize`
      request, validate protocol version, store transport
- [ ] 2.3 Implement `CopilotClient::is_alive()` — check if the child process is still running (non-blocking
      `try_wait()`)
- [ ] 2.4 Implement crash recovery in `ensure_initialized()` — if `is_alive()` returns false, drop the old
      transport/process and respawn
- [ ] 2.5 Implement `Drop` for `CopilotClient` — close stdin, wait with timeout, send SIGTERM if needed
- [ ] 2.6 Write unit tests using a mock subprocess (e.g., a simple Rust binary or shell script that echoes
      predetermined NDJSON responses) to validate spawn, initialize, and shutdown lifecycle

## 3. CompletionModel Trait Implementation (`src/providers/copilot.rs`)

- [ ] 3.1 Implement `CopilotCompletionModel` struct wrapping a shared `CopilotClient` (via `Arc<Mutex<...>>` or
      `Arc<tokio::sync::Mutex<...>>`)
- [ ] 3.2 Implement `rig::completion::CompletionModel` for `CopilotCompletionModel`:
      - Call `client.ensure_initialized()`
      - Send `session/new`, extract `sessionId`
      - Translate rig completion request (system prompt + user message) into ACP `session/prompt` params
      - Loop reading messages: accumulate `agent_message_chunk` text, respond to `request_permission` with cancelled,
        log warnings for unexpected `tool_call` notifications
      - On `session/prompt` response, check `stopReason` — map `end_turn` to success, `refusal` to error
      - Return assembled text as `CompletionResponse`
- [ ] 3.3 Implement token usage metadata: record `latency_ms` via `std::time::Instant`, set token counts to 0,
      set `model_id` to `"copilot"` or agent info name if available
- [ ] 3.4 Write integration tests with a mock ACP server subprocess that validates the full completion lifecycle
      (initialize → session/new → session/prompt → update notifications → prompt response)

## 4. Provider Factory Registration (`src/providers/factory.rs`)

- [ ] 4.1 Add `"copilot"` match arm in the provider factory function that constructs a `CopilotCompletionModel`
      (no API key required)
- [ ] 4.2 Ensure the Copilot provider works with existing `prompt_with_retry` and `chat_with_retry` helpers
      without special-case logic
- [ ] 4.3 Write a factory test verifying that `"copilot"` provider name resolves to a `CopilotCompletionModel`

## 5. Error Mapping and Validation

- [ ] 5.1 Verify all subprocess failures (spawn, IO, crash) map to `TradingError::Rig` with `"copilot"` provider
      context
- [ ] 5.2 Verify ACP protocol errors (JSON-RPC error responses, version mismatch) map to `TradingError::Rig`
- [ ] 5.3 Verify no raw prompts, responses, or credentials leak in error messages
- [ ] 5.4 Write tests for each error scenario: binary not found, protocol mismatch, JSON-RPC error, process crash
      mid-request

## 6. Documentation and CI

- [ ] 6.1 Add inline doc comments (`///`) for all public types and functions in `acp.rs` and `copilot.rs`
- [ ] 6.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [ ] 6.3 Ensure `cargo fmt -- --check` passes
- [ ] 6.4 Ensure `cargo test` passes all new and existing tests
