# Key Design Decisions

## Crate Boundary

`scorpio-core` owns the runtime/domain surface; `scorpio-reporters` owns reporter traits/rendering; `scorpio-cli` and `scorpio-server` are consumers. New contributors should prefer:

- `scorpio_core::app::AnalysisRuntime` and `scorpio_core::settings` as runtime entry points
- `scorpio_reporters::{Reporter, ReporterChain}` for output extensions
- `crates/scorpio-server/src/controllers/health.rs` + `crates/scorpio-server/src/app.rs` as the canonical HTTP endpoint pattern

Broader direct module imports remain available only where the extraction slice still needs them. **No module inside `scorpio-core` may depend on anything under `scorpio_cli`, `scorpio_reporters`, or `scorpio_server`.**

## State Management

All inter-agent data flows through a strongly-typed `TradingState` struct via `graph_flow::Context` — agents read/write specific struct fields, not free-text chat buffers. This eliminates the "telephone effect" where data degrades through natural language handoffs.

Adding a new data field means updating `TradingState` and the relevant state module in `crates/scorpio-core/src/state/`.

## Phase 0 Preflight

The graph starts at `PreflightTask` (`crates/scorpio-core/src/workflow/tasks/preflight.rs`), **not** analyst fan-out. It:

- Canonicalizes the symbol
- Loads prior thesis memory
- Resolves `analysis_pack` into `TradingState.analysis_runtime_policy`
- Seeds context keys such as `KEY_RESOLVED_INSTRUMENT`, `KEY_PROVIDER_CAPABILITIES`, `KEY_REQUIRED_COVERAGE_INPUTS`, `KEY_RUNTIME_POLICY`, `KEY_ROUTING_FLAGS`

`PreflightTask` is the **sole** writer of `state.analysis_runtime_policy`, the sole runner of `validate_active_pack_completeness`, and the sole writer of `KEY_RUNTIME_POLICY` / `KEY_ROUTING_FLAGS` to context.

## Phase 1 Dual-Write

`AnalystSyncTask` (`crates/scorpio-core/src/workflow/tasks/analyst.rs`) still fills legacy analyst fields (`fundamental_metrics`, `market_sentiment`, etc.) but also populates `evidence_*`, `data_coverage`, `provenance_summary`, and `derived_valuation`. Keep both paths in sync when changing analyst outputs; prefer typed evidence for new consumers.

## Dual-Tier LLM Routing

- **`ModelTier::QuickThinking`** (analysts) — gpt-4o-mini, claude-haiku, gemini-flash
- **`ModelTier::DeepThinking`** (researchers, trader, risk, fund manager) — o3, claude-opus, etc.

Configured in the runtime `[llm]` settings from the user config / env merge. `ProviderId` enum covers OpenAI, Anthropic, Gemini, Copilot, OpenRouter.

## Concurrency

- Fan-out tasks use `tokio::spawn`.
- Per-field `Arc<RwLock<Option<T>>>` locking on `TradingState` (not a single struct-level lock).
- **Never hold `std::sync::Mutex` across `.await`** — use `tokio::sync::RwLock`.

## Custom GitHub Copilot Provider

`crates/scorpio-core/src/providers/copilot.rs` + `crates/scorpio-core/src/providers/acp.rs` implement a custom `rig` provider via ACP (Agent Client Protocol) over JSON-RPC 2.0/NDJSON, spawning `copilot --acp --stdio`.

## Token Usage Tracking

Every LLM call records model ID, wall-clock latency, and provider-reported token counts into a `TokenUsageTracker` on `TradingState`. Providers that don't expose authoritative counts (e.g. Copilot via ACP) record documented unavailable metadata. Per-phase and per-agent breakdowns are displayed after every run.

## Phase Snapshots

Each pipeline phase persists its output to SQLite (`SnapshotStore`) for audit trail and recovery.

`SnapshotStore::new` / `from_config` runs `sqlx::migrate!()` over `crates/scorpio-core/migrations/` (currently `0001_create_phase_snapshots.sql` and `0002_add_symbol_and_schema_version.sql`). The directory resolves via `CARGO_MANIFEST_DIR` of `scorpio-core`, so the migrations move with the core crate.

## Transcript Cache

`TranscriptCacheStore` persists Alpha Vantage transcript results in a **dedicated** SQLite database at `~/.scorpio-analyst/transcript_cache.db` (overridable via `SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH`).

- Migrations live in `crates/scorpio-core/migrations/transcript_cache/` — a subdirectory that `SnapshotStore`'s `sqlx::migrate!()` does **not** recurse into.
- Only `TranscriptFetch::Found` results are cached; negative outcomes are re-fetchable.
- `AlphaVantageClient::cache_failure_count` (exposed in `Debug` impl) tracks **write** failures only — read-side failures emit sanitized `warn!` logs but do not bump the counter.

## TradingState Schema Evolution

`TradingState` is serialized into `phase_snapshots.trading_state_json`. Old snapshots may not deserialize with a newer struct. Rules:

- Every new field on `TradingState` **must** carry `#[serde(default)]`; omitting it makes all existing snapshots unreadable.
- When a field is **renamed**, **removed**, or has its **type changed** in a backward-incompatible way, bump `THESIS_MEMORY_SCHEMA_VERSION` in `crates/scorpio-core/src/workflow/snapshot/thesis.rs`. The thesis lookup skips rows whose version does not match the constant *in either direction* (newer or older), so bumping it explicitly retires incompatible data and a binary downgrade after the bump still ignores newer rows safely.
- The thesis lookup degrades gracefully (warn + skip) when deserialization fails. The `warn!` line emits only `symbol`, `schema_version`, and `error.kind = "deserialize"` — never `serde_json` error text, which can echo payload bytes. Relying on warn-and-skip for every deploy is still a smell; `#[serde(default)]` + version bumps are the real fix.
- Snapshotted state structs serialized into `phase_snapshots.trading_state_json` (anything reachable from `TradingState` via serde) must not use `#[serde(deny_unknown_fields)]` — it converts every additive field into a backward-incompatible change. This rule does NOT apply to RPC, tool-argument, or config types where typo detection is more valuable than forward-compat.

## Pack-Owned Prompts (Centralized)

`AnalysisPackManifest.prompt_bundle` is the single source of every system prompt for active packs. The runtime contract:

- Active packs must populate every required prompt slot for the configured topology (analysts, debate stage when `max_debate_rounds > 0`, risk stage when `max_risk_rounds > 0`, plus trader and fund manager). Failures surface as `TaskExecutionFailed` from preflight before any analyst or model task fires.
- Prompt builders take `&RuntimePolicy` directly; the renderer reads `policy.prompt_bundle.<role>` with no legacy fallback. The exhaustive `Role` → `PromptSlot` match in `workflow/topology.rs` makes adding a `Role` variant a compile error until the role-to-slot table is extended.
- `{analysis_emphasis}` substitution is sanitized at preflight (strict 0x20–0x7E ASCII, role-injection-tag rejection, ≤256 chars). `{ticker}` is not re-validated by this refactor — it continues to flow through the existing `validate_symbol` syntactic gate plus data-API existence chain.

## Topology-Driven Routing

`RoutingFlags` (written to `KEY_ROUTING_FLAGS` by preflight) governs *entry* into the debate and risk stages. Loop-back conditionals (`round < max`) keep using the per-iteration round counters. Tests that bypass preflight should hydrate runtime policy via `crate::testing::with_baseline_runtime_policy`.

## Reporter Split

`scorpio analyze` builds a `ReporterChain` in `crates/scorpio-cli/src/cli/analyze.rs`; terminal/JSON implementations live in `crates/scorpio-reporters/` and run concurrently via `ReporterChain::run_all`. Add new output legs by implementing `scorpio_reporters::Reporter` first, then wiring CLI selection.

`scorpio report` lives in `crates/scorpio-cli/src/cli/report.rs`, reads persisted snapshots through `SnapshotStore::from_runtime_storage()`, and does not require API keys.

## HTTP Server Surface

`scorpio-server` is a separate Loco app. Route registration lives in `crates/scorpio-server/src/app.rs`; documented handlers should follow `crates/scorpio-server/src/controllers/health.rs` (`#[utoipa::path]` + `#[debug_handler]` + `openapi(get(handler), routes!(handler))`).

`scorpio-server` skips `OpenapiInitializerWithSetup` in `Environment::Test` (`crates/scorpio-server/src/app.rs`) to avoid process-global route bleed across tests.
