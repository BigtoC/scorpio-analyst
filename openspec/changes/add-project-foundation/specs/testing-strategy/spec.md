# `testing-strategy` Capability

## ADDED Requirements

### Requirement: Foundation Test Structure

The repository MUST establish a reusable testing structure for the foundation layer, including a `tests/` directory and
shared test helper patterns that later changes can extend without redefining common setup.

#### Scenario: Adding A Downstream Test Suite

When a later capability introduces new integration tests, it reuses the established test directory structure and helper
patterns instead of creating an incompatible parallel harness.

### Requirement: Property-Based Serialization Coverage

The system MUST integrate `proptest` for automated property-based testing of foundational serialization boundaries,
ensuring `TradingState`, token-usage structures, and related state/data models round-trip without panics or silent data
loss.

#### Scenario: Round-Tripping Trading State

A property test generator populates representative `TradingState` inputs, serializes them to JSON, deserializes them
back into the typed model, and verifies the required fields and token-usage structures remain intact.

### Requirement: Mocking Baselines

The repository MUST define `mockall`-based patterns for trait and provider mocking so downstream work can test
interfaces without invoking live APIs or model providers.

#### Scenario: Running Core Pipeline Local Tests

Developers running `cargo test` trigger local checks that substitute mock implementations for external dependencies,
avoiding networking while validating internal contracts.

### Requirement: Foundation Edge-Case Coverage

The foundation test suite MUST include focused coverage for secret redaction behavior and foundational error and timeout
edge cases.

#### Scenario: Validating Secret And Timeout Behavior

When tests exercise a redacted configuration value and a timeout-related `TradingError`, the assertions confirm secrets
never appear in debug or log-facing output and timeout handling surfaces deterministic typed failures.
