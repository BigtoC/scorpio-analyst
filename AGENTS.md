# AGENTS.md

Rust-native multi-agent LLM trading system. Single crate, no workspace. Edition 2024 (Rust 1.93+).

## Commands

```bash
cargo fmt -- --check          # CI step 1
cargo clippy --all-targets -- -D warnings   # CI step 2 (warnings = errors)
cargo nextest run --all-features --locked   # CI step 3 (NOT cargo test)
```

CI uses **nextest**, not `cargo test`. Run all three in order before claiming work is done.

Quick smoke run: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run`

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
4. `/ce:review` — multi-agent code review
5. `/ce:compound` — knowledge consolidation

### Coding Mode

Trigger: User explicitly says "write code" or uses , `/opsx:apply`, `/spec-code-developer` commands.
1. `/superpowers:test-driven-development` — TDD implementation
2. `/ce:review` — multi-agent code review
3. `/ce:compound` — knowledge consolidation

## Testing

- Integration tests in `tests/` require the `test-helpers` feature flag: `cargo nextest run --features test-helpers`
- CI runs `--all-features`, which includes `test-helpers`
- Integration tests use `tempfile` for SQLite snapshot databases -- no external services needed
- Test support modules live in `tests/support/` and are included via `#[path = "support/..."]`

## Configuration

Loading order (later overrides earlier):
1. `config.toml` -- checked-in defaults
2. `.env` via `dotenvy` -- local secrets (git-ignored)
3. Env vars with prefix `SCORPIO__` (double underscore for nesting: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=5`)

API keys use a flat `SCORPIO_` prefix (single underscore) -- see `.env.example`.

## Architecture gotchas

- **State passing**: Agents read/write typed fields on `TradingState` via `graph_flow::Context`, not chat buffers. Adding a new data field means updating `TradingState` and the relevant state module in `src/state/`.
- **Concurrency**: Per-field `Arc<RwLock<Option<T>>>` locking on `TradingState`. Never hold `std::sync::Mutex` across `.await` -- use `tokio::sync::RwLock`.
- **SQLite snapshots**: `migrations/0001_create_phase_snapshots.sql` is applied programmatically by `SnapshotStore::new`. No separate migration CLI step.
- **Custom Copilot provider**: `src/providers/copilot.rs` + `src/providers/acp.rs` implement a custom `rig` provider over JSON-RPC 2.0/NDJSON via `copilot --acp --stdio`.
- **Dual-tier models**: `ModelTier::QuickThinking` (analysts) vs `ModelTier::DeepThinking` (researchers, trader, risk, fund manager). Configured in `config.toml` under `[llm]`.

## Adding things

| Task             | Files to touch                                                                      |
|------------------|-------------------------------------------------------------------------------------|
| New agent        | `src/agents/<role>/`, `src/workflow/tasks/`                                         |
| New data source  | `src/data/`, expose via `#[tool]` macro                                             |
| New indicator    | `src/indicators/core_math.rs` + `src/indicators/tools.rs`                           |
| New LLM provider | Extend `ProviderId` in `src/providers/mod.rs`, add case in `src/providers/factory/` |

## Coding conventions

Detailed Rust conventions are in `.github/instructions/rust.instructions.md`. Non-obvious points:
- `lib.rs` allows `clippy::absurd_extreme_comparisons` globally
- Error handling: `thiserror` for `TradingError` variants, `anyhow` for context propagation within tasks
- Module refactoring: use Facade pattern in `mod.rs`, re-export only the public API. Split files >300 lines.
- All public types must derive `Debug`

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
