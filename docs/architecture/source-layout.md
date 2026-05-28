# Source Layout

The repository is a Cargo workspace with **four active crates** under `crates/`:

- **`scorpio-core`** — shared runtime/domain logic (agents, workflow, providers, data clients, state, indicators, analysis packs, errors, observability, rate limiting, config, settings file boundary, async application facade).
- **`scorpio-cli`** — binary crate hosting the clap/inquire command surface, setup wizard, update notices, terminal banner, and dispatch for `analyze`, `report`, and `upgrade`. Depends on `scorpio-core` and `scorpio-reporters`.
- **`scorpio-reporters`** — shared output/reporting crate (reporter trait, reporter chain, terminal rendering, JSON artifacts). Depends on `scorpio-core`; consumed by `scorpio-cli`.
- **`scorpio-server`** — Loco-based HTTP/OpenAPI surface with environment-specific YAML config under `crates/scorpio-server/config/`.

Edition 2024 (Rust 1.93+).

```
crates/
├── scorpio-core/              # Shared runtime/domain crate (library, publish = false)
│   ├── Cargo.toml
│   ├── migrations/            # sqlx::migrate! resolves via scorpio-core's CARGO_MANIFEST_DIR
│   │   ├── 0001_create_phase_snapshots.sql
│   │   ├── 0002_add_symbol_and_schema_version.sql
│   │   └── transcript_cache/  # SEPARATE migrations dir; snapshot-store migrate! does NOT recurse here
│   └── src/
│       ├── lib.rs             # pub mod declarations + `pub use app::AnalysisRuntime`
│       ├── app/               # Application facade (AnalysisRuntime::new / ::run)
│       ├── settings.rs        # PartialConfig + atomic load/save (non-interactive)
│       ├── config.rs          # Runtime Config loader (env > user file > defaults)
│       ├── constants.rs       # Constants (HEALTH_CHECK_TIMEOUT_SECS, etc.)
│       ├── error.rs           # TradingError + RetryPolicy
│       ├── observability.rs   # Tracing/logging setup (used by every surface)
│       ├── rate_limit.rs      # Governor-based rate limiting
│       ├── agents/            # LLM agent implementations (analyst/researcher/trader/risk/fund_manager/shared)
│       ├── state/             # Shared pipeline state (TradingState + per-phase types)
│       ├── workflow/          # Graph orchestration (TradingPipeline, tasks, snapshot/**)
│       ├── data/              # Market data clients (finnhub/fred/yfinance/symbol/adapters)
│       ├── indicators/        # Technical indicators (kand-based)
│       ├── providers/         # LLM provider factory (rig-core, copilot ACP)
│       ├── analysis_packs/    # Pack manifests + runtime policy
│       └── backtest/          # Backtesting skeleton (core-internal per R13)
│
├── scorpio-cli/               # Binary crate: clap-based command surface
│   ├── Cargo.toml             # Depends on scorpio-core and scorpio-reporters
│   └── src/
│       ├── main.rs            # #[tokio::main] entry; dispatch analyze/report/setup/upgrade
│       ├── lib.rs             # pub mod cli; pub mod report; (library surface for in-crate tests)
│       └── cli/
│           ├── mod.rs         # Cli + Commands structs; clap derive
│           ├── analyze.rs     # Builds ReporterChain, runs AnalysisRuntime, dispatches reporters
│           ├── report.rs      # `scorpio report` — reads persisted snapshots, no API keys required
│           ├── update.rs      # Release check + `scorpio upgrade` self-update
│           └── setup/
│               ├── mod.rs     # Wizard orchestrator, recovery UX, run()
│               └── steps.rs   # Interactive step fns (1-5) + pure helpers
│
├── scorpio-reporters/         # Reporter trait + terminal/JSON implementations
│   └── src/                   # ReporterChain runs implementations concurrently via run_all
│
└── scorpio-server/            # Loco-based HTTP/OpenAPI surface
    ├── config/                # Environment-specific YAML (development/test/production)
    └── src/
        ├── app.rs             # Route registration; skips OpenapiInitializerWithSetup in Environment::Test
        └── controllers/
            └── health.rs      # Canonical endpoint pattern (#[utoipa::path] + #[debug_handler])
```

## Test Layout

- **Core integration tests** — `crates/scorpio-core/tests/` (pipeline, state, workflow, foundation, app facade).
- **CLI integration tests** — `crates/scorpio-cli/tests/` (release-archive contract only).
- **Reporter integration tests** — `crates/scorpio-reporters/tests/` (reporter chain, JSON artifact, terminal rendering).
- **Server integration tests** — `crates/scorpio-server/tests/` (`health` end-to-end).
- Test support modules live in `crates/scorpio-core/tests/support/` and are included via `#[path = "support/..."]`.

## Phased UI Roadmap

- **Phase 1 = CLI** (`clap` + `inquire`) — **done**; `scorpio analyze <SYMBOL>` runs the pipeline, `scorpio setup` is an interactive wizard that writes `~/.scorpio-analyst/config.toml`.
- **Phase 2 = interactive TUI** (`ratatui`/`crossterm`).
- **Phase 3 = native desktop app** (`gpui`, behind `--features gui`).

All phases depend on `scorpio-core` — the shared crate exposes `AnalysisRuntime`, `settings::PartialConfig`, and the runtime `Config` type as the preferred entry points.
