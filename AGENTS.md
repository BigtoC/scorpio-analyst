# AGENTS.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

## Project

Rust-native multi-agent LLM trading system. Cargo workspace with **four active crates** under `crates/`:

- `scorpio-core` — shared runtime/domain logic (agents, workflow, providers, data clients, state, indicators, packs).
- `scorpio-cli` — clap/inquire command surface; depends on `scorpio-core` and `scorpio-reporters`.
- `scorpio-reporters` — reporter trait + terminal/JSON rendering; depends on `scorpio-core`.
- `scorpio-server` — Loco-based HTTP/OpenAPI surface.

Edition 2024 (Rust 1.93+).

## Commands

```bash
cargo fmt -- --check                                                  # CI step 1
cargo clippy --workspace --all-targets -- -D warnings                 # CI step 2 (warnings = errors)
cargo nextest run --workspace --all-features --locked --no-fail-fast  # CI step 3 (NOT cargo test)
```

CI uses **nextest**, not `cargo test`. Run all three in order before claiming work is done.

Quick smoke run: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze AAPL`

Other useful focused loops: `cargo run -p scorpio-cli -- report list`, `cargo run -p scorpio-server -- start`

## Build prerequisite

Protobuf compiler (`protoc`) is required by transitive dependencies. CI installs it via `apt-get install protobuf-compiler`. On macOS: `brew install protobuf`.

## Architecture details

This file is intentionally short. Architecture, gotchas, dependency table, config order, dev-task map, testing, and CI all live in dedicated docs:

- [`docs/architecture/source-layout.md`](docs/architecture/source-layout.md) — 4-crate tree, test layout, phased UI roadmap
- [`docs/architecture/design-decisions.md`](docs/architecture/design-decisions.md) — crate boundary, Phase 0 preflight, dual-write, schema evolution, pack-owned prompts, transcript cache, reporter split, HTTP server pattern
- [`docs/architecture/dependencies.md`](docs/architecture/dependencies.md) — crate dependency table, protoc prerequisite, workspace pinning
- [`docs/architecture/config-and-errors.md`](docs/architecture/config-and-errors.md) — config loading order, storage paths, error-handling pattern
- [`docs/architecture/dev-tasks.md`](docs/architecture/dev-tasks.md) — running/debugging, CI verification, testing, common dev-task map, coding conventions

> **Before editing `TradingState`, analysis packs, workflow routing, or migration directories**, read `design-decisions.md` — it documents invariants (`#[serde(default)]`, schema version bumps, preflight ownership of policy/routing, transcript-cache migration boundary) that are easy to violate silently.

## Work Mode
> Based on the complexity of the tasks, choose the appropriate work mode

### Direct Execution Model (Default)

Trigger: bug fixes, small features, <30 line changes
Behavior: write code directly, do not invoke any skills

### Full Development Mode

Trigger: user explicitly says "full flow" or uses one of the `/full` command.
Behavior: follow this sequence strictly:
1. `/superpowers:brainstorming` — requirements exploration
2. `/ce-plan` — technical plan, auto-search `docs/solutions/`
3. `/superpowers:test-driven-development` — TDD implementation
4. `/ce-code-review` — multi-agent code review, code quality checks should also reference `.github/instructions/rust.instructions.md`.
5. `/ce-compound` — knowledge consolidation

### Coding Mode

Trigger: User explicitly says "write code" or uses `/opsx:apply` or `/spec-code-developer`.
1. `/superpowers:test-driven-development` — TDD implementation
2. `/ce-code-review` — multi-agent code review, code quality checks should also reference `.github/instructions/rust.instructions.md`.
3. `/ce-compound` — knowledge consolidation

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

- `CLAUDE.md` — sibling instruction file (same behavioral guidelines, references the same `docs/architecture/`).
- `.github/instructions/rust.instructions.md` — Rust coding conventions (auto-applied to `**/*.rs`).
- `README.md` — current execution graph, CLI usage, known limitations, and OpenSpec workflow shortcuts.
- `crates/scorpio-server/README.md` — server-specific build/run/config/OpenAPI conventions and the canonical endpoint wiring pattern.
