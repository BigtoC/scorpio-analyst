# Copilot Phase 1 MCP Tool Calling Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make phase 1 analysts work with Copilot as the only provider by routing analyst tools through a session-scoped MCP helper, replacing the shared ACP bottleneck with isolated workers, and preventing stale ACP responses after timeout.

**Architecture:** First refresh the ACP wire contract and split the current Copilot monolith into focused modules so worker lifecycle, pooling, request parsing, and shared contracts each have one clear home. Then add a hidden stdio MCP helper backed by serializable analyst tool bundles, introduce a Copilot-specific bundle-aware agent path in the provider factory, and finally migrate the four phase-1 analysts onto that path with regression coverage for timeouts, tool calls, and fan-out concurrency.

**Tech Stack:** Rust 2024, `tokio`, `rig-core`, `serde`, `serde_json`, `rmcp`, `finnhub`, `yfinance-rs`, `tracing`

---

## File Map

- `Cargo.toml` - add `rmcp` and any required feature flags for the MCP helper server
- `src/providers/acp.rs` - typed ACP wire contracts, nested `session/update` parsing, typed `mcpServers`, `session/cancel`
- `src/providers/copilot.rs` - delete after splitting into a package module
- `src/providers/copilot/mod.rs` - Copilot provider facade, shared exports, error surface
- `src/providers/copilot/contracts.rs` - `CopilotToolSessionMeta`, `CopilotMcpSessionSpec`, tool policy structs, validation helpers
- `src/providers/copilot/request.rs` - prompt rendering, request metadata extraction, session-update accumulation logic
- `src/providers/copilot/worker.rs` - one Copilot subprocess, ACP session execution, helper startup/teardown, timeout invalidation
- `src/providers/copilot/pool.rs` - bounded worker pool, FIFO checkout, respawn, taint/discard logic
- `src/providers/factory.rs` - bundle-aware agent builder for Copilot, `LlmAgentInner` extension, request cleanup ownership
- `src/agents/analyst/tool_bundle.rs` - `AnalystToolBundle`, `AnalystToolSpec`, local-tool conversion, MCP session-spec serialization
- `src/agents/analyst/mod.rs` - export the new tool-bundle module
- `src/agents/analyst/fundamental.rs` - replace raw tool vector with a fundamental bundle
- `src/agents/analyst/sentiment.rs` - replace raw tool vector with cached/live news-aware bundle logic
- `src/agents/analyst/news.rs` - replace raw tool vector with cached/live news + macro bundle logic
- `src/agents/analyst/technical.rs` - replace raw tool vector with shared-context technical bundle logic
- `src/cli/mod.rs` - expose the hidden MCP entrypoint
- `src/cli/mcp.rs` - hidden `mcp serve --session-spec <path>` parser and `rmcp` stdio server
- `src/main.rs` - dispatch the hidden MCP helper command before normal app startup
- `tests/copilot_phase1_mcp.rs` - end-to-end regression tests for Copilot worker timeout recovery, MCP attachment, and phase-1 analyst fan-out

## Constraints

- Preserve non-Copilot provider behavior; OpenAI, Anthropic, and Gemini should keep using the existing local `ToolDyn` execution path.
- Do not expand scope into researcher/risk/trader/fund-manager tool bundles.
- Keep the security boundary at app tools only; do not expose shell, file write, browser, or arbitrary URL capabilities.
- Keep the hidden MCP helper intentionally narrow; do not migrate the whole app to `clap` just to add this one subcommand.
- Follow TDD: every behavior change starts with a failing test, then the minimal implementation, then targeted verification.

## Chunk 1: ACP Contract And Copilot Worker Foundations

### Task 1: Refresh the ACP wire contract for nested updates and MCP server attachment

**Files:**
- Modify: `src/providers/acp.rs`
- Test: `src/providers/acp.rs`

- [ ] **Step 1: Write failing ACP contract tests**

Add tests with these exact names to `src/providers/acp.rs`:

```rust
#[test]
fn session_update_params_deserialize_nested_update_payload() {}

#[test]
fn new_session_params_serialize_stdio_mcp_server() {}

#[test]
fn cancel_session_params_serialize_session_id() {}
```

- [ ] **Step 2: Run the ACP tests and verify they fail for the right reason**

Run:

```bash
cargo test providers::acp::tests::session_update_params_deserialize_nested_update_payload -- --nocapture
cargo test providers::acp::tests::new_session_params_serialize_stdio_mcp_server -- --nocapture
cargo test providers::acp::tests::cancel_session_params_serialize_session_id -- --nocapture
```

Expected: FAIL because `SessionUpdateParams` still expects flat `params.type`, `NewSessionParams` still hardcodes `Vec<Value>`, and `session/cancel` helpers do not exist yet.

- [ ] **Step 3: Implement typed ACP contracts with the smallest correct surface**

Use concrete types instead of raw `Value` where this plan needs stability:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateParams {
    pub session_id: String,
    pub update: SessionUpdateEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateEnvelope {
    pub session_update: SessionUpdate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionUpdate {
    AgentMessageChunk { content: Value },
    AgentThoughtChunk { content: Value },
    ToolCall { content: Value },
    ToolCallUpdate { content: Value },
    Plan { content: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "camelCase")]
pub enum McpServerConfig {
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: std::collections::BTreeMap<String, String>,
    },
}
```

Also add:

- `send_session_new(&mut self, params: NewSessionParams)`
- `send_session_cancel(&mut self, session_id: &str)`

- [ ] **Step 4: Re-run the ACP tests and keep fixing until they pass**

Run:

```bash
cargo test providers::acp::tests:: -- --nocapture
```

Expected: PASS for the new nested-update/MCP/cancel coverage, with existing permission-response tests still green.

- [ ] **Step 5: Commit the ACP contract refresh**

```bash
git add src/providers/acp.rs
git commit -m "refactor: refresh acp wire contracts for copilot mcp sessions"
```

### Task 2: Split the Copilot monolith and add request/update parsing tests

**Files:**
- Create: `src/providers/copilot/mod.rs`
- Create: `src/providers/copilot/request.rs`
- Delete after move: `src/providers/copilot.rs`
- Modify: `src/providers/mod.rs`
- Test: `src/providers/copilot/request.rs`

- [ ] **Step 1: Write failing request-parsing tests before moving code**

Add tests with these exact names to `src/providers/copilot/request.rs`:

```rust
#[test]
fn accumulate_agent_message_chunk_from_nested_session_update() {}

#[test]
fn ignore_tool_progress_updates_for_text_accumulation() {}

#[test]
fn build_prompt_text_includes_documents_schema_and_history() {}
```

- [ ] **Step 2: Run the new request tests and verify they fail**

Run:

```bash
cargo test providers::copilot::request::tests:: -- --nocapture
```

Expected: FAIL because the module does not exist yet and the old parser still expects flat `params.type`.

- [ ] **Step 3: Move only request-specific logic into focused files**

Create this package split:

```rust
// src/providers/copilot/mod.rs
mod request;
pub use request::{build_prompt_text, handle_session_update};
```

Move the remaining existing production code from `src/providers/copilot.rs` into `src/providers/copilot/mod.rs` in the same task so the crate still compiles after the file split. `mod.rs` should temporarily own:

- `CopilotClient`
- `CopilotCompletionModel`
- `CopilotProviderClient`
- `CopilotError`
- the existing non-request unit tests

Move into `request.rs`:

- `build_prompt_text(...)`
- nested-update parsing helpers
- text accumulation behavior for `agent_message_chunk`

Also add one focused parser test here:

```rust
#[test]
fn nested_plan_or_thought_updates_do_not_append_text() {}
```

Do not move worker/pool code yet; keep this task limited to request parsing and the module split.

- [ ] **Step 4: Re-run the request tests and the old Copilot tests**

Run:

```bash
cargo test providers::copilot::request::tests:: -- --nocapture
cargo test providers::copilot::tests::build_prompt_includes_output_schema -- --nocapture
```

Expected: PASS, with no behavior change outside request parsing.

- [ ] **Step 5: Commit the Copilot module split foundation**

```bash
git add src/providers/mod.rs src/providers/copilot/mod.rs src/providers/copilot/request.rs
git commit -m "refactor: split copilot request parsing into focused module"
```

### Task 3: Add isolated Copilot workers and a bounded FIFO pool

**Files:**
- Create: `src/providers/copilot/worker.rs`
- Create: `src/providers/copilot/pool.rs`
- Modify: `src/providers/copilot/mod.rs`
- Test: `src/providers/copilot/worker.rs`
- Test: `src/providers/copilot/pool.rs`

- [ ] **Step 1: Write failing worker/pool tests first**

Add tests with these exact names:

```rust
#[tokio::test]
async fn worker_timeout_discards_process_and_marks_it_tainted() {}

#[tokio::test]
async fn pool_checkout_waits_fifo_when_all_workers_are_busy() {}

#[tokio::test]
async fn pool_respawns_after_discard_to_restore_capacity() {}

#[tokio::test]
async fn pool_checkout_times_out_when_no_worker_returns_in_time() {}

#[tokio::test]
async fn pool_fast_fails_when_capacity_cannot_be_restored() {}
```

- [ ] **Step 2: Run the worker/pool tests and verify they fail**

Run:

```bash
cargo test providers::copilot::worker::tests:: -- --nocapture
cargo test providers::copilot::pool::tests:: -- --nocapture
```

Expected: FAIL because the worker and pool modules do not exist yet.

- [ ] **Step 3: Implement the smallest worker/pool boundary that satisfies the spec**

Use these core shapes:

```rust
pub struct CopilotWorker {
    client: CopilotClient,
    tainted: bool,
}

pub struct CopilotWorkerPool {
    // fixed target size: 4
}

pub struct CopilotWorkerLease {
    // returns or discards on drop/finalize
}
```

Rules to implement now:

- one checked-out worker per request
- FIFO waiting when all workers are busy
- explicit checkout timeout error when no worker becomes available in time
- discard on timeout/protocol mismatch/helper failure
- background respawn to restore target capacity `4`
- fast-fail `TradingError::Rig` when all workers are tainted or repeated respawn/startup attempts cannot restore capacity

- [ ] **Step 4: Re-run the worker/pool tests and keep the scope narrow**

Run:

```bash
cargo test providers::copilot::worker::tests:: -- --nocapture
cargo test providers::copilot::pool::tests:: -- --nocapture
```

Expected: PASS, with the current provider still not using MCP yet.

- [ ] **Step 5: Commit the worker-pool foundation**

```bash
git add src/providers/copilot/mod.rs src/providers/copilot/worker.rs src/providers/copilot/pool.rs
git commit -m "feat: add isolated copilot workers and bounded pool"
```

## Chunk 2: Shared Contracts, MCP Helper, And Analyst Bundles

### Task 4: Add shared Copilot session metadata and session-spec contracts

**Files:**
- Create: `src/providers/copilot/contracts.rs`
- Create: `src/agents/analyst/tool_specs.rs`
- Modify: `src/providers/copilot/mod.rs`
- Modify: `src/agents/analyst/mod.rs`
- Test: `src/providers/copilot/contracts.rs`
- Test: `src/agents/analyst/tool_specs.rs`

- [ ] **Step 1: Write failing contract tests**

Add tests with these exact names:

```rust
#[test]
fn copilot_tool_session_meta_rejects_missing_session_spec_path() {}

#[test]
fn mcp_session_spec_round_trips_with_version_one() {}

#[test]
fn mcp_session_spec_rejects_unknown_tool_bundle_kind() {}

#[test]
fn analyst_tool_spec_round_trips_without_provider_dependency() {}
```

- [ ] **Step 2: Run the contract tests and verify they fail**

Run:

```bash
cargo test providers::copilot::contracts::tests:: -- --nocapture
cargo test agents::analyst::tool_specs::tests:: -- --nocapture
```

Expected: FAIL because the new contracts module does not exist yet.

- [ ] **Step 3: Implement the shared serde contracts exactly once**

Split ownership cleanly to avoid cycles:

- `src/agents/analyst/tool_specs.rs` owns provider-neutral tool spec enums and shared-state spec types used by both bundles and Copilot contracts.
- `src/providers/copilot/contracts.rs` owns only Copilot session metadata and the session-spec wrapper.

Use these concrete types so provider and helper share one schema:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotToolSessionMeta {
    pub version: u32,
    pub helper_kind: String,
    pub tool_bundle_kind: String,
    pub helper_name: String,
    pub session_spec_path: std::path::PathBuf,
    pub checkout_timeout_ms: u64,
    pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotMcpSessionSpec {
    pub version: u32,
    pub session_id: uuid::Uuid,
    pub tool_bundle_kind: String,
    pub tools: Vec<crate::agents::analyst::tool_specs::AnalystToolSpec>,
    pub shared_state: crate::agents::analyst::tool_specs::AnalystSharedStateSpec,
    pub policy: CopilotMcpSessionPolicy,
}
```

At the same time, add provider-neutral spec types in `src/agents/analyst/tool_specs.rs`, for example:

```rust
pub enum AnalystToolSpec {
    GetFundamentals { symbol: String },
    GetEarnings { symbol: String },
    GetNews { symbol: String },
    GetCachedNews { symbol: String },
    GetMarketNews,
    GetEconomicIndicators,
    GetOhlcv { symbol: String, start: String, end: String, context_id: uuid::Uuid },
    CalculateAllIndicators { context_id: uuid::Uuid },
    CalculateRsi { context_id: uuid::Uuid },
    CalculateMacd { context_id: uuid::Uuid },
    CalculateAtr { context_id: uuid::Uuid },
    CalculateBollingerBands { context_id: uuid::Uuid },
    CalculateIndicatorByName { context_id: uuid::Uuid },
}

pub enum AnalystSharedStateSpec {
    None,
    CachedNews { news: crate::state::NewsData },
    TechnicalContexts {
        contexts: Vec<TechnicalContextSpec>,
    },
}

pub struct TechnicalContextSpec {
    pub context_id: uuid::Uuid,
    pub symbol: String,
    pub start: String,
    pub end: String,
}
```

Add `validate()` helpers here instead of spreading version/path checks across multiple files.

- [ ] **Step 4: Re-run the contract tests and keep them green**

Run:

```bash
cargo test providers::copilot::contracts::tests:: -- --nocapture
cargo test agents::analyst::tool_specs::tests:: -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit the shared Copilot contracts**

```bash
git add src/providers/copilot/mod.rs src/providers/copilot/contracts.rs src/agents/analyst/tool_specs.rs src/agents/analyst/mod.rs
git commit -m "feat: add shared copilot session metadata contracts"
```

### Task 5: Add the hidden stdio MCP helper server

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/cli/mod.rs`
- Create: `src/cli/mcp.rs`
- Modify: `src/main.rs`
- Test: `src/cli/mcp.rs`

- [ ] **Step 1: Write a failing helper smoke test using an `rmcp` client**

Add tests with these exact names to `src/cli/mcp.rs`:

```rust
#[tokio::test]
async fn helper_serves_only_declared_tools_from_session_spec() {}

#[tokio::test]
async fn helper_uses_policy_helper_name_as_the_only_server_name() {}

#[tokio::test]
async fn helper_executes_declared_tool_calls_and_returns_results() {}

#[tokio::test]
async fn helper_returns_tool_error_for_invalid_declared_call() {}

#[tokio::test]
async fn helper_reconstructs_cached_news_and_shared_technical_context_from_spec() {}
```

The tests should collectively prove that the helper:

- write a temp `CopilotMcpSessionSpec`
- spawn `scorpio-analyst mcp serve --session-spec <path>`
- connect with an `rmcp` stdio client
- assert only the declared tool names are listed
- assert the advertised server name exactly matches `policy.helper_name`
- execute at least one real `call_tool` round-trip
- surface a structured MCP tool error when arguments/spec state are invalid
- reconstruct cached-news and shared technical-context state from the spec file

Cover all four phase-1 analyst bundle families across these tests:

- fundamental: list and call `GetFundamentals` or `GetEarnings`
- sentiment: cached/live news reconstruction and tool exposure
- news: cached/live news plus market-news and economic-indicator exposure
- technical: `GetOhlcv` plus indicator tools sharing one reconstructed context

- [ ] **Step 2: Run the helper test and verify it fails**

Run:

```bash
cargo test cli::mcp::tests::helper_serves_only_declared_tools_from_session_spec -- --nocapture
```

Expected: FAIL because `rmcp` is not wired in and the hidden command does not exist yet.

- [ ] **Step 3: Implement the narrow helper path without a full CLI rewrite**

Make only these changes:

- add `rmcp` to `Cargo.toml`
- update `src/main.rs` to short-circuit when argv matches `mcp serve --session-spec <path>`
- keep the normal `cargo run` flow unchanged for every other argv shape
- put the hidden MCP entrypoint in `src/cli/mcp.rs`

Recommended entrypoint shape:

```rust
pub async fn maybe_run_hidden_mcp_command(args: &[String]) -> anyhow::Result<bool>;
pub async fn serve_session_spec(path: &std::path::Path) -> anyhow::Result<()>;
```

Inside `serve_session_spec(...)`, implement all of the spec-required helper behavior in this task:

- parse the session spec file
- validate `version == 1`
- reject unknown `tool_bundle_kind`
- reconstruct only the declared tools
- reconstruct shared state for cached news and the technical `context_id`
- advertise exactly one server name from `policy.helper_name`
- return structured MCP tool errors on failed `call_tool`
- exit cleanly when stdin closes

- [ ] **Step 4: Re-run the helper test and a normal startup smoke test**

Run:

```bash
cargo test cli::mcp::tests:: -- --nocapture
cargo test providers::copilot::tests::copilot_client_stores_model_id -- --nocapture
```

Expected: PASS for the helper test, and the existing Copilot unit test still passes to confirm the normal app path did not regress.

- [ ] **Step 5: Commit the hidden MCP helper command**

```bash
git add Cargo.toml src/cli/mod.rs src/cli/mcp.rs src/main.rs
git commit -m "feat: add hidden mcp helper server for copilot tools"
```

### Task 6: Add serializable phase-1 analyst tool bundles

**Files:**
- Create: `src/agents/analyst/tool_bundle.rs`
- Modify: `src/agents/analyst/tool_specs.rs`
- Modify: `src/agents/analyst/mod.rs`
- Test: `src/agents/analyst/tool_bundle.rs`

- [ ] **Step 1: Write failing bundle tests before any implementation**

Add tests with these exact names:

```rust
#[test]
fn fundamental_bundle_contains_get_fundamentals_and_get_earnings() {}

#[test]
fn sentiment_bundle_prefers_cached_news_when_available() {}

#[test]
fn news_bundle_contains_live_news_market_news_and_economic_indicators_when_cache_is_absent() {}

#[test]
fn news_bundle_contains_cached_news_market_news_and_economic_indicators_when_cache_is_present() {}

#[test]
fn technical_bundle_contains_full_required_tool_set() {}

#[test]
fn technical_bundle_serializes_one_shared_context_id_for_ohlcv_and_indicators() {}
```

- [ ] **Step 2: Run the bundle tests and verify they fail**

Run:

```bash
cargo test agents::analyst::tool_bundle::tests:: -- --nocapture
```

Expected: FAIL because the bundle module does not exist yet.

- [ ] **Step 3: Implement a focused bundle abstraction, not a generic tool framework**

Use a phase-1-specific interface:

```rust
pub enum AnalystToolBundle {
    Fundamental {
        symbol: String,
        finnhub: crate::data::FinnhubClient,
    },
    Sentiment {
        symbol: String,
        finnhub: crate::data::FinnhubClient,
        cached_news: Option<std::sync::Arc<crate::state::NewsData>>,
    },
    News {
        symbol: String,
        finnhub: crate::data::FinnhubClient,
        cached_news: Option<std::sync::Arc<crate::state::NewsData>>,
    },
    Technical {
        symbol: String,
        start_date: String,
        end_date: String,
        yfinance: crate::data::YFinanceClient,
        context: crate::data::OhlcvToolContext,
    },
}

impl AnalystToolBundle {
    pub fn local_tools(&self) -> Vec<Box<dyn ToolDyn>> { ... }
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> { ... }
    pub fn to_mcp_session_spec(&self, request_id: &str) -> CopilotMcpSessionSpec { ... }
}
```

Keep file responsibilities tight:

- `tool_specs.rs` owns serializable spec enums and shared-state spec types
- `tool_bundle.rs` owns bundle assembly and conversion between runtime tools, tool definitions, and session specs

Do not add future-phase variants yet.

- [ ] **Step 4: Re-run the bundle tests and verify parity between local and serialized forms**

Run:

```bash
cargo test agents::analyst::tool_bundle::tests:: -- --nocapture
```

Expected: PASS, including the cached-news selection rule and the shared technical context rule.

- [ ] **Step 5: Commit the phase-1 tool bundle layer**

```bash
git add src/agents/analyst/mod.rs src/agents/analyst/tool_specs.rs src/agents/analyst/tool_bundle.rs
git commit -m "feat: add serializable phase1 analyst tool bundles"
```

## Chunk 3: Bundle-Aware Agent Path, MCP Attachment, And Analyst Migration

### Task 7: Add a bundle-aware Copilot agent path in the provider factory

**Files:**
- Modify: `src/providers/factory.rs`
- Modify: `src/providers/copilot/request.rs`
- Modify: `src/providers/copilot/contracts.rs`
- Test: `src/providers/factory.rs`

- [ ] **Step 1: Write failing factory tests for the new agent path**

Add tests with these exact names to `src/providers/factory.rs`:

```rust
#[tokio::test]
async fn copilot_tool_aware_agent_builds_request_with_additional_params() {}

#[tokio::test]
async fn native_bundle_agent_still_uses_local_tool_path() {}

#[tokio::test]
async fn copilot_tool_aware_agent_deletes_temp_session_spec_after_request() {}
```

- [ ] **Step 2: Run the factory tests and verify they fail**

Run:

```bash
cargo test providers::factory::tests::copilot_tool_aware_agent_builds_request_with_additional_params -- --nocapture
cargo test providers::factory::tests::native_bundle_agent_still_uses_local_tool_path -- --nocapture
cargo test providers::factory::tests::copilot_tool_aware_agent_deletes_temp_session_spec_after_request -- --nocapture
```

Expected: FAIL because there is no bundle-aware agent builder or temp-spec cleanup path yet.

- [ ] **Step 3: Implement the narrowest new builder surface**

Add a bundle-aware helper instead of overloading raw `Vec<Box<dyn ToolDyn>>`:

```rust
pub fn build_agent_with_tool_bundle(
    handle: &CompletionModelHandle,
    system_prompt: &str,
    bundle: AnalystToolBundle,
) -> LlmAgent
```

Implementation rules:

- native providers: call `bundle.local_tools()` and continue using `rig::AgentBuilder::tools(...)`
- Copilot: create a `CopilotToolAware` variant that writes a temp session spec, injects `CopilotToolSessionMeta` via `CompletionRequest.additional_params`, and cleans up the temp file after the provider future resolves

- [ ] **Step 4: Re-run the factory tests and the existing typed-prompt retry tests**

Run:

```bash
cargo test providers::factory::tests::copilot_tool_aware_agent_builds_request_with_additional_params -- --nocapture
cargo test providers::factory::tests::native_bundle_agent_still_uses_local_tool_path -- --nocapture
cargo test providers::factory::tests::copilot_tool_aware_agent_deletes_temp_session_spec_after_request -- --nocapture
cargo test providers::factory::tests::chat_with_retry_details_retries_and_truncates_partial_history -- --nocapture
```

Expected: PASS, and existing retry/history behavior remains unchanged for non-Copilot variants.

- [ ] **Step 5: Commit the bundle-aware factory path**

```bash
git add src/providers/factory.rs src/providers/copilot/request.rs src/providers/copilot/contracts.rs
git commit -m "feat: add bundle aware copilot agent path"
```

### Task 8: Attach the MCP helper to real Copilot requests and enforce the security policy

**Files:**
- Modify: `src/providers/copilot/worker.rs`
- Modify: `src/providers/copilot/pool.rs`
- Modify: `src/providers/copilot/mod.rs`
- Test: `tests/copilot_mcp_session.rs`
- Test: `tests/copilot_preflight.rs`

- [ ] **Step 1: Write failing provider integration tests**

Create focused test files with these tests:

```rust
// tests/copilot_mcp_session.rs
#[tokio::test]
async fn copilot_request_attaches_stdio_mcp_server_in_session_new() {}

#[tokio::test]
async fn nested_tool_progress_updates_are_parsed_without_appending_response_text() {}

#[tokio::test]
async fn timed_out_copilot_request_sends_session_cancel_before_worker_discard() {}

#[tokio::test]
async fn timed_out_copilot_request_discards_worker_and_next_request_has_no_stale_ids() {}

#[tokio::test]
async fn permission_request_is_treated_as_compatibility_fault_and_invalidates_worker() {}

#[tokio::test]
async fn invalid_or_missing_copilot_tool_session_metadata_fails_before_worker_checkout() {}

// tests/copilot_preflight.rs
#[tokio::test]
async fn preflight_fails_when_tool_visibility_policy_cannot_be_verified() {}
```

- [ ] **Step 2: Run the new provider integration tests and verify they fail**

Run:

```bash
cargo test --test copilot_mcp_session copilot_request_attaches_stdio_mcp_server_in_session_new -- --nocapture
cargo test --test copilot_mcp_session nested_tool_progress_updates_are_parsed_without_appending_response_text -- --nocapture
cargo test --test copilot_mcp_session timed_out_copilot_request_sends_session_cancel_before_worker_discard -- --nocapture
cargo test --test copilot_mcp_session timed_out_copilot_request_discards_worker_and_next_request_has_no_stale_ids -- --nocapture
cargo test --test copilot_mcp_session permission_request_is_treated_as_compatibility_fault_and_invalidates_worker -- --nocapture
cargo test --test copilot_mcp_session invalid_or_missing_copilot_tool_session_metadata_fails_before_worker_checkout -- --nocapture
cargo test --test copilot_preflight preflight_fails_when_tool_visibility_policy_cannot_be_verified -- --nocapture
```

Expected: FAIL because the worker still calls `session/new` with `mcpServers: []`, does not launch the helper, and does not invalidate stale workers after timeout.

- [ ] **Step 3: Implement helper launch, timeout invalidation, and preflight policy checks**

In `worker.rs`, make the execution loop do exactly this:

```rust
checkout worker
validate CopilotToolSessionMeta
spawn helper subprocess
send session/new with stdio mcp server entry
send session/prompt
collect nested session/update text chunks
on timeout -> send session/cancel -> short grace wait -> kill worker + helper -> discard worker
on success -> tear down helper -> return worker
```

Before worker checkout, validate request metadata and fail early when:

- `additional_params` does not contain `copilot_tool_session`
- the metadata cannot be deserialized
- `session_spec_path` is missing or unreadable

When ACP sends `session/request_permission`, treat it as a compatibility fault:

- respond with cancellation per ACP contract
- invalidate the worker
- fail the request with a security/compatibility error instead of continuing

In `preflight()`:

- verify Copilot CLI version `>= 1.0.12`
- verify required tool-filtering flags are present
- verify the startup probe does not expose unexpected tool namespaces

- [ ] **Step 4: Re-run the integration tests and confirm stale-ID recovery is real**

Run:

```bash
cargo test --test copilot_mcp_session -- --nocapture
cargo test --test copilot_preflight -- --nocapture
```

Expected: PASS, including the timeout/discard/no-stale-id regression.

- [ ] **Step 5: Commit the real Copilot MCP execution path**

```bash
git add src/providers/copilot/mod.rs src/providers/copilot/worker.rs src/providers/copilot/pool.rs tests/copilot_mcp_session.rs tests/copilot_preflight.rs
git commit -m "feat: route copilot requests through session scoped mcp helper"
```

### Task 9: Migrate the four phase-1 analysts and run full verification

**Files:**
- Modify: `src/agents/analyst/fundamental.rs`
- Modify: `src/agents/analyst/sentiment.rs`
- Modify: `src/agents/analyst/news.rs`
- Modify: `src/agents/analyst/technical.rs`
- Modify: `src/agents/analyst/mod.rs`
- Test: `tests/phase1_analyst_migration.rs`

- [ ] **Step 1: Write failing analyst migration tests before touching production code**

Add these exact tests to `tests/phase1_analyst_migration.rs`:

```rust
#[tokio::test]
async fn fundamental_analyst_uses_bundle_aware_builder() {}

#[tokio::test]
async fn news_and_sentiment_choose_cached_news_bundle_when_available() {}

#[tokio::test]
async fn four_phase1_analysts_can_share_the_copilot_pool_without_cross_talk() {}
```

- [ ] **Step 2: Run the analyst migration tests and verify they fail**

Run:

```bash
cargo test --test phase1_analyst_migration fundamental_analyst_uses_bundle_aware_builder -- --nocapture
cargo test --test phase1_analyst_migration news_and_sentiment_choose_cached_news_bundle_when_available -- --nocapture
cargo test --test phase1_analyst_migration four_phase1_analysts_can_share_the_copilot_pool_without_cross_talk -- --nocapture
```

Expected: FAIL because the analyst modules still build raw tool vectors with `build_agent_with_tools(...)`.

- [ ] **Step 3: Update each analyst to build a concrete `AnalystToolBundle`**

Change each analyst only as far as needed:

- `fundamental.rs`: construct a fundamental bundle for `GetFundamentals` + `GetEarnings`
- `sentiment.rs`: choose `GetCachedNews` vs `GetNews` bundle variant from `cached_news`
- `news.rs`: choose cached/live news plus `GetMarketNews` and `GetEconomicIndicators`
- `technical.rs`: create one shared technical context spec and pass it through the bundle

All four should call the new builder:

```rust
let agent = build_agent_with_tool_bundle(&self.handle, &system_prompt, bundle);
```

- [ ] **Step 4: Re-run targeted analyst tests, then full verification**

Run:

```bash
cargo test --test phase1_analyst_migration -- --nocapture
cargo test --test copilot_mcp_session -- --nocapture
cargo test --test copilot_preflight -- --nocapture
cargo test
cargo clippy -- -D warnings
cargo fmt -- --check
```

Expected: all targeted Copilot regression tests PASS, full `cargo test` PASS, `cargo clippy -- -D warnings` PASS, and formatting check PASS.

- [ ] **Step 5: Commit the analyst migration and verified end state**

```bash
git add src/agents/analyst/fundamental.rs src/agents/analyst/sentiment.rs src/agents/analyst/news.rs src/agents/analyst/technical.rs src/agents/analyst/mod.rs tests/phase1_analyst_migration.rs
git commit -m "feat: enable phase1 analyst tools with copilot mcp bridge"
```
