# Design for `add-project-foundation`

## Context

This foundation defines the stable contracts every later change will build on. It intentionally focuses on shared
state, configuration, errors, observability, rate limiting, testing conventions, and the module skeleton rather than on
provider implementations, agent logic, or workflow execution.

## Goals / Non-Goals

- **Goals**
  - Define `TradingState` and nested domain types comprehensively enough that downstream specs can reuse them without
    modifying foundation-owned shapes.
  - Standardize `Config`, `TradingError`, tracing, and rate-limiter contracts before provider and agent work begins.
  - Pre-declare the module tree and config artifacts needed for parallel downstream implementation.
- **Non-Goals**
  - Implementing `rig-core` providers, Copilot/ACP integration, or financial data clients.
  - Implementing `graph-flow` tasks, execution routing, persistence backends, or rollback behavior.
  - Defining CLI/TUI/GUI behavior or backtesting execution.

## Architectural Overview

This foundation explicitly defines the data structures and resilience layers detached from execution frameworks,
enforcing separation of concerns.

- `core-types` owns the stable data contracts in `src/state/`, including `TradingState`, debate and risk discussion
  representations, final execution status, and token accounting structures.
- `config` resolves its layered inputs at application boot, exposing immutable typed configuration to downstream
  factories and services.
- `error-handling` standardizes retry, timeout, degradation, and abort boundaries so downstream capabilities share a
  consistent failure model.
- `observability` defines structured tracing conventions for phase transitions, tool calls, and LLM invocations while
  preserving secret redaction.
- `rate-limiting` defines provider-scoped limiter injection points shared via `Arc` across concurrent tasks.
- The module skeleton is pre-declared so later changes can add localized files without needing to modify the root
  structure.

## Key Decisions

- `TradingState` SHALL be serializable and comprehensive for all five PRD phases, but execution mechanics that populate
  it are deferred.
- Debate and risk-history structures SHALL be defined at the foundation layer so researcher, trader, risk, and manager
  specs share the same audit model.
- Configuration SHALL include model selection, round limits, timeout settings, provider secrets, and provider quota
  inputs needed by later layers.
- Foundation tests SHALL focus on serde round-trips, error boundaries, and reusable mocking/test harness patterns,
  while full integration and backtesting tests are deferred.
