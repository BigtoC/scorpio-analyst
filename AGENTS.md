# AGENTS.md

Rust-native multi-agent LLM trading system. Cargo workspace with two active crates under `crates/`:

- `scorpio-core` — shared runtime/domain logic (agents, workflow, providers, data clients, state, indicators, analysis packs, errors, observability, rate limiting, config, settings file boundary, async application facade).
- `scorpio-cli` — binary crate hosting the clap/inquire command surface, setup wizard, update notices, terminal banner, and terminal report formatting. Depends on `scorpio-core`.

Edition 2024 (Rust 1.93+).

## Commands

```bash
cargo fmt -- --check                                         # CI step 1
cargo clippy --workspace --all-targets -- -D warnings        # CI step 2 (warnings = errors)
cargo nextest run --workspace --all-features --locked --no-fail-fast   # CI step 3 (NOT cargo test)
```

CI uses **nextest**, not `cargo test`. Run all three in order before claiming work is done.

Quick smoke run: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze AAPL`

## Build prerequisite

Protobuf compiler (`protoc`) is required by transitive dependencies. CI installs it via `apt-get install protobuf-compiler`. On macOS: `brew install protobuf`.

## Work Mode
> Based on the complexity of the tasks, choose the appropriate work mode

### Direct Execution Model (Default)

Trigger: bug fixes, small features, <30 line changes
Behavior: write code directly, do not invoke any skills

### Full Development Mode

Trigger: user explicitly says "full flow" or uses one of the `/full` command.
Behavior: follow this sequence strictly:
1. `/superpowers:brainstorming` — requirements exploration
2. `/ce:plan` — technical plan, auto-search `docs/solutions/`
3. `/superpowers:test-driven-development` — TDD implementation
4. `/ce:review` — multi-agent code review, code quality checks should also reference `.github/instructions/rust.instructions.md`.
5. `/ce:compound` — knowledge consolidation

### Coding Mode

Trigger: User explicitly says "write code" or uses `/opsx:apply` or `/spec-code-developer`.
1. `/superpowers:test-driven-development` — TDD implementation
2. `/ce:review` — multi-agent code review, code quality checks should also reference `.github/instructions/rust.instructions.md`.
3. `/ce:compound` — knowledge consolidation

## Testing

- Core integration tests live in `crates/scorpio-core/tests/` (pipeline, state, workflow, foundation, app facade); CLI integration tests live in `crates/scorpio-cli/tests/` (release-archive contract only).
- Integration tests require the `test-helpers` feature flag: `cargo nextest run --workspace --features test-helpers`. The feature's canonical home is `scorpio-core`; `scorpio-cli` declares `test-helpers = ["scorpio-core/test-helpers"]` as a forwarder so `cargo test -p scorpio-cli --all-features` still enables the gated helpers.
- CI runs `--workspace --all-features`, which includes `test-helpers`.
- Integration tests use `tempfile` for SQLite snapshot databases -- no external services needed.
- Test support modules live in `crates/scorpio-core/tests/support/` and are included via `#[path = "support/..."]`.

## Configuration

Loading order (later overrides earlier):
1. `~/.scorpio-analyst/config.toml` -- user-level config written by `scorpio setup`
2. `.env` via `dotenvy` -- local secrets (git-ignored)
3. Env vars with prefix `SCORPIO__` (double underscore for nesting: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=5`)
4. Flat API-key env vars (`SCORPIO_OPENAI_API_KEY`, `SCORPIO_FINNHUB_API_KEY`, etc.) -- override matching secrets from the user config file

Repo-root `config.toml` is deprecated/inert and is not read at runtime.

API keys use a flat `SCORPIO_` prefix (single underscore) -- see `.env.example`. The asset symbol is a CLI argument to `scorpio analyze <SYMBOL>`, not a config key.

## Architecture gotchas

- **Crate boundary**: `scorpio-core` owns the runtime/domain surface; `scorpio-cli` is a consumer. New contributors should prefer `scorpio_core::app::AnalysisRuntime` and `scorpio_core::settings` as entry points; broader direct module imports remain available only where the extraction slice still needs them. No module inside `scorpio-core` may depend on anything under `scorpio_cli`.
- **State passing**: Agents read/write typed fields on `TradingState` via `graph_flow::Context`, not chat buffers. Adding a new data field means updating `TradingState` and the relevant state module in `crates/scorpio-core/src/state/`.
- **Phase 0 preflight**: The graph starts at `PreflightTask` (`crates/scorpio-core/src/workflow/tasks/preflight.rs`), not analyst fan-out. It canonicalizes the symbol, loads prior thesis memory, resolves `analysis_pack` into `TradingState.analysis_runtime_policy`, and seeds context keys such as `KEY_RESOLVED_INSTRUMENT`, `KEY_PROVIDER_CAPABILITIES`, `KEY_REQUIRED_COVERAGE_INPUTS`, and `KEY_RUNTIME_POLICY`.
- **Concurrency**: Per-field `Arc<RwLock<Option<T>>>` locking on `TradingState`. Never hold `std::sync::Mutex` across `.await` -- use `tokio::sync::RwLock`.
- **Phase 1 dual-write**: `AnalystSyncTask` (`crates/scorpio-core/src/workflow/tasks/analyst.rs`) still fills legacy analyst fields (`fundamental_metrics`, `market_sentiment`, etc.) but also populates `evidence_*`, `data_coverage`, `provenance_summary`, and `derived_valuation`. Keep both paths in sync when changing analyst outputs; prefer typed evidence for new consumers.
- **SQLite snapshots**: `SnapshotStore::new` / `from_config` runs `sqlx::migrate!()` over `crates/scorpio-core/migrations/` (currently including `0001_create_phase_snapshots.sql` and `0002_add_symbol_and_schema_version.sql`). The directory resolves via `CARGO_MANIFEST_DIR` of `scorpio-core`, so the migrations move with the core crate.
- **TradingState schema evolution**: `TradingState` is serialized into `phase_snapshots.trading_state_json`. Every new
  field **must** have `#[serde(default)]` or existing snapshots will fail to deserialize. When a field is renamed,
  removed, or its type changes incompatibly, bump `THESIS_MEMORY_SCHEMA_VERSION` in
  `crates/scorpio-core/src/workflow/snapshot/thesis.rs` — this explicitly retires old rows rather than silently skipping them at runtime.
- **Custom Copilot provider**: `crates/scorpio-core/src/providers/copilot.rs` + `crates/scorpio-core/src/providers/acp.rs` implement a custom `rig` provider over JSON-RPC 2.0/NDJSON via `copilot --acp --stdio`.
- **Dual-tier models**: `ModelTier::QuickThinking` (analysts) vs `ModelTier::DeepThinking` (researchers, trader, risk, fund manager). Configured in the runtime `[llm]` settings from the user config / env merge.

## Adding things

| Task                  | Files to touch                                                                                                                                                                                                 |
|-----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| New agent             | `crates/scorpio-core/src/agents/<role>/`, `crates/scorpio-core/src/workflow/tasks/`                                                                                                                            |
| New data source       | `crates/scorpio-core/src/data/`, expose via `#[tool]` macro                                                                                                                                                    |
| New indicator         | `crates/scorpio-core/src/indicators/core_math.rs` + `crates/scorpio-core/src/indicators/tools.rs`                                                                                                              |
| New LLM provider      | Extend `ProviderId` in `crates/scorpio-core/src/providers/mod.rs`, add case in `crates/scorpio-core/src/providers/factory/`                                                                                    |
| New analysis pack     | Add `PackId` variant in `crates/scorpio-core/src/analysis_packs/manifest/pack_id.rs`, add match arm in `crates/scorpio-core/src/analysis_packs/builtin.rs`                                                     |
| New CLI subcommand    | Add variant to `Commands` in `crates/scorpio-cli/src/cli/mod.rs`, create `crates/scorpio-cli/src/cli/<name>.rs`, dispatch in `crates/scorpio-cli/src/main.rs`                                                  |
| New wizard config key | Add field to `PartialConfig` in `crates/scorpio-core/src/settings.rs`, add step in `crates/scorpio-cli/src/cli/setup/steps.rs`, inject in `Config::load_from_user_path` in `crates/scorpio-core/src/config.rs` |

## Coding conventions

Detailed Rust conventions are in `.github/instructions/rust.instructions.md`. Non-obvious points:
- `crates/scorpio-core/src/lib.rs` allows `clippy::absurd_extreme_comparisons` globally
- Error handling: `thiserror` for `TradingError` variants, `anyhow` for context propagation within tasks
- Module refactoring: use Facade pattern in `mod.rs`, re-export only the public API. Split files mixing multiple concerns or exceeding ~500 lines.
- All public types must derive `Debug`
- Performance optimization: prioritize `O`-complexity before micro-optimizing. Use pre-allocation (`with_capacity`) and avoid unnecessary cloning.
- Eliminate unnecessary wrapper functions that simply call another function without adding logic.

## Knowledge Consolidation

After resolving a non-trivial problem, run `/ce:compound` to persist the solution for future reference.

- `docs/solutions/` — documented solved problems (bug fixes, best practices, workflow patterns), organized by category
- `/ce:plan` auto-searches `docs/solutions/` at planning time to surface relevant prior solutions before implementation begins
- Each solution document includes: problem description, root cause, fix applied, and tags for search

When to invoke `/ce:compound`:
- After a tricky bug is fixed (especially build/CI failures, async issues, borrow-checker patterns)
- After establishing a new architectural pattern or workflow convention
- After integrating a new dependency or provider that required non-obvious configuration

## Other instruction files

- `CLAUDE.md` -- comprehensive project context (architecture, dependencies, design decisions)
- `.github/instructions/rust.instructions.md` -- Rust coding conventions (auto-applied to `**/*.rs`)
- `README.md` -- current execution graph, CLI usage, known limitations, and OpenSpec workflow shortcuts
