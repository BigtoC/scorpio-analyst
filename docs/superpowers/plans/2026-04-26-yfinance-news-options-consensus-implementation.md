# YFinance News, Options Snapshot, and Extended Consensus Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Add Yahoo company news, a Technical Analyst options snapshot tool, and extended analyst-consensus enrichment without breaking persisted snapshots, live fail-open behavior, or existing prompt/report contracts.

**Architecture:** Keep the live graph path centered on `crates/scorpio-core/src/workflow/pipeline/runtime.rs` and `crates/scorpio-core/src/workflow/tasks/analyst.rs`. Extend the existing Yahoo/Finnhub wrappers and `StubbedFinancialResponses`, keep consensus as pre-debate enrichment, merge Yahoo news only into analyst cached news, and keep options scoped to `crates/scorpio-core/src/agents/analyst/equity/technical.rs` instead of adding new routing or pack-wide provider plumbing.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `chrono`, `rig`, `yfinance-rs`, `finnhub`, `cargo nextest`, `cargo fmt`, `cargo clippy`.

---

**Worktree:** Execute from `feature/enrich-news-sources`. Confirm with `git worktree list` first.

## Guardrails

### Schema and snapshot compatibility

- Additive fields stay on `THESIS_MEMORY_SCHEMA_VERSION = 3` and deserialize via `#[serde(default)]`. Only renames, removals, or backward-incompatible type changes bump the constant. The design doc's `THESIS_MEMORY_SCHEMA_VERSION 3 -> 4` line is stale for additive changes; the same-PR design-doc update is tracked in Task 11 Step 1.
- **`#[serde(deny_unknown_fields)]` must be removed from `state::news::{NewsData, NewsArticle}` and `state::technical::TechnicalData` in Task 1.** Without this, the additive-field invariant breaks for any reader that has not yet upgraded — older binaries reject the new keys instead of falling back to defaults. Document the contract relaxation in CLAUDE.md alongside the existing `#[serde(default)]` rule.
- Extend `crates/scorpio-core/src/state/news.rs::NewsArticle` with `url: Option<String>` even though the original design draft said `NewsData` would stay unchanged; cross-provider dedupe and provenance need a stored URL.

### Upstream-to-domain field mapping

- Keep `crates/scorpio-core/src/data/adapters/estimates.rs::PriceTargetSummary` grounded in the real upstream type: `mean`, `high`, `low`, and `analyst_count`. Do not invent `median`. The upstream `paft::fundamentals::analysis::PriceTarget` exposes the analyst count as `number_of_analysts: Option<u32>`; map it through the wrapper as `analyst_count` (do not add a Yahoo-shaped `number_of_analysts` field to our domain type).
- The Yahoo company-news article (`paft::market::news::NewsArticle`, re-exported as `yfinance_rs::news::NewsArticle`) carries the article hyperlink in `link: Option<String>`, not `url`. The `YFinanceNewsProvider` must source `state::news::NewsArticle.url` from the upstream `link` field.

### No-data taxonomy (replaces collapsed `Ok(None)`)

- Both the consensus and options providers return a structured outcome enum instead of bare `Ok(None)`. Bare `Ok(None)` silently masks degraded providers (consensus) and conflates four distinct conditions (options). The taxonomies are:

  ```rust
  // crates/scorpio-core/src/data/adapters/estimates.rs
  pub enum ConsensusOutcome {
      Data(ConsensusEvidence),
      NoCoverage,         // all branches succeeded, none produced usable data
      ProviderDegraded,   // ≥1 branch errored, remaining branches yield no usable data
  }

  // crates/scorpio-core/src/data/traits/options.rs
  pub enum OptionsOutcome {
      Snapshot(OptionsSnapshot),
      NoListedInstrument, // expirations list is empty
      SparseChain,        // expirations exist but no usable contracts in NTM band
      HistoricalRun,      // target_date is not market-local today
      MissingSpot,        // get_latest_close returned None
  }
  ```

  Surface the variant in `tracing::warn!` (`reason=...`) and downgrade to `Ok(NoCoverage)` / `Ok(NoListedInstrument)` only after explicit branch-by-branch evidence; treat unexplained "all-empty 200-OK" as `ProviderDegraded`. The Technical Analyst tool surfaces `OptionsOutcome` variants with human-readable `reason` strings so the LLM can distinguish absence-of-data from absence-of-instrument.

  **`ProviderDegraded` half-life + retry.** A symbol with no analyst coverage plus an intermittent network blip would otherwise stick at `ProviderDegraded → ConsensusStatus::FetchFailed` forever. The runtime applies the following policy in `hydrate_consensus()`:

  1. On the first `Ok(ProviderDegraded)` for a symbol within an analysis cycle, retry the consensus fetch once immediately (no backoff — the rate limiter handles spacing). If the retry returns `Ok(Data(_))` or `Ok(NoCoverage)`, use that; if it returns `Ok(ProviderDegraded)` again, persist the outcome.
  2. Track `consecutive_provider_degraded_cycles` per symbol in the snapshot store (additive `#[serde(default)] u32` field on the consensus enrichment payload). After `CONSENSUS_PROVIDER_DEGRADED_HALF_LIFE_CYCLES = 3` consecutive degraded cycles, downgrade the runtime status from `FetchFailed` to `NotAvailable` and log `tracing::warn!(symbol, cycles, "provider_degraded persisted; treating as no_coverage after half-life")`. The next non-degraded outcome (Data or NoCoverage) resets the counter.

  Add the `consecutive_provider_degraded_cycles` field per the additive-fields rule (no `deny_unknown_fields`, `#[serde(default)]`).

### Options scope and clock

- Keep options data Technical-Analyst-scoped for v1. Do not route it through `crates/scorpio-core/src/data/traits/derivatives.rs` or `crates/scorpio-core/src/data/routing.rs`. Cross-analyst options access (Sentiment, Risk) is a deferred decision tracked as the Task 11 Step 2 follow-up note.
- The "today" comparison in `OptionsProvider::fetch_snapshot` uses **market-local US/Eastern**, not UTC. Use `chrono_tz::US::Eastern` to convert `target_date` and `now()` before equality. This matches options-market-data semantics and avoids UTC-dateline misfires for after-hours runs. **`chrono-tz` is not currently a workspace dep — add `chrono-tz = "0.10"` to `[workspace.dependencies]` in the root `Cargo.toml` and add `chrono-tz.workspace = true` to `crates/scorpio-core/Cargo.toml` as part of Task 6 Step 4b.** Calendar-equality only (this v1) deliberately ignores half-day closes (e.g., day-after-Thanksgiving) and US market holidays: today's calendar date in ET equal to `target_date` is the gate, even on closed sessions. A market-clock-aware gate (NYSE session calendar) is a deferred decision — track in Task 11 Step 2.
- Do not add a true skew field to the first options snapshot contract. The current Yahoo chain data does not provide usable delta/greeks for a real 25-delta skew calculation in this slice. The Task 8 prompt edit must explicitly warn the model that the snapshot omits skew and forbid it from making directional vol calls without skew context.

### News and live behavior

- Keep `EventNewsEvidence` Finnhub-only. Yahoo company news supplements analyst cached news only.
- Preserve the live fallback behavior in `crates/scorpio-core/src/agents/analyst/mod.rs::prefetch_analyst_news`: return `None` when both prefetch providers fail so `GetNews` stays available to the live analyst tools.

### Quality

- Live smoke (Task 9) verifies upstream data exists. Task 11 adds deterministic outcome smoke proving the structured `OptionsOutcome` variants serialize correctly through the live tool path on frozen fixtures. Neither is an analysis-quality eval — the goal of "higher signal" is not gated by CI in v1; real-LLM rubric eval is a separately-tracked decision outside this plan.

## File Map

| Action | Path                                                                          | Responsibility                                                                                                          |
|--------|-------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| Modify | `crates/scorpio-core/src/state/news.rs`                                       | Add snapshot-safe `NewsArticle.url`; remove `#[serde(deny_unknown_fields)]` from `NewsData`/`NewsArticle`               |
| Modify | `crates/scorpio-core/src/state/technical.rs`                                  | Add snapshot-safe `TechnicalData.options_summary`; remove `#[serde(deny_unknown_fields)]` from `TechnicalData`          |
| Modify | `crates/scorpio-core/src/data/adapters/estimates.rs`                          | Extend `ConsensusEvidence` and implement partial-fail-open price-target/recommendation fetches                          |
| Modify | `crates/scorpio-core/src/data/yfinance/ohlcv.rs`                              | Extend `StubbedFinancialResponses` for consensus, Yahoo news, and options fixtures                                      |
| Modify | `crates/scorpio-core/src/data/yfinance/financials.rs`                         | Add result-preserving Yahoo wrappers for price target and recommendation summary                                        |
| Create | `crates/scorpio-core/src/data/yfinance/news.rs`                               | Add Yahoo company-news wrapper helpers and `YFinanceNewsProvider`                                                       |
| Create | `crates/scorpio-core/src/data/traits/options.rs`                              | Define the equity options snapshot contract                                                                             |
| Modify | `crates/scorpio-core/src/data/traits/mod.rs`                                  | Export `OptionsProvider`                                                                                                |
| Create | `crates/scorpio-core/src/data/yfinance/options.rs`                            | Normalize Yahoo option chains, compute summary metrics, and expose `GetOptionsSnapshot`                                 |
| Modify | `crates/scorpio-core/src/data/yfinance/mod.rs`                                | Export the new Yahoo news/options modules                                                                               |
| Modify | `crates/scorpio-core/src/data/mod.rs`                                         | Re-export the new Yahoo news/options types needed by agents, examples, and tests                                        |
| Modify | `crates/scorpio-core/src/data/finnhub.rs`                                     | Normalize Finnhub article URLs and timestamps into the shared state contract                                            |
| Modify | `crates/scorpio-core/src/agents/analyst/mod.rs`                               | Merge Finnhub + Yahoo prefetch news with dedupe and live fallback preservation                                          |
| Modify | `crates/scorpio-core/src/agents/analyst/equity/technical.rs`                  | Bind `GetOptionsSnapshot`, parse `options_summary`, and keep options local to the Technical Analyst                     |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md`  | Add Technical Analyst guidance for `get_options_snapshot` and `options_summary`                                         |
| Modify | `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt`      | Regenerated by the prompt-bundle regression gate after `technical_analyst.md` changes                                   |
| Modify | `crates/scorpio-core/src/agents/shared/prompt.rs`                             | Render richer consensus enrichment context                                                                              |
| Modify | `crates/scorpio-core/src/workflow/pipeline/runtime.rs`                        | Call the dual-provider cached-news prefetch from the live graph path                                                    |
| Modify | `crates/scorpio-core/src/workflow/tasks/analyst.rs`                           | Include `options_snapshot` in technical evidence datasets when the summary is present                                   |
| Modify | `crates/scorpio-core/src/workflow/pipeline/tests.rs`                          | Add runtime hydration and stale-state regressions                                                                       |
| Modify | `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`            | Prove additive fields deserialize under schema version `3`                                                              |
| Modify | `crates/scorpio-core/tests/state_roundtrip.rs`                                | Extend proptest strategies for the additive fields                                                                      |
| Modify | `crates/scorpio-reporters/src/terminal/final_report.rs`                       | Update `ConsensusEvidence` fixture literals so the workspace still compiles after the struct expands                    |
| Modify | `crates/scorpio-core/examples/yfinance_live_test.rs`                          | Add live Yahoo news/options/extended-consensus smoke sections                                                           |
| Modify | `crates/scorpio-core/src/data/provider_impls.rs`                              | Promote `require_equity_ticker` to `pub(crate)` so the options provider can reuse it                                    |
| Modify | `CLAUDE.md`                                                                   | Add the `deny_unknown_fields`-must-not-return rule alongside the existing `#[serde(default)]` rule                      |
| Modify | `docs/superpowers/specs/2026-04-24-yfinance-news-options-consensus-design.md` | Retract or cross-reference CLAUDE.md so design spec + plan + CLAUDE.md agree on the schema-bump rule                    |
| Modify | `Cargo.toml` (root) and `crates/scorpio-core/Cargo.toml`                      | Pin `chrono-tz = "0.10"` in `[workspace.dependencies]` and consume it in scorpio-core for the options market-clock gate |
| Create | `crates/scorpio-core/tests/fixtures/options_outcomes/`                        | Frozen `StubbedFinancialResponses` JSON per `OptionsOutcome` variant; double-purposed by Task 6 unit tests              |
| Create | `crates/scorpio-core/tests/options_outcome_smoke.rs`                          | Deterministic integration smoke asserting each fixture maps to the expected `OptionsOutcome` variant                    |

## Literal Update Surfaces

- `NewsArticle` literal sites to update after adding `url`: `crates/scorpio-core/src/agents/analyst/equity/news.rs`, `crates/scorpio-core/src/agents/analyst/mod.rs`, `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/fund_manager/tests.rs`, `crates/scorpio-core/src/agents/trader/tests.rs`, `crates/scorpio-core/src/data/finnhub.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`.
- `TechnicalData` literal sites to update after adding `options_summary`: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, `crates/scorpio-core/src/agents/analyst/mod.rs`, `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/fund_manager/tests.rs`, `crates/scorpio-core/src/agents/trader/tests.rs`, `crates/scorpio-core/src/indicators/batch.rs`, `crates/scorpio-core/src/testing/prompt_render.rs`, `crates/scorpio-core/src/workflow/pipeline/tests.rs`, `crates/scorpio-core/src/workflow/tasks/test_helpers.rs`, `crates/scorpio-core/src/workflow/tasks/tests.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`, `crates/scorpio-core/tests/support/workflow_observability_task_support.rs`, `crates/scorpio-core/tests/workflow_pipeline_structure.rs`.
- `ConsensusEvidence` literal and generator sites to update after extending the struct: `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/shared/prompt.rs`, `crates/scorpio-core/src/data/adapters/estimates.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`, `crates/scorpio-reporters/src/terminal/final_report.rs`.

## Chunk 1: Snapshot-Safe State and Extended Consensus

### Task 1: Add snapshot-safe state fields and backward-compatibility regressions

**Files:**
- Modify: `crates/scorpio-core/src/state/news.rs`
- Modify: `crates/scorpio-core/src/state/technical.rs`
- Modify: `crates/scorpio-core/src/data/adapters/estimates.rs`
- Modify: `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`
- Modify: `crates/scorpio-core/tests/state_roundtrip.rs`
- Modify: `crates/scorpio-reporters/src/terminal/final_report.rs`
- Modify: `crates/scorpio-core/src/agents/analyst/equity/news.rs`
- Modify: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`
- Modify: the exact literal-update surfaces listed above

- [x] **Step 1: Add the missing-URL regression in `crates/scorpio-core/src/agents/analyst/equity/news.rs`**

Add a unit test named `news_article_missing_url_defaults_to_none` that deserializes a `NewsData` JSON object whose article omits `url` and asserts `data.articles[0].url.is_none()`.

- [x] **Step 2: Add the missing-options-summary regression in `crates/scorpio-core/src/agents/analyst/equity/technical.rs`**

Add a unit test named `technical_data_missing_options_summary_defaults_to_none` that deserializes a `TechnicalData` JSON object without `options_summary` and asserts the field defaults to `None`.

- [x] **Step 3: Add the missing-extended-consensus regression in `crates/scorpio-core/src/data/adapters/estimates.rs`**

Add a unit test named `consensus_evidence_missing_extended_fields_defaults_to_none` that deserializes legacy JSON without `price_target` or `recommendations` and asserts both default to `None`.

- [x] **Step 4: Add the additive-fields-on-schema-v3 regression in `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`**

Add a test named `additive_consensus_and_technical_fields_do_not_require_schema_bump` that writes a phase-5 snapshot row stamped with the current `THESIS_MEMORY_SCHEMA_VERSION`, removes the new additive keys from the stored JSON, and proves `load_prior_thesis_for_symbol()` still returns the thesis instead of skipping the row. The test enforces CLAUDE.md's "additive fields stay on v3 with `#[serde(default)]`" rule. The companion design-doc retraction lives in Task 11 Step 1.

- [x] **Step 5: Run the focused compatibility slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(news_article_missing_url_defaults_to_none) | test(technical_data_missing_options_summary_defaults_to_none) | test(consensus_evidence_missing_extended_fields_defaults_to_none) | test(additive_consensus_and_technical_fields_do_not_require_schema_bump)'`

Expected: FAIL because the additive fields do not exist yet.

- [x] **Step 6: Sweep `#[serde(deny_unknown_fields)]` from every snapshotted state struct**

The plan's "additive fields stay on schema 3" guarantee requires that older binaries reading newer snapshots tolerate the new keys, not reject them. Strip the attribute from EVERY occurrence in `crates/scorpio-core/src/state/`. The full list (run `grep -rn 'deny_unknown_fields' crates/scorpio-core/src/state/` to confirm) currently includes occurrences in:

- `state/news.rs` — `NewsData`, `NewsArticle`, `MacroEvent`
- `state/technical.rs` — `TechnicalData`, `MacdValues`
- `state/sentiment.rs` — three structs
- `state/fundamental.rs` — two structs
- `state/proposal.rs` — one struct
- `state/market_volatility.rs` — one struct

Strip every one of them. **Also delete the existing test `news_article_extra_fields_rejected` in `crates/scorpio-core/src/agents/analyst/equity/news.rs`** — it explicitly asserts `result.is_err()` for an unknown field on `NewsArticle` and will now pass parsing instead of failing. Remove the test (it codifies the contract this step is intentionally relaxing); do not invert it (a `result.is_ok()` assertion would be a tautology after the strip).

Then update CLAUDE.md (the "TradingState schema evolution" bullet list) with this exact text scoped to snapshotted state, NOT a project-wide ban: "Snapshotted state structs serialized into `phase_snapshots.trading_state_json` (anything reachable from `TradingState` via serde) must not use `#[serde(deny_unknown_fields)]` — it converts every additive field into a backward-incompatible change. This rule does NOT apply to RPC, tool-argument, or config types where typo detection is more valuable than forward-compat."

- [x] **Step 6b: Add a downgrade-compatibility regression in `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`**

Add a test named `additive_fields_deserialize_when_struct_lacks_field`. Write a JSON object with extra unknown keys (`"future_field": ...`) into a phase-5 snapshot row stamped at the current `THESIS_MEMORY_SCHEMA_VERSION`, and prove the loader deserializes the row as if those keys were absent. This codifies the contract that future additive fields land safely.

- [x] **Step 7: Add the additive fields with `#[serde(default)]` and keep schema version `3`**

Make these exact shape changes:

```rust
// crates/scorpio-core/src/state/news.rs
#[serde(default)]
pub url: Option<String>,

// crates/scorpio-core/src/state/technical.rs
#[serde(default)]
pub options_summary: Option<String>,

// crates/scorpio-core/src/data/adapters/estimates.rs
#[serde(default)]
pub price_target: Option<PriceTargetSummary>,
#[serde(default)]
pub recommendations: Option<RecommendationsSummary>,
```

Also add `PriceTargetSummary` and `RecommendationsSummary` in `crates/scorpio-core/src/data/adapters/estimates.rs` with these exact shapes (the upstream-to-domain rename for `analyst_count` is the guardrail rule):

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PriceTargetSummary {
    #[serde(default)]
    pub mean: Option<f64>,
    #[serde(default)]
    pub high: Option<f64>,
    #[serde(default)]
    pub low: Option<f64>,
    #[serde(default)]
    pub analyst_count: Option<u32>, // mapped from upstream `number_of_analysts`
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct RecommendationsSummary {
    #[serde(default)]
    pub strong_buy: Option<u32>,
    #[serde(default)]
    pub buy: Option<u32>,
    #[serde(default)]
    pub hold: Option<u32>,
    #[serde(default)]
    pub sell: Option<u32>,
    #[serde(default)]
    pub strong_sell: Option<u32>,
}
```

Do not touch `crates/scorpio-core/src/workflow/snapshot/thesis.rs`.

- [x] **Step 8: Update every explicit `NewsArticle` literal after the new field lands**

Set `url: None` or a concrete URL in each explicit constructor under `crates/scorpio-core/src/agents/analyst/equity/news.rs`, `crates/scorpio-core/src/agents/analyst/mod.rs`, `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/fund_manager/tests.rs`, `crates/scorpio-core/src/agents/trader/tests.rs`, `crates/scorpio-core/src/data/finnhub.rs`, and `crates/scorpio-core/tests/state_roundtrip.rs`.

- [x] **Step 9: Update every explicit `TechnicalData` literal after the new field lands**

Set `options_summary: None` unless the fixture should explicitly exercise options behavior in `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, `crates/scorpio-core/src/agents/analyst/mod.rs`, `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/fund_manager/tests.rs`, `crates/scorpio-core/src/agents/trader/tests.rs`, `crates/scorpio-core/src/indicators/batch.rs`, `crates/scorpio-core/src/testing/prompt_render.rs`, `crates/scorpio-core/src/workflow/pipeline/tests.rs`, `crates/scorpio-core/src/workflow/tasks/test_helpers.rs`, `crates/scorpio-core/src/workflow/tasks/tests.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`, `crates/scorpio-core/tests/support/workflow_observability_task_support.rs`, and `crates/scorpio-core/tests/workflow_pipeline_structure.rs`.

- [x] **Step 10: Update every explicit `ConsensusEvidence` literal and proptest generator**

Add `price_target: None` and `recommendations: None` in `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/shared/prompt.rs`, `crates/scorpio-core/src/data/adapters/estimates.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`, and `crates/scorpio-reporters/src/terminal/final_report.rs`.

- [x] **Step 11: Extend the proptest generators in `crates/scorpio-core/tests/state_roundtrip.rs`**

Teach `arb_news_article`, `arb_technical_data`, and `arb_consensus_evidence` about the new optional fields so the round-trip property test keeps covering the expanded persisted shape.

- [x] **Step 12: Re-run the compatibility slice plus the round-trip integration test**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(news_article_missing_url_defaults_to_none) | test(technical_data_missing_options_summary_defaults_to_none) | test(consensus_evidence_missing_extended_fields_defaults_to_none) | test(additive_consensus_and_technical_fields_do_not_require_schema_bump) | test(additive_fields_deserialize_when_struct_lacks_field) | binary(state_roundtrip)'`

Expected: PASS.

- [x] **Step 13: Commit the additive-state foundation**

Run: `git add crates/scorpio-core/src/state/news.rs crates/scorpio-core/src/state/technical.rs crates/scorpio-core/src/data/adapters/estimates.rs crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs crates/scorpio-core/tests/state_roundtrip.rs crates/scorpio-reporters/src/terminal/final_report.rs crates/scorpio-core/src/agents/analyst/equity/news.rs crates/scorpio-core/src/agents/analyst/equity/technical.rs crates/scorpio-core/src/agents/analyst/mod.rs crates/scorpio-core/src/agents/fund_manager/prompt.rs crates/scorpio-core/src/agents/fund_manager/tests.rs crates/scorpio-core/src/agents/trader/tests.rs crates/scorpio-core/src/indicators/batch.rs crates/scorpio-core/src/testing/prompt_render.rs crates/scorpio-core/src/workflow/pipeline/tests.rs crates/scorpio-core/src/workflow/tasks/test_helpers.rs crates/scorpio-core/src/workflow/tasks/tests.rs crates/scorpio-core/tests/support/workflow_observability_task_support.rs crates/scorpio-core/tests/workflow_pipeline_structure.rs && git commit -m "feat(core): add snapshot-safe news and technical fields"`

### Task 2: Extend Yahoo consensus wrappers and implement partial-fail-open enrichment

**Files:**
- Modify: `crates/scorpio-core/src/data/yfinance/ohlcv.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/financials.rs`
- Modify: `crates/scorpio-core/src/data/adapters/estimates.rs`

- [x] **Step 1: Add result-preserving Yahoo wrapper tests in `crates/scorpio-core/src/data/yfinance/financials.rs`**

Add these exact tests:

```rust
#[tokio::test]
async fn get_analyst_price_target_result_preserves_yahoo_failure_reason() { ... }

#[tokio::test]
async fn get_recommendations_summary_result_preserves_yahoo_failure_reason() { ... }

#[tokio::test]
async fn empty_price_target_payload_returns_none() { ... }

#[tokio::test]
async fn empty_recommendations_summary_payload_returns_none() { ... }
```

- [x] **Step 2: Add the provider-behavior regressions in `crates/scorpio-core/src/data/adapters/estimates.rs`**

Add these exact tests (names align with the `ConsensusOutcome` taxonomy from Step 6):

```rust
#[tokio::test]
async fn fetch_consensus_populates_price_target_and_recommendations() { ... } // returns Data

#[tokio::test]
async fn fetch_consensus_classifies_partial_data_with_one_branch_error_as_data_with_warn() { ... }

#[tokio::test]
async fn fetch_consensus_returns_no_coverage_when_all_endpoints_return_no_data() { ... }

#[tokio::test]
async fn fetch_consensus_returns_provider_degraded_when_price_target_errors_and_others_empty() { ... }

#[tokio::test]
async fn fetch_consensus_returns_err_when_all_three_endpoints_fail() { ... }
```

- [x] **Step 3: Run the focused consensus slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(get_analyst_price_target_result_preserves_yahoo_failure_reason) | test(get_recommendations_summary_result_preserves_yahoo_failure_reason) | test(fetch_consensus_populates_price_target_and_recommendations) | test(fetch_consensus_classifies_partial_data_with_one_branch_error_as_data_with_warn) | test(fetch_consensus_returns_no_coverage_when_all_endpoints_return_no_data) | test(fetch_consensus_returns_provider_degraded_when_price_target_errors_and_others_empty) | test(fetch_consensus_returns_err_when_all_three_endpoints_fail)'`

Expected: FAIL because the new wrappers and provider logic do not exist yet.

- [x] **Step 4: Extend `StubbedFinancialResponses` with consensus fixtures in `crates/scorpio-core/src/data/yfinance/ohlcv.rs`**

Add these exact test-only fields:

```rust
pub price_target: Option<yfinance_rs::analysis::PriceTarget>,
pub price_target_error: Option<String>,
pub recommendation_summary: Option<yfinance_rs::analysis::RecommendationSummary>,
pub recommendation_summary_error: Option<String>,
```

- [x] **Step 5: Add result-preserving Yahoo wrappers in `crates/scorpio-core/src/data/yfinance/financials.rs`**

Add `get_analyst_price_target_result()` and `get_recommendations_summary_result()` next to `get_earnings_trend_result()`, mirror the existing test-stub pattern, and convert all-empty upstream payloads into `Ok(None)` instead of `Some(default_struct)`.

- [x] **Step 6: Implement partial-fail-open consensus fetch in `crates/scorpio-core/src/data/adapters/estimates.rs`**

Define the structured outcome enum and update `fetch_consensus` to return it (replaces the prior `Result<Option<ConsensusEvidence>, TradingError>` shape so degraded providers cannot be silently confused with no-coverage):

```rust
pub enum ConsensusOutcome {
    Data(ConsensusEvidence),
    NoCoverage,        // all three branches Ok with no usable fields
    ProviderDegraded,  // ≥1 branch errored AND remaining branches yield no usable fields
}
```

Use `tokio::join!` to fetch earnings trend, analyst price target, and recommendation summary concurrently. Keep these exact semantics:

- `Ok(Data(evidence))` when at least one upstream branch produced usable data. If `get_earnings_trend_result()` failed but price target and/or recommendations still produced usable fields, set `eps_estimate`, `revenue_estimate_m`, and `analyst_count` to `None` in `evidence` and emit one `tracing::warn!` per failed branch with `provider="yfinance"`, `endpoint=...`, `reason=...`.
- `Ok(NoCoverage)` when all three branches returned successful empty payloads (no errors, no data). This is the only "no analyst coverage available for this symbol" branch.
- `Ok(ProviderDegraded)` when at least one branch errored and no remaining successful branch yielded usable fields. Emit `tracing::warn!` per failed branch.
- `Err(TradingError::...)` when all three branches errored.
- The runtime maps `Ok(NoCoverage)` to `ConsensusStatus::NotAvailable` and `Ok(ProviderDegraded)` to `ConsensusStatus::FetchFailed` so downstream agents get the same operational signal as before but can no longer mistake one for the other.

- [x] **Step 6b: Update the runtime call site so the workspace builds at this commit**

The `EstimatesProvider::fetch_consensus` return type changes from `Result<Option<ConsensusEvidence>, TradingError>` to `Result<ConsensusOutcome, TradingError>`. The current consumer in `crates/scorpio-core/src/workflow/pipeline/runtime.rs::hydrate_consensus()` (around line 414) pattern-matches `Ok(Ok(Some(_)))` and `Ok(Ok(None))` against the old shape — this must be updated in the same commit, otherwise the workspace stops compiling. Update:

```rust
// crates/scorpio-core/src/workflow/pipeline/runtime.rs (hydrate_consensus)
match tokio::time::timeout(timeout, provider.fetch_consensus(symbol, target_date)).await {
    Ok(Ok(ConsensusOutcome::Data(evidence))) => { /* same body as Ok(Some(evidence)) before */ }
    Ok(Ok(ConsensusOutcome::NoCoverage)) => { /* same body as Ok(None) before, ConsensusStatus::NotAvailable */ }
    Ok(Ok(ConsensusOutcome::ProviderDegraded)) => { /* ConsensusStatus::FetchFailed with reason="provider_degraded" */ }
    Ok(Err(err)) => { /* unchanged */ }
    Err(_) => { /* timeout — unchanged */ }
}
```

Add `crates/scorpio-core/src/workflow/pipeline/runtime.rs` to the Task 2 Step 8 commit (already present in the File Map for other reasons; just ensure it's staged). Without this step, every commit between Task 2 Step 8 and Task 5 Step 6 leaves the workspace non-compiling and `cargo nextest --workspace` fails on bisect.

- [x] **Step 7: Re-run the focused consensus slice**

Run the command from Step 3.

Expected: PASS.

- [x] **Step 8: Commit the extended consensus provider work**

Run: `git add crates/scorpio-core/src/data/yfinance/ohlcv.rs crates/scorpio-core/src/data/yfinance/financials.rs crates/scorpio-core/src/data/adapters/estimates.rs crates/scorpio-core/src/workflow/pipeline/runtime.rs && git commit -m "feat(core): extend yahoo consensus enrichment with structured outcome"`

### Task 3: Render the richer consensus payload and prove live hydration still works

**Files:**
- Modify: `crates/scorpio-core/src/agents/shared/prompt.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/tests.rs`

- [x] **Step 1: Add the richer prompt-render regression in `crates/scorpio-core/src/agents/shared/prompt.rs`**

Add a unit test named `build_enrichment_context_includes_price_target_and_recommendations` that asserts the rendered prompt context now includes mean/high/low target values plus the five recommendation buckets.

- [x] **Step 2: Add the runtime hydration regression in `crates/scorpio-core/src/workflow/pipeline/tests.rs`**

Add an async test named `run_analysis_cycle_hydrates_extended_consensus_enrichment` that:

- clones `resolve_pack(PackId::Baseline)`
- sets `pack.enrichment_intent.consensus_estimates = true`
- builds the pipeline via `TradingPipeline::from_pack(...)`
- builds the input `TradingState` with `target_date = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string()` so the real `hydrate_consensus()` live-date gate is exercised
- injects `YFinanceClient::with_stubbed_financials(...)` with trend, price-target, and recommendation fixtures
- replaces downstream graph tasks with the existing stub helpers
- asserts `final_state.enrichment_consensus.payload` carries the new fields

- [x] **Step 3: Run the focused render/hydration slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_enrichment_context_includes_price_target_and_recommendations) | test(run_analysis_cycle_hydrates_extended_consensus_enrichment)'`

Expected: FAIL because prompt rendering does not mention the new fields yet.

- [x] **Step 4: Update `crates/scorpio-core/src/agents/shared/prompt.rs` to render the new consensus fields**

Keep the existing status lines, keep `N/A` for absent fields, and render raw numbers in this shape:

```text
Consensus estimates (as of 2026-04-26):
  - EPS estimate: 2.15
  - Revenue estimate: $94200M
  - Analyst count: 28
  - Price target mean: $215.00
  - Price target range: $170.00 - $265.00
  - Price target analyst count: 42
  - Recommendations: strong_buy=12, buy=18, hold=10, sell=2, strong_sell=0
```

- [x] **Step 5: Re-run the focused render/hydration slice**

Run the command from Step 3.

Expected: PASS.

- [x] **Step 6: Commit the prompt-render and hydration coverage**

Run: `git add crates/scorpio-core/src/agents/shared/prompt.rs crates/scorpio-core/src/workflow/pipeline/tests.rs && git commit -m "feat(core): expose richer consensus context"`

## Chunk 2: Cross-Provider Analyst News

### Task 4: Normalize Finnhub news boundaries and add the Yahoo company-news provider

**Files:**
- Modify: `crates/scorpio-core/src/data/finnhub.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/ohlcv.rs`
- Create: `crates/scorpio-core/src/data/yfinance/news.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/mod.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/tests.rs`

> **Dependency note:** Complete Chunk 1 Task 1 first. This chunk consumes the new `crates/scorpio-core/src/state/news.rs::NewsArticle.url` field added there.

- [x] **Step 1: Add the Finnhub normalization regressions in `crates/scorpio-core/src/data/finnhub.rs`**

Add these exact tests near the existing news helpers:

```rust
#[test]
fn normalize_finnhub_article_preserves_url() { ... }

#[test]
fn normalize_finnhub_article_formats_rfc3339_timestamp() { ... }
```

Both tests should exercise the shared normalization path used by `build_news_data()` and `get_market_news()`.

- [x] **Step 2: Add the Yahoo news-provider regressions in `crates/scorpio-core/src/data/yfinance/news.rs` and wire the module into `crates/scorpio-core/src/data/yfinance/mod.rs`**

Add these exact tests:

```rust
#[tokio::test]
async fn fetches_and_normalizes_articles() { ... }

#[tokio::test]
async fn empty_feed_returns_empty_news_data() { ... }
```

Assert RFC3339 timestamps, preserved URLs, `snippet == ""` for Yahoo articles, and an empty `macro_events` list.

In the same step, add `pub mod news;` to `crates/scorpio-core/src/data/yfinance/mod.rs` so the new test file is compiled before the first red-state run.

- [x] **Step 3: Run the focused news-normalization slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(normalize_finnhub_article_preserves_url) | test(normalize_finnhub_article_formats_rfc3339_timestamp) | test(fetches_and_normalizes_articles) | test(empty_feed_returns_empty_news_data)'`

Expected: FAIL because the shared helper and Yahoo provider do not exist yet.

- [x] **Step 4: Extract a shared Finnhub article normalizer in `crates/scorpio-core/src/data/finnhub.rs`**

Create one small helper that both `build_news_data()` and `get_market_news()` use. It must:

- trim/empty-check URLs into `Option<String>`
- convert unix-second timestamps into RFC3339 strings
- keep the existing title/snippet sanitization rules

- [x] **Step 5: Extend `StubbedFinancialResponses` and implement `YFinanceNewsProvider`**

Add these exact test-only stub fields in `crates/scorpio-core/src/data/yfinance/ohlcv.rs`:

```rust
pub news: Option<Vec<yfinance_rs::news::NewsArticle>>,
pub news_error: Option<String>,
```

Then add `crates/scorpio-core/src/data/yfinance/news.rs` with:

- a small `YFinanceClient` result wrapper for company news
- `YFinanceNewsProvider::new(client: YFinanceClient)`
- a `NewsProvider` impl that keeps only articles inside the existing `NEWS_ANALYSIS_DAYS` window, stores `url`, emits RFC3339 timestamps, leaves `macro_events` empty, and builds a short count-based `summary`

Also update the explicit `StubbedFinancialResponses { ... }` literals in `crates/scorpio-core/src/workflow/tasks/tests.rs` to use the new fields or `..StubbedFinancialResponses::default()` so the test build stays green after the struct expands.

- [x] **Step 6: Export the Yahoo news provider surface**

Keep the `pub mod news;` declaration from Step 2, then add any needed `pub use` exports in `crates/scorpio-core/src/data/yfinance/mod.rs` and `crates/scorpio-core/src/data/mod.rs` so runtime code, examples, and tests can import the provider without reaching into private modules.

- [x] **Step 7: Re-run the focused news-normalization slice**

Run the command from Step 3.

Expected: PASS.

- [x] **Step 8: Commit the normalized news-provider foundation**

Run: `git add crates/scorpio-core/src/data/finnhub.rs crates/scorpio-core/src/data/yfinance/ohlcv.rs crates/scorpio-core/src/data/yfinance/news.rs crates/scorpio-core/src/data/yfinance/mod.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/workflow/tasks/tests.rs && git commit -m "feat(core): add yahoo analyst news provider"`

### Task 5: Merge Finnhub and Yahoo cached news without breaking the live fallback

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/mod.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs`

- [x] **Step 1: Add the merge/dedupe regressions in `crates/scorpio-core/src/agents/analyst/mod.rs`**

Add these exact tests, including the explicit edge cases that the original "URL-then-title" rule both over- and under-merges:

```rust
#[tokio::test]
async fn merge_dedupes_by_url() { ... }

#[tokio::test]
async fn merge_dedupes_by_headline_when_url_missing() { ... }

#[tokio::test]
async fn merge_dedupes_same_article_when_canonical_url_differs_via_redirect_resolution() {
    // Finnhub stores the canonical publisher URL (reuters.com/...).
    // Yahoo stores a yhoo.it shortener for the same article.
    // The merge must resolve to the canonical URL (or fall back to title hash) and dedupe.
    // Without this, the analyst sees a fake "two independent sources reported X" signal.
}

#[tokio::test]
async fn merge_preserves_multi_outlet_coverage_for_wire_republication() {
    // Same AP/Reuters wire copy republished verbatim by 5 outlets with distinct URLs and
    // very-near-identical (but not byte-identical) titles. The merge must NOT collapse
    // these into one — broad coverage is itself a decision-relevant signal. Title-hash
    // dedupe should be lossy on near-but-not-identical titles, not lossless.
}

#[tokio::test]
async fn merge_falls_back_to_single_provider_on_partial_failure() { ... }

#[tokio::test]
async fn prefetch_analyst_news_returns_none_when_both_prefetch_providers_fail() { ... }

#[tokio::test]
async fn merge_sorts_articles_newest_first() { ... }
```

The two new tests above codify intentional behavior decisions. Implement: (1) URL canonicalization (strip known shorteners like `yhoo.it`, follow `?utm_*=` query params, normalize trailing slashes) before comparison; (2) title-hash dedupe uses an exact match after Unicode-NFKC normalization, not a fuzzy/Levenshtein match — so "Apple Posts Strong Q4" and "Apple Reports Strong Q4" stay distinct.

- [x] **Step 2: Run the focused merge slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(merge_dedupes_by_url) | test(merge_dedupes_by_headline_when_url_missing) | test(merge_dedupes_same_article_when_canonical_url_differs_via_redirect_resolution) | test(merge_preserves_multi_outlet_coverage_for_wire_republication) | test(merge_falls_back_to_single_provider_on_partial_failure) | test(prefetch_analyst_news_returns_none_when_both_prefetch_providers_fail) | test(merge_sorts_articles_newest_first)'`

Expected: FAIL because the merge helper still only prefetches Finnhub.

- [x] **Step 3: Implement the cached-news merge helpers in `crates/scorpio-core/src/agents/analyst/mod.rs`**

Keep the public helper string-based and minimal:

```rust
pub async fn prefetch_analyst_news(
    finnhub_news: &impl NewsProvider,
    yfinance_news: &impl NewsProvider,
    symbol: &str,
) -> Option<Arc<NewsData>>
```

Inside the helper:

- resolve the string once into the typed `Symbol`
- `tokio::join!` both providers
- dedupe by normalized URL first, then normalized title when URL is missing
- sort newest-first on RFC3339 timestamps
- keep a small local cap such as `const NEWS_PREFETCH_MAX_ARTICLES: usize = 20`
- return `None` only when both providers failed
- preserve Finnhub `macro_events` when one side has them

- [x] **Step 4: Update the live and legacy callers to use both news providers**

In `crates/scorpio-core/src/workflow/pipeline/runtime.rs` and the legacy `run_analyst_team()` path inside `crates/scorpio-core/src/agents/analyst/mod.rs`, construct `YFinanceNewsProvider::new(yfinance.clone())` locally and pass both providers into `prefetch_analyst_news()`. Do not change `EventNewsEvidence` or the cached-news context key shape.

- [x] **Step 5: Re-run the focused merge slice**

Run the command from Step 2.

Expected: PASS.

- [x] **Step 6: Commit the merged cached-news path**

Run: `git add crates/scorpio-core/src/agents/analyst/mod.rs crates/scorpio-core/src/workflow/pipeline/runtime.rs && git commit -m "feat(core): merge finnhub and yahoo cached news"`

## Chunk 3: Technical Analyst Options Snapshot

### Task 6: Add the equity options contract, Yahoo provider, and scoped tool

**Files:**
- Create: `crates/scorpio-core/src/data/traits/options.rs`
- Modify: `crates/scorpio-core/src/data/traits/mod.rs`
- Modify: `crates/scorpio-core/src/data/adapters/estimates.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/ohlcv.rs`
- Create: `crates/scorpio-core/src/data/yfinance/options.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/mod.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/tests.rs`

- [x] **Step 1: Add the Yahoo options-provider regressions in `crates/scorpio-core/src/data/yfinance/options.rs` and wire the module into `crates/scorpio-core/src/data/yfinance/mod.rs`**

Add these exact tests (names align with the `OptionsOutcome` taxonomy):

```rust
#[tokio::test]
async fn returns_snapshot_with_atm_iv_from_front_month_chain() { ... }

#[tokio::test]
async fn snapshot_includes_put_call_ratios_over_all_strikes() { ... }

#[tokio::test]
async fn snapshot_max_pain_uses_front_month_only() { ... }

#[tokio::test]
async fn snapshot_near_term_slice_uses_band_then_min_strikes_fallback() { ... }

#[tokio::test]
async fn returns_no_listed_instrument_when_expirations_empty() { ... }

#[tokio::test]
async fn returns_sparse_chain_when_band_and_fallback_yield_nothing() { ... }

#[tokio::test]
async fn returns_historical_run_when_target_date_is_not_market_local_today() { ... }

#[tokio::test]
async fn returns_missing_spot_when_get_latest_close_is_none() { ... }

#[tokio::test]
async fn returns_err_when_expiration_lookup_fails() { ... }

#[tokio::test]
async fn returns_err_when_option_chain_fetch_fails() { ... }

#[tokio::test]
async fn ignores_missing_greeks_and_skips_true_skew_metric() { ... }

#[tokio::test]
async fn target_date_uses_market_local_us_eastern_not_utc() { ... }
```

In the same step, add `pub mod options;` to `crates/scorpio-core/src/data/yfinance/mod.rs` so the new module is compiled before the first red-state run.

- [x] **Step 2: Run the focused options slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(returns_snapshot_with_atm_iv_from_front_month_chain) | test(snapshot_includes_put_call_ratios_over_all_strikes) | test(snapshot_max_pain_uses_front_month_only) | test(snapshot_near_term_slice_uses_band_then_min_strikes_fallback) | test(returns_no_listed_instrument_when_expirations_empty) | test(returns_sparse_chain_when_band_and_fallback_yield_nothing) | test(returns_historical_run_when_target_date_is_not_market_local_today) | test(returns_missing_spot_when_get_latest_close_is_none) | test(returns_err_when_expiration_lookup_fails) | test(returns_err_when_option_chain_fetch_fails) | test(ignores_missing_greeks_and_skips_true_skew_metric) | test(target_date_uses_market_local_us_eastern_not_utc)'`

Expected: FAIL because the contract and provider do not exist yet.

- [x] **Step 3: Create `crates/scorpio-core/src/data/traits/options.rs` with a structured outcome enum and no skew field**

Define `OptionsProvider`, `OptionsOutcome`, `OptionsSnapshot`, `IvTermPoint`, and `NearTermStrike`. The provider returns the outcome enum so consumers can distinguish absence-of-data from absence-of-instrument:

```rust
#[async_trait]
pub trait OptionsProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn fetch_snapshot(
        &self,
        symbol: &crate::domain::Symbol,
        target_date: &str,
    ) -> Result<OptionsOutcome, TradingError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OptionsOutcome {
    Snapshot(OptionsSnapshot),
    NoListedInstrument, // expirations list is empty for this symbol
    SparseChain,        // expirations exist but no usable contracts in the NTM band
    HistoricalRun,      // target_date is not market-local US/Eastern today
    MissingSpot,        // get_latest_close returned None for this date
}
```

Import `schemars::JsonSchema` and derive `Debug`, `Clone`, `PartialEq`, `Serialize`, `Deserialize`, and `JsonSchema` on `OptionsOutcome`, `OptionsSnapshot`, `IvTermPoint`, and `NearTermStrike`.

Keep the summary grounded in the upstream data that actually exists:

- `spot_price`
- `atm_iv`
- `iv_term_structure`
- `put_call_volume_ratio`
- `put_call_oi_ratio`
- `max_pain_strike`
- `near_term_expiration`
- `near_term_strikes`

Do not add `skew_25d` or any other pseudo-delta metric in this slice. The Task 8 prompt edit must instruct the model to refuse directional vol calls when only `atm_iv` and term structure are available.

- [x] **Step 4: Extend `StubbedFinancialResponses` with options fixtures in `crates/scorpio-core/src/data/yfinance/ohlcv.rs`**

Add these exact test-only fields:

```rust
pub option_expirations: Option<Vec<i64>>,
pub option_expirations_error: Option<String>,
pub option_chains: std::collections::BTreeMap<i64, yfinance_rs::ticker::OptionChain>,
pub option_chain_errors: std::collections::BTreeMap<i64, String>,
```

Also update the explicit `StubbedFinancialResponses { ... }` literals in `crates/scorpio-core/src/workflow/tasks/tests.rs` to use the new fields or `..StubbedFinancialResponses::default()` so test-only struct expansion does not break unrelated task tests.

Also update the explicit `StubbedFinancialResponses { ... }` literal in `crates/scorpio-core/src/data/adapters/estimates.rs` the same way so the focused consensus tests still compile after the struct expands.

- [x] **Step 4b: Promote `require_equity_ticker` to `pub(crate)` in `crates/scorpio-core/src/data/provider_impls.rs`**

Change the function signature from `fn require_equity_ticker(...)` to `pub(crate) fn require_equity_ticker(...)` so `data/yfinance/options.rs` can call it without duplicating the logic. Add a one-line module comment noting the visibility was widened for cross-module reuse.

- [x] **Step 5: Implement `crates/scorpio-core/src/data/yfinance/options.rs`**

Add:

- small `YFinanceClient` wrappers for expiration dates and option chains
- reuse `crates/scorpio-core/src/data/yfinance/price.rs::get_latest_close(...)` for the underlying spot price instead of inventing a new quote path
- call `crate::data::provider_impls::require_equity_ticker(symbol)` (now `pub(crate)` per Step 4b) before calling Yahoo helpers; reject non-equity symbols with `TradingError::SchemaViolation`
- `YFinanceOptionsProvider::new(client: YFinanceClient)`
- `OptionsProvider for YFinanceOptionsProvider`
- `GetOptionsSnapshot`

Keep the implementation minimal and local:

- use a local `const OPTIONS_NTM_STRIKE_BAND_PCT: f64 = 0.05` (±5% band)
- use a local `const OPTIONS_NTM_MIN_STRIKES_PER_SIDE: usize = 2` so low-priced underlyings with coarse strike spacing still emit a non-trivial slice
- use a local `const OPTIONS_NTM_MAX_BAND_EXPANSION_PCT: f64 = 0.20` so the fallback can never expand beyond ±20% of spot — anything past that is no longer "near the money" and the chain should be classified as sparse instead
- use a local `const OPTIONS_FETCH_TIMEOUT_SECS: u64 = 30`
- compute the near-term strike slice as: take all strikes inside `spot * (1 ± OPTIONS_NTM_STRIKE_BAND_PCT)`. If fewer than `OPTIONS_NTM_MIN_STRIKES_PER_SIDE` qualify on either side, expand the band toward the nearest qualifying strike on that side **but never past `spot * (1 ± OPTIONS_NTM_MAX_BAND_EXPANSION_PCT)`**. If after capped expansion either side still has fewer than the minimum, return `Ok(OptionsOutcome::SparseChain)` instead of fabricating an NTM slice from far-OTM/ITM strikes. This makes `near_term_strikes` comparable across `$1.50` small caps and `$215` AAPL while preserving `SparseChain` as a reachable variant for genuinely thin chains.

Add an additional test in Task 6 Step 1: `near_term_slice_returns_sparse_chain_when_capped_expansion_still_short` (front month with strikes at $0.50, $1.00, $5.00 and spot $1.50 — the OTM-side fallback is capped at $1.80 and finds nothing, so the provider returns `SparseChain`, not a `Snapshot` containing the +233% $5.00 strike).
- compare `target_date` to **market-local US/Eastern** today via `chrono_tz::US::Eastern` (add `chrono-tz` to `[workspace.dependencies]` if not already present). Do not compare against `Utc::now().date_naive()`.
- return outcomes per the structured taxonomy: `Ok(OptionsOutcome::HistoricalRun)` if `target_date != market_local_today_eastern`, `Ok(NoListedInstrument)` if expirations are empty, `Ok(SparseChain)` if expirations exist but no usable contracts after the (expanded) band, `Ok(MissingSpot)` if `get_latest_close(...)` returns `None`, `Ok(Snapshot(_))` otherwise. Only expiration-lookup or chain-fetch errors propagate as `Err(TradingError::...)`.
- compute front-month `atm_iv`, term structure, put/call ratios, max pain, and the near-term strike slice for the `Snapshot` arm

- [x] **Step 6: Export the options surface**

Keep the `pub mod options;` declaration from Step 1, then re-export the new trait from `crates/scorpio-core/src/data/traits/mod.rs` and add any needed `pub use` exports in `crates/scorpio-core/src/data/yfinance/mod.rs` and `crates/scorpio-core/src/data/mod.rs`.

- [x] **Step 7: Re-run the focused options slice**

Run the command from Step 2.

Expected: PASS.

- [x] **Step 8: Commit the options contract and provider**

Run: `git add crates/scorpio-core/src/data/traits/options.rs crates/scorpio-core/src/data/traits/mod.rs crates/scorpio-core/src/data/adapters/estimates.rs crates/scorpio-core/src/data/yfinance/ohlcv.rs crates/scorpio-core/src/data/yfinance/options.rs crates/scorpio-core/src/data/yfinance/mod.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/workflow/tasks/tests.rs && git commit -m "feat(core): add yahoo options snapshot provider"`

### Task 7: Wire the scoped options tool into the Technical Analyst and persist `options_summary`

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/tests.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/tests.rs`

> **Dependency note:** Complete Chunk 1 Task 1 first so `crates/scorpio-core/src/state/technical.rs::TechnicalData.options_summary` and the broad `TechnicalData` literal updates already exist before starting this task. This task is not startable from current repo HEAD until that earlier commit lands on the branch. The prompt-fixture refresh chunk will edit `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md`; this task intentionally avoids prompt-file changes so the repo does not stay red between chunks.

> **Runtime note:** This task only wires the code path and persisted output for `GetOptionsSnapshot`. The Technical Analyst does not learn to call the tool until Chunk 4 Task 8 updates `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` and refreshes the prompt fixtures.

- [x] **Step 1: Add the Technical Analyst parser and prompt regressions in `crates/scorpio-core/src/agents/analyst/equity/technical.rs`**

Add these exact tests:

```rust
#[test]
fn parses_technical_with_options_summary() { ... }

#[test]
fn technical_tool_renders_options_outcome_variant_with_reason() { ... }

#[test]
fn technical_analyst_new_stays_infallible_for_canonical_equity_symbol() { ... }

#[test]
fn technical_analyst_new_propagates_symbol_from_state_without_reparsing() { ... }
```

Keep the earlier `technical_data_missing_options_summary_defaults_to_none` test from Task 1.

- [x] **Step 2: Add the stale-state regression in `crates/scorpio-core/src/workflow/pipeline/tests.rs`**

Add an async test named `run_analysis_cycle_clears_stale_options_summary_from_reused_state`. Seed a reused `TradingState` with a stale `options_summary`, run the stubbed pipeline, and assert the final state clears or overwrites it. If the test passes today against the existing `reset_cycle_outputs()` / `clear_equity()` path, also add a sibling test `clear_equity_resets_options_summary_unit` that explicitly proves the lifecycle invariant at the unit level so the next refactor can't silently break it. (This is the load-bearing invariant the original plan deferred; codify it instead of relying on integration coverage.)

- [x] **Step 3: Add the technical-evidence dataset regression in `crates/scorpio-core/src/workflow/tasks/tests.rs`**

Add a test named `technical_evidence_includes_options_snapshot_dataset_when_options_summary_present` that proves `EvidenceSource.datasets` becomes `vec!["ohlcv", "options_snapshot"]` when the technical payload contains an options summary and remains `vec!["ohlcv"]` otherwise.

- [x] **Step 4: Run the focused Technical Analyst slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(parses_technical_with_options_summary) | test(technical_tool_renders_options_outcome_variant_with_reason) | test(technical_analyst_new_stays_infallible_for_canonical_equity_symbol) | test(technical_analyst_new_propagates_symbol_from_state_without_reparsing) | test(run_analysis_cycle_clears_stale_options_summary_from_reused_state) | test(clear_equity_resets_options_summary_unit) | test(technical_evidence_includes_options_snapshot_dataset_when_options_summary_present)'`

Expected: FAIL because the tool is not wired and the dataset logic does not know about `options_summary` yet.

- [x] **Step 5: Bind `GetOptionsSnapshot` inside `crates/scorpio-core/src/agents/analyst/equity/technical.rs`**

Keep the live graph plumbing minimal. **Make `TechnicalAnalyst::new` fallible** (`-> Result<Self, TradingError>`) so a missing/unparseable Symbol is a loud, propagated error instead of a release-mode silent no-op:

- Source the typed `crate::domain::Symbol` from `state.symbol: Option<Symbol>`. If `state.symbol.is_none()`, return `Err(TradingError::SchemaViolation { ... })` with a message naming the offending callsite (e.g. `"TechnicalAnalyst::new called with state.symbol = None; expected canonicalized symbol from TradingState::new"`). Do not re-parse the rendered ticker string and do not `debug_assert` (that would no-op in release).
- Update existing call sites that construct `TechnicalAnalyst::new(...)` (live graph spawning + test helpers) to propagate the `Result`. The cascading change is intentional: the live graph already returns `Result` from analyst-task setup; test helpers can `.expect("test fixture must canonicalize symbol")` because their state is curated.
- Widen `TechnicalAnalyst` to carry both the rendered ticker string (still used for prompt/tool scoping for backward compatibility) and the typed `Symbol`.
- Construct `Arc::new(YFinanceOptionsProvider::new(self.yfinance.clone()))` inside `TechnicalAnalyst::run()`, add `GetOptionsSnapshot` to the existing tool vector, and allow the parsed output to carry `options_summary`.

Make the tool/provider contract align with the structured outcome enum from Task 6: `OptionsProvider::fetch_snapshot(...) -> Result<OptionsOutcome, TradingError>`. The `GetOptionsSnapshot` tool serializes the variant **with a sibling `reason: String` on every non-`Snapshot` arm** so the LLM never sees a bare discriminant tag. Concretely, serialize as:

```json
{ "kind": "no_listed_instrument", "reason": "this symbol has no listed options on Yahoo" }
{ "kind": "sparse_chain",         "reason": "options exist but no usable contracts within ±20% of spot" }
{ "kind": "historical_run",       "reason": "target_date is not market-local US/Eastern today; live options intentionally skipped" }
{ "kind": "missing_spot",         "reason": "no underlying close price available for target_date" }
{ "kind": "snapshot", ...payload }
```

The `reason` strings are static (not LLM-templated) and live next to the enum definition so they evolve with the variant. Leave prompt-level discoverability to Chunk 4 Task 8.

- [x] **Step 6: Update the technical evidence datasets in `crates/scorpio-core/src/workflow/tasks/analyst.rs`**

Append `"options_snapshot"` to the technical `EvidenceSource.datasets` only when `data.options_summary.is_some()`. Leave the news evidence source list and `EventNewsEvidence` path unchanged.

- [x] **Step 7: Re-run the focused Technical Analyst slice**

Run the command from Step 4.

Expected: PASS.

- [x] **Step 8: Hold the Technical Analyst options wiring commit until Task 8 lands**

Stage the changes but do NOT commit yet. The wired-but-unprompted state would silently ship a tool the model can't discover. Continue directly into Task 8 to edit the prompt + refresh fixtures, then commit Tasks 7 and 8 together via the Task 8 Step 6 commit. This intentionally collapses what was previously two commits into one to close the prompt/code drift window.

Stage:

```bash
git add crates/scorpio-core/src/agents/analyst/equity/technical.rs \
        crates/scorpio-core/src/workflow/tasks/analyst.rs \
        crates/scorpio-core/src/workflow/tasks/tests.rs \
        crates/scorpio-core/src/workflow/pipeline/tests.rs
```

## Chunk 4: Prompt Fixtures, Live Smoke, and Final Verification

### Task 8: Refresh the technical prompt fixture after the markdown change

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md`
- Modify: `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt`

- [x] **Step 1: Edit the Technical Analyst markdown prompt for the options tool**

Edit `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` so it:

- lists `get_options_snapshot` in the runtime tools
- adds `options_summary` to the allowed output fields
- tells the model to omit options analysis when the options snapshot is `null` / unavailable, including historical runs where live options data is intentionally skipped
- keeps the rest of the prompt unchanged

- [x] **Step 2: Run the prompt-bundle regression gate without fixture updates**

Run: `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`

Expected: FAIL because `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` changed intentionally.

- [x] **Step 3: Regenerate the prompt fixtures with the exact blessed command**

Run: `UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`

- [x] **Step 4: Enforce a precise fixture-diff scope assertion**

Run `git status --short crates/scorpio-core/tests/fixtures/prompt_bundle/`. Expected: exactly one modified path, `technical_analyst.txt`. If any other fixture file changed (e.g. `fundamental_analyst.txt`, `news_analyst.txt`, `sentiment_analyst.txt`, `user/*.txt`), the cascade is unintentional — Task 3's `agents/shared/prompt.rs` edits are leaking into roles they shouldn't, or rendering order changed. Stop, investigate the leak, and either fix the upstream rendering change or scope-limit the fixture regeneration. Do not commit any unintended fixture drift; the `UPDATE_FIXTURES=1` flag silently rewrites all fixtures, which is exactly the failure mode this step exists to catch.

- [x] **Step 5: Re-run the gate without `UPDATE_FIXTURES`**

Run: `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`

Expected: PASS.

- [x] **Step 6: Commit Task 7 wiring + Task 8 prompt + fixture refresh together**

Task 7 Step 8 staged the wiring; this step adds the prompt + fixture and creates a single commit closing the prompt/code drift window. The `git add` must explicitly include the Task 7 staged files so they land in the same commit:

```bash
git add crates/scorpio-core/src/agents/analyst/equity/technical.rs \
        crates/scorpio-core/src/workflow/tasks/analyst.rs \
        crates/scorpio-core/src/workflow/tasks/tests.rs \
        crates/scorpio-core/src/workflow/pipeline/tests.rs \
        crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md \
        crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt && \
git commit -m "feat(core): add technical analyst options snapshot tool with prompt"
```

### Task 9: Extend the live Yahoo smoke example for the new data streams

**Files:**
- Modify: `crates/scorpio-core/examples/yfinance_live_test.rs`

> **Dependency note:** Complete Chunk 1 Task 1 first. Section 7 assumes `crates/scorpio-core/src/state/news.rs::NewsArticle.url` already exists and is populated by the Finnhub/Yahoo normalization work.

- [x] **Step 1: Add sections 7-10 to `crates/scorpio-core/examples/yfinance_live_test.rs`**

Add these exact manual smoke sections:

- Section 7: `YFinanceNewsProvider::fetch(AAPL)` asserts non-empty `articles`, RFC3339 `published_at`, and non-empty URLs.
- Section 8: extended `YFinanceEstimatesProvider::fetch_consensus(AAPL, today)` asserts a positive price-target mean and at least one non-zero recommendation bucket; partial success logs `WARN` instead of failing when exactly one extra endpoint is temporarily unavailable.
- Section 9: `YFinanceOptionsProvider::fetch_snapshot(AAPL, today)` asserts `spot_price > 0`, plausible `atm_iv`, non-empty term structure, and non-empty near-term strikes.
- Section 10: SPY degradation coverage where Yahoo news may be empty with a `WARN`, options are expected to succeed (`OptionsOutcome::Snapshot`), and consensus may legitimately return `Ok(ConsensusOutcome::NoCoverage)` (or `Ok(ConsensusOutcome::Data)` with most fields `None`) without panicking. Do NOT expect the legacy `Ok(None)` shape — that arm no longer exists after Task 2 Step 6.

- [x] **Step 2: Run the live smoke example from the dedicated worktree**

Run: `cargo run -p scorpio-core --example yfinance_live_test`

Expected: the pass/fail tracker finishes with zero FAIL lines for the accepted AAPL and SPY scenarios above.

- [x] **Step 3: Adjust only the example assertions if live upstream behavior differs in an accepted way**

Keep the provider code unchanged unless the example exposed a real implementation bug. Use `WARN`, not `FAIL`, for accepted sparse-yet-valid upstream behavior.

- [x] **Step 4: Re-run the live smoke example**

Run the command from Step 2 again.

Expected: PASS.

- [x] **Step 5: Commit the live smoke coverage**

Run: `git add crates/scorpio-core/examples/yfinance_live_test.rs && git commit -m "test(core): expand yahoo live smoke coverage"`

### Task 10: Run final verification and hand off execution correctly

**Files:**
- No new file edits are expected in this task unless verification exposes a real bug.

- [x] **Step 1: Re-run a focused confidence slice across the new surfaces**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_enrichment_context_includes_price_target_and_recommendations) | test(merge_dedupes_by_url) | test(returns_snapshot_with_atm_iv_from_front_month_chain) | test(target_date_uses_market_local_us_eastern_not_utc) | test(returns_no_listed_instrument_when_expirations_empty) | test(returns_sparse_chain_when_band_and_fallback_yield_nothing) | test(run_analysis_cycle_hydrates_extended_consensus_enrichment) | test(run_analysis_cycle_clears_stale_options_summary_from_reused_state) | test(clear_equity_resets_options_summary_unit) | test(additive_fields_deserialize_when_struct_lacks_field) | test(fetch_consensus_returns_provider_degraded_when_price_target_errors_and_others_empty)'`

- [x] **Step 2: Run formatting exactly as CI does**

Run: `cargo fmt -- --check`

- [x] **Step 3: Run clippy exactly as CI does**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

- [x] **Step 4: Run nextest exactly as CI does**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast --test-threads=2`

- [x] **Step 5: Inspect the final worktree state**

Run: `git status --short`

Expected: only the intended plan-task changes remain.

- [x] **Step 6: Make one final cleanup commit only if verification required post-task fixes**

If Steps 2-4 forced last-minute code edits, stage only those fixes and create one small follow-up commit. Otherwise leave the branch as the task-by-task commit stack created above.

- [x] **Step 7: Hand off implementation via subagents, not a single long-running shell session**

Use `superpowers:subagent-driven-development` from the dedicated `feature/enrich-news-sources` worktree. Execute one task per fresh subagent, keep the focused test commands and commit boundaries above, and do not stop before Steps 2-4 are green.

### Task 11: Fixture-driven outcome smoke + design-doc retraction + CLAUDE.md update

**Files:**
- Create: `crates/scorpio-core/tests/fixtures/options_outcomes/` (frozen `StubbedFinancialResponses` per scenario)
- Create: `crates/scorpio-core/tests/options_outcome_smoke.rs`
- Modify: `docs/superpowers/specs/2026-04-24-yfinance-news-options-consensus-design.md` (the design spec carrying the stale `THESIS_MEMORY_SCHEMA_VERSION 3 -> 4` line at lines 96, 384, 432) to retract the schema-bump rule
- Modify: `CLAUDE.md` to land the `deny_unknown_fields` rule (Task 1 Step 6 already mandates this; Task 11 closes the loop on the doc edit)

> **Rationale.** The earlier draft of this task added a rubric-driven LLM eval with a stub keyed by prompt hash. Three review passes converged that the stub-LLM gate tests the stub, not the analyst, and that a per-fixture non-regression gate ratchets prompt evolution without measuring real signal quality. Scoping back to fixture-driven outcome smoke: deterministic, fast, no LLM, no rubric. Real-LLM quality eval is a separate decision tracked outside this plan.

- [x] **Step 1: Update the design spec to retract the stale schema-bump rule**

Edit `docs/superpowers/specs/2026-04-24-yfinance-news-options-consensus-design.md` so its `THESIS_MEMORY_SCHEMA_VERSION 3 -> 4` references at lines 96, 384, and 432 either (a) are removed or (b) explicitly cross-reference CLAUDE.md's "additive fields stay on v3 with `#[serde(default)]`" rule. This converts the precedent from "plan silently overrides design spec in prose" to "design spec + CLAUDE.md + plan agree on the same rule."

> **Doc-ownership note.** The design spec lives under `docs/superpowers/specs/` and may have a different author from this implementation plan. If the spec author / owner is not this plan's author, raise the retraction as a separate micro-PR for their sign-off before merging this branch — do NOT roll it silently into the Task 11 commit. Only commit the spec edit here if the implementer is also the spec author or has explicit approval.

- [x] **Step 2: Document the deferred Sentiment/Risk options-routing decision**

Add a one-paragraph "Deferred decisions" section to `docs/superpowers/specs/2026-04-24-yfinance-news-options-consensus-design.md` stating: options data is Technical-Analyst-scoped for v1; cross-analyst access via `data/routing.rs::derivatives` (and reconciliation between the new `OptionsProvider` and the existing `DerivativesProvider` placeholder) is a deferred decision pending a concrete request from a Sentiment or Risk agent author. Note that the trigger is a written request, not an unowned demand signal.

- [x] **Step 3: Update CLAUDE.md with the snapshotted-state-only `deny_unknown_fields` rule**

Edit CLAUDE.md's "TradingState schema evolution" bullet list to add the rule from Task 1 Step 6. Keep the wording scoped: snapshotted state structs reachable from `TradingState` must not use `#[serde(deny_unknown_fields)]`; RPC, tool-argument, and config types are unaffected.

- [x] **Step 4: Build the outcome-smoke fixture set**

Create `crates/scorpio-core/tests/fixtures/options_outcomes/` with frozen `StubbedFinancialResponses` JSON per scenario:

- `aapl_snapshot.json` — AAPL-shaped chain that should return `Snapshot(_)` with non-empty `near_term_strikes`.
- `no_listed.json` — empty expirations list → `NoListedInstrument`.
- `sparse_chain.json` — strikes at $0.50, $1.00, $5.00 with spot $1.50 (the +233% case from the NTM-cap test) → `SparseChain` after capped expansion.
- `historical_run.json` — `target_date` = 30 days before market-local today → `HistoricalRun`.
- `missing_spot.json` — empty `daily_close` → `MissingSpot`.

These fixtures double-purpose as input data for Task 6 Step 1's outcome tests. Reuse, don't duplicate.

- [x] **Step 5: Add a deterministic outcome-smoke test in `crates/scorpio-core/tests/options_outcome_smoke.rs`**

For each fixture, the test:

1. Loads the JSON into `StubbedFinancialResponses`.
2. Calls `YFinanceOptionsProvider::fetch_snapshot(&symbol, target_date)` directly (no LLM, no rig pipeline).
3. Asserts the returned `OptionsOutcome` variant matches the fixture's expected variant.
4. For `Snapshot(_)`, asserts the JSON-serialized payload has the expected keys (`spot_price`, `atm_iv`, `near_term_strikes`, etc.) — schema-level not value-level.

This is integration-test-level coverage that the live pipeline path serializes outcomes correctly. It does NOT measure analysis quality.

- [x] **Step 6: Run the smoke**

Run: `cargo nextest run -p scorpio-core --test options_outcome_smoke --features test-helpers`

Expected: PASS.

- [x] **Step 7: Commit the smoke + CLAUDE.md edit (and the design-spec retraction only if Step 1's doc-ownership condition was met)**

If the design-spec retraction was raised as a separate PR per Step 1's note, omit the spec from this commit:

```bash
git add crates/scorpio-core/tests/fixtures/options_outcomes/ \
        crates/scorpio-core/tests/options_outcome_smoke.rs \
        CLAUDE.md && \
git commit -m "test(core): add options outcome smoke and snapshotted-state serde rule"
```

If the spec author approved the retraction inline, include it:

```bash
git add crates/scorpio-core/tests/fixtures/options_outcomes/ \
        crates/scorpio-core/tests/options_outcome_smoke.rs \
        docs/superpowers/specs/2026-04-24-yfinance-news-options-consensus-design.md \
        CLAUDE.md && \
git commit -m "test(core): add options outcome smoke and reconcile snapshot-version docs"
```
