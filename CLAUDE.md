# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

## Project Overview

Rust-native reimplementation of the [TradingAgents](https://github.com/TauricResearch/TradingAgents/) framework (originally Python/LangGraph). A multi-agent LLM-powered financial trading system that simulates a trading firm with specialized agent roles. Based on the paper [arXiv:2412.20138](https://arxiv.org/pdf/2412.20138).

The project is in early development — see PRD.md for the full specification.

## Build Commands

```bash
cargo build                                                           # Build the project
cargo run -p scorpio-cli -- --help                                    # Run the CLI binary
cargo nextest run --workspace --all-features --locked --no-fail-fast  # Run all tests (matches CI)
cargo clippy --workspace --all-targets -- -D warnings                 # Lint (warnings = errors)
cargo fmt -- --check                                                  # Check formatting
```

CI uses **nextest**, not `cargo test`. Requires **Rust 1.93+** (edition 2024). See [`docs/architecture/dev-tasks.md`](docs/architecture/dev-tasks.md) for run/debug commands and the `test-helpers` feature flag.

## Architecture (Summary)

The system follows a 5-phase execution pipeline orchestrated by `graph-flow`, with `rig-core` agents as the cognitive layer:

1. **Analyst Team** (parallel fan-out) — Fundamental, Sentiment, News, Technical analysts fetch and interpret market data concurrently
2. **Researcher Team** (cyclic debate) — Bullish vs. Bearish researchers argue in rounds, moderated by a Debate Moderator (`max_debate_rounds`)
3. **Trader Agent** (sequential) — Synthesizes debate into a structured `TradeProposal`
4. **Risk Management Team** (parallel fan-out + cyclic debate) — Aggressive, Conservative, Neutral risk agents debate, coordinated by a Risk Moderator (`max_risk_rounds`)
5. **Fund Manager** (sequential) — Final approve/reject decision, with deterministic fallback: reject if Conservative + Neutral risk agents both flag a violation

The repository is a Cargo workspace with **four active crates**: `scorpio-core` (shared runtime/domain library), `scorpio-cli` (binary), `scorpio-reporters` (reporter trait + terminal/JSON rendering), and `scorpio-server` (Loco-based HTTP/OpenAPI surface).

**For details, see:**
- [`docs/architecture/source-layout.md`](docs/architecture/source-layout.md) — directory tree, test layout, phased UI roadmap
- [`docs/architecture/design-decisions.md`](docs/architecture/design-decisions.md) — state management, schema evolution, pack-owned prompts, routing
- [`docs/architecture/dependencies.md`](docs/architecture/dependencies.md) — crate dependency table, workspace pinning
- [`docs/architecture/config-and-errors.md`](docs/architecture/config-and-errors.md) — config loading order, error handling pattern
- [`docs/architecture/dev-tasks.md`](docs/architecture/dev-tasks.md) — running, debugging, common dev task map, CI/CD

> **Before editing `TradingState`, analysis packs, or workflow routing**, read `design-decisions.md` — it documents invariants (`#[serde(default)]`, schema version bumps, preflight ownership of policy/routing) that are easy to violate silently.

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
- `/ce-plan` auto-searches `docs/solutions/` at planning time to surface relevant prior solutions before implementation begins
- Each solution document includes: problem description, root cause, fix applied, and tags for search

When to invoke `/ce-compound`:
- After a tricky bug is fixed (especially build/CI failures, async issues, borrow-checker patterns)
- After establishing a new architectural pattern or workflow convention
- After integrating a new dependency or provider that required non-obvious configuration

## Rust Guidelines

Detailed Rust coding conventions are in `.github/instructions/rust.instructions.md`. Key points:
- Prefer borrowing (`&T`) over cloning; use `&str` over `String` for function params when ownership isn't needed.
- Use `serde` for serialization, `thiserror`/`anyhow` for errors.
- Async code uses `tokio` runtime with `async/await`.
- Implement common traits (`Debug`, `Clone`, `PartialEq`) on public types.
- Use enums over flags/booleans for type safety.
- Warnings are treated as errors in CI (`-D warnings`).
