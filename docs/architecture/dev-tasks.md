# Development Tasks

## Running & Debugging

```bash
cargo run -p scorpio-cli -- setup                     # Interactive wizard → ~/.scorpio-analyst/config.toml
cargo run -p scorpio-cli -- analyze AAPL              # Run pipeline for AAPL
cargo run -p scorpio-cli -- analyze --help            # Show analyze flags
RUST_LOG=debug cargo run -p scorpio-cli -- analyze AAPL                  # Full trace output
SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze AAPL   # Quick smoke run (1 debate round)
cargo run -p scorpio-cli -- report list               # List persisted snapshot reports (no API keys needed)
cargo run -p scorpio-server -- start                  # Start the Loco HTTP server
cargo run -p scorpio-cli -- --version                 # Print version
```

Use `cargo run -p <crate> -- …` from the repo root to target a specific crate.

## CI Verification (Required Before Claiming Done)

```bash
cargo fmt -- --check                                                  # CI step 1
cargo clippy --workspace --all-targets -- -D warnings                 # CI step 2 (warnings = errors)
cargo nextest run --workspace --all-features --locked --no-fail-fast  # CI step 3 (NOT cargo test)
```

CI uses **nextest**, not `cargo test`. Run all three in order before claiming work is done.

## Testing

- Integration tests require the `test-helpers` feature flag: `cargo nextest run --workspace --features test-helpers`.
- The feature's canonical home is `scorpio-core`; `scorpio-cli` declares `test-helpers = ["scorpio-core/test-helpers"]` as a forwarder so `cargo test -p scorpio-cli --all-features` still enables the gated helpers.
- CI runs `--workspace --all-features`, which includes `test-helpers`.
- Integration tests use `tempfile` for SQLite snapshot databases — no external services needed.
- Test support modules live in `crates/scorpio-core/tests/support/` and are included via `#[path = "support/..."]`.

## Common Development Tasks

| Task                  | Files to touch                                                                                                                                                                                                                  |
|-----------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| New agent             | `crates/scorpio-core/src/agents/<role>/`, `crates/scorpio-core/src/workflow/tasks/`                                                                                                                                             |
| New data source       | `crates/scorpio-core/src/data/`, expose via `#[tool]` macro                                                                                                                                                                     |
| New indicator         | `crates/scorpio-core/src/indicators/core_math.rs` + `crates/scorpio-core/src/indicators/tools.rs`                                                                                                                               |
| New LLM provider      | Extend `ProviderId` in `crates/scorpio-core/src/providers/mod.rs`, add case in `crates/scorpio-core/src/providers/factory/`                                                                                                     |
| New analysis pack     | Add `PackId` variant in `crates/scorpio-core/src/analysis_packs/manifest/pack_id.rs`, add match arm in `crates/scorpio-core/src/analysis_packs/builtin.rs`                                                                      |
| New CLI subcommand    | Add variant to `Commands` in `crates/scorpio-cli/src/cli/mod.rs`, create `crates/scorpio-cli/src/cli/<name>.rs`, dispatch in `crates/scorpio-cli/src/main.rs`                                                                   |
| New reporter          | Implement `scorpio_reporters::Reporter` in `crates/scorpio-reporters/src/`, wire selection in `crates/scorpio-cli/src/cli/analyze.rs`, add tests in `crates/scorpio-reporters/tests/`                                           |
| New HTTP endpoint     | Create `crates/scorpio-server/src/controllers/<name>.rs`, re-export in `crates/scorpio-server/src/controllers/mod.rs`, wire its `routes()` into `crates/scorpio-server/src/app.rs`, add `crates/scorpio-server/tests/<name>.rs` |
| New wizard config key | Add field to `PartialConfig` in `crates/scorpio-core/src/settings.rs`, add step in `crates/scorpio-cli/src/cli/setup/steps.rs`, inject in `Config::load_from_user_path` in `crates/scorpio-core/src/config.rs`                  |

## Coding Conventions

Detailed Rust conventions are in `.github/instructions/rust.instructions.md`. Non-obvious points:

- `crates/scorpio-core/src/lib.rs` allows `clippy::absurd_extreme_comparisons` globally.
- Error handling: `thiserror` for `TradingError` variants, `anyhow` for context propagation within tasks.
- Module refactoring: use Facade pattern in `mod.rs`, re-export only the public API. Split files mixing multiple concerns or exceeding ~500 lines.
- All public types must derive `Debug`.
- Performance optimization: prioritize `O`-complexity before micro-optimizing. Use pre-allocation (`with_capacity`) and avoid unnecessary cloning.
- Eliminate unnecessary wrapper functions that simply call another function without adding logic.

## CI/CD

GitHub Actions (`.github/workflows/tests.yml`):

- Triggers on push/PR to `main` (only when `crates/**`, `.cargo/**`, `Cargo.toml`, or `Cargo.lock` change, plus the workflow file itself).
- Installs Protobuf compiler (required by dependencies).
- Steps: `cargo fmt -- --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo nextest run --workspace --all-features --locked --no-fail-fast`.
