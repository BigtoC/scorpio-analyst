# Tasks for `add-project-foundation`

## 1. Foundation scaffolding

- [x] 1.1 Update `Cargo.toml` with the dependencies required for the foundation layer (`serde`, `serde_json`,
      `thiserror`, `anyhow`, `tokio`, `tracing`, `tracing-subscriber`, `governor`, `secrecy`, `dotenvy`, `config`,
      `mockall`, `proptest`, and related support crates).
- [x] 1.2 Create `src/lib.rs` with the full module skeleton needed by downstream changes (for example `agents`,
      `state`, `error`, `providers`, `data`, `config`, `rate_limit`, `indicators`, `workflow`, `cli`).
- [x] 1.3 Create directories and empty `mod.rs` stubs so downstream specs can proceed in parallel without changing the
      root module tree.
- [x] 1.4 Add foundation configuration artifacts: checked-in `config.toml` defaults and a redacted `.env.example`.

## 2. Core types (`core-types`)

- [x] 2.1 Implement foundational state structs in `src/state/*`, including `TradingState`, `FundamentalData`,
      `TechnicalData`, `SentimentData`, `NewsData`, `TradeProposal`, `RiskReport`, `ExecutionStatus`, debate/risk
      history types, and `TokenUsageTracker` with nested usage structs.
- [x] 2.2 Ensure all foundational state types support serialization/deserialization for JSON snapshotting and downstream
      reuse.
- [x] 2.3 Add serde round-trip tests covering `TradingState` and token usage structures.

## 3. Configuration (`config`)

- [x] 3.1 Define configuration domain structs including `Config`, `LLMConfig`, `TradingConfig`, and `ApiConfig` with
      fields for model selection, round limits, timeouts, provider credentials, and provider quota inputs.
- [x] 3.2 Implement layered config loading from `config.toml` → `.env` via `dotenvy` → environment variables.
- [x] 3.3 Wrap sensitive fields in `secrecy::SecretString` and ensure startup validation fails fast on missing required
      settings.

## 4. Error handling (`error-handling`)

- [x] 4.1 Implement `TradingError` in `src/error.rs` with typed variants for analyst failures, rate limiting, network
      timeouts, schema violations, and `rig`-originated errors.
- [x] 4.2 Implement retry, timeout, and helper error mapping utilities aligned with the shared foundation contract.
- [x] 4.3 Encode graceful degradation rules for analyst fan-out failures (1 failure continues, 2+ failures abort).

## 5. Observability (`observability`)

- [x] 5.1 Implement `tracing-subscriber` initialization for structured JSON logging.
- [x] 5.2 Define span/log conventions for phase transitions, tool calls, and LLM invocations.
- [x] 5.3 Ensure secret-bearing values are redacted from logs and debug output.

## 6. Rate limiting (`rate-limiting`)

- [x] 6.1 Define provider-scoped rate-limiter wrappers leveraging `governor` and sharing limiters via `Arc`.
- [x] 6.2 Support configuration-driven instantiation with per-provider quotas, including Finnhub's default 30 req/s.
- [x] 6.3 Expose dependency-injection-friendly limiter access for downstream data and agent tasks.

## 7. Testing strategy (`testing-strategy`)

- [x] 7.1 Establish the testing directory structure (`tests/`) and reusable test helpers for downstream work.
- [x] 7.2 Configure `proptest` coverage for foundational state/data serialization boundaries.
- [x] 7.3 Define `mockall`-based mocking patterns for provider and service traits owned by later changes.
- [x] 7.4 Add focused tests for secret redaction and foundational error/timeout edge cases.

### Cross-Owner Touch-points

- Approved for `add-analyst-team` to update `src/state/token_usage.rs`, `src/state/trading_state.rs`, and
  `src/config.rs` to align token-accounting semantics, analyst timeout naming, and analyst fan-out state writes with
  the `analyst-team` spec.
