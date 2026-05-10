# Agent Output Schemas Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce strict, locally validated structured output at LLM agent boundaries. On text paths, each migrated agent response is parsed from raw JSON, checked against a runtime `JsonSchema` contract (`additionalProperties: false`, max lengths, array bounds, numeric ranges), then projected into snapshot-safe state. On typed-provider paths, the plan re-validates the typed value locally for bounds/ranges and keeps provider parsing as the raw unknown-field gate unless raw provider payloads become available.

**Architecture:** Three layers.
1. **Snapshot-safe state types stay backward compatible.** Only reusable nested types gain `JsonSchema` derives or bounds when an envelope embeds them directly.
2. **Per-agent envelope types** carry `#[serde(deny_unknown_fields)]` and agent-facing bounds. The envelope is the LLM contract; the persisted state type remains backward-compatible.
3. **A shared contract helper** reuses `agents::shared::extract_json_object`, applies a raw payload size guard, validates a `serde_json::Value` against `schemars::schema_for!(T)` with a local runtime JSON Schema validator, then deserializes into the envelope. Provider-native typed output is only a generation aid; local re-validation still runs on the typed value, but raw unknown-field enforcement stays provider-side unless the raw payload is exposed.

**Tech Stack:** Rust, `schemars` 1, `serde_json`, `rig-core` 0.36 (existing), one new runtime JSON Schema validation crate wired into `scorpio-core`.

---

## Decision Summary

| Question                     | Answer                                                                                                                                                                                                                                                                         |
|------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **Effort**                   | Medium (~4–6 days). Most of the work is mechanical (5 type derives + ~12 envelopes + per-agent wiring).                                                                                                                                                                        |
| **Risk**                     | Low–Medium. Risk concentrated in: (a) stricter contracts rejecting outputs that currently limp through; (b) brownfield integration with existing retry/validation helpers; (c) adding a runtime validator dependency. Mitigation in the plan.                                  |
| **Data source dependencies** | **NONE.** This is a cross-cutting validation/contract change. ✓                                                                                                                                                                                                                |
| **Schema migrations**        | **NONE.** All envelope types are NEW; existing snapshot-reachable types only gain `#[derive(JsonSchema)]` (Serde-compatible no-op).                                                                                                                                            |
| **Provider compatibility**   | Reuse the repo's existing brownfield split instead of inventing a new capability layer: typed path where `prompt_typed_with_retry` already works, Gemini text fallback on schema failure, OpenRouter/DeepSeek text-with-corrective-feedback, Copilot post-hoc text validation. |
| **Highest-leverage payoff**  | Trader, risk, and fund-manager outputs gain deterministic local validation first. Analysts then reuse the same helper. Researchers stay shape-compatible with today's plain-text downstream contract instead of introducing a parallel debate model.                           |

**Recommendation:** Worth doing, but make it brownfield-first. Do **shared validator + existing JSON extraction hardening → one analyst slice → trader/risk/fund-manager → remaining analysts → minimal researcher envelopes**. Do not introduce a new provider-capability abstraction in the first pass.

---

## Why envelopes? Why not just tighten state types?

Per `CLAUDE.md`:

> Snapshotted state structs serialized into `phase_snapshots.trading_state_json` (anything reachable from `TradingState` via serde) must not use `#[serde(deny_unknown_fields)]` — it converts every additive field into a backward-incompatible change.

So the strict schema (what the LLM is held to) must live on a **separate envelope type** that is NOT reachable from `TradingState`. The envelope is constructed at agent-call time, validated locally, and its data is copied into the snapshot-safe state type.

For agents whose downstream contract is already plain text (Bull/Bear debate turns, Debate Moderator summary, Risk Moderator synthesis), the first pass should use a **minimal envelope that preserves that existing text artifact**. This plan is about strict validation at the seam, not redesigning downstream consumers.

This is the same separation `anthropics/financial-services` uses: subagent `output_schema` in YAML is strict; the eventual artifact written to disk is loose markdown/Excel.

---

## File Structure

### Files to create

| Path                                                              | Responsibility                                                                                                       |
|-------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/agents/shared/contracts/mod.rs`          | `pub trait OutputContract` + shared runtime schema-validation helper.                                                |
| `crates/scorpio-core/src/agents/shared/contracts/analyst.rs`      | Envelope types: `FundamentalAnalystOutput`, `SentimentAnalystOutput`, `NewsAnalystOutput`; `TechnicalAnalystResponse` stays local and adopts `OutputContract`. |
| `crates/scorpio-core/src/agents/shared/contracts/researcher.rs`   | Minimal envelopes that preserve today's Bull/Bear/Moderator plain-text artifacts.                                   |
| `crates/scorpio-core/src/agents/shared/contracts/risk.rs`         | Envelopes for Aggressive/Conservative/Neutral risk JSON responses and the Risk Moderator's plain-text synthesis wrapper. |
| `crates/scorpio-core/src/agents/shared/contracts/tests.rs`        | Crate-private contract tests for malformed inputs (oversize array/string, unknown field, bad range).               |

### Files to modify

| Path                                                 | Change                                                                                         |
|------------------------------------------------------|------------------------------------------------------------------------------------------------|
| `Cargo.toml`                                         | Add one runtime JSON Schema validator to `[workspace.dependencies]`.                           |
| `crates/scorpio-core/Cargo.toml`                     | Opt `scorpio-core` into the new runtime validator dependency.                                  |
| `crates/scorpio-core/src/agents/shared/json.rs`      | Harden `extract_json_object` instead of introducing a parallel extractor.                      |
| `crates/scorpio-core/src/agents/shared/mod.rs`       | Re-export the new contracts module alongside the existing JSON helper.                         |
| `crates/scorpio-core/src/agents/analyst/equity/common.rs` | Reuse the existing typed/text retry split while running local contract validation on both paths. |
| `crates/scorpio-core/src/state/risk.rs`              | Add `#[derive(JsonSchema)]` to `RiskReport`, `RiskLevel`.                                      |
| `crates/scorpio-core/src/state/execution.rs`         | Add `#[derive(JsonSchema)]` to `ExecutionStatus`, `Decision`.                                  |
| `crates/scorpio-core/src/state/thesis.rs`            | Add `#[derive(JsonSchema)]` to `ThesisMemory`.                                                 |
| `crates/scorpio-core/src/state/news.rs`              | Add bounds only to nested reusable state types that envelopes embed directly.                  |
| `crates/scorpio-core/src/state/sentiment.rs`         | Add bounds only to nested reusable state types that envelopes embed directly.                  |
| `crates/scorpio-core/src/state/fundamental.rs`       | Add bounds only to nested reusable state types that envelopes embed directly.                  |
| `crates/scorpio-core/src/state/technical.rs`         | Add bounds only to nested reusable state types that envelopes embed directly.                  |
| `crates/scorpio-core/src/agents/analyst/equity/*.rs` | Replace ad-hoc parse closures with envelope-based validation via `run_analyst_inference`.      |
| `crates/scorpio-core/src/agents/researcher/*.rs`     | Wrap output in minimal `BullishResearcherOutput` / `BearishResearcherOutput` / `DebateModeratorOutput` envelopes. |
| `crates/scorpio-core/src/agents/trader/schema.rs`    | Promote `TraderProposalResponse` into an `OutputContract` with stricter bounds.                |
| `crates/scorpio-core/src/agents/trader/mod.rs`       | Re-run local contract validation after typed extraction, then keep existing trader validators. |
| `crates/scorpio-core/src/agents/risk/*.rs`           | Replace direct `RiskReport` parsing with risk envelopes while preserving current text validators and redaction. |
| `crates/scorpio-core/src/agents/fund_manager/validation.rs` | Promote `ExecutionStatusResponse` into an `OutputContract` and preserve existing semantic validation. |
| `crates/scorpio-core/src/agents/fund_manager/agent.rs` | Keep runtime timestamp overwrite and current validation flow after contract parse.              |

---

## Envelope Pattern (canonical example)

```rust
// crates/scorpio-core/src/agents/shared/contracts/risk.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::state::risk::{RiskLevel, RiskReport};

/// What the LLM emits when running the Conservative Risk role.
/// Strict schema — rejects unknown fields. Copied into `RiskReport` after validation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConservativeRiskOutput {
    pub risk_level: RiskLevel,

    #[schemars(length(max = 2048))]
    pub assessment: String,

    #[schemars(length(max = 10))]
    pub recommended_adjustments: Vec<String>,

    pub flags_violation: bool,
}

impl ConservativeRiskOutput {
    pub fn into_state(self) -> RiskReport {
        RiskReport {
            risk_level: self.risk_level,
            assessment: self.assessment,
            recommended_adjustments: self.recommended_adjustments,
            flags_violation: self.flags_violation,
        }
    }
}
```

The same shape applies to each JSON-producing agent: a strict `*Output` type with `deny_unknown_fields` + bounded fields, plus an `into_state()` method when the persisted state type differs. For plain-text downstream artifacts (researcher turns, moderator summaries), use a minimal strict wrapper around the existing string contract instead of redesigning the stored artifact.

---

## OutputContract trait

```rust
// crates/scorpio-core/src/agents/shared/contracts/mod.rs
pub mod analyst;
pub mod researcher;
pub mod risk;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::TradingError;
use crate::agents::shared::extract_json_object;

pub trait OutputContract: DeserializeOwned + Serialize + JsonSchema {
    const NAME: &'static str;

    /// Parse the raw model output into `serde_json::Value`, validate it against the
    /// locally generated schema, then deserialize it into the strict contract type.
    fn parse(raw: &str) -> Result<Self, TradingError> {
        let payload = extract_json_object(Self::NAME, raw)?;
        let value: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
            TradingError::SchemaViolation {
                message: format!("{}: failed to parse JSON: {e}", Self::NAME),
            }
        })?;

        validate_value::<Self>(&value)?;

        serde_json::from_value(value).map_err(|e| TradingError::SchemaViolation {
            message: format!("{}: {e}", Self::NAME),
        })
    }

    fn validate(value: &Self) -> Result<(), TradingError> {
        let as_value = serde_json::to_value(value).map_err(|e| TradingError::SchemaViolation {
            message: format!("{}: failed to serialize for validation: {e}", Self::NAME),
        })?;
        validate_value::<Self>(&as_value)
    }
}
```

`validate_value::<T>` is the new shared helper that converts `schemars::schema_for!(T)` into the runtime validator crate's schema type, validates a `serde_json::Value`, and maps failures into `TradingError::SchemaViolation { message }`. Add a shared raw-payload length limit before `serde_json::from_str` so oversized responses fail before full JSON materialization.

---

## Phased Task Breakdown

### Task 1: Implement local runtime schema validation on top of the existing JSON extractor

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/scorpio-core/Cargo.toml`
- Create: `crates/scorpio-core/src/agents/shared/contracts/mod.rs`
- Modify: `crates/scorpio-core/src/agents/shared/json.rs`
- Modify: `crates/scorpio-core/src/agents/shared/mod.rs`

- [ ] **Step 1: Add a runtime JSON Schema validator dependency**

In workspace `Cargo.toml`, add a dependency such as:

```toml
jsonschema = "0.26"
```

Then opt `scorpio-core` into it:

```toml
# crates/scorpio-core/Cargo.toml
[dependencies]
jsonschema.workspace = true
```

- [ ] **Step 2: Extend tests for `extract_json_object` in the existing file**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_from_fenced_json_block() {
        let raw = "Here you go:\n```json\n{\"a\":1}\n```\nDone.";
        assert_eq!(extract_json_object("test", raw).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn extracts_from_bare_object_when_no_fence() {
        let raw = "Some prose then {\"a\":1, \"b\":2} trailing text";
        assert_eq!(extract_json_object("test", raw).unwrap(), r#"{"a":1, "b":2}"#);
    }

    #[test]
    fn rejects_multiple_json_objects_in_prose() {
        let raw = r#"prefix {"a":1} middle {"b":2}"#;
        let err = extract_json_object("test", raw).unwrap_err();
        assert!(format!("{err}").contains("multiple"));
    }

    #[test]
    fn handles_nested_braces_inside_strings() {
        let raw = r#"```json
{"message":"literal { brace } text","nested":{"ok":true}}
```"#;
        let extracted = extract_json_object("test", raw).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&extracted).is_ok());
    }
}
```

- [ ] **Step 3: Run failing tests**

Run: `cargo test -p scorpio-core rejects_multiple_json_objects_in_prose -- --nocapture`
Expected: existing tests still pass; new multiple-object test fails until the helper is hardened.

- [ ] **Step 4: Harden `extract_json_object` instead of creating `extract_json_block`**

Keep the current fast path and code-fence path. Replace the `find('{')` + `rfind('}')` fallback with a single-object scanner that tracks brace depth, string state, and escapes. If it finds more than one top-level JSON object candidate, fail closed with `TradingError::SchemaViolation { message: "... multiple JSON objects ..." }` rather than taking the first/last brace span.

- [ ] **Step 5: Implement `OutputContract` with local schema validation**

In `crates/scorpio-core/src/agents/shared/contracts/mod.rs`:

1. Add `validate_value::<T>(&serde_json::Value)`.
2. Generate `schemars::schema_for!(T)`.
3. Compile that schema with the runtime validator crate.
4. Return a `TradingError::SchemaViolation { message }` that includes the contract name and the validator error.
5. Make `OutputContract::parse` call `extract_json_object`, parse to `serde_json::Value`, run `validate_value`, then deserialize.
6. Make `OutputContract::validate(&self)` re-run the same local validation on typed-path results.

- [ ] **Step 6: Add a contract meta-test that proves `schemars` bounds are really enforced**

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SmokeContract {
    #[schemars(range(min = 0, max = 10))]
    value: i64,
}

impl OutputContract for SmokeContract {
    const NAME: &'static str = "smoke";
}

#[test]
fn parse_rejects_unknown_field() {
    let raw = r#"```json
{"value": 1, "extra": "should fail"}
```"#;
    let err = SmokeContract::parse(raw).unwrap_err();
    assert!(format!("{err}").contains("unknown field"));
}

#[test]
fn parse_rejects_out_of_range_value() {
    let raw = r#"```json
{"value": 99}
```"#;
    let err = SmokeContract::parse(raw).unwrap_err();
    assert!(format!("{err}").contains("value"));
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p scorpio-core agents::shared::contracts`
Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/scorpio-core/Cargo.toml crates/scorpio-core/src/agents/shared/json.rs crates/scorpio-core/src/agents/shared/contracts/ crates/scorpio-core/src/agents/shared/mod.rs
git commit -m "feat(agents): add local OutputContract schema validation"
```

---

### Task 2: Add `JsonSchema` derive to risk/execution/thesis state types

**Files:**
- Modify: `crates/scorpio-core/src/state/risk.rs`
- Modify: `crates/scorpio-core/src/state/execution.rs`
- Modify: `crates/scorpio-core/src/state/thesis.rs`

- [ ] **Step 1: Write a meta-test**

In `crates/scorpio-core/tests/state_roundtrip.rs`:

```rust
#[test]
fn risk_report_has_jsonschema() {
    fn assert_has_schema<T: schemars::JsonSchema>() {}
    assert_has_schema::<scorpio_core::state::risk::RiskReport>();
    assert_has_schema::<scorpio_core::state::risk::RiskLevel>();
    assert_has_schema::<scorpio_core::state::execution::ExecutionStatus>();
    assert_has_schema::<scorpio_core::state::execution::Decision>();
    assert_has_schema::<scorpio_core::state::thesis::ThesisMemory>();
}
```

- [ ] **Step 2: Run failing test**

Run: `cargo test -p scorpio-core --test state_roundtrip risk_report_has_jsonschema --no-run`
Expected: compile error (`JsonSchema` not implemented).

- [ ] **Step 3: Add the derives**

Across the three files containing these five types, add `JsonSchema` to the existing derive list and `use schemars::JsonSchema;` at the top.

`crates/scorpio-core/src/state/risk.rs`:
```rust
use schemars::JsonSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RiskLevel { Aggressive, Neutral, Conservative }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RiskReport { /* unchanged fields */ }
```

`crates/scorpio-core/src/state/execution.rs`: same pattern for `Decision` and `ExecutionStatus`.

`crates/scorpio-core/src/state/thesis.rs`: same for `ThesisMemory`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scorpio-core --test state_roundtrip`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/state/risk.rs crates/scorpio-core/src/state/execution.rs crates/scorpio-core/src/state/thesis.rs
git commit -m "feat(state): derive JsonSchema for RiskReport, ExecutionStatus, ThesisMemory"
```

---

### Task 3: Add `#[schemars(...)]` bounds only to reusable nested state types

**Files:**
- Modify: `crates/scorpio-core/src/state/news.rs`
- Modify: `crates/scorpio-core/src/state/sentiment.rs`
- Modify: `crates/scorpio-core/src/state/fundamental.rs`
- Modify: `crates/scorpio-core/src/state/technical.rs`

These bounds **do not** add `deny_unknown_fields` (would break snapshots). Only add bounds where a shared nested state type is reused directly inside a strict envelope. Keep top-level agent-specific list caps and summary caps on the envelope, not on persisted state structs.

- [ ] **Step 1: Write failing schema-shape tests**

In `crates/scorpio-core/tests/state_roundtrip.rs`:

```rust
#[test]
fn news_article_title_is_bounded() {
    let schema = schemars::schema_for!(scorpio_core::state::news::NewsArticle);
    let json = serde_json::to_value(&schema).unwrap();
    let title_max = json.pointer("/properties/title/maxLength")
        .and_then(|v| v.as_u64())
        .expect("title must have maxLength set");
    assert!(title_max <= 256, "title maxLength should be ≤ 256, got {title_max}");
}

```

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p scorpio-core --test state_roundtrip news_article_title_is_bounded`
Expected: FAIL — bounds not present.

- [ ] **Step 3: Add bounds**

`crates/scorpio-core/src/state/news.rs`:
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NewsArticle {
    #[schemars(length(max = 256))]
    pub title: String,
    #[schemars(length(max = 128))]
    pub source: String,
    #[schemars(length(max = 64))]
    pub published_at: String,
    #[schemars(length(max = 1024))]
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 512))]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MacroEvent {
    #[schemars(length(max = 256))]
    pub event: String,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub confidence: f64,
}
```

Apply analogous nested-type bounds to:

- `sentiment.rs`: `SentimentSource.source_name`, `score`, `EngagementPeak.platform`, `timestamp`
- `fundamental.rs`: `InsiderTransaction.name`, `transaction_date`
- `technical.rs`: `TechnicalOptionsContext::FetchFailed.reason`

Do **not** push envelope-only array caps or summary caps into `NewsData`, `SentimentData`, `FundamentalData`, or `TechnicalData`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scorpio-core --test state_roundtrip`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/state/
git commit -m "feat(state): add schemars bounds to nested contract types"
```

---

### Task 4: Build envelopes for analyst outputs

**Files:**
- Create: `crates/scorpio-core/src/agents/shared/contracts/analyst.rs`

- [ ] **Step 1: Write the three shared analyst envelope types**

```rust
// crates/scorpio-core/src/agents/shared/contracts/analyst.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::state::fundamental::{FundamentalData, InsiderTransaction};
use crate::state::news::{MacroEvent, NewsArticle, NewsData};
use crate::state::sentiment::{EngagementPeak, SentimentData, SentimentSource};

use super::OutputContract;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FundamentalAnalystOutput {
    pub revenue_growth_pct: Option<f64>,
    pub pe_ratio: Option<f64>,
    pub eps: Option<f64>,
    pub current_ratio: Option<f64>,
    pub debt_to_equity: Option<f64>,
    pub gross_margin: Option<f64>,
    pub net_income: Option<f64>,
    #[schemars(length(max = 30))]
    pub insider_transactions: Vec<InsiderTransaction>,
    #[schemars(length(max = 4096))]
    pub summary: String,
}

impl OutputContract for FundamentalAnalystOutput {
    const NAME: &'static str = "fundamental_analyst";
}

impl FundamentalAnalystOutput {
    pub fn into_state(self) -> FundamentalData {
        FundamentalData {
            revenue_growth_pct: self.revenue_growth_pct,
            pe_ratio: self.pe_ratio,
            eps: self.eps,
            current_ratio: self.current_ratio,
            debt_to_equity: self.debt_to_equity,
            gross_margin: self.gross_margin,
            net_income: self.net_income,
            insider_transactions: self.insider_transactions,
            summary: self.summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SentimentAnalystOutput {
    #[schemars(range(min = -1.0, max = 1.0))]
    pub overall_score: f64,
    #[schemars(length(max = 8))]
    pub source_breakdown: Vec<SentimentSource>,
    #[schemars(length(max = 12))]
    pub engagement_peaks: Vec<EngagementPeak>,
    #[schemars(length(max = 4096))]
    pub summary: String,
}

impl OutputContract for SentimentAnalystOutput {
    const NAME: &'static str = "sentiment_analyst";
}

impl SentimentAnalystOutput {
    pub fn into_state(self) -> SentimentData {
        SentimentData {
            overall_score: self.overall_score,
            source_breakdown: self.source_breakdown,
            engagement_peaks: self.engagement_peaks,
            summary: self.summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewsAnalystOutput {
    #[schemars(length(max = 50))]
    pub articles: Vec<NewsArticle>,
    #[schemars(length(max = 20))]
    pub macro_events: Vec<MacroEvent>,
    #[schemars(length(max = 4096))]
    pub summary: String,
}

impl OutputContract for NewsAnalystOutput {
    const NAME: &'static str = "news_analyst";
}

impl NewsAnalystOutput {
    pub fn into_state(self) -> NewsData {
        NewsData {
            articles: self.articles,
            macro_events: self.macro_events,
            summary: self.summary,
        }
    }
}

// Technical stays in `agents/analyst/equity/technical.rs` because `TechnicalAnalystResponse`
// already exists there and merges runtime-owned `options_context` before persistence.
```

- [ ] **Step 2: Add validation tests**

In `crates/scorpio-core/src/agents/shared/contracts/tests.rs` (or inline `#[cfg(test)]` modules inside the contract files):

```rust
use scorpio_core::agents::shared::contracts::OutputContract;
use scorpio_core::agents::shared::contracts::analyst::FundamentalAnalystOutput;

#[test]
fn fundamental_envelope_rejects_unknown_field() {
    let raw = r#"```json
{
    "revenue_growth_pct": 12.5,
    "pe_ratio": 28.0,
    "eps": 5.0,
    "current_ratio": 1.2,
    "debt_to_equity": 0.5,
    "gross_margin": 0.42,
    "net_income": 1000000.0,
    "insider_transactions": [],
    "summary": "ok",
    "evil_extra_field": "injected"
}
```"#;
    let err = FundamentalAnalystOutput::parse(raw).unwrap_err();
    assert!(format!("{err}").contains("unknown field"));
}

#[test]
fn fundamental_envelope_accepts_minimal_valid_payload() {
    let raw = r#"```json
{
    "revenue_growth_pct": null, "pe_ratio": null, "eps": null,
    "current_ratio": null, "debt_to_equity": null, "gross_margin": null,
    "net_income": null, "insider_transactions": [], "summary": "ok"
}
```"#;
    let out = FundamentalAnalystOutput::parse(raw).unwrap();
    assert_eq!(out.summary, "ok");
}
```

(Add analogous tests for `SentimentAnalystOutput` and `NewsAnalystOutput`. For Technical, add tests in `agents/analyst/equity/technical.rs` against `TechnicalAnalystResponse` so the plan reuses the existing local contract instead of duplicating it.)

- [ ] **Step 3: Run tests**

Run: `cargo test -p scorpio-core contracts::`
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-core/src/agents/shared/contracts/
git commit -m "feat(contracts): add analyst output envelopes"
```

---

### Task 5: Wire one analyst (Fundamental) through the contract

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/equity/fundamental.rs` (path inferred from recon)

This task is the proof-of-concept and the first shippable checkpoint. Stop after this task, run the full repo verification sequence, and confirm the retry/error behavior is acceptable before migrating more agents.

- [ ] **Step 1: Find the current parsing site**

Locate where the agent currently parses LLM output (likely a `serde_json::from_str::<FundamentalData>` call somewhere in the agent's `run()` method).

- [ ] **Step 2: Replace the parse hook, not the whole inference path**

```rust
use crate::agents::shared::contracts::OutputContract;
use crate::agents::shared::contracts::analyst::FundamentalAnalystOutput;

pub(crate) fn parse_fundamental(json_str: &str) -> Result<FundamentalData, TradingError> {
    let envelope = FundamentalAnalystOutput::parse(json_str)?;
    Ok(envelope.into_state())
}
```

Do **not** replace `run_analyst_inference(...)`. The existing provider routing, Gemini fallback, and corrective-feedback retry loop in `agents/analyst/equity/common.rs` stay authoritative.

- [ ] **Step 3: Update or add the existing per-analyst test**

Find the Fundamental analyst's existing tests and add both layers:

```rust
#[test]
fn parse_fundamental_rejects_unknown_field() {
    let raw = r#"{"revenue_growth_pct":1.0,"pe_ratio":2.0,"eps":3.0,"current_ratio":1.0,"debt_to_equity":0.2,"gross_margin":0.4,"net_income":1.0,"insider_transactions":[],"summary":"ok","evil_extra":1}"#;
    let err = parse_fundamental(raw).unwrap_err();
    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}
```

Then add one `run_analyst_inference`-level test proving the text fallback retries after a schema violation message.

- [ ] **Step 4: Pull analyst typed-path local validation into this checkpoint**

In `crates/scorpio-core/src/agents/analyst/equity/common.rs`, update the typed path in `run_analyst_inference` so the first checkpoint validates both provider branches:

```rust
validate(&outcome.result.output)?;
OutputContract::validate(&outcome.result.output)?;
```

Add one typed-path unit test proving a deserializable-but-out-of-bounds value is rejected before the analyst output is accepted.

- [ ] **Step 5: Run tests, commit**

```bash
cargo fmt -- --check
cargo clippy -p scorpio-core --all-targets -- -D warnings
cargo nextest run -p scorpio-core --all-features --locked
git commit -m "feat(agents): route Fundamental analyst through OutputContract envelope"
```

---

### Task 6: After Task 5 is green, tighten existing trader, risk, and fund-manager contracts without replacing their semantic validators

**Files:**
- Create: `crates/scorpio-core/src/agents/shared/contracts/risk.rs`
- Modify: `crates/scorpio-core/src/agents/trader/schema.rs`
- Modify: `crates/scorpio-core/src/agents/fund_manager/validation.rs`

- [ ] **Step 1: Promote `TraderProposalResponse` to an `OutputContract`**

```rust
// crates/scorpio-core/src/agents/trader/schema.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::agents::shared::contracts::OutputContract;
use crate::state::{TradeAction, TradeProposal};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TraderProposalResponse {
    pub action: TradeAction,
    pub target_price: f64,
    pub stop_loss: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub confidence: f64,
    #[schemars(length(max = 4096))]
    pub rationale: String,
    #[serde(default)]
    #[schemars(length(max = 2048))]
    pub valuation_assessment: Option<String>,
}

impl OutputContract for TraderProposalResponse {
    const NAME: &'static str = "trader";
}
```

- [ ] **Step 2: Create risk envelopes that still map directly into `RiskReport`**

Replicate the `ConservativeRiskOutput` pattern for Aggressive and Neutral. Keep `recommended_adjustments: Vec<String>` so the envelope projects into the current `RiskReport` without inventing a lossy intermediate format.

Preserve the existing per-role `risk_level` invariant. Either omit the model-authored `risk_level` and stamp it in `into_state()`, or keep the field and add an explicit equality check in each risk agent after parse.

For `RiskModerator`, do **not** replace the current plain-text artifact with a structured summary object. Add a minimal wrapper:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RiskModeratorOutput {
    #[schemars(length(min = 1, max = 4096))]
    pub content: String,
}
```

and continue to run `validate_moderator_output_shape(...)` plus `prepend_violation_status_if_missing(...)` afterward.

- [ ] **Step 3: Promote `ExecutionStatusResponse` into an `OutputContract` in-place**

```rust
// crates/scorpio-core/src/agents/fund_manager/validation.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::agents::shared::contracts::OutputContract;
use crate::state::execution::{Decision, ExecutionStatus};
use crate::state::proposal::TradeAction;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecutionStatusResponse {
    pub decision: Decision,
    pub action: TradeAction,
    #[schemars(length(max = 4096))]
    pub rationale: String,
    /// Optional ISO 8601 timestamp from the model; the runtime remains authoritative
    /// and overwrites this field after parsing.
    #[serde(default)]
    pub decided_at: Option<String>,
    #[serde(default)]
    #[schemars(length(max = 2048))]
    pub entry_guidance: Option<String>,
    #[serde(default)]
    #[schemars(length(max = 1024))]
    pub suggested_position: Option<String>,
}

impl OutputContract for ExecutionStatusResponse {
    const NAME: &'static str = "fund_manager";
}

impl ExecutionStatusResponse {
    pub fn into_state(self) -> ExecutionStatus {
        ExecutionStatus {
            decision: self.decision,
            action: self.action,
            rationale: self.rationale,
            decided_at: self.decided_at.unwrap_or_default(),
            entry_guidance: self.entry_guidance,
            suggested_position: self.suggested_position,
        }
    }
}
```

Keep the current semantic checks after parsing:

- missing-data acknowledgment
- dual-risk first-line prefix rules
- same-direction rejection prohibition
- runtime `decided_at` overwrite in `fund_manager/agent.rs` (the model field is advisory only)

- [ ] **Step 4: Validation tests**

For each migrated contract, write at least 2 tests: rejects unknown field, rejects an out-of-bounds field. Keep the current trader/risk/fund-manager domain tests that verify semantic behavior beyond schema shape.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/agents/shared/contracts/ crates/scorpio-core/src/agents/trader/schema.rs crates/scorpio-core/src/agents/fund_manager/validation.rs
git commit -m "feat(contracts): harden trader, risk, and fund manager outputs"
```

---

### Task 7: After Task 6 is green, extend local validation across the remaining typed/text routing surfaces

**Files:**
- Modify: `crates/scorpio-core/src/agents/trader/mod.rs`
- Modify: `crates/scorpio-core/src/agents/fund_manager/agent.rs` (only if a typed path is proven necessary)
- Modify: `crates/scorpio-core/src/agents/shared/contracts/mod.rs`

The repo already has the brownfield routing we need. This task is not "invent `supports_structured_output()`"; it is "make every migrated path apply the strongest local validation available on that path." Text paths validate raw JSON locally. Typed paths locally re-check bounds/ranges on typed values while still relying on provider parsing for dropped unknown fields unless raw provider payloads become available.

- [ ] **Step 1: Tighten trader typed-path validation**

After `prompt_typed_with_retry::<TraderProposalResponse>` succeeds in `crates/scorpio-core/src/agents/trader/mod.rs`, run:

```rust
TraderProposalResponse::validate(&outcome.result.output)?;
```

before converting into `TradeProposal` and before the existing trader semantic validators.

- [ ] **Step 2: Keep Fund Manager on the validated text path unless a real typed need appears**

`FundManagerAgent` already uses `prompt_with_retry_validated_details` and a validator closure, which preserves corrective-feedback retries. Keep that path in the first pass. If a typed path is introduced later, it must preserve the same retry semantics and still run the local contract validator.

- [ ] **Step 3: Add tests proving the remaining paths enforce local validation**

Add targeted tests:

- trader: typed path rejects a locally invalid typed output before `into()`
- shared contract parse: oversized raw payload is rejected before `serde_json::from_str`
- fund manager: validated text path still retries on schema violation via the existing validator closure

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-core/src/agents/trader/mod.rs crates/scorpio-core/src/agents/shared/contracts/mod.rs
git commit -m "feat(agents): extend local validation across remaining routing paths"
```

---

### Task 8: After Task 7 is green, roll out remaining non-researcher agents in value order

Roll out in this order:

1. Trader
2. Aggressive / Conservative / Neutral risk agents
3. Fund Manager
4. Sentiment / News / Technical analysts
5. Stop. Researchers and Risk Moderator are the final slice in Task 9.

- [ ] **Step 1: For each agent, migrate the local parse boundary only**

Patterns by area:

- Analysts: keep `run_analyst_inference`; swap only the parse hook.
- Trader: keep `prompt_typed_with_retry::<TraderProposalResponse>`; add `TraderProposalResponse::validate(&typed_output)?` before converting to `TradeProposal`, then keep `validate_trade_proposal(...)`.
- Risk agents: replace `serde_json::from_str::<RiskReport>(&cleaned)` with `<RiskEnvelope>::parse(&output)?.into_state()`, then keep `validate_raw_model_output_size`, `validate_risk_text`, and redaction.
- Fund Manager: replace raw struct parse with `ExecutionStatusResponse::parse(raw)?.into_state()`, then keep the current semantic validation path.
- Do **not** wire Bull / Bear / Debate Moderator / Risk Moderator in this task.

- [ ] **Step 2: One commit per agent**

This makes review easier and lets us roll back individual agents if a provider has compatibility issues:

```bash
git commit -m "feat(agents): route Sentiment analyst through OutputContract envelope"
# … one per agent
```

---

### Task 9: Final slice — add minimal researcher envelopes and wire Bull/Bear/Debate Moderator/Risk Moderator last

**Files:**
- Create: `crates/scorpio-core/src/agents/shared/contracts/researcher.rs`
- Modify: `crates/scorpio-core/src/agents/researcher/bullish.rs`
- Modify: `crates/scorpio-core/src/agents/researcher/bearish.rs`
- Modify: `crates/scorpio-core/src/agents/researcher/moderator.rs`
- Modify: `crates/scorpio-core/src/agents/risk/moderator.rs`

Do not redesign debate storage in this change. These agents currently produce plain strings that downstream logic validates semantically. The contract work here should harden the seam while preserving those existing artifacts and prompting the model to emit the minimal JSON wrapper the parser expects.

- [ ] **Step 1: Define minimal researcher envelopes**

```rust
// crates/scorpio-core/src/agents/shared/contracts/researcher.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use super::OutputContract;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BullishResearcherOutput {
    #[schemars(length(min = 1, max = 4096))]
    pub content: String,
}

impl OutputContract for BullishResearcherOutput {
    const NAME: &'static str = "bullish_researcher";
}

// Mirror BearishResearcherOutput with the same single `content` field.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebateModeratorOutput {
    #[schemars(length(max = 4096))]
    pub consensus_summary: String,
}

impl OutputContract for DebateModeratorOutput {
    const NAME: &'static str = "debate_moderator";
}
```

- [ ] **Step 2: Update prompts and parse paths for Bull / Bear / Debate Moderator**

Update the local prompt builders so they request minimal JSON, not free prose:

```rust
// bullish.rs / bearish.rs user prompt tail
"Return JSON only: {\"content\": \"<your rebuttal>\"}"

// researcher/moderator.rs user prompt tail
"Return JSON only: {\"consensus_summary\": \"<summary>\"}"
```

Then parse the wrapper and feed the string back through the existing semantic validators:

```rust
let wrapped = BullishResearcherOutput::parse(&outcome.result.output)?;
build_debate_result(..., wrapped.content, ...)
```

- [ ] **Step 3: Wire Risk Moderator last using the minimal wrapper already introduced in Task 6**

Update `crates/scorpio-core/src/agents/risk/moderator.rs` so its prompt asks for:

```json
{"content":"..."}
```

Then parse with `RiskModeratorOutput::parse(&output)?` and continue to run:

- `validate_moderator_output_shape(...)`
- `prepend_violation_status_if_missing(...)`

- [ ] **Step 4: Validation and semantic tests**

Add tests for:

- unknown-field rejection on the new researcher envelopes
- wrapped JSON acceptance on Bull / Bear / Debate Moderator / Risk Moderator
- continued enforcement of `validate_debate_content(...)`, `validate_consensus_summary(...)`, and `validate_moderator_output_shape(...)` after unwrap

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/agents/shared/contracts/researcher.rs crates/scorpio-core/src/agents/researcher/ crates/scorpio-core/src/agents/risk/moderator.rs
git commit -m "feat(agents): harden researcher and risk moderator output seams"
```

---

## Out of Scope (explicitly)

- **Tool-call argument validation.** This plan covers agent *output*. Tool *input* validation is a separate concern and likely already enforced by `#[tool]` macros + `schemars`.
- **End-to-end JSON schema published as a public contract.** Not a goal — these are internal contracts.
- **Streaming partial output.** The envelope is parsed at end of completion, not incrementally.
- **Cross-version envelope migration.** Envelopes are NOT snapshot-reachable, so they don't need to evolve gracefully — bump them with the binary.

---

## Self-Review Checklist

- [x] Every task has exact file paths.
- [x] Every step has verbatim code or specific instruction.
- [x] No placeholders.
- [x] Type names are consistent with the brownfield design: `OutputContract` trait, `*AnalystOutput` envelopes, `*RiskOutput`, and in-place trader/fund-manager contract types where those modules already own the schema boundary.
- [x] Data dependencies: NONE — confirmed in Decision Summary.
- [x] CLAUDE.md compliance: envelopes (NOT snapshot-reachable) are the only place `deny_unknown_fields` is used; existing snapshot-reachable types only gain `JsonSchema` derive + bounds.

---

## Attribution

Pattern adapted from `anthropics/financial-services` (Apache 2.0):
- `managed-agent-cookbooks/*/subagents/*.yaml` — strict `output_schema` with `additionalProperties: false`, `maxItems`, and `maxLength` constraints.
