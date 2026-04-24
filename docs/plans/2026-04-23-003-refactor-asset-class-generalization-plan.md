---
title: "refactor: Asset Class Generalization"
type: refactor
status: draft
date: 2026-04-23
---

# refactor: Asset Class Generalization

## Overview

Reshape `scorpio-core` so that adding new asset classes (crypto first, later commodities / FX) becomes purely additive. The refactor lifts equity-specific framing out of seven architectural layers — domain types, analyst composition, data providers, prompts, valuation, state shape, and workflow assembly — replacing hard-coded call graphs with trait-driven registries that the analysis pack drives at runtime.

The refactor is **behavior-preserving** for fresh equity (baseline) runs at every phase boundary. Thesis-memory continuity across the Phase 6 upgrade is an accepted breaking change: pre-v2 snapshot rows become unsupported and are not carried forward. Crypto lands as file-level scaffolding only (empty stubs, unreachable pack) so the abstractions are exercised by two consumers during design but only one produces output. A follow-up change populates the crypto pack.

No new workspace member is introduced; the existing three-crate layout (`scorpio-core`, `scorpio-cli`, `scorpio-reporters`) is preserved.

## Problem Frame

The codebase is ~70% symbol-agnostic at the type level — indicators, debate orchestration, snapshot persistence, and reporters don't care what an `asset_symbol` represents. But four concrete coupling points force equity framing throughout:

1. **Analyst fan-out is hard-coded.** `workflow/pipeline/runtime.rs:94-107` and `agents/analyst/mod.rs:104-149` spawn `FundamentalAnalyst`, `SentimentAnalyst`, `NewsAnalyst`, `TechnicalAnalyst` unconditionally. The pack manifest's `required_inputs: Vec<String>` field is already read by `workflow/tasks/analyst.rs:77` for graceful degradation but not for composition. Crypto needs a different analyst set (tokenomics, on-chain, derivatives, social), not the equity four.
2. **Data providers are concrete structs embedded in the pipeline.** `FinnhubClient`, `YFinanceClient`, `FredClient` are held as fields on `TradingPipeline` (`pipeline/mod.rs:97-108`) with no trait surface. Any crypto provider (Messari / DeFiLlama / GeckoTerminal) would have to either pretend to be Finnhub-shaped or fork the pipeline struct.
3. **Prompts are inlined as `const X_SYSTEM_PROMPT: &str`.** 17 agent files own their own prompt literals. Swapping voice per asset class means either duplicating agent modules or pack-parameterizing the prompt source.
4. **`TradingState` has equity-shaped fields at root.** `fundamental_metrics`, `market_volatility` (VIX-derived), `macro_news`, `market_sentiment`, `evidence_*` sit alongside domain-agnostic fields (`debate_history`, `trader_proposal`). A crypto pipeline would leave half unset and need orthogonal fields (unlock calendar, funding rate, on-chain flow).

These four pain points are the primary runtime hotspots. Most surrounding infrastructure — indicators, rate limiter, snapshot store, token accounting, and CLI — already operates on types that don't assume an asset class, while reporters need explicit compatibility work in Phase 6 because they read current `TradingState` root fields directly.

## Scope Boundaries

**In scope:**

- Typed `Symbol` / `AssetClass` / expanded `AssetShape`.
- `Analyst` trait + `AnalystRegistry`; dynamic fan-out composition from pack.
- Domain-split `DataProvider` traits; existing clients migrate behind them.
- `PromptBundle` on the pack manifest; prompts move to `.md` files loaded via `include_str!`.
- `Valuator` trait; `derive_valuation` becomes a compat shim.
- `TradingState` reshape (option C: coexisting optional `equity` / `crypto` / shared fields).
- `THESIS_MEMORY_SCHEMA_VERSION` bump (1 → 2) with explicit same-version-only snapshot handling.

**Explicitly out of scope:**

- **Actual crypto implementation.** All crypto analyst / provider / valuator / state files exist as empty placeholders with `// TODO: implement in crypto-pack change` comments. The crypto pack is registered but not user-selectable in this slice; its manifest remains valid under existing pack validation.
- **Workspace crate split.** No new `scorpio-domain`, `scorpio-providers`, or `scorpio-packs` crate.
- **Prompt A/B framework.** `PromptBundle` is content-hashed; no eval harness or A/B routing.
- **Runtime-loaded packs from filesystem.** `Cow<'static, str>` allows it; no loader code this slice.
- **Removal of the transitional `asset_symbol: String` field.** Kept through Phase 6 for serde back-compat; removed in a later cleanup.
- **Reporter crate feature expansion.** No new reporter formats or presentation features in this slice beyond the compatibility work required by the Phase 6 `TradingState` reshape.
- **SQL migration for snapshot DB.** No SQL migration runs in this slice; pre-v2 snapshot rows become unsupported after Phase 6 and are skipped / rejected by schema-version checks.
- **Automatic local snapshot cleanup.** Developers may delete `~/.scorpio-analyst/phase_snapshots.db` manually for a clean slate, but release behavior must not depend on truncating or deleting the DB.
- **Backward-compat shim for `validate_symbol(&str)`.** Stays callable; CLI uses it for fail-fast UX.

## Context & Research

### Current-State Coupling Hotspots

- **`asset_symbol: String` appears in 44 files.** Every analyst task, preflight, researcher prompt, and reporter reads it as `&str`. Phase 1 keeps the field as a `Display`-backed transitional alias of the typed `Symbol` so migrations proceed without a flag-day rewrite.
- **`validate_symbol(&str) -> Result<&str, TradingError>`** in `data/symbol.rs` is the only grammar enforcer today. `resolve_symbol` wraps it into `ResolvedInstrument` but that type is underused (9 call sites, mostly preflight); the rest of the codebase threads raw strings.
- **`AssetShape`** already exists in `state/derived.rs` with three variants (`CorporateEquity`, `Fund`, `Unknown`). All existing `match` arms route `Fund | Unknown => NotAssessed`, so a `_ => NotAssessed` catch-all preserves exhaustiveness when crypto variants land.
- **Four equity analysts share a runtime surface** (`AnalystRuntimeConfig`, `run_analyst_inference`, `validate_summary_content`) but are instantiated by name in two places — `build_graph` (`workflow/pipeline/runtime.rs:94-107`) and `run_analyst_team` (`agents/analyst/mod.rs:104-149`). Both need registry-driven dispatch.
- **17 files own prompt literals** as `const X_SYSTEM_PROMPT: &str = "..."` — fundamental, sentiment, news, technical, bullish researcher, bearish researcher, moderator, trader, aggressive risk, conservative risk, neutral risk, risk moderator, fund manager, and a handful of shared builders.
- **Pack system shape is ready.** `AnalysisPackManifest` already distinguishes from `RuntimePolicy`. `PackId` is a single-variant enum awaiting extension. `required_inputs: Vec<String>` is already intended to drive fan-out.
- **Data clients have no traits.** `FinnhubClient`, `YFinanceClient`, `FredClient` are concrete with inherent methods. Trait migration is in-place: add `impl FundamentalsProvider for FinnhubClient {}` without changing signatures.
- **`derive_valuation`** unconditionally consumes `yfinance_rs::profile::Profile`. Phase 5 does not rip it out — it wraps it in a `DcfValuator` / `MultiplesValuator` trait impl, keeping the `pub fn` and its 16 tests intact.
- **Reporter crate coupling is shape-sensitive.** `scorpio-reporters` imports `scorpio_core::state::TradingState` and reads root fields like `asset_symbol`, `fundamental_metrics`, `market_sentiment`, `market_volatility`, `derived_valuation`, and `evidence_*` directly. Phase 6 therefore includes explicit reporter compatibility work in addition to `state/mod.rs` facade re-exports.
- **Snapshot compat primitive exists.** `workflow/snapshot/thesis.rs:12` holds `THESIS_MEMORY_SCHEMA_VERSION: i64 = 1`. Phase 6 tightens the current future-version guard into same-version-only handling for thesis lookup and direct snapshot reads, so old rows are treated as unsupported before deserialization. No SQL migration is needed.

### Institutional Learnings

- `docs/solutions/logic-errors/thesis-memory-deserialization-crash-on-stale-snapshot-2026-04-13.md` — Fail-open handling for stale snapshots is load-bearing. Phase 6 now enforces explicit same-version-only checks instead of relying on deserialization failure, but the broader requirement remains: stale rows must never crash a live run.
- `docs/solutions/best-practices/config-test-isolation-inline-toml-2026-04-11.md` — `ENV_LOCK` and inline TOML fixtures must be preserved; no direct Phase interaction but a reminder during any config-adjacent edit.

## Key Decisions

Five decisions must be locked before Phase 1 code begins. All five have recommended defaults; users confirmed the full set on 2026-04-23.

| ID | Question                                                                                                                  | Decision                                                                                                                                                                                  |
|----|---------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| D1 | `AnalystId` as enum vs. string-newtype?                                                                                   | **Enum with `#[non_exhaustive]`** — compile-time exhaustive dispatch, SemVer-safe for external packs.                                                                                     |
| D2 | `DataNeed` granularity — coarse (`Fundamentals`, `News`, `PriceHistory`) or fine (`RevenueGrowth`, `EpsForwardEstimate`)? | **Coarse** — matches today's `required_inputs: Vec<String>` vocabulary. Fine granularity is a v2 concern.                                                                                 |
| D3 | `PromptBundle` slot type — `&'static str`, `Cow<'static, str>`, or `String`?                                              | **`Cow<'static, str>`** — baseline stays zero-alloc via `include_str!`; runtime-loaded packs supported via `Cow::Owned`.                                                                  |
| D4 | Phase 6 access pattern — accessor methods, or require call sites to pattern-match `state.equity`?                         | **Accessor methods** (`state.fundamental_metrics() -> Option<&FundamentalData>`) — turns 40 structural edits into mechanical search-replace. Accessors can be dropped in a later cleanup. |
| D5 | Reporters merge ordering?                                                                                                 | **Non-issue.** Reporters already merged (commit `ecf73c6`). Start fresh from the current branch.                                                                                          |

Other naming decisions locked by user on 2026-04-23:

- Crypto pack name: **Digital Asset** (`PackId::CryptoDigitalAsset`, manifest under `analysis_packs/crypto/`).
- `data/traits/macroeconomic.rs` (not `macro_.rs`).

Review follow-ups locked on 2026-04-24:

- Thesis-memory continuity across the Phase 6 upgrade is an accepted breaking change. Migration semantics are same-version-only; v1 snapshot rows are unsupported after the schema bump. Deleting `~/.scorpio-analyst/phase_snapshots.db` is optional local cleanup, not a required migration step.
- Phase 5 uses a composite `ValuatorId` selection model. Packs choose one strategy id per `AssetShape` (for example `CorporateEquity → ValuatorId::EquityDefault`), and the registry hides any internal composition such as DCF + multiples.
- Phase 4 preserves current placeholder tokens in the baseline prompt bundle. The new renderer must support `{ticker}` / `{current_date}` as-is so prompt extraction can remain byte-identical.

## Phased Implementation

### Phase 1 — Domain types

**Goal:** Introduce typed `Symbol`, `AssetClass`, expand `AssetShape`, migrate `&str` symbol sites to the typed form.

**Files created:**

- `crates/scorpio-core/src/domain/mod.rs` — facade; `pub use symbol::Symbol; pub use class::AssetClass;`
- `crates/scorpio-core/src/domain/symbol.rs` — `Symbol` enum (`Equity(Ticker)`, `Crypto(CaipAssetId)`), `Ticker` newtype, `CaipAssetId` placeholder newtype, `Symbol::parse(&str) -> Result<Symbol, TradingError>`.
- `crates/scorpio-core/src/domain/class.rs` — `AssetClass` enum (`Equity`, `Crypto`), `#[non_exhaustive]`.
- `crates/scorpio-core/src/domain/tests.rs` — parse coverage: corporate tickers, CAIP placeholders, malformed input rejection, `Display` / serde round-trip.

**Files modified:**

- `crates/scorpio-core/src/lib.rs` — add `pub mod domain;`.
- `crates/scorpio-core/src/state/derived.rs` — expand `AssetShape` with `NativeChainAsset`, `Erc20Token`, `Stablecoin`, `LpToken`; existing shape-routing `match` sites gain `_ => ValuationAssessment::NotAssessed`.
- `crates/scorpio-core/src/state/trading_state.rs` — keep `asset_symbol: String` (transitional); add `#[serde(default)] pub symbol: Option<Symbol>` with `Display` synced to `asset_symbol`.
- `crates/scorpio-core/src/data/symbol.rs` — `validate_symbol` stays; add `pub fn parse_symbol(s: &str) -> Result<Symbol, TradingError>` delegating to `Symbol::parse`.
- `crates/scorpio-core/src/data/entity.rs` — `ResolvedInstrument` gains `pub symbol: Symbol` field (`#[serde(default)]`); `resolve_symbol` populates it.
- `crates/scorpio-core/src/analysis_packs/manifest/schema.rs` — `resolve_valuation(shape)` adds crypto arms returning `NotAssessed`.

**Tests affected:**

- `state/derived.rs` — round-trip tests for new `AssetShape` variants.
- `domain/tests.rs` — new: `parse_equity_ticker_succeeds`, `parse_empty_rejects`, `parse_invalid_chars_rejects`, `symbol_display_round_trips`, `symbol_serde_round_trips`.
- `analysis_packs/manifest/tests.rs:148-172` — extend `resolve_valuation` coverage with one test per new crypto variant (all expect `NotAssessed`).
- `data/entity.rs:112-118` — update `resolve_symbol_stage1_metadata_fields_are_none` to assert `symbol` matches `Symbol::Equity(...)`.

**Validation:** `cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace --all-features --locked --no-fail-fast`. Smoke: `cargo run -p scorpio-cli -- analyze AAPL --no-terminal`.

**Risk + mitigation:** Adding `Option<Symbol>` to `TradingState` changes JSON shape but stays backward-compatible via `#[serde(default)]`. Reporter JSON output gains an additional field, tolerated by its `serde_json::to_value` path.

### Phase 2 — `Analyst` trait + registry

**Goal:** Make analysts pluggable so pack `required_inputs` drives fan-out composition.

**Files created:**

- `crates/scorpio-core/src/agents/analyst/traits.rs` — `trait Analyst: Send + Sync`, `AnalystId` enum (`Fundamental`, `Sentiment`, `News`, `Technical`, plus placeholders `Tokenomics`, `OnChain`, `Social`, `Derivatives`), `DataNeed` enum, `AnalystOutput` union.
- `crates/scorpio-core/src/agents/analyst/registry.rs` — `AnalystRegistry { inner: HashMap<AnalystId, Arc<dyn Analyst>> }`, `register`, `get`, `for_inputs`.
- `crates/scorpio-core/src/agents/analyst/equity/mod.rs` — facade re-exporting the four equity analysts.
- `crates/scorpio-core/src/agents/analyst/crypto/mod.rs` — facade (empty).
- `crates/scorpio-core/src/agents/analyst/crypto/tokenomics.rs` — stub (`// TODO: implement in crypto-pack change`).
- `crates/scorpio-core/src/agents/analyst/crypto/onchain.rs` — stub.
- `crates/scorpio-core/src/agents/analyst/crypto/social.rs` — stub.
- `crates/scorpio-core/src/agents/analyst/crypto/derivatives.rs` — stub.

**Files modified:**

- `crates/scorpio-core/src/agents/analyst/mod.rs` — add module decls; keep `pub use equity::{FundamentalAnalyst, ...}` via facade so call sites continue compiling.
- Four equity analyst files (`fundamental.rs`, `sentiment.rs`, `news.rs`, `technical.rs`) — add `impl Analyst for X` delegating to existing `run`; existing methods unchanged.
- `crates/scorpio-core/src/workflow/pipeline/runtime.rs:94-107` — `FanOutTask::new` becomes data-driven: `build_analyst_tasks(registry, policy.required_inputs, quick_handle, finnhub, yfinance, fred, llm_config)` returns `Vec<Arc<dyn Task>>`. Defensive fallback to the four-tuple if policy is absent.

**Files moved/renamed:**

- `agents/analyst/fundamental.rs` → `agents/analyst/equity/fundamental.rs`
- `agents/analyst/sentiment.rs` → `agents/analyst/equity/sentiment.rs`
- `agents/analyst/news.rs` → `agents/analyst/equity/news.rs`
- `agents/analyst/technical.rs` → `agents/analyst/equity/technical.rs`
- `agents/analyst/common.rs` → `agents/analyst/equity/common.rs`

**Tests affected:**

- `agents/analyst/mod.rs` — existing four-analyst apply tests stay green.
- New: `agents/analyst/registry.rs` — `registry_returns_analyst_by_id`, `for_inputs_maps_strings_to_analysts`, `unknown_id_returns_none`.
- `workflow/tasks/tests.rs` — new: `fan_out_respects_pack_required_inputs` constructing a pack with only `fundamentals` and asserting one analyst task spawned.

**Validation:** all cargo commands; smoke analyze on AAPL.

**Risk + mitigation:** Dynamic fan-out changes the `FanOutTask` invocation shape. The existing four-input baseline produces identical tasks in identical order, so pipeline behavior is byte-identical. `REPLACEABLE_TASK_IDS` in `workflow/pipeline/constants.rs` must cover the dynamically-built tasks — verify test-helper stub replacement still works.

### Phase 3 — `DataProvider` traits, domain-split

**Goal:** Abstract providers behind traits, keep concrete clients, introduce routing.

**Files created:**

- `crates/scorpio-core/src/data/traits/mod.rs` — re-exports.
- `crates/scorpio-core/src/data/traits/fundamentals.rs` — `trait FundamentalsProvider: Send + Sync { async fn fetch(&self, symbol: &Symbol) -> Result<FundamentalData, TradingError>; }`.
- `crates/scorpio-core/src/data/traits/price_history.rs` — `trait PriceHistoryProvider` (OHLCV).
- `crates/scorpio-core/src/data/traits/news.rs` — `trait NewsProvider`.
- `crates/scorpio-core/src/data/traits/macroeconomic.rs` — `trait MacroProvider` (FRED).
- `crates/scorpio-core/src/data/traits/tokenomics.rs` — placeholder; methods return `TradingError::NotImplemented`.
- `crates/scorpio-core/src/data/traits/onchain.rs` — placeholder.
- `crates/scorpio-core/src/data/traits/derivatives.rs` — placeholder.
- `crates/scorpio-core/src/data/traits/social.rs` — placeholder.
- `crates/scorpio-core/src/data/routing.rs` — `fn resolve_fundamentals_provider(symbol: &Symbol, reg: &ProviderRegistry) -> Arc<dyn FundamentalsProvider>`, one per trait.
- `crates/scorpio-core/src/data/equity/mod.rs` — facade; re-exports current top-level `pub use`.
- `crates/scorpio-core/src/data/crypto/mod.rs` — empty facade.
- `crates/scorpio-core/src/data/crypto/.gitkeep` — directory marker.

**Files modified:**

- `crates/scorpio-core/src/data/mod.rs` — add module decls; retain `pub use` lines via equity facade.
- `crates/scorpio-core/src/data/finnhub.rs` → `crates/scorpio-core/src/data/equity/finnhub.rs` — add `impl FundamentalsProvider for FinnhubClient`, `impl NewsProvider for FinnhubClient`; no method-signature changes.
- `crates/scorpio-core/src/data/yfinance/ohlcv.rs` → `crates/scorpio-core/src/data/equity/yfinance/ohlcv.rs` — add `impl PriceHistoryProvider for YFinanceClient`.
- `crates/scorpio-core/src/data/fred.rs` → `crates/scorpio-core/src/data/equity/fred.rs` — add `impl MacroProvider for FredClient`.

**Files moved/renamed:**

- `data/finnhub.rs` → `data/equity/finnhub.rs`
- `data/fred.rs` → `data/equity/fred.rs`
- `data/yfinance/` → `data/equity/yfinance/`
- `data/adapters/` stays in place (shared enrichment contracts, not equity-specific).

**Tests affected:**

- Inline `#[cfg(test)]` tests in moved files stay green (`use super::*;` resolves unchanged).
- New: `data/routing.rs` — `routes_equity_symbol_to_finnhub`, `routes_crypto_symbol_returns_unimplemented`.

**Validation:** all cargo commands; smoke analyze on AAPL and BRK.B (dot-suffix ticker) to exercise both `Symbol::Equity` resolution paths.

**Risk + mitigation:** Moving `yfinance/` subdirectory changes file paths. `pub use equity::yfinance::...` at `data/mod.rs` keeps the `scorpio_core::data::yfinance::Candle` public path valid.

### Phase 4 — Prompt bundles in pack manifest

**Goal:** Externalize prompts out of agent source; pack manifest carries per-role prompt bundle.

**Files created:**

- `crates/scorpio-core/src/prompts/mod.rs` — facade.
- `crates/scorpio-core/src/prompts/bundle.rs` — `pub struct PromptBundle { fundamental_analyst: Cow<'static, str>, sentiment_analyst: ..., news_analyst: ..., technical_analyst: ..., bullish_researcher: ..., bearish_researcher: ..., debate_moderator: ..., trader: ..., aggressive_risk: ..., conservative_risk: ..., neutral_risk: ..., risk_moderator: ..., fund_manager: ... }`.
- `crates/scorpio-core/src/prompts/templating.rs` — `fn render(template: &str, vars: &HashMap<&str, &str>) -> String` supporting current baseline placeholders `{ticker}` / `{current_date}` as-is, plus `{asset_class}` / `{analysis_emphasis}` for future packs.
- `crates/scorpio-core/src/prompts/versioning.rs` — `fn content_hash(bundle: &PromptBundle) -> String` (blake3 / sha256 of concatenated slots).
- `crates/scorpio-core/src/analysis_packs/equity/mod.rs` — pack facade.
- `crates/scorpio-core/src/analysis_packs/equity/prompts/fundamental_analyst.md` — extracted from current `const FUNDAMENTAL_SYSTEM_PROMPT`.
- Plus one `.md` per remaining slot: `sentiment_analyst.md`, `news_analyst.md`, `technical_analyst.md`, `bullish_researcher.md`, `bearish_researcher.md`, `debate_moderator.md`, `trader.md`, `aggressive_risk.md`, `conservative_risk.md`, `neutral_risk.md`, `risk_moderator.md`, `fund_manager.md`.
- `crates/scorpio-core/src/analysis_packs/crypto/prompts/.gitkeep` — empty directory marker.

**Files modified:**

- `crates/scorpio-core/src/lib.rs` — add `pub mod prompts;`.
- `crates/scorpio-core/src/analysis_packs/manifest/schema.rs` — `AnalysisPackManifest` gains `pub prompt_bundle: PromptBundle` (required, no default).
- `crates/scorpio-core/src/analysis_packs/selection.rs` — `RuntimePolicy` gains `prompt_bundle: PromptBundle`; `hydrate_policy` copies from manifest.
- `crates/scorpio-core/src/analysis_packs/builtin.rs` — `baseline_pack()` populates `prompt_bundle: equity_baseline_bundle()` where the latter returns a `PromptBundle` via `include_str!("equity/prompts/...")`.
- 17 agent files — remove `const X_SYSTEM_PROMPT`; replace builders with `fn build_prompt(bundle_slot: &str, symbol: &str, target_date: &str, emphasis: &str) -> String` calling `prompts::templating::render`.
- Agent constructors accept `prompt: Cow<'static, str>` from the pack's `prompt_bundle.X`; the pipeline passes them through from the runtime policy on `TradingState`.

**Tests affected:**

- Agent test fixtures gain a `crates/scorpio-core/src/prompts/testing.rs` with `fn sample_bundle() -> PromptBundle` gated on `#[cfg(test)]`.
- New: `prompts/templating.rs` — `renders_ticker`, `renders_current_date`, `unknown_placeholder_passes_through`.
- New: `prompts/versioning.rs` — `same_bundle_same_hash`, `different_slot_different_hash`, `hash_is_stable_across_runs`.
- New one-shot diff test: render the new templating engine output against the old `build_fundamental_system_prompt` helper with fixed ticker / date — must be byte-identical.

**Validation:** all cargo commands; smoke analyze on AAPL; compare final report to a recorded baseline to confirm byte-identical migration.

**Risk + mitigation:** Baseline prompt files retain the current `{ticker}` / `{current_date}` placeholder vocabulary during extraction. Any typo during extraction or renderer wiring reshapes what the LLM sees. The byte-identical diff test listed above gates the phase.

### Phase 5 — `Valuator` trait

**Goal:** Replace `ValuationAssessment` enum variants with pluggable composite strategies keyed on `AssetShape`.

**Files created:**

- `crates/scorpio-core/src/valuation/mod.rs` — facade; `trait Valuator { fn assess(&self, state: &TradingState, shape: &AssetShape) -> ValuationReport; }`, `ValuationReport` enum, and `ValuatorId` enum (`EquityDefault`, placeholders for crypto strategies).
- `crates/scorpio-core/src/valuation/equity/mod.rs` — facade.
- `crates/scorpio-core/src/valuation/equity/default.rs` — `struct EquityDefaultValuator` composing existing DCF + multiples logic.
- `crates/scorpio-core/src/valuation/equity/dcf.rs` — `struct DcfValuator` with existing DCF logic factored in.
- `crates/scorpio-core/src/valuation/equity/multiples.rs` — `struct MultiplesValuator` for EV/EBITDA, P/E, PEG.
- `crates/scorpio-core/src/valuation/crypto/mod.rs` — empty facade.
- `crates/scorpio-core/src/valuation/crypto/tokenomics.rs` — stub.
- `crates/scorpio-core/src/valuation/crypto/network_value.rs` — stub.
- `crates/scorpio-core/src/valuation/registry.rs` — resolves `ValuatorId → Arc<dyn Valuator>` so composition stays hidden behind a single manifest-selected strategy id.

**Files modified:**

- `crates/scorpio-core/src/lib.rs` — add `pub mod valuation;`.
- `crates/scorpio-core/src/state/valuation_derive.rs` — `pub fn derive_valuation(...)` becomes a compat shim internally using `EquityDefaultValuator`; its 16 tests continue to pass.
- `crates/scorpio-core/src/analysis_packs/manifest/schema.rs` — `AnalysisPackManifest` gains `pub valuator_selection: HashMap<AssetShape, ValuatorId>` defaulting to `CorporateEquity → ValuatorId::EquityDefault` for baseline.
- `crates/scorpio-core/src/workflow/tasks/analyst.rs` (`AnalystSyncTask` portion calling `derive_valuation`) — routes through the manifest-selected `ValuatorId` for the resolved `AssetShape`; falls through to `ValuationReport::NotAssessed` otherwise.

**Tests affected:**

- Every `derive_valuation` test in `state/valuation_derive.rs` stays green (shim preserves behavior).
- New: `valuation/equity/default.rs` — composition tests ensuring `EquityDefaultValuator` matches today's combined DCF + multiples output.
- New: `valuation/equity/dcf.rs` — DCF tests moved from `valuation_derive.rs`.
- New: `valuation/equity/multiples.rs` — EV/EBITDA, P/E, PEG tests moved from existing inline tests.

**Validation:** all cargo commands; smoke analyze on AAPL and SPY (ETF → `Fund → NotAssessed` path).

**Risk + mitigation:** `derive_valuation` is called from `AnalystSyncTask`; signature preserved — internal structure only.

### Phase 6 — `TradingState` reshape (SCHEMA-BREAKING)

**Goal:** Organize `TradingState` by asset class; introduce `AnalystOutput` sum type; bump schema version.

**Files created:**

- `crates/scorpio-core/src/state/shared/mod.rs` — facade.
- `crates/scorpio-core/src/state/shared/thesis.rs` — moved from `state/thesis.rs`.
- `crates/scorpio-core/src/state/shared/token_usage.rs` — moved.
- `crates/scorpio-core/src/state/shared/proposal.rs` — moved.
- `crates/scorpio-core/src/state/shared/execution.rs` — moved.
- `crates/scorpio-core/src/state/shared/risk.rs` — moved.
- `crates/scorpio-core/src/state/shared/reporting.rs` — moved.
- `crates/scorpio-core/src/state/shared/provenance.rs` — moved.
- `crates/scorpio-core/src/state/shared/evidence.rs` — moved.
- `crates/scorpio-core/src/state/shared/debate.rs` — extracted from `trading_state.rs` (`DebateMessage`).
- `crates/scorpio-core/src/state/equity/mod.rs` — facade.
- `crates/scorpio-core/src/state/equity/fundamental.rs` — moved from `state/fundamental.rs`.
- `crates/scorpio-core/src/state/equity/technical.rs` — moved.
- `crates/scorpio-core/src/state/equity/sentiment.rs` — moved.
- `crates/scorpio-core/src/state/equity/news.rs` — moved.
- `crates/scorpio-core/src/state/equity/market_volatility.rs` — moved.
- `crates/scorpio-core/src/state/equity/valuation_derive.rs` — moved (Phase 5 shim stays).
- `crates/scorpio-core/src/state/equity/derived.rs` — moved.
- `crates/scorpio-core/src/state/crypto/mod.rs` — empty facade.
- `crates/scorpio-core/src/state/crypto/tokenomics.rs` — empty struct, `// TODO`.
- `crates/scorpio-core/src/state/crypto/onchain.rs` — stub.
- `crates/scorpio-core/src/state/crypto/derivatives.rs` — stub.
- `crates/scorpio-core/src/state/analyst_output.rs` — `enum AnalystOutput { Fundamental(FundamentalData), Sentiment(SentimentData), News(NewsData), Technical(TechnicalData), Tokenomics(()), OnChain(()), Derivatives(()) }` keyed by `AnalystId`.

**Files modified:**

- `crates/scorpio-core/src/state/mod.rs` — restructure module decls; preserve every `scorpio_core::state::TradingState`, `scorpio_core::state::FundamentalData`, `scorpio_core::state::ThesisMemory` path via `pub use shared::*; pub use equity::*; pub use analyst_output::*; pub use trading_state::*;`.
- `crates/scorpio-core/src/state/trading_state.rs`:
  - `pub execution_id: Uuid` — stays.
  - `pub symbol: Symbol` — replaces `asset_symbol: String` as primary; `#[serde(default)] pub asset_symbol: String` kept as `Display` fallback for old snapshots.
  - `pub target_date: String` — stays.
  - `pub current_price: Option<f64>` — stays (`#[serde(default)]`).
  - Equity-only fields (`fundamental_metrics`, `technical_indicators`, `market_sentiment`, `macro_news`, `evidence_*`, `market_volatility`, `derived_valuation`) move into `pub equity: Option<EquityState>` (`#[serde(default)]`).
  - New `pub crypto: Option<CryptoState>` (`#[serde(default)]`), always `None` this slice.
  - Shared fields stay at top level.
- `crates/scorpio-core/src/workflow/snapshot/thesis.rs:12` — `THESIS_MEMORY_SCHEMA_VERSION: i64 = 1` → `2`.
- `crates/scorpio-core/src/workflow/snapshot.rs` — `INSERT` binding moves from `.bind(1_i64)` to `.bind(2_i64)`; `load_snapshot` gains explicit incompatible-schema handling instead of attempting to deserialize pre-v2 rows.
- `crates/scorpio-reporters/src/json.rs` — bump `JsonReport.schema_version`, keep emitting full `TradingState`, and update tests to assert the new schema version and reshaped state layout.
- `crates/scorpio-reporters/src/terminal/final_report.rs` — switch root-field reads (`fundamental_metrics`, `market_sentiment`, `macro_news`, `technical_indicators`, `market_volatility`, `derived_valuation`) to `TradingState` accessors so terminal output remains behaviorally equivalent for equity runs.
- `crates/scorpio-reporters/src/terminal/provenance.rs` and `crates/scorpio-reporters/src/terminal/valuation.rs` — route evidence / valuation reads through the new Phase 6 accessors.
- ~40 call sites reading `state.fundamental_metrics` etc. — rewrite via accessor methods (`state.fundamental_metrics() -> Option<&FundamentalData>`, `state.equity_mut() -> &mut EquityState`).

**Files moved/renamed:**

- `state/fundamental.rs` → `state/equity/fundamental.rs`
- `state/technical.rs` → `state/equity/technical.rs`
- `state/sentiment.rs` → `state/equity/sentiment.rs`
- `state/news.rs` → `state/equity/news.rs`
- `state/market_volatility.rs` → `state/equity/market_volatility.rs`
- `state/derived.rs` → `state/equity/derived.rs`
- `state/valuation_derive.rs` → `state/equity/valuation_derive.rs`
- `state/thesis.rs` → `state/shared/thesis.rs`
- `state/token_usage.rs` → `state/shared/token_usage.rs`
- `state/proposal.rs` → `state/shared/proposal.rs`
- `state/execution.rs` → `state/shared/execution.rs`
- `state/risk.rs` → `state/shared/risk.rs`
- `state/reporting.rs` → `state/shared/reporting.rs`
- `state/provenance.rs` → `state/shared/provenance.rs`
- `state/evidence.rs` → `state/shared/evidence.rs`

**Tests affected:**

- `state/trading_state.rs` inline tests — assertions rewritten via accessor methods.
- `tests/state_roundtrip.rs` — new: read a v1 snapshot row and confirm preflight thesis lookup returns `None` via schema-version skip without attempting deserialization.
- `workflow/snapshot/tests/thesis_compat.rs` — add same-version-only coverage for `THESIS_MEMORY_SCHEMA_VERSION = 2`; direct snapshot reads of v1 rows now fail with an explicit incompatible-schema error.
- `crates/scorpio-reporters/tests/json.rs` — update expected `JsonReport.schema_version`, assert the reshaped `TradingState` serializes as intended, and explicitly cover the preserved `asset_symbol` header field.
- `crates/scorpio-reporters/src/terminal/*` tests — update valuation, provenance, and final-report fixtures to read equity-scoped data through accessors while preserving current rendered output for baseline equity runs.

**Validation:** all cargo commands; smoke analyze on AAPL (fresh DB) AND on AAPL with a pre-existing `~/.scorpio-analyst/phase_snapshots.db` from a v1 run. Expected: prior thesis is not loaded from v1 rows, run completes, new row written with `schema_version = 2`.

**Risk + mitigation:** 40+ call sites shift shape, including reporter code that currently reads equity-root fields directly. Accessor methods (D4 decision) turn this into mechanical search-replace across both `scorpio-core` and `scorpio-reporters`, with current terminal / JSON output preserved for baseline equity runs and a deliberate JSON schema-version bump for the reporter artifact.

### Phase 7 — Workflow builder

**Goal:** Finalize dynamic pack composition without renaming the existing `analysis_packs` namespace.

**Files created:**

- `crates/scorpio-core/src/workflow/builder.rs` — `impl TradingPipeline { pub fn from_pack(pack: &AnalysisPackManifest, deps: PipelineDeps) -> Self { ... } }` calling new `fn build_graph_from_pack(pack, ...)` that reads `pack.required_inputs`, looks up via `AnalystRegistry::for_inputs`, wires fan-out with selected analysts, inserts `DebateTask` / `RiskTask` / `FundManagerTask` unchanged.
- `crates/scorpio-core/src/analysis_packs/registry.rs` — `fn resolve(id: PackId) -> AnalysisPackManifest` with `Baseline` (equity) and `CryptoDigitalAsset` (valid but non-selectable stub).
- `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` — `fn baseline_pack() -> AnalysisPackManifest` with `prompt_bundle` populated via `include_str!("prompts/...")`.
- `crates/scorpio-core/src/analysis_packs/crypto/mod.rs` — facade.
- `crates/scorpio-core/src/analysis_packs/crypto/digital_asset.rs` — stub manifest with a valid placeholder `required_inputs` list and dummy prompt bundle so manifest validation continues to pass. This pack is excluded from CLI / config selection in this slice, so no runtime path executes it yet.

**Files modified:**

- `crates/scorpio-core/src/lib.rs` — keep `pub mod analysis_packs;`; no namespace rename in this slice.
- `crates/scorpio-core/src/workflow/pipeline/runtime.rs:77` — replace hand-wired `build_graph` with `workflow::builder::build_graph_from_pack(pack, ...)`.
- `crates/scorpio-core/src/analysis_packs/manifest/pack_id.rs:9` — add `CryptoDigitalAsset` variant, but keep `FromStr` / user-facing selection restricted to `baseline` in this slice so the stub pack remains non-selectable until crypto implementation lands.

**Files moved/renamed:**

- `analysis_packs/builtin.rs` — split into `analysis_packs/registry.rs` + `analysis_packs/equity/baseline.rs` (deleted).

**Files deleted:**

- `analysis_packs/builtin.rs` (split across registry + equity/baseline).

**Tests affected:**

- `analysis_packs/manifest/tests.rs` (24 tests) stay in place; extend for `CryptoDigitalAsset` validation and prompt / valuator additions.
- `analysis_packs/builtin.rs` inline tests (lines 55-139) move to `analysis_packs/equity/baseline.rs`; stay green.
- New: `workflow/builder.rs` — `pipeline_from_baseline_pack_has_four_analyst_tasks`, plus a guard test proving `CryptoDigitalAsset` is not user-selectable yet even though its manifest validates.

**Validation:** all cargo commands; smoke analyze on AAPL; end-to-end backtest over a week of AAPL dates to confirm no behavioral regression vs. pre-refactor.

**Risk + mitigation:** Keeping the existing `analysis_packs` namespace avoids downstream import churn and lets this phase focus only on dynamic builder composition.

## Ordering Rationale

1. **Phase 1 first** because `Symbol` is consumed by every subsequent phase. Analyst traits (Phase 2) or data traits (Phase 3) written against `&str` would need to be redone.
2. **Phase 2 before Phase 3** because the `Analyst::required_data` method declares what `DataNeed`s it consumes, informing trait boundaries in Phase 3. Reversed order forces speculative trait shapes.
3. **Phase 4 (prompts) before Phase 5 (valuators)** because `Valuator` renders prompt-like valuation explanations; sharing `templating.rs` avoids a second templating engine.
4. **Phase 5 before Phase 6** because `ValuationReport` is a new type Phase 6 must slot into `EquityState`. Reversed order forces a refactor-within-refactor.
5. **Phase 6 last of the breaking work** because state reshape is the only schema-breaking change; it sits against fully-ready scaffolding.
6. **Phase 7 is synthesis**: only after Phases 1–6 can `TradingPipeline::from_pack` do its job — it needs the `AnalystRegistry` (Phase 2), `PromptBundle` (Phase 4), `valuator_selection` (Phase 5), and reshaped `TradingState` (Phase 6).

Intermediate-state safety: after **every** phase, `cargo nextest run` is green and a live analyze pass on AAPL succeeds. No phase is a "trust me, it'll compile after phase N+1" situation.

## Snapshot Compatibility

Only Phase 6 is schema-incompatible. This plan accepts a one-time breaking change for thesis-memory continuity across the upgrade.

- **Current state**: `THESIS_MEMORY_SCHEMA_VERSION = 1`. All written rows carry `schema_version = 1`.
- **After Phase 6**: `THESIS_MEMORY_SCHEMA_VERSION = 2`. Newly written rows carry `schema_version = 2`.
- **Compatibility contract after the bump:**
  1. `SnapshotStore::load_prior_thesis_for_symbol` treats any row whose `schema_version != THESIS_MEMORY_SCHEMA_VERSION` as incompatible and skips it before deserializing `TradingState`.
  2. `SnapshotStore::load_snapshot` is same-version-only after Phase 6: incompatible rows return a clear unsupported-schema error instead of attempting deserialization.
  3. Newly written rows use `schema_version = 2` and become the only rows eligible for thesis-memory reuse.
- **Behavior on old snapshots:**
  1. User's existing `~/.scorpio-analyst/phase_snapshots.db` has v1 rows.
  2. On next `scorpio analyze <SYMBOL>`, preflight calls `SnapshotStore::load_prior_thesis_for_symbol`.
  3. v1 rows are skipped immediately by the same-version check and are never deserialized.
  4. With no matching v2 rows, preflight returns `None` → pipeline runs with `prior_thesis = None`.
  5. The completed run writes fresh v2 rows that future runs can reuse.
- **Migration approach**: **No SQL migration.** Older rows remain on disk but are intentionally unsupported after the bump. Developers may optionally delete `~/.scorpio-analyst/phase_snapshots.db` locally for a clean slate, but release behavior must not depend on DB truncation or file deletion.
- **User-visible impact**: On the first post-refactor run, prior thesis continuity is reset once. No crash is expected; the next successful run seeds new v2 thesis-memory rows.
- **Release note**: Call out as schema-breaking in v0.4.0 release notes — "existing thesis-memory continuity is reset; prior-run theses will not be carried forward."

## Validation Strategy

After every phase boundary:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
cargo run -p scorpio-cli -- analyze AAPL --no-terminal  # smoke
```

Phase-specific smoke additions:

- Phase 3: BRK.B (dot-suffix ticker) to exercise `Symbol::Equity` edge case.
- Phase 4: byte-identical prompt-diff test (new templating output vs. recorded baseline, preserving `{ticker}` / `{current_date}` placeholders in baseline prompt files).
- Phase 5: SPY (ETF → `Fund → NotAssessed`).
- Phase 6: AAPL against a pre-existing v1 `phase_snapshots.db` to confirm same-version-only skip semantics.
- Phase 7: end-to-end backtest over a week of AAPL dates comparing final reports to pre-refactor baseline.

## Out-of-Scope Followups

Deliberately **not** in this refactor:

- **Crypto implementation proper** — separate `crypto-pack-implementation` change populates analyst / provider / valuator stubs, adds real CAIP parsing to `Symbol::parse`, wires `analysis_packs::crypto::digital_asset` with real `required_inputs`.
- **Workspace crate split** — no `scorpio-domain`, `scorpio-providers`, `scorpio-packs` crate. If `scorpio-core` grows past ~80kLoC, a split becomes a separate concern.
- **Prompt A/B framework** — `prompt_bundle` is content-hashed, but no built-in A/B routing or eval harness.
- **Runtime-loaded packs from filesystem** — `Cow` allows it; `resolve_pack` only handles compile-time built-ins this cycle.
- **Crypto reporter specialization** — no crypto-specific reporter formatting ships in this slice. Reporter work is limited to preserving baseline equity output across the Phase 6 `TradingState` reshape and bumping the JSON artifact schema version accordingly.
- **`PackId` persistence-format shift** — `analysis_pack_name: Option<String>` continues to hold `PackId::as_str()` rendering.
- **`validate_symbol(&str)` removal** — stays callable for CLI fail-fast UX.
- **`asset_symbol: String` field removal** — kept through Phase 6 for serde back-compat; removed in v0.5.0 cleanup after v1 snapshots are confirmed deprecated.

## Critical Files for Implementation

- `crates/scorpio-core/src/state/trading_state.rs`
- `crates/scorpio-core/src/workflow/pipeline/runtime.rs`
- `crates/scorpio-core/src/agents/analyst/mod.rs`
- `crates/scorpio-core/src/analysis_packs/manifest/schema.rs`
- `crates/scorpio-core/src/workflow/snapshot/thesis.rs`
