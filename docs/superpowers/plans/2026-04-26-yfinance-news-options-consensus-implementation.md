# YFinance News, Options Snapshot, and Extended Consensus Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Yahoo company news, a Technical Analyst options snapshot tool, and extended analyst-consensus enrichment without breaking persisted snapshots, live fail-open behavior, or existing prompt/report contracts.

**Architecture:** Keep the live graph path centered on `crates/scorpio-core/src/workflow/pipeline/runtime.rs` and `crates/scorpio-core/src/workflow/tasks/analyst.rs`. Extend the existing Yahoo/Finnhub wrappers and `StubbedFinancialResponses`, keep consensus as pre-debate enrichment, merge Yahoo news only into analyst cached news, and keep options scoped to `crates/scorpio-core/src/agents/analyst/equity/technical.rs` instead of adding new routing or pack-wide provider plumbing.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `chrono`, `rig`, `yfinance-rs`, `finnhub`, `cargo nextest`, `cargo fmt`, `cargo clippy`.

---

**Worktree:** Execute this plan from the dedicated `feature/enrich-news-sources` worktree created during brainstorming. Do not implement it from any of the prunable cleanup worktrees listed by `git worktree list`.

## Guardrails

- Treat the design doc's `THESIS_MEMORY_SCHEMA_VERSION 3 -> 4` line as stale for this repo. Per `AGENTS.md` and `docs/solutions/logic-errors/thesis-memory-deserialization-crash-on-stale-snapshot-2026-04-13.md`, additive fields stay on schema version `3` and deserialize via `#[serde(default)]`; only renamed, removed, or otherwise incompatible field changes trigger a schema bump.
- Extend `crates/scorpio-core/src/state/news.rs::NewsArticle` with `url: Option<String>` even though the original design draft said `NewsData` would stay unchanged; cross-provider dedupe and provenance need a stored URL.
- Keep `crates/scorpio-core/src/data/adapters/estimates.rs::PriceTargetSummary` grounded in the real upstream type: `mean`, `high`, `low`, and `analyst_count`. Do not invent `median`.
- Keep options data Technical-Analyst-scoped. Do not route it through `crates/scorpio-core/src/data/traits/derivatives.rs` or `crates/scorpio-core/src/data/routing.rs`.
- Do not add a true skew field to the first options snapshot contract. The current Yahoo chain data does not provide usable delta/greeks for a real 25-delta skew calculation in this slice.
- Keep `EventNewsEvidence` Finnhub-only. Yahoo company news supplements analyst cached news only.
- Preserve the live fallback behavior in `crates/scorpio-core/src/agents/analyst/mod.rs::prefetch_analyst_news`: return `None` when both prefetch providers fail so `GetNews` stays available to the live analyst tools.

## File Map

| Action | Path                                                                         | Responsibility                                                                                       |
|--------|------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------|
| Modify | `crates/scorpio-core/src/state/news.rs`                                      | Add snapshot-safe `NewsArticle.url` while keeping `NewsData` backward-compatible                     |
| Modify | `crates/scorpio-core/src/state/technical.rs`                                 | Add snapshot-safe `TechnicalData.options_summary`                                                    |
| Modify | `crates/scorpio-core/src/data/adapters/estimates.rs`                         | Extend `ConsensusEvidence` and implement partial-fail-open price-target/recommendation fetches       |
| Modify | `crates/scorpio-core/src/data/yfinance/ohlcv.rs`                             | Extend `StubbedFinancialResponses` for consensus, Yahoo news, and options fixtures                   |
| Modify | `crates/scorpio-core/src/data/yfinance/financials.rs`                        | Add result-preserving Yahoo wrappers for price target and recommendation summary                     |
| Create | `crates/scorpio-core/src/data/yfinance/news.rs`                              | Add Yahoo company-news wrapper helpers and `YFinanceNewsProvider`                                    |
| Create | `crates/scorpio-core/src/data/traits/options.rs`                             | Define the equity options snapshot contract                                                          |
| Modify | `crates/scorpio-core/src/data/traits/mod.rs`                                 | Export `OptionsProvider`                                                                             |
| Create | `crates/scorpio-core/src/data/yfinance/options.rs`                           | Normalize Yahoo option chains, compute summary metrics, and expose `GetOptionsSnapshot`              |
| Modify | `crates/scorpio-core/src/data/yfinance/mod.rs`                               | Export the new Yahoo news/options modules                                                            |
| Modify | `crates/scorpio-core/src/data/mod.rs`                                        | Re-export the new Yahoo news/options types needed by agents, examples, and tests                     |
| Modify | `crates/scorpio-core/src/data/finnhub.rs`                                    | Normalize Finnhub article URLs and timestamps into the shared state contract                         |
| Modify | `crates/scorpio-core/src/agents/analyst/mod.rs`                              | Merge Finnhub + Yahoo prefetch news with dedupe and live fallback preservation                       |
| Modify | `crates/scorpio-core/src/agents/analyst/equity/technical.rs`                 | Bind `GetOptionsSnapshot`, parse `options_summary`, and keep options local to the Technical Analyst  |
| Modify | `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` | Add Technical Analyst guidance for `get_options_snapshot` and `options_summary`                      |
| Modify | `crates/scorpio-core/src/agents/shared/prompt.rs`                            | Render richer consensus enrichment context                                                           |
| Modify | `crates/scorpio-core/src/workflow/pipeline/runtime.rs`                       | Call the dual-provider cached-news prefetch from the live graph path                                 |
| Modify | `crates/scorpio-core/src/workflow/tasks/analyst.rs`                          | Include `options_snapshot` in technical evidence datasets when the summary is present                |
| Modify | `crates/scorpio-core/src/workflow/pipeline/tests.rs`                         | Add runtime hydration and stale-state regressions                                                    |
| Modify | `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`           | Prove additive fields deserialize under schema version `3`                                           |
| Modify | `crates/scorpio-core/tests/state_roundtrip.rs`                               | Extend proptest strategies for the additive fields                                                   |
| Modify | `crates/scorpio-reporters/src/terminal/final_report.rs`                      | Update `ConsensusEvidence` fixture literals so the workspace still compiles after the struct expands |
| Modify | `crates/scorpio-core/examples/yfinance_live_test.rs`                         | Add live Yahoo news/options/extended-consensus smoke sections                                        |

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

- [ ] **Step 1: Add the missing-URL regression in `crates/scorpio-core/src/agents/analyst/equity/news.rs`**

Add a unit test named `news_article_missing_url_defaults_to_none` that deserializes a `NewsData` JSON object whose article omits `url` and asserts `data.articles[0].url.is_none()`.

- [ ] **Step 2: Add the missing-options-summary regression in `crates/scorpio-core/src/agents/analyst/equity/technical.rs`**

Add a unit test named `technical_data_missing_options_summary_defaults_to_none` that deserializes a `TechnicalData` JSON object without `options_summary` and asserts the field defaults to `None`.

- [ ] **Step 3: Add the missing-extended-consensus regression in `crates/scorpio-core/src/data/adapters/estimates.rs`**

Add a unit test named `consensus_evidence_missing_extended_fields_defaults_to_none` that deserializes legacy JSON without `price_target` or `recommendations` and asserts both default to `None`.

- [ ] **Step 4: Add the additive-fields-on-schema-v3 regression in `crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs`**

Add a test named `additive_consensus_and_technical_fields_do_not_require_schema_bump` that writes a phase-5 snapshot row stamped with the current `THESIS_MEMORY_SCHEMA_VERSION`, removes the new additive keys from the stored JSON, and proves `load_prior_thesis_for_symbol()` still returns the thesis instead of skipping the row. This test intentionally codifies the repo correction that the design doc's version-bump line is stale for additive fields.

- [ ] **Step 5: Run the focused compatibility slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(news_article_missing_url_defaults_to_none) | test(technical_data_missing_options_summary_defaults_to_none) | test(consensus_evidence_missing_extended_fields_defaults_to_none) | test(additive_consensus_and_technical_fields_do_not_require_schema_bump)'`

Expected: FAIL because the additive fields do not exist yet.

- [ ] **Step 6: Add the additive fields with `#[serde(default)]` and keep schema version `3`**

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

Also add `PriceTargetSummary` and `RecommendationsSummary` in `crates/scorpio-core/src/data/adapters/estimates.rs`, but do not touch `crates/scorpio-core/src/workflow/snapshot/thesis.rs`. This is an intentional repo-level correction to the stale version-bump note in the design doc, not an omission.

- [ ] **Step 7: Update every explicit `NewsArticle` literal after the new field lands**

Set `url: None` or a concrete URL in each explicit constructor under `crates/scorpio-core/src/agents/analyst/equity/news.rs`, `crates/scorpio-core/src/agents/analyst/mod.rs`, `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/fund_manager/tests.rs`, `crates/scorpio-core/src/agents/trader/tests.rs`, `crates/scorpio-core/src/data/finnhub.rs`, and `crates/scorpio-core/tests/state_roundtrip.rs`.

- [ ] **Step 8: Update every explicit `TechnicalData` literal after the new field lands**

Set `options_summary: None` unless the fixture should explicitly exercise options behavior in `crates/scorpio-core/src/agents/analyst/equity/technical.rs`, `crates/scorpio-core/src/agents/analyst/mod.rs`, `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/fund_manager/tests.rs`, `crates/scorpio-core/src/agents/trader/tests.rs`, `crates/scorpio-core/src/indicators/batch.rs`, `crates/scorpio-core/src/testing/prompt_render.rs`, `crates/scorpio-core/src/workflow/pipeline/tests.rs`, `crates/scorpio-core/src/workflow/tasks/test_helpers.rs`, `crates/scorpio-core/src/workflow/tasks/tests.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`, `crates/scorpio-core/tests/support/workflow_observability_task_support.rs`, and `crates/scorpio-core/tests/workflow_pipeline_structure.rs`.

- [ ] **Step 9: Update every explicit `ConsensusEvidence` literal and proptest generator**

Add `price_target: None` and `recommendations: None` in `crates/scorpio-core/src/agents/fund_manager/prompt.rs`, `crates/scorpio-core/src/agents/shared/prompt.rs`, `crates/scorpio-core/src/data/adapters/estimates.rs`, `crates/scorpio-core/tests/state_roundtrip.rs`, and `crates/scorpio-reporters/src/terminal/final_report.rs`.

- [ ] **Step 10: Extend the proptest generators in `crates/scorpio-core/tests/state_roundtrip.rs`**

Teach `arb_news_article`, `arb_technical_data`, and `arb_consensus_evidence` about the new optional fields so the round-trip property test keeps covering the expanded persisted shape.

- [ ] **Step 11: Re-run the compatibility slice plus the round-trip integration test**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(news_article_missing_url_defaults_to_none) | test(technical_data_missing_options_summary_defaults_to_none) | test(consensus_evidence_missing_extended_fields_defaults_to_none) | test(additive_consensus_and_technical_fields_do_not_require_schema_bump) | binary(state_roundtrip)'`

Expected: PASS.

- [ ] **Step 12: Commit the additive-state foundation**

Run: `git add crates/scorpio-core/src/state/news.rs crates/scorpio-core/src/state/technical.rs crates/scorpio-core/src/data/adapters/estimates.rs crates/scorpio-core/src/workflow/snapshot/tests/thesis_compat.rs crates/scorpio-core/tests/state_roundtrip.rs crates/scorpio-reporters/src/terminal/final_report.rs crates/scorpio-core/src/agents/analyst/equity/news.rs crates/scorpio-core/src/agents/analyst/equity/technical.rs crates/scorpio-core/src/agents/analyst/mod.rs crates/scorpio-core/src/agents/fund_manager/prompt.rs crates/scorpio-core/src/agents/fund_manager/tests.rs crates/scorpio-core/src/agents/trader/tests.rs crates/scorpio-core/src/indicators/batch.rs crates/scorpio-core/src/testing/prompt_render.rs crates/scorpio-core/src/workflow/pipeline/tests.rs crates/scorpio-core/src/workflow/tasks/test_helpers.rs crates/scorpio-core/src/workflow/tasks/tests.rs crates/scorpio-core/tests/support/workflow_observability_task_support.rs crates/scorpio-core/tests/workflow_pipeline_structure.rs && git commit -m "feat(core): add snapshot-safe news and technical fields"`

### Task 2: Extend Yahoo consensus wrappers and implement partial-fail-open enrichment

**Files:**
- Modify: `crates/scorpio-core/src/data/yfinance/ohlcv.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/financials.rs`
- Modify: `crates/scorpio-core/src/data/adapters/estimates.rs`

- [ ] **Step 1: Add result-preserving Yahoo wrapper tests in `crates/scorpio-core/src/data/yfinance/financials.rs`**

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

- [ ] **Step 2: Add the provider-behavior regressions in `crates/scorpio-core/src/data/adapters/estimates.rs`**

Add these exact tests:

```rust
#[tokio::test]
async fn fetch_consensus_populates_price_target_and_recommendations() { ... }

#[tokio::test]
async fn fetch_consensus_returns_partial_when_earnings_trend_fails() { ... }

#[tokio::test]
async fn fetch_consensus_returns_partial_when_price_target_fails() { ... }

#[tokio::test]
async fn fetch_consensus_returns_partial_when_recommendations_fail() { ... }

#[tokio::test]
async fn fetch_consensus_returns_ok_none_when_all_endpoints_return_no_data() { ... }

#[tokio::test]
async fn fetch_consensus_returns_err_when_price_target_errors_and_other_endpoints_return_no_data() { ... }

#[tokio::test]
async fn fetch_consensus_returns_err_when_all_three_endpoints_fail() { ... }
```

- [ ] **Step 3: Run the focused consensus slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(get_analyst_price_target_result_preserves_yahoo_failure_reason) | test(get_recommendations_summary_result_preserves_yahoo_failure_reason) | test(fetch_consensus_populates_price_target_and_recommendations) | test(fetch_consensus_returns_partial_when_earnings_trend_fails) | test(fetch_consensus_returns_partial_when_price_target_fails) | test(fetch_consensus_returns_partial_when_recommendations_fail) | test(fetch_consensus_returns_ok_none_when_all_endpoints_return_no_data) | test(fetch_consensus_returns_err_when_price_target_errors_and_other_endpoints_return_no_data) | test(fetch_consensus_returns_err_when_all_three_endpoints_fail)'`

Expected: FAIL because the new wrappers and provider logic do not exist yet.

- [ ] **Step 4: Extend `StubbedFinancialResponses` with consensus fixtures in `crates/scorpio-core/src/data/yfinance/ohlcv.rs`**

Add these exact test-only fields:

```rust
pub price_target: Option<yfinance_rs::analysis::PriceTarget>,
pub price_target_error: Option<String>,
pub recommendation_summary: Option<yfinance_rs::analysis::RecommendationSummary>,
pub recommendation_summary_error: Option<String>,
```

- [ ] **Step 5: Add result-preserving Yahoo wrappers in `crates/scorpio-core/src/data/yfinance/financials.rs`**

Add `get_analyst_price_target_result()` and `get_recommendations_summary_result()` next to `get_earnings_trend_result()`, mirror the existing test-stub pattern, and convert all-empty upstream payloads into `Ok(None)` instead of `Some(default_struct)`.

- [ ] **Step 6: Implement partial-fail-open consensus fetch in `crates/scorpio-core/src/data/adapters/estimates.rs`**

Use `tokio::join!` to fetch earnings trend, analyst price target, and recommendation summary concurrently. Keep these exact semantics:

- `Ok(Some(evidence))` when at least one upstream branch produced usable data.
- `Ok(Some(evidence))` when `get_earnings_trend_result()` fails but price target and/or recommendation summary still produce usable fields; in that branch set `eps_estimate`, `revenue_estimate_m`, and `analyst_count` to `None`.
- `Ok(None)` only when all three branches succeeded and none produced usable data.
- `Err(...)` when all three branches failed with errors.
- `Err(...)` when one or more branches error and the remaining successful branches still yield no usable fields; this keeps the runtime status on `FetchFailed` instead of silently degrading to `NotAvailable`.
- `tracing::warn!` once per failed branch when the overall result still degrades to `Ok(Some(...))`.

- [ ] **Step 7: Re-run the focused consensus slice**

Run the command from Step 3.

Expected: PASS.

- [ ] **Step 8: Commit the extended consensus provider work**

Run: `git add crates/scorpio-core/src/data/yfinance/ohlcv.rs crates/scorpio-core/src/data/yfinance/financials.rs crates/scorpio-core/src/data/adapters/estimates.rs && git commit -m "feat(core): extend yahoo consensus enrichment"`

### Task 3: Render the richer consensus payload and prove live hydration still works

**Files:**
- Modify: `crates/scorpio-core/src/agents/shared/prompt.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/tests.rs`

- [ ] **Step 1: Add the richer prompt-render regression in `crates/scorpio-core/src/agents/shared/prompt.rs`**

Add a unit test named `build_enrichment_context_includes_price_target_and_recommendations` that asserts the rendered prompt context now includes mean/high/low target values plus the five recommendation buckets.

- [ ] **Step 2: Add the runtime hydration regression in `crates/scorpio-core/src/workflow/pipeline/tests.rs`**

Add an async test named `run_analysis_cycle_hydrates_extended_consensus_enrichment` that:

- clones `resolve_pack(PackId::Baseline)`
- sets `pack.enrichment_intent.consensus_estimates = true`
- builds the pipeline via `TradingPipeline::from_pack(...)`
- builds the input `TradingState` with `target_date = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string()` so the real `hydrate_consensus()` live-date gate is exercised
- injects `YFinanceClient::with_stubbed_financials(...)` with trend, price-target, and recommendation fixtures
- replaces downstream graph tasks with the existing stub helpers
- asserts `final_state.enrichment_consensus.payload` carries the new fields

- [ ] **Step 3: Run the focused render/hydration slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_enrichment_context_includes_price_target_and_recommendations) | test(run_analysis_cycle_hydrates_extended_consensus_enrichment)'`

Expected: FAIL because prompt rendering does not mention the new fields yet.

- [ ] **Step 4: Update `crates/scorpio-core/src/agents/shared/prompt.rs` to render the new consensus fields**

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

- [ ] **Step 5: Re-run the focused render/hydration slice**

Run the command from Step 3.

Expected: PASS.

- [ ] **Step 6: Commit the prompt-render and hydration coverage**

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

- [ ] **Step 1: Add the Finnhub normalization regressions in `crates/scorpio-core/src/data/finnhub.rs`**

Add these exact tests near the existing news helpers:

```rust
#[test]
fn normalize_finnhub_article_preserves_url() { ... }

#[test]
fn normalize_finnhub_article_formats_rfc3339_timestamp() { ... }
```

Both tests should exercise the shared normalization path used by `build_news_data()` and `get_market_news()`.

- [ ] **Step 2: Add the Yahoo news-provider regressions in `crates/scorpio-core/src/data/yfinance/news.rs` and wire the module into `crates/scorpio-core/src/data/yfinance/mod.rs`**

Add these exact tests:

```rust
#[tokio::test]
async fn fetches_and_normalizes_articles() { ... }

#[tokio::test]
async fn empty_feed_returns_empty_news_data() { ... }
```

Assert RFC3339 timestamps, preserved URLs, `snippet == ""` for Yahoo articles, and an empty `macro_events` list.

In the same step, add `pub mod news;` to `crates/scorpio-core/src/data/yfinance/mod.rs` so the new test file is compiled before the first red-state run.

- [ ] **Step 3: Run the focused news-normalization slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(normalize_finnhub_article_preserves_url) | test(normalize_finnhub_article_formats_rfc3339_timestamp) | test(fetches_and_normalizes_articles) | test(empty_feed_returns_empty_news_data)'`

Expected: FAIL because the shared helper and Yahoo provider do not exist yet.

- [ ] **Step 4: Extract a shared Finnhub article normalizer in `crates/scorpio-core/src/data/finnhub.rs`**

Create one small helper that both `build_news_data()` and `get_market_news()` use. It must:

- trim/empty-check URLs into `Option<String>`
- convert unix-second timestamps into RFC3339 strings
- keep the existing title/snippet sanitization rules

- [ ] **Step 5: Extend `StubbedFinancialResponses` and implement `YFinanceNewsProvider`**

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

- [ ] **Step 6: Export the Yahoo news provider surface**

Keep the `pub mod news;` declaration from Step 2, then add any needed `pub use` exports in `crates/scorpio-core/src/data/yfinance/mod.rs` and `crates/scorpio-core/src/data/mod.rs` so runtime code, examples, and tests can import the provider without reaching into private modules.

- [ ] **Step 7: Re-run the focused news-normalization slice**

Run the command from Step 3.

Expected: PASS.

- [ ] **Step 8: Commit the normalized news-provider foundation**

Run: `git add crates/scorpio-core/src/data/finnhub.rs crates/scorpio-core/src/data/yfinance/ohlcv.rs crates/scorpio-core/src/data/yfinance/news.rs crates/scorpio-core/src/data/yfinance/mod.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/workflow/tasks/tests.rs && git commit -m "feat(core): add yahoo analyst news provider"`

### Task 5: Merge Finnhub and Yahoo cached news without breaking the live fallback

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/mod.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs`

- [ ] **Step 1: Add the merge/dedupe regressions in `crates/scorpio-core/src/agents/analyst/mod.rs`**

Add these exact tests:

```rust
#[tokio::test]
async fn merge_dedupes_by_url() { ... }

#[tokio::test]
async fn merge_dedupes_by_headline_when_url_missing() { ... }

#[tokio::test]
async fn merge_falls_back_to_single_provider_on_partial_failure() { ... }

#[tokio::test]
async fn prefetch_analyst_news_returns_none_when_both_prefetch_providers_fail() { ... }

#[tokio::test]
async fn merge_sorts_articles_newest_first() { ... }
```

- [ ] **Step 2: Run the focused merge slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(merge_dedupes_by_url) | test(merge_dedupes_by_headline_when_url_missing) | test(merge_falls_back_to_single_provider_on_partial_failure) | test(prefetch_analyst_news_returns_none_when_both_prefetch_providers_fail) | test(merge_sorts_articles_newest_first)'`

Expected: FAIL because the merge helper still only prefetches Finnhub.

- [ ] **Step 3: Implement the cached-news merge helpers in `crates/scorpio-core/src/agents/analyst/mod.rs`**

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

- [ ] **Step 4: Update the live and legacy callers to use both news providers**

In `crates/scorpio-core/src/workflow/pipeline/runtime.rs` and the legacy `run_analyst_team()` path inside `crates/scorpio-core/src/agents/analyst/mod.rs`, construct `YFinanceNewsProvider::new(yfinance.clone())` locally and pass both providers into `prefetch_analyst_news()`. Do not change `EventNewsEvidence` or the cached-news context key shape.

- [ ] **Step 5: Re-run the focused merge slice**

Run the command from Step 2.

Expected: PASS.

- [ ] **Step 6: Commit the merged cached-news path**

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

- [ ] **Step 1: Add the Yahoo options-provider regressions in `crates/scorpio-core/src/data/yfinance/options.rs` and wire the module into `crates/scorpio-core/src/data/yfinance/mod.rs`**

Add these exact tests:

```rust
#[tokio::test]
async fn computes_atm_iv_from_chain() { ... }

#[tokio::test]
async fn computes_put_call_ratios_over_all_strikes() { ... }

#[tokio::test]
async fn computes_max_pain_front_month_only() { ... }

#[tokio::test]
async fn near_term_slice_filters_to_ntm_band() { ... }

#[tokio::test]
async fn returns_none_when_no_options_listed() { ... }

#[tokio::test]
async fn returns_none_for_historical_target_date() { ... }

#[tokio::test]
async fn returns_err_when_expiration_lookup_fails() { ... }

#[tokio::test]
async fn returns_err_when_option_chain_fetch_fails() { ... }

#[tokio::test]
async fn ignores_missing_greeks_and_skips_true_skew_metric() { ... }
```

In the same step, add `pub mod options;` to `crates/scorpio-core/src/data/yfinance/mod.rs` so the new module is compiled before the first red-state run.

- [ ] **Step 2: Run the focused options slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(computes_atm_iv_from_chain) | test(computes_put_call_ratios_over_all_strikes) | test(computes_max_pain_front_month_only) | test(near_term_slice_filters_to_ntm_band) | test(returns_none_when_no_options_listed) | test(returns_none_for_historical_target_date) | test(returns_err_when_expiration_lookup_fails) | test(returns_err_when_option_chain_fetch_fails) | test(ignores_missing_greeks_and_skips_true_skew_metric)'`

Expected: FAIL because the contract and provider do not exist yet.

- [ ] **Step 3: Create `crates/scorpio-core/src/data/traits/options.rs` without a skew field**

Define `OptionsProvider`, `OptionsSnapshot`, `IvTermPoint`, and `NearTermStrike`. Match the existing `crates/scorpio-core/src/data/traits/*.rs` pattern exactly:

```rust
#[async_trait]
pub trait OptionsProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn fetch_snapshot(
        &self,
        symbol: &crate::domain::Symbol,
        target_date: &str,
    ) -> Result<Option<OptionsSnapshot>, TradingError>;
}
```

Import `schemars::JsonSchema` and derive `Debug`, `Clone`, `PartialEq`, `Serialize`, `Deserialize`, and `JsonSchema` on `OptionsSnapshot`, `IvTermPoint`, and `NearTermStrike`.

Keep the summary grounded in the upstream data that actually exists:

- `spot_price`
- `atm_iv`
- `iv_term_structure`
- `put_call_volume_ratio`
- `put_call_oi_ratio`
- `max_pain_strike`
- `near_term_expiration`
- `near_term_strikes`

Do not add `skew_25d` or any other pseudo-delta metric in this slice.

- [ ] **Step 4: Extend `StubbedFinancialResponses` with options fixtures in `crates/scorpio-core/src/data/yfinance/ohlcv.rs`**

Add these exact test-only fields:

```rust
pub option_expirations: Option<Vec<i64>>,
pub option_expirations_error: Option<String>,
pub option_chains: std::collections::BTreeMap<i64, yfinance_rs::ticker::OptionChain>,
pub option_chain_errors: std::collections::BTreeMap<i64, String>,
```

Also update the explicit `StubbedFinancialResponses { ... }` literals in `crates/scorpio-core/src/workflow/tasks/tests.rs` to use the new fields or `..StubbedFinancialResponses::default()` so test-only struct expansion does not break unrelated task tests.

Also update the explicit `StubbedFinancialResponses { ... }` literal in `crates/scorpio-core/src/data/adapters/estimates.rs` the same way so the focused consensus tests still compile after the struct expands.

- [ ] **Step 5: Implement `crates/scorpio-core/src/data/yfinance/options.rs`**

Add:

- small `YFinanceClient` wrappers for expiration dates and option chains
- reuse `crates/scorpio-core/src/data/yfinance/price.rs::get_latest_close(...)` for the underlying spot price instead of inventing a new quote path
- extract the equity ticker with the same pattern used in `crates/scorpio-core/src/data/provider_impls.rs::require_equity_ticker` before calling Yahoo helpers; reject non-equity symbols with `TradingError::SchemaViolation`
- `YFinanceOptionsProvider::new(client: YFinanceClient)`
- `OptionsProvider for YFinanceOptionsProvider`
- `GetOptionsSnapshot`

Keep the implementation minimal and local:

- use a local `const OPTIONS_NTM_STRIKE_BAND: f64 = 0.05`
- use a local `const OPTIONS_FETCH_TIMEOUT_SECS: u64 = 30`
- match `symbol.as_equity()` (or an equivalent local helper mirroring `require_equity_ticker`) and call `get_latest_close(&client, ticker.as_str(), target_date)` before computing ATM IV or the near-term strike band
- return `Ok(None)` when there are no expirations, no usable contracts, or `target_date` is not today; this prevents present-day option chains from leaking into historical analyses
- return `Ok(None)` when `get_latest_close(...)` returns `None`; without a spot price the provider cannot identify ATM or build the near-term strike slice
- return `Err(TradingError::...)` when expiration lookup or chain fetches fail; only true no-data and historical-date branches degrade to `Ok(None)`
- compute front-month `atm_iv`, term structure, put/call ratios, max pain, and the near-term strike slice

- [ ] **Step 6: Export the options surface**

Keep the `pub mod options;` declaration from Step 1, then re-export the new trait from `crates/scorpio-core/src/data/traits/mod.rs` and add any needed `pub use` exports in `crates/scorpio-core/src/data/yfinance/mod.rs` and `crates/scorpio-core/src/data/mod.rs`.

- [ ] **Step 7: Re-run the focused options slice**

Run the command from Step 2.

Expected: PASS.

- [ ] **Step 8: Commit the options contract and provider**

Run: `git add crates/scorpio-core/src/data/traits/options.rs crates/scorpio-core/src/data/traits/mod.rs crates/scorpio-core/src/data/adapters/estimates.rs crates/scorpio-core/src/data/yfinance/ohlcv.rs crates/scorpio-core/src/data/yfinance/options.rs crates/scorpio-core/src/data/yfinance/mod.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/workflow/tasks/tests.rs && git commit -m "feat(core): add yahoo options snapshot provider"`

### Task 7: Wire the scoped options tool into the Technical Analyst and persist `options_summary`

**Files:**
- Modify: `crates/scorpio-core/src/agents/analyst/equity/technical.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/tests.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/tests.rs`

> **Dependency note:** Complete Chunk 1 Task 1 first so `crates/scorpio-core/src/state/technical.rs::TechnicalData.options_summary` and the broad `TechnicalData` literal updates already exist before starting this task. This task is not startable from current repo HEAD until that earlier commit lands on the branch. The prompt-fixture refresh chunk will edit `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md`; this task intentionally avoids prompt-file changes so the repo does not stay red between chunks.

> **Runtime note:** This task only wires the code path and persisted output for `GetOptionsSnapshot`. The Technical Analyst does not learn to call the tool until Chunk 4 Task 8 updates `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` and refreshes the prompt fixtures.

- [ ] **Step 1: Add the Technical Analyst parser and prompt regressions in `crates/scorpio-core/src/agents/analyst/equity/technical.rs`**

Add these exact tests:

```rust
#[test]
fn parses_technical_with_options_summary() { ... }

#[test]
fn technical_tool_contract_treats_null_options_snapshot_as_skip_signal() { ... }
```

Keep the earlier `technical_data_missing_options_summary_defaults_to_none` test from Task 1.

- [ ] **Step 2: Add the stale-state regression in `crates/scorpio-core/src/workflow/pipeline/tests.rs`**

Add an async test named `run_analysis_cycle_clears_stale_options_summary_from_reused_state`. Seed a reused `TradingState` with a stale `options_summary`, run the stubbed pipeline, and assert the final state either clears it or overwrites it from the new cycle. If the test already passes after the additive-field work, do not change `reset_cycle_outputs()`.

- [ ] **Step 3: Add the technical-evidence dataset regression in `crates/scorpio-core/src/workflow/tasks/tests.rs`**

Add a test named `technical_evidence_includes_options_snapshot_dataset_when_options_summary_present` that proves `EvidenceSource.datasets` becomes `vec!["ohlcv", "options_snapshot"]` when the technical payload contains an options summary and remains `vec!["ohlcv"]` otherwise.

- [ ] **Step 4: Run the focused Technical Analyst slice and confirm the red state**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(parses_technical_with_options_summary) | test(technical_tool_contract_treats_null_options_snapshot_as_skip_signal) | test(run_analysis_cycle_clears_stale_options_summary_from_reused_state) | test(technical_evidence_includes_options_snapshot_dataset_when_options_summary_present)'`

Expected: FAIL because the tool is not wired and the dataset logic does not know about `options_summary` yet.

- [ ] **Step 5: Bind `GetOptionsSnapshot` inside `crates/scorpio-core/src/agents/analyst/equity/technical.rs`**

Keep the live graph plumbing minimal: widen `TechnicalAnalyst` to store both the rendered ticker string already used in prompts/tool scoping and a typed `crate::domain::Symbol` parsed during construction, then construct `Arc::new(YFinanceOptionsProvider::new(self.yfinance.clone()))` inside `TechnicalAnalyst::run()`, add `GetOptionsSnapshot` to the existing tool vector, and allow the parsed output to carry `options_summary`.

Make the tool/provider contract consistent: `OptionsProvider::fetch_snapshot(...) -> Result<Option<OptionsSnapshot>, TradingError>` and the tool should surface `null`/absent snapshot as the no-data signal rather than inventing a text payload like `"no listed options"`. Leave prompt-level discoverability to Chunk 4 Task 8.

- [ ] **Step 6: Update the technical evidence datasets in `crates/scorpio-core/src/workflow/tasks/analyst.rs`**

Append `"options_snapshot"` to the technical `EvidenceSource.datasets` only when `data.options_summary.is_some()`. Leave the news evidence source list and `EventNewsEvidence` path unchanged.

- [ ] **Step 7: Re-run the focused Technical Analyst slice**

Run the command from Step 4.

Expected: PASS.

- [ ] **Step 8: Commit the Technical Analyst options wiring**

Run: `git add crates/scorpio-core/src/agents/analyst/equity/technical.rs crates/scorpio-core/src/workflow/tasks/analyst.rs crates/scorpio-core/src/workflow/tasks/tests.rs crates/scorpio-core/src/workflow/pipeline/tests.rs && git commit -m "feat(core): add technical analyst options snapshot tool"`

## Chunk 4: Prompt Fixtures, Live Smoke, and Final Verification

### Task 8: Refresh the technical prompt fixture after the markdown change

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md`
- Modify: `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt`

- [ ] **Step 1: Edit the Technical Analyst markdown prompt for the options tool**

Edit `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` so it:

- lists `get_options_snapshot` in the runtime tools
- adds `options_summary` to the allowed output fields
- tells the model to omit options analysis when the options snapshot is `null` / unavailable, including historical runs where live options data is intentionally skipped
- keeps the rest of the prompt unchanged

- [ ] **Step 2: Run the prompt-bundle regression gate without fixture updates**

Run: `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`

Expected: FAIL because `crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md` changed intentionally.

- [ ] **Step 3: Regenerate the prompt fixtures with the exact blessed command**

Run: `UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`

- [ ] **Step 4: Inspect the fixture diff and keep only intentional prompt changes**

Confirm the expected prompt diff lands in `crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt`. If other fixture files changed, keep them only when the diff is a direct consequence of the intended Technical Analyst prompt update.

- [ ] **Step 5: Re-run the gate without `UPDATE_FIXTURES`**

Run: `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`

Expected: PASS.

- [ ] **Step 6: Commit the prompt fixture refresh**

Run: `git add crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md crates/scorpio-core/tests/fixtures/prompt_bundle/technical_analyst.txt && git commit -m "test(core): refresh technical analyst prompt fixture"`

### Task 9: Extend the live Yahoo smoke example for the new data streams

**Files:**
- Modify: `crates/scorpio-core/examples/yfinance_live_test.rs`

> **Dependency note:** Complete Chunk 1 Task 1 first. Section 7 assumes `crates/scorpio-core/src/state/news.rs::NewsArticle.url` already exists and is populated by the Finnhub/Yahoo normalization work.

- [ ] **Step 1: Add sections 7-10 to `crates/scorpio-core/examples/yfinance_live_test.rs`**

Add these exact manual smoke sections:

- Section 7: `YFinanceNewsProvider::fetch(AAPL)` asserts non-empty `articles`, RFC3339 `published_at`, and non-empty URLs.
- Section 8: extended `YFinanceEstimatesProvider::fetch_consensus(AAPL, today)` asserts a positive price-target mean and at least one non-zero recommendation bucket; partial success logs `WARN` instead of failing when exactly one extra endpoint is temporarily unavailable.
- Section 9: `YFinanceOptionsProvider::fetch_snapshot(AAPL, today)` asserts `spot_price > 0`, plausible `atm_iv`, non-empty term structure, and non-empty near-term strikes.
- Section 10: SPY degradation coverage where Yahoo news may be empty with a `WARN`, options are expected to succeed, and consensus may legitimately return `Ok(None)` or a fully-empty optional payload without panicking.

- [ ] **Step 2: Run the live smoke example from the dedicated worktree**

Run: `cargo run -p scorpio-core --example yfinance_live_test`

Expected: the pass/fail tracker finishes with zero FAIL lines for the accepted AAPL and SPY scenarios above.

- [ ] **Step 3: Adjust only the example assertions if live upstream behavior differs in an accepted way**

Keep the provider code unchanged unless the example exposed a real implementation bug. Use `WARN`, not `FAIL`, for accepted sparse-yet-valid upstream behavior.

- [ ] **Step 4: Re-run the live smoke example**

Run the command from Step 2 again.

Expected: PASS.

- [ ] **Step 5: Commit the live smoke coverage**

Run: `git add crates/scorpio-core/examples/yfinance_live_test.rs && git commit -m "test(core): expand yahoo live smoke coverage"`

### Task 10: Run final verification and hand off execution correctly

**Files:**
- No new file edits are expected in this task unless verification exposes a real bug.

- [ ] **Step 1: Re-run a focused confidence slice across the new surfaces**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_enrichment_context_includes_price_target_and_recommendations) | test(merge_dedupes_by_url) | test(computes_atm_iv_from_chain) | test(run_analysis_cycle_hydrates_extended_consensus_enrichment) | test(run_analysis_cycle_clears_stale_options_summary_from_reused_state)'`

- [ ] **Step 2: Run formatting exactly as CI does**

Run: `cargo fmt -- --check`

- [ ] **Step 3: Run clippy exactly as CI does**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 4: Run nextest exactly as CI does**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`

- [ ] **Step 5: Inspect the final worktree state**

Run: `git status --short`

Expected: only the intended plan-task changes remain.

- [ ] **Step 6: Make one final cleanup commit only if verification required post-task fixes**

If Steps 2-4 forced last-minute code edits, stage only those fixes and create one small follow-up commit. Otherwise leave the branch as the task-by-task commit stack created above.

- [ ] **Step 7: Hand off implementation via subagents, not a single long-running shell session**

Use `superpowers:subagent-driven-development` from the dedicated `feature/enrich-news-sources` worktree. Execute one task per fresh subagent, keep the focused test commands and commit boundaries above, and do not stop before Steps 2-4 are green.
