# Add Project Foundation

## Motivation

To reimplement the TradingAgents multi-agent trading system in Rust, we must establish a robust foundation. This
proposal initializes the shared contracts that every downstream capability depends on: core state and domain types,
configuration management, error handling patterns, observability conventions, coordinated rate limiting, testing
infrastructure, and the project module skeleton. By defining these first, subsequent provider, data, agent,
orchestration, and interface layers can be built in parallel without risking code conflicts or circular dependencies.

## Goals

- Establish the core domain terminology and shared data types across the application, especially `TradingState` and its
  nested structs, so downstream specs do not need to redefine foundational state.
- Define a unified `TradingError` handling mechanism to standardize retry, timeout, degradation, and abort behavior.
- Set up a secure, tiered configuration loader integrating `config.toml`, `.env`, and environment variables.
- Standardize observability using `tracing` for phase transitions, tool calls, LLM invocations, and secret redaction.
- Introduce coordinated rate-limiting primitives shared across concurrent tasks.
- Standardize foundation-level testing using `mockall` and `proptest`, including serde round-trip coverage.
- Pre-declare the module skeleton and config artifacts needed for downstream implementation changes.

## Scope

- **In Scope:** Foundational types and traits, module skeleton files, configuration structures and artifacts
  (`config.toml`, `.env.example`), error structures and resilience rules, observability conventions, coordinated
  rate-limiting structures, and testing standards.
- **Out of Scope / Deferred to later changes:** LLM provider implementations, Copilot/ACP integration, financial data
  clients, agent prompts and behaviors, `graph-flow` workflow orchestration, persistence backends, CLI/TUI/GUI
  behavior, and backtesting execution.

## Capabilities Changed

- **`core-types` (Added):** Specifies the full foundational domain structures and serialization contracts used by
  downstream capabilities.
- **`error-handling` (Added):** Specifies typed error structures, retry logic, timeout rules, and fault-tolerance
  policies.
- **`config` (Added):** Specifies multi-layer configuration loading, validation, defaults, and secure secret handling.
- **`observability` (Added):** Specifies structured tracing conventions and redaction requirements.
- **`rate-limiting` (Added):** Specifies shared `governor`-based rate-limiting structures and provider quota contracts.
- **`testing-strategy` (Added):** Specifies mock, property-based, and serde round-trip testing expectations for the
  foundation.

## Impact

- **Affected specs:** `core-types`, `error-handling`, `config`, `observability`, `rate-limiting`, `testing-strategy`
- **Affected code:** `Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/config.rs`, `src/state/*`, `src/rate_limit.rs`,
  `config.toml`, `.env.example`, `tests/`
- **Downstream benefit:** Later changes can assume stable `TradingState`, `TradingError`, config fields, tracing
  conventions, and rate-limiter injection points without reopening foundation docs.
