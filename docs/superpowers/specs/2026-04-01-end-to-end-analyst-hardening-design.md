# Design: End-to-end analyst hardening for OpenRouter and tool-call reliability

**Date:** 2026-04-01
**Status:** Approved

## Goal

Make the phase 1 analyst team complete reliably when backed by OpenRouter by fixing the observed provider, tool-schema, and technical-tool sequencing failures without widening scope beyond analyst execution.

## Why this design is needed

The current analyst path assumes all quick-thinking providers can safely use rig's typed prompt path with tool calling. That assumption does not hold for the observed OpenRouter run.

- All four analysts currently call `prompt_typed_with_retry::<...>()`.
- The failing run logged `Structured outputs currently not supported for OpenRouter`.
- `news` and `sentiment` also hit tool-call failures caused by argument-shape mismatches.
- `technical` hit a second `get_ohlcv` call that failed because the OHLCV context is write-once.
- Because all four analysts failed in the same fanout, the pipeline aborted before downstream phases.

This is not one isolated prompt issue. The failure cluster spans three boundaries:

1. provider compatibility for structured analyst output
2. Rust-side acceptance of model-generated tool arguments
3. technical-tool behavior after a valid first OHLCV fetch

The fix should harden those boundaries directly instead of relying only on prompt tweaks.

## Chosen approach

Add a provider-aware analyst response path that falls back to untyped JSON generation for OpenRouter, keep local Rust validation as the schema gate, make empty-object macro tools actually accept `{}`, make scoped duplicate `get_ohlcv` calls idempotent after the first successful fetch, and tighten analyst prompts to match exact tool signatures.

## Scope

This design includes:

- provider-aware analyst output handling for `fundamental`, `news`, `sentiment`, and `technical`
- local JSON parsing and validation for the OpenRouter analyst path
- macro tool argument acceptance for `get_market_news` and `get_economic_indicators`
- idempotent duplicate handling for repeated scoped `get_ohlcv` calls within one technical analysis cycle
- prompt updates that describe exact tool signatures and call sequencing
- regression tests for the provider fallback, macro tool argument shape, and technical duplicate-fetch behavior

This design does not include:

- trader, researcher, risk, or fund-manager prompt-path changes
- broad provider abstraction redesign outside the analyst path
- new tools or new analyst schema fields
- unrelated prompt tuning for downstream phases
- retries on permanent schema failures beyond existing retry-policy behavior

Provider selection rule for this pass:

- use an explicit `ProviderId::OpenRouter` check for the fallback path
- do not generalize to capability-detection in this change

## Root-cause summary

### 1. OpenRouter analyst typed prompts are not reliable

`src/providers/factory/agent.rs` currently dispatches `LlmAgentInner::OpenRouter` through the same `prompt_typed::<T>` path used by providers with working structured-output support. The failing run showed this assumption is invalid for OpenRouter in the current stack.

### 2. Empty-object macro tools advertise `{}` but deserialize as `()`

`GetMarketNews` and `GetEconomicIndicators` publish an object schema with no properties, but both use `type Args = ()`. In practice the model calls them with `{}`, which is consistent with the published schema, but the Rust side rejects that payload before the tool executes.

### 3. Technical analysis turns a harmless repeat into a hard failure

`GetOhlcv` is scoped to one symbol and date range and writes into `OhlcvToolContext`. After the first successful fetch, later identical calls fail with a schema violation because the context is write-once. In the observed run, this converted extra tool turns into a terminal error instead of allowing the already-fetched candles to be reused.

## Interface summary

| Unit | Responsibility | Input contract | Output contract |
|---|---|---|---|
| `run_analyst_inference` helper | Execute one analyst request using typed or fallback final-response parsing based on provider while preserving tool turns | `LlmAgent`, prompt, timeout, retry policy, max turns, parse hook, validate hook | validated analyst output plus usage details |
| `prompt_text_with_retry` tool-enabled response API | Run a prompt with tool turns and return final assistant text instead of typed output | `LlmAgent`, prompt, timeout, retry policy, max turns | final response text plus usage details |
| `parse_and_validate_*` helpers | Convert untyped JSON text into typed analyst structs | JSON string | typed analyst state or `TradingError::SchemaViolation` |
| `EmptyObjectArgs` | Accept explicit empty-object tool calls and reject unexpected properties | `{}` | zero-field Rust value |
| `OhlcvToolContext` | Store and serve analysis-scoped candles | one scoped candle set | cached candles for indicator tools |

## Architecture changes

### 1. Provider-aware analyst inference path

The fallback requires one explicit response API boundary.

Proposed contract:

- location: `src/providers/factory/retry.rs` with any minimal dispatch plumbing needed in `src/providers/factory/agent.rs`
- shape: `prompt_text_with_retry(agent, prompt, timeout, retry_policy, max_turns) -> RetryOutcome<TextPromptResponse>` or equivalent
- `TextPromptResponse` contains:
  - `text: String`
  - `usage: Option<CompletionUsage>` or the repo's equivalent existing usage payload type representing the provider's aggregate usage for the full multi-turn request

Behavioral rule:

- this API must preserve the same tool-enabled multi-turn execution model used by the current analyst path
- it changes only the final response representation from typed output to raw text

Each analyst currently owns the same structure:

1. build tools
2. build agent
3. call `prompt_typed_with_retry::<T>`
4. validate output
5. collect usage

This design keeps that structure but changes step 3 into a provider-aware helper.

Required behavior:

- For providers other than OpenRouter, preserve the current typed path.
- For OpenRouter, do not call `prompt_typed_with_retry::<T>`.
- Instead, call a tool-enabled untyped prompt path that preserves the existing multi-turn tool loop and `max_turns` budget, but returns the final assistant response as raw text plus usage metadata.
- Parse the returned text as JSON into the existing analyst state type.
- Reuse the existing per-analyst validation functions as the final schema gate.

This keeps provider-specific behavior at the prompt boundary while preserving the current typed analyst state models.

The fallback changes only the final-response parsing mode. It must not remove tool access, reduce tool-turn limits, or bypass the current retry ownership at the analyst prompt boundary.

The fallback prompt contract must remain narrow:

- instruct the model to return exactly one JSON object matching the analyst schema
- no markdown fences
- no prose before or after the object

The implementation should stay analyst-scoped rather than changing all typed-agent call sites. The trader and later phases still use typed prompt paths and are out of scope for this change.

### 2. Local JSON schema enforcement for OpenRouter analysts

The fallback path must not weaken validation.

For each analyst type:

- deserialize JSON into the existing state struct with strict unknown-field rejection, either by adding `deny_unknown_fields` to the analyst output structs or by enforcing an equivalent strict-deserialization boundary in the fallback parser
- map deserialization failures to `TradingError::SchemaViolation`
- run the existing semantic validator such as `validate_fundamental`, `validate_news`, `validate_sentiment`, or `validate_technical`

`run_analyst_inference` must accept explicit parser and validator hooks so its boundary is fully defined:

- parser hook: `fn(&str) -> Result<T, TradingError>` or equivalent closure/function item
- validator hook: `fn(&T) -> Result<(), TradingError>` or equivalent closure/function item

The helper lives in `src/agents/analyst/common.rs` unless implementation constraints force a smaller adjacent helper module. The helper owns provider selection, retry-loop ownership, timeout application, and usage extraction. The analyst module remains responsible for building tools, composing prompts, and providing the parser and validator for its output type.

This preserves the current contract that downstream workflow code receives fully typed and validated analyst outputs.

Usage metadata requirements for the fallback path:

- if the untyped provider call returns authoritative usage counts for the full multi-turn request, preserve them exactly
- if provider metadata is unavailable or partial, keep the existing `usage_from_response` behavior and record documented unavailable metadata rather than inventing counts
- latency measurement remains wall-clock based at the analyst layer, unchanged from the current path
- the fallback response API must therefore carry whatever usage payload the current untyped prompt path already exposes so the analyst layer can continue using existing accounting rules

### 3. Accept real empty-object tool calls

`GetMarketNews` and `GetEconomicIndicators` should accept the shape they already advertise.

Design:

- introduce a shared zero-field args type, for example `EmptyObjectArgs`
- derive `Deserialize` and `Serialize`
- reject unknown fields explicitly so Rust deserialization matches the published `additionalProperties: false` contract
- use it as `Args` for tools whose schema is an empty JSON object

Behavioral rule:

- `{}` is valid
- the canonical model-visible call shape remains `{}`
- any additional properties remain invalid because the schema still sets `additionalProperties: false`

This is the smallest fix that aligns runtime behavior with the published tool definition.

### 4. Make repeated identical scoped `get_ohlcv` calls idempotent

The technical path should tolerate redundant identical retrieval requests from the model after the first successful fetch.

Required behavior:

- the first successful scoped `get_ohlcv` call fetches candles and stores them in `OhlcvToolContext`
- a later identical scoped `get_ohlcv` call in the same analysis cycle returns the already-cached candles instead of failing
- a mismatched symbol or date range must still fail through the existing scope-validation path

Minimal implementation direction:

- keep scope validation in `GetOhlcv::call`
- check whether the context already contains candles before attempting a second store
- if candles already exist, return the cached candles directly

`src/data/yfinance.rs` is in scope because both `GetOhlcv` and `OhlcvToolContext` live there, and the idempotent-reuse rule belongs at that tool/context boundary rather than in analyst orchestration code.

This preserves the security boundary against adversarial overwrites while avoiding a terminal failure for harmless duplicate calls.

### 5. Prompt hardening for exact tool signatures

Prompt changes should support the boundary fixes rather than substitute for them.

Required prompt updates:

- `news` and `sentiment` must state that `get_news` requires `{"symbol":"<ticker>"}`
- `news` must state that `get_market_news` takes `{}`
- `news` must state that `get_economic_indicators` takes `{}`
- `technical` must state that `get_ohlcv` is scoped to the provided symbol and dates and should be called at most once
- `technical` must direct the model to use cached context via indicator tools after the initial fetch
- all analysts on the fallback path must be told to return exactly one JSON object and nothing else

These prompt changes are narrow and tied to observed failure modes.

## Implementation boundaries

### Units to change

- `src/agents/analyst/fundamental.rs`
- `src/agents/analyst/news.rs`
- `src/agents/analyst/sentiment.rs`
- `src/agents/analyst/technical.rs`
- shared analyst helper location if extraction is justified
- `src/providers/factory/agent.rs` and or `src/providers/factory/retry.rs` only as needed for analyst fallback plumbing
- `src/data/finnhub.rs`
- `src/data/yfinance.rs`

### Units not to change unless required by tests

- trader inference flow
- researcher and risk-agent prompt execution paths
- provider client construction
- workflow orchestration semantics beyond existing analyst failure handling

## Error handling

The hardened path should preserve current error categories.

Retry rule for this pass: keep existing retry-policy ownership and continue treating schema failures as terminal, so OpenRouter fallback JSON parse failures and semantic validation failures are not retried.

- invalid JSON from the OpenRouter fallback path becomes `TradingError::SchemaViolation`
- provider transport and timeout failures remain `TradingError::Rig` or `TradingError::NetworkTimeout`
- invalid tool arguments remain schema violations
- fallback JSON parse failures and semantic validation failures are treated as terminal schema violations for that attempt sequence and are not retried
- duplicate identical `get_ohlcv` calls no longer surface as schema violations
- mismatched duplicate `get_ohlcv` calls remain schema violations because scope checks still apply

## Testing strategy

Implementation must follow TDD.

Required regression coverage:

1. OpenRouter analyst fallback selection
   - fails first: analyst inference chooses a typed path for OpenRouter or cannot parse untyped JSON fallback output
   - passes after fix: OpenRouter analysts use local JSON parsing and validation

2. Empty-object macro tool args
   - fails first: `{}` sent to `get_market_news` or `get_economic_indicators` is rejected
   - passes after fix: `{}` deserializes and the tool call reaches the implementation layer

3. Technical duplicate OHLCV fetch
   - fails first: second identical scoped `get_ohlcv` call returns schema violation
   - passes after fix: second identical call reuses cached candles

4. Technical mismatched duplicate OHLCV fetch remains rejected
   - fails first: second scoped `get_ohlcv` call with different symbol or dates is rejected
   - passes after fix: it is still rejected with schema violation

5. Prompt contract checks
   - assert the updated prompts mention exact argument shapes and one-time OHLCV guidance where applicable

Test placement should stay close to the changed units unless an integration test is clearly necessary.

## Verification

Before claiming completion:

- run targeted Rust tests for the changed analyst and tool modules
- run at least one higher-level analyst workflow verification if a deterministic existing test path exists
- if practical in the local environment, re-run the failing `cargo run` scenario or a focused equivalent for OpenRouter-backed analysts

## Risks and mitigations

### Risk: OpenRouter fallback is too analyst-specific

Mitigation:

- keep the fallback behind an explicit provider check
- keep shared helper logic generic enough to reuse later if another provider needs the same path
- avoid changing non-analyst call sites in this pass

### Risk: untyped fallback weakens guarantees

Mitigation:

- require local `serde_json` parsing
- preserve existing semantic validation functions
- fail closed on any extra prose, malformed JSON, or invalid field values

### Risk: idempotent OHLCV reuse could mask bad requests

Mitigation:

- perform scope validation before reuse
- only reuse candles for identical scoped calls within the same context
- continue rejecting mismatched symbol or date arguments

## Success criteria

- OpenRouter-backed analysts no longer fail solely because structured outputs are unavailable
- empty-object macro tools accept the argument shape they publish
- redundant identical `get_ohlcv` calls no longer abort technical analysis
- analyst prompts explicitly match runtime tool signatures
- regression tests cover the fixed failure modes
