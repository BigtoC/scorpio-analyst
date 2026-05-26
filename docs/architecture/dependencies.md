# Crate Dependencies

| Crate                              | Purpose                                                                            |
|------------------------------------|------------------------------------------------------------------------------------|
| `rig-core` 0.32                    | LLM provider abstraction (OpenAI, Anthropic, Gemini, custom Copilot)               |
| `graph-flow` 0.5 (feature `"rig"`) | Stateful directed graph orchestration (LangGraph equivalent)                       |
| `schemars` 1                       | JSON schema generation for `#[tool]` macros                                        |
| `clap` 4 (feature `"derive"`)      | CLI argument parsing (`scorpio analyze <SYMBOL>`, `scorpio setup`)                 |
| `inquire` 0.9                      | Interactive setup wizard prompts (Password, Select, Confirm)                       |
| `toml` 1                           | Serialise `PartialConfig` to `~/.scorpio-analyst/config.toml`                      |
| `tempfile` 3                       | Atomic config writes (`NamedTempFile` + rename)                                    |
| `finnhub` 0.2                      | Corporate fundamentals, earnings, news, insider transactions                       |
| `yfinance-rs` 0.7                  | Historical OHLCV pricing data                                                      |
| `kand` 0.2                         | Technical indicators (RSI, MACD, ATR, Bollinger, SMA, EMA, VWMA) in pure Rust f64  |
| `tokio` 1 (full)                   | Async runtime                                                                      |
| `serde` / `serde_json`             | State serialization                                                                |
| `thiserror` 2 / `anyhow` 1         | Error handling (thiserror for typed domain errors, anyhow for context propagation) |
| `governor` 0.10                    | Global rate limiting (shared via `Arc` across concurrent agents)                   |
| `tracing` / `tracing-subscriber`   | Structured observability (json + env-filter features)                              |
| `secrecy` 0.10                     | API key management (zeroed on drop, excluded from Debug/logs)                      |
| `config` 0.15 / `dotenvy` 0.15     | TOML config loading + .env file support                                            |
| `reqwest` 0.13                     | HTTP client (json + query features)                                                |
| `sqlx` 0.8                         | SQLite for phase snapshot persistence                                              |
| `uuid` 1                           | Unique execution IDs (v4 + serde)                                                  |
| `chrono` 0.4                       | Date/time handling                                                                 |
| `async-trait` 0.1                  | Async trait support                                                                |
| `colored` 3 / `comfy-table` 7      | Human-readable output formatting                                                   |
| `figlet-rs` 1.0                    | ASCII art header                                                                   |
| `futures` 0.3                      | Async combinators                                                                  |
| `nonzero_ext` 0.3                  | Non-zero integer utilities                                                         |

**Dev dependencies:** `proptest` 1, `mockall` 0.13, `pretty_assertions` 1, `paft-money` 0.7, `rust_decimal` 1, `tempfile` 3, `flate2` 1, `tar` 0.4, `zip` 8.

## Build Prerequisite

Protobuf compiler (`protoc`) is required by transitive dependencies.

- CI installs it via `apt-get install protobuf-compiler`.
- macOS: `brew install protobuf`.

## Workspace Pinning

Shared dep versions are pinned centrally under `[workspace.dependencies]` in the root `Cargo.toml`; each crate consumes them via `foo.workspace = true`.

- **Core** owns the runtime dep set: rig-core, graph-flow, kand, finnhub, yfinance-rs, sqlx, secrecy, config, dotenvy, governor, schemars, nonzero_ext.
- **CLI** owns the presentation/binary-specific set: clap, inquire, colored, comfy-table, figlet-rs, self_update, semver, sha2, hex.
- **Reporters** consumes core plus presentation deps (colored, comfy-table) for terminal rendering.
- **Server** is a Loco app — its own dep set lives under `crates/scorpio-server/Cargo.toml`.
- **Dual-consumed** deps live as workspace entries: tokio, serde, serde_json, anyhow, thiserror, tracing, chrono, uuid, reqwest, async-trait, futures, tempfile.
