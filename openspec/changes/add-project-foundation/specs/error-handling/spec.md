# `error-handling` Capability

## ADDED Requirements

### Requirement: Unified Custom Error Enum

The core system MUST export a generalized, explicitly defined `TradingError` enum leveraging `thiserror` and preserving
context needed by downstream callers.

- `AnalystError` for analytical synthesis or task failure, including the responsible agent identity
- `RateLimitExceeded` for provider throttling boundaries, including the affected provider identity
- `NetworkTimeout` for external timeouts, including retry or timeout context
- `SchemaViolation` for serialization/deserialization mismatch during agent tool calling or LLM parsing execution
- `Rig` for lower-level errors originating from the underlying LLM processing dependency

#### Scenario: Processing API Failures

When an HTTP request drops during data ingestion against `Finnhub`, the engine maps the timeout into
`TradingError::NetworkTimeout` while preserving localized retry or provider context.

### Requirement: Retry And Backoff Protocols

To tolerate transient turbulence, functions relying on external connections MUST wrap execution steps in retry
capabilities governed by exponential backoff with a maximum of 3 retries and a base wait of 500ms.

#### Scenario: Intermittent LLM Failures

When an LLM endpoint returns a 503 response due to temporary congestion, the operation wrapper intercepts it as an
expected fault path, delaying via the configured backoff schedule before issuing an automated retry.

### Requirement: Configurable Timeout Boundaries

The foundation MUST define a per-analyst timeout contract with a default of 30 seconds, sourced from configuration and
applied consistently by downstream async execution layers.

#### Scenario: Analyst Exceeds Runtime Budget

When an analyst task exceeds the configured timeout budget, the task is terminated through the shared timeout contract
and surfaced as a typed timeout-related `TradingError` rather than hanging indefinitely.

### Requirement: Agent Graceful Degradation Rules

If repeated intermittent errors persist beyond tolerance protocols, analyst fan-out execution MUST adopt degradation
behaviors: a single subordinate analyst failure allows processing to continue with partial data, while 2 or more
analyst failures escalate automatically to an abort boundary.

#### Scenario: Analyzing Graceful Faults

When executing concurrent fan-out tasks across 4 analyst agents, if only the News Analyst fails entirely, the
aggregator resolves without it and forwards the remaining `TradingState` segment. If a second analyst also fails, the
cycle aborts with a structured `TradingError` instead of panicking.
