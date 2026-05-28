# EtfBaseline Phase 2 — Dealer Greeks + Prompt Integration

**Date:** 2026-05-22
**Status:** Draft — awaiting user review
**Parent:** [`2026-05-21-etf-baseline-pack-design.md`](./2026-05-21-etf-baseline-pack-design.md)

## Goal

Land the dealer-positioning signal the Phase 1 design reserved (`EtfValuation.options_gex`) through a staged rollout. Stages 1-2 validate a compact near-term GEX + gamma-wall overlay using the existing options-chain fetch and leverage-warning prompt hooks; contingent Stage 3 then adds the broader BSM context work - VEX/CEX surfacing, broad all-expirations GEX, and the FRED-backed risk-free-rate/report-polish path - once that surfaced overlay proves useful.

Within `EtfBaseline`, dealer positioning stays a compact secondary risk/liquidity overlay for mainstream ETF users rather than a pack-defining options-specialist mode. Premium/discount, composition, and tracking evidence remain the primary baseline anchors.

Pack selection, prompt scaffolding, manifest topology, and analyst-slot mapping are unchanged from Phase 1. The ETF report header only changes for the conditional risk-free-rate fallback warning line described below.

## Problem

Phase 1 shipped premium/discount, composition, and tracking-error analysis for ETFs but left several first-class holes behind explicit feature flags:

- [`state/derived.rs:290`](../../../crates/scorpio-core/src/state/derived.rs) declares `GexSummary` but the field is always `None` because [`valuation/etf/premium_discount.rs:75`](../../../crates/scorpio-core/src/valuation/etf/premium_discount.rs) hardcodes `options_gex: None`.
- `OptionsSnapshot.near_term_strikes` carries front-month data only — sufficient for the equity Technical Analyst path but throws away the per-expiration per-strike rows the yfinance provider already fetches internally to compute the term structure.
- `etf_tracking_options_focus.md` contains placeholder language ("options-chain branch deferred to Phase 2") that the Technical analyst cannot act on.
- `etf_leverage_warning.md` is wired into `etf/baseline.rs` as a constant but no code path injects it; Conservative-risk / Neutral-risk / Auditor prompts treat leveraged ETFs the same as unlevered ones.
- BSM gamma is weakly sensitive to the risk-free rate at near-term expirations, but the existing FRED client already exposes `get_series_latest(series_id)` and the codebase has no general "fetch a treasury rate" precedent — leaving the rate hardcoded would diverge from the project's "deterministic inputs flow through state" pattern.

Phase 2 closes all five gaps in a single design pass while testing a specific product hypothesis: baseline ETF readers benefit from a compact dealer-positioning overlay around catalysts, rebalance windows, and expiration clusters without needing a separate options-specialist pack.

## Decisions

| Decision                                 | Choice                                                                                                                                                                                 | Rationale                                                                                                                                                                                                                                                                                                                                 |
|------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Spec scope                               | All five Phase 2 items in one design                                                                                                                                                   | They share the ETF options-chain path, the FRED-fed valuation inputs, and the prompt/report updates that surface dealer-positioning evidence; keeping them together preserves one coherent slice.                                                                                                                                         |
| Validation gate                          | Stage 2 is the go/no-go checkpoint for Stage 3                                                                                                                                         | Stage 2 is the first user-visible baseline ETF surface, so the product hypothesis can only be validated there; Stage 3 is contingent work, not a prerequisite for Stage 1/2 completion.                                                                                                                                                   |
| Baseline-pack role                       | Dealer positioning remains a secondary ETF baseline risk/liquidity overlay                                                                                                             | Baseline ETF users still benefit from knowing whether dealer hedging is likely to dampen or amplify moves around catalysts, rebalances, and expiration windows without turning the pack into an options-specialist mode.                                                                                                                  |
| Dealer-positioning model                 | SqueezeMetrics convention (dealers short calls, long puts) for signed GEX / VEX / CEX exposure                                                                                         | Industry-default convention used by SpotGamma and major sell-side desks. Gamma can be read directly as stabilizing vs destabilizing dealer flow; VEX and CEX keep the same sign convention for consistency but must be described as conditional sensitivities to vol moves and time decay rather than as stand-alone stabilizing signals. |
| BSM volatility input                     | Per-strike IV (`NearTermStrike.call_iv` / `put_iv`) with `OptionsSnapshot.atm_iv` fallback                                                                                             | Preserves the skew/smile signal where the chain exposes per-strike IVs; degrades gracefully when individual rows are sparse.                                                                                                                                                                                                              |
| Risk-free rate `r`                       | FRED `DGS3MO` fetched at preflight when pack = `EtfBaseline`; const `0.045` fallback when FRED fails                                                                                   | Retained in this phase because one ETF-only preflight fetch on the existing `FredClient` gives the run a stamped, replayable rate input for both live analysis and snapshot/report parity. The broad all-expirations GEX line stays explicitly labeled as a single-rate approximation rather than a curve-precise output.                 |
| Dividend yield `q`                       | Heuristic proxy: use `EtfComposition.distribution_yield_ttm_pct` only when the existing TTM dividend-history fetch returned `Some(yield)` with `yield > 0.0`, else `0.0`               | Phase 1 already plumbs the TTM distribution yield. Using that existing positive-yield signal as the gate keeps `q` implementable with data already fetched and avoids inventing a separate "regular cash-distribution profile" classifier; missing or non-positive histories fall back to `0.0` with a warning.                           |
| Sign convention propagation              | Same SqueezeMetrics convention applied to GEX, VEX, and CEX uniformly                                                                                                                  | Avoids per-Greek polarity surprises in prompts/report; "positive = dealer-stabilizing flow" interpretation generalizes.                                                                                                                                                                                                                   |
| Greeks scope                             | Gamma + Vanna + Charm only                                                                                                                                                             | The three Greeks practitioners cite alongside dealer flow analysis. Higher-order (Vomma, Speed, Color, Zomma) are out of scope.                                                                                                                                                                                                           |
| Multi-expiration aggregation             | In scope for contingent Stage 3 — emit a `broad: Option<BroadGex>` derived from an in-run transient `all_expirations` view alongside the front-month `net/gross_gex_usd_per_1pct_move` | yfinance already fetches all expirations for the term-structure ATM-IV vector. Reusing those rows within the current run preserves practitioner parity for broad GEX without persisting an unbounded per-expiration strike payload; snapshots keep only bounded `BroadGex` counts.                                                        |
| Per-strike gamma walls                   | Top-3 strikes by `\|net_gex\|` emitted in `GexSummary.strikes: Vec<StrikeGex>`                                                                                                         | Aggregator already computes per-strike gamma; surfacing the top concentrations lets the LLM cite specific gamma walls. Capping at 3 keeps state and prompts compact.                                                                                                                                                                      |
| Leverage warning injection               | Renderer-side at prompt-assembly time: append `etf_leverage_warning.md` to Conservative / Neutral / Auditor system prompts when ETF's `leverage_factor != 1.0`                         | Manifest stays leverage-agnostic; injection lives next to the existing `{ticker}` / `{analysis_emphasis}` substitution path.                                                                                                                                                                                                              |
| Deterministic GEX trigger                | None — GEX/VEX/CEX stay LLM-visible evidence, not deterministic fund-manager vetoes                                                                                                    | Phase 1's dual-risk audit contract already covers `tracking_failure`, `extreme_premium`, `leverage_decay`. A GEX magnitude threshold would commit the contract to a single heuristic.                                                                                                                                                     |
| Prompt update for tracking/options focus | Update `etf_tracking_options_focus.md` in place                                                                                                                                        | Phase 1 wired this file in with placeholder language; Phase 2 replaces it with real GEX/VEX/CEX-aware guidance. No manifest churn.                                                                                                                                                                                                        |
| Smoke-test discipline                    | Every new or extended fetch surface gets a `crates/scorpio-core/examples/*.rs` smoke                                                                                                   | Standing convention per `<source>_live_test.rs`; carries forward through any later phases.                                                                                                                                                                                                                                                |
| Warning discipline                       | All fail-soft paths emit `tracing::warn!` with stable target + structured fields; never log payload bytes                                                                              | Matches the snapshot-deserialize warn rule in CLAUDE.md (`error.kind = "deserialize"`, never `serde_json` text).                                                                                                                                                                                                                          |

The staged ship order is the validation path for that hypothesis, but the validation gate is Stage 2 because that is the first user-visible baseline ETF surface. Stage 3 is contingent work, not a prerequisite for Stage 1 or Stage 2 completion.

### Incremental ship order

1. **Stage 1 — near-term dealer positioning core:** ETF options hydration, near-term GEX aggregation from the existing front-month snapshot, gamma walls, and the 0DTE / sparse-front-expiry fallback rule.
2. **Stage 2 — surfaced validation slice:** exact technical-prompt update, leverage-warning injection through the existing risk/auditor prompt builders, and terminal rendering with explicit partial-data notes plus a plain-English dealer-positioning summary line. This stage is the go/no-go gate for the hypothesis.
3. **Stage 3 — contingent context expansion:** broad all-expirations GEX (labeled as a single-rate approximation), secondary VEX/CEX surfacing, transient `all_expirations` plumbing, live `DGS3MO` + persisted risk-free-rate source, fallback banner wiring, and the associated smoke coverage. Stage 3 proceeds only if Stage 2 clears the validation gate.

### Validation gate

- **Success signal:** the Stage 2 near-term GEX + gamma-wall overlay adds a distinct, non-redundant risk/liquidity takeaway in a representative ETF validation sample while staying clearly secondary to premium/discount, composition, and tracking evidence.
- **Stop signal:** if the Stage 2 overlay is usually absent, redundant with the existing ETF anchors, or makes the baseline report feel options-specialist, stop after Stage 2 and do not schedule Stage 3.
- **Decision owner:** the writing-plans handoff must record an explicit proceed/stop call before any Stage 3 implementation unit is scheduled.

## Architecture

### Component layout

```
crates/scorpio-core/src/
├── indicators/
│   └── gex.rs                          (NEW) BSM gamma + Vanna + Charm + aggregation helpers
│
├── agents/
│   ├── auditor/prompt.rs               (UPDATE) build_system_prompt applies ETF leverage warning helper for auditor
│   └── risk/common.rs                  (UPDATE) render_risk_system_prompt applies ETF leverage warning helper for Conservative / Neutral
│
├── valuation/
│   └── etf/
│       └── premium_discount.rs         (UPDATE) consume etf_options + etf_risk_free_rate; inline the GEX-summary mapping over indicators::gex output
│
├── data/traits/options.rs              (UPDATE, Stage 3 only) OptionsSnapshot gains transient all_expirations: Vec<ExpirationStrikes> with #[serde(skip, default)]
├── data/yfinance/options.rs            (UPDATE, Stage 3 only) normalizer plumbs per-expiration strikes into the transient field
│
├── workflow/
│   ├── builder.rs                      (UPDATE, Stage 3 only) threads shared FredClient into PreflightTask
│   ├── tasks/preflight.rs              (UPDATE, Stage 3 only) opportunistic DGS3MO fetch when pack = EtfBaseline; persist rate + source on TradingState
│   └── tasks/analyst.rs                (UPDATE) ETF branch reads OptionsOutcome::Snapshot into ValuationInputs.etf_options
│
├── state/
│   ├── derived.rs                      (UPDATE) GexSummary additive fields; StrikeGex (Stage 1/2), BroadGex/VexSummary/CexSummary (Stage 3)
│   ├── mod.rs                          (UPDATE) re-exports the expanded TradingState surface
│   └── trading_state.rs                (UPDATE) TradingState + snapshot wire boundary gain etf_risk_free_rate + etf_risk_free_rate_source with #[serde(default)]
│
├── analysis_packs/etf/
│   ├── baseline.rs                     (UPDATE) append_leverage_warning_if_needed helper shared by risk + auditor prompt builders
│   └── prompts/
│       └── etf_tracking_options_focus.md (UPDATE) replace placeholder block with real GEX/VEX/CEX guidance

crates/scorpio-core/examples/
├── fred_live_test.rs                   (UPDATE, Stage 3 only) add DGS3MO assertion alongside FEDFUNDS + CPI
├── yfinance_options_chain_live_test.rs (NEW, Stage 3 only) transient OptionsSnapshot.all_expirations smoke
└── etf_options_gex_live_test.rs        (NEW, Stage 3 only) end-to-end ETF run with full Stage 3 GexSummary populated

crates/scorpio-reporters/src/terminal/
└── etf.rs                              (UPDATE) DEALER POSITIONING block + summary line + gamma walls + partial-data note + secondary sensitivity rows
```

### State schema additions

```rust
// crates/scorpio-core/src/state/derived.rs

pub struct GexSummary {
    // Phase 1 fields (unchanged):
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub call_put_oi_ratio: f64,
    pub max_pain_strike: f64,
    pub near_term_expiration: chrono::NaiveDate,

    // Phase 2/3 additions. Stage 2 requires `strikes`; contingent Stage 3
    // adds `broad`, `vex_summary`, and `cex_summary`. These fields stay
    // additive with `#[serde(default)]`, which keeps Phase 1 snapshots
    // compatible because they serialized `options_gex: None`, not
    // `Some(GexSummary { ... })`.
    #[serde(default)]
    pub strikes: Vec<StrikeGex>,
    #[serde(default)]
    pub broad: Option<BroadGex>,
    #[serde(default)]
    pub vex_summary: Option<VexSummary>,
    #[serde(default)]
    pub cex_summary: Option<CexSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StrikeGex {
    pub strike: f64,
    pub net_gex_usd_per_1pct_move: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BroadGex {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub expirations_used: u32,
    #[serde(default)]
    pub expirations_total_considered: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VexSummary {
    /// Per 1.0 vol-point change (i.e., per 100 percentage points of σ —
    /// callers typically interpret as "per 1% absolute IV move" by dividing
    /// by 100 at display time).
    pub net_vex_usd_per_volpt: f64,
    pub gross_vex_usd_per_volpt: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CexSummary {
    /// Per 1 calendar day of time decay.
    pub net_cex_usd_per_day: f64,
    pub gross_cex_usd_per_day: f64,
}
```

`TradingState` gains a new top-level field:

```rust
// crates/scorpio-core/src/state/trading_state.rs

pub struct TradingState {
    // ... existing fields ...

    /// Risk-free rate (decimal fraction, e.g. 0.0427) sourced from FRED
    /// DGS3MO at preflight when the active pack is `EtfBaseline`. `None`
    /// when pack != EtfBaseline OR when the FRED fetch failed. Consumers
    /// (the ETF valuator) fall back to a const `0.045` in the `None` case.
    #[serde(default)]
    pub etf_risk_free_rate: Option<f64>,

    /// Persisted origin of the ETF risk-free-rate input so live runs and
    /// `scorpio report` can render the same fallback banner from snapshots.
    #[serde(default)]
    pub etf_risk_free_rate_source: Option<EtfRiskFreeRateSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum EtfRiskFreeRateSource {
    FredDgs3Mo,
    FallbackConst,
}
```

Per `CLAUDE.md`'s `TradingState` schema evolution rules:

- `etf_risk_free_rate` and `etf_risk_free_rate_source` carry `#[serde(default)]`; old snapshots without the fields deserialize unchanged.
- `GexSummary.strikes`, `GexSummary.broad`, `GexSummary.vex_summary`, `GexSummary.cex_summary`, and `BroadGex.expirations_total_considered` remain additive via `#[serde(default)]`. This stays compatible with Phase 1 snapshots because Phase 1 emitted `options_gex: None` rather than populated `GexSummary` payloads.
- No `THESIS_MEMORY_SCHEMA_VERSION` bump is required (no renames, removals, or type changes).
- No `#[serde(deny_unknown_fields)]` is introduced on any state struct touched here.

### `OptionsSnapshot` extension

```rust
// crates/scorpio-core/src/data/traits/options.rs

pub struct OptionsSnapshot {
    // ... existing fields unchanged ...

    /// Transient per-expiration per-strike rows for listed expirations beyond
    /// the authoritative front-month slice already carried in
    /// `near_term_expiration` / `near_term_strikes`.
    ///
    /// Stage 3 carries this through the technical -> analyst-sync handoff so
    /// the ETF valuator can derive `BroadGex`, then strips it immediately
    /// before state/context serialization so persisted technical snapshots keep
    /// only the bounded broad summary, not the full non-front-month chain
    /// payload.
    pub all_expirations: Vec<ExpirationStrikes>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExpirationStrikes {
    /// ISO-8601 expiration date.
    pub expiration: String,
    /// Per-strike rows for this expiration. Same row shape as the existing
    /// front-month `near_term_strikes`.
    pub strikes: Vec<NearTermStrike>,
}
```

`YFinanceOptionsProvider::fetch_snapshot_impl` already iterates expirations to build the term-structure ATM-IV vector. Contingent Stage 3 retains that existing iteration but additionally collects normalized strike rows for non-near-term expirations into the transient field. Broad aggregation consumes the authoritative front-month slice plus `all_expirations` together; no additional network calls.

Because `TechnicalOptionsContext::Available { outcome }` is serialized between tasks, `all_expirations` remains available through the technical -> analyst-sync handoff so the ETF valuator can derive broad GEX during the live run. The field is then explicitly cleared immediately before `serialize_state_to_context(...)` / snapshot save, making it **derive-don't-persist** at the durable boundary. The bounded persisted artifact is `BroadGex { net, gross, expirations_used, expirations_total_considered }` inside `EtfValuation.options_gex`; any future consumer that needs per-expiration rows after snapshot reload must refetch or define a separate persisted contract.

When the provider hits any `OptionsOutcome` other than `Snapshot(_)` (e.g. `SparseChain`, `NoListedInstrument`, `MissingSpot`, `HistoricalRun`), no change — the outcome is propagated as-is.

### `ValuationInputs` extension

```rust
// crates/scorpio-core/src/valuation/mod.rs

pub struct ValuationInputs<'a> {
    // ... existing fields including Phase 1 ETF fields ...

    /// Live ETF options snapshot threaded through from the persisted
    /// `TechnicalOptionsContext` before valuation runs.
    pub etf_options: Option<&'a crate::data::traits::options::OptionsSnapshot>,

    /// FRED DGS3MO snapshot threaded through from preflight when the
    /// active pack is `EtfBaseline`. `None` when pack != EtfBaseline OR
    /// when FRED was unreachable at preflight time. The ETF valuator
    /// substitutes a const `0.045` when `None`.
    pub etf_risk_free_rate: Option<f64>,
}
```

`AnalystSyncTask::fetch_valuation_inputs` reads the persisted `TechnicalOptionsContext`, copies `etf_options` into the carrier for the live valuation pass, and threads `state.etf_risk_free_rate` alongside the Phase 1 ETF fields. Equity valuators ignore the ETF-only additions.

The live run therefore derives broad GEX before the technical snapshot is durably serialized; `scorpio report` consumes the persisted `EtfValuation.options_gex.broad` summary rather than trying to replay broad aggregation from a reloaded `TechnicalOptionsContext`. As the last ETF-only cleanup step before `serialize_state_to_context(...)`, `AnalystSyncTask` strips `all_expirations` out of `state.technical_indicators.options_context` so snapshots keep the bounded summary but not the full non-front-month chain.

Persisted snapshot/report boundary: the new `etf_risk_free_rate_source` field is added on the real `TradingState` serialization boundary in `crates/scorpio-core/src/state/trading_state.rs` and on its wire-format snapshot representation so `AnalysisRuntime` and `scorpio report` render the same fallback banner from stored snapshots.

### FRED preflight integration

`PreflightTask` already records routing pack, profile presence, and fallback metadata for the ETF runtime classification. Contingent Stage 3 extends it with one additional opportunistic call. `workflow/builder.rs::build_graph_from_pack` already owns a shared `FredClient`; this phase threads that client into a new `PreflightTask` field / constructor argument so the preflight node can perform:

```rust
// crates/scorpio-core/src/workflow/tasks/preflight.rs (sketch)

if matches!(resolved_pack, PackId::EtfBaseline) {
    let rate = match self.fred.get_series_latest("DGS3MO").await {
        Ok(Some(pct)) => {
            // FRED returns observations as percent (e.g. "4.27"); convert
            // to decimal fraction.
            let frac = pct / 100.0;
            tracing::info!(target: "scorpio_core::workflow::preflight",
                series = "DGS3MO", rate_pct = pct, "fetched ETF risk-free rate");
            state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FredDgs3Mo);
            Some(frac)
        }
        Ok(None) => {
            tracing::warn!(target: "scorpio_core::workflow::preflight",
                series = "DGS3MO", "DGS3MO observation empty — falling back to const 0.045");
            state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FallbackConst);
            None
        }
        Err(e) => {
            tracing::warn!(target: "scorpio_core::workflow::preflight",
                series = "DGS3MO", error.kind = "fred_fetch",
                "DGS3MO fetch failed — falling back to const 0.045");
            // error.kind only — never the raw error text per CLAUDE.md
            let _ = e;
            state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FallbackConst);
            None
        }
    };
    state.etf_risk_free_rate = rate;
}
```

The fetch is gated on the resolved pack to avoid burning the FRED rate-limit budget on equity / fallback-to-baseline runs. When the pack is anything other than `EtfBaseline`, `etf_risk_free_rate` stays `None` and no FRED call is made.

Keeping live `DGS3MO` in this phase is still justified despite gamma's weak near-term sensitivity to `r`: once Stage 3 broad GEX is in scope, the run needs one stamped, replayable rate input that survives into snapshot/report output. One ETF-only preflight fetch on the existing `FredClient` keeps that input deterministic without introducing a second runtime source.

`RoutingFlags` remains a typed stage-entry control struct (`skip_debate`, `skip_risk`, `skip_auditor`) and is not extended for presentation metadata. Instead, `state.etf_risk_free_rate_source` is serialized with the snapshot and rendered in the report header when the value is `FallbackConst`:

```
  Analysis Pack    ETF Baseline
  ⚠ Risk-free rate fallback — FRED DGS3MO unavailable; using 0.045 const
```

When `state.etf_risk_free_rate_source == Some(EtfRiskFreeRateSource::FredDgs3Mo)` no banner is shown (success is the silent default).

### BSM math (`indicators/gex.rs`)

A new module of pure functions — no I/O, no `unsafe`, no panics. Public surface:

```rust
// crates/scorpio-core/src/indicators/gex.rs

/// Common BSM input bundle. All values are positive decimals; `t_years` is
/// the time-to-expiration in calendar years (e.g. 7/365 for a 1-week option).
pub struct BsmInputs {
    pub spot: f64,
    pub strike: f64,
    pub iv: f64,           // decimal vol, e.g. 0.18 for 18% annual
    pub r: f64,            // decimal risk-free rate
    pub q: f64,            // decimal dividend yield
    pub t_years: f64,
}

/// Black-Scholes-Merton gamma with continuous dividend yield.
/// Γ = e^{-q·t} · φ(d1) / (S · σ · √t)
/// Returns 0.0 for degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0).
pub fn bsm_gamma(inputs: BsmInputs) -> f64;

/// Black-Scholes-Merton vanna (call and put have the same vanna).
/// Vanna = -e^{-q·t} · φ(d1) · d2 / σ
/// Returns 0.0 for degenerate inputs.
pub fn bsm_vanna(inputs: BsmInputs) -> f64;

/// Black-Scholes-Merton call charm (∂Δ_call / ∂t, per year).
/// Charm_call = q·e^{-q·t}·N(d1) - e^{-q·t}·φ(d1)·[2(r-q)·t - d2·σ·√t] / (2·t·σ·√t)
pub fn bsm_charm_call(inputs: BsmInputs) -> f64;

/// Black-Scholes-Merton put charm.
pub fn bsm_charm_put(inputs: BsmInputs) -> f64;

/// Per-strike aggregated GEX exposure (post-OI, post-sign-convention, post-scaling).
/// Surfaced for the gamma-wall extraction step in
/// `valuation/etf/premium_discount.rs`. Only
/// net GEX is emitted per strike — Phase 2's `GexSummary.strikes` is a
/// gamma-walls list; per-strike VEX/CEX series are explicitly out of scope.
pub struct PerStrikeAggregate {
    pub strike: f64,
    pub net_gex_usd_per_1pct_move: f64,
}

/// Input bundle for chain-level aggregation.
pub struct AggregateInputs<'a> {
    pub spot: f64,
    pub r: f64,
    pub q: f64,
    pub as_of: chrono::NaiveDate,
    pub near_term_expiration: &'a str,
    pub near_term_strikes: &'a [NearTermStrike],
    pub expirations: &'a [ExpirationStrikes],
    pub atm_iv_fallback: f64,    // OptionsSnapshot.atm_iv
}

/// Result bundle covering near-term + broad aggregations.
pub struct AggregateResult {
    pub near_term: Option<NearTermAggregate>,
    pub broad: Option<BroadAggregate>,
    pub iv_fallback_count: u32,
    pub strikes_total: u32,
    pub strikes_used: u32,
}

pub struct NearTermAggregate {
    pub expiration: chrono::NaiveDate,
    /// Per-strike aggregates for the front-month chain. Each row carries the
    /// already-applied OI multiplier, SqueezeMetrics sign convention, and
    /// USD scaling. The wrapper layer extracts gamma walls by sorting on
    /// `|net_gex_usd_per_1pct_move|`.
    pub per_strike: Vec<PerStrikeAggregate>,
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub net_vex_usd_per_volpt: f64,
    pub gross_vex_usd_per_volpt: f64,
    pub net_cex_usd_per_day: f64,
    pub gross_cex_usd_per_day: f64,
}

pub struct BroadAggregate {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub expirations_used: u32,
    pub expirations_total_considered: u32,
}

pub fn aggregate(inputs: AggregateInputs) -> AggregateResult;
```

**IV-fallback rule.** For each strike row, the per-Greek call leg uses `NearTermStrike.call_iv` when `Some(_)`, else `atm_iv_fallback`; the put leg uses `put_iv` analogously. Each fallback increments `iv_fallback_count`. When both `call_iv` and `put_iv` are `None` *and* `atm_iv_fallback <= 0.0`, the row is skipped (`strikes_total` increments, `strikes_used` does not).

**Sign convention (SqueezeMetrics, applied uniformly to GEX / VEX / CEX).** Dealers are assumed net short calls (each call OI contributes positively to dealer exposure) and net long puts (each put OI contributes negatively). For each Greek, two aggregates are emitted:

- **Net** applies the dealer sign convention and sums contributions; positive net means dealer-stabilizing flow direction.
- **Gross** sums the absolute value of each contract type's contribution; gross is a magnitude scalar, never negative.

Per-strike formulas:

```
# GEX (gamma is always ≥ 0; |·| around contributions is redundant for gross)
net_gex_strike   = ( gamma_call · call_oi - gamma_put · put_oi)               · 100 · spot² · 0.01
gross_gex_strike = ( gamma_call · call_oi + gamma_put · put_oi)               · 100 · spot² · 0.01

# VEX (vanna can be signed; gross uses absolute contributions)
net_vex_strike   = ( vanna_call · call_oi - vanna_put · put_oi)               · 100 · spot
gross_vex_strike = (|vanna_call · call_oi| + |vanna_put · put_oi|)            · 100 · spot

# CEX (charm can be signed; gross uses absolute contributions)
net_cex_strike   = ( charm_call · call_oi - charm_put · put_oi)               · 100 · spot / 365
gross_cex_strike = (|charm_call · call_oi| + |charm_put · put_oi|)            · 100 · spot / 365
```

Multipliers:

- `100` — standard equity-option contract multiplier (shares per contract).
- `spot²` × `0.01` (GEX only) — converts gamma's per-share-per-dollar units into dollar exposure per 1% spot move.
- `spot` (VEX, CEX) — converts vanna/charm's per-share units into dollar exposure per 1.0 vol-point (VEX) or per 1 year (CEX), then `/365` for CEX to express per calendar day decay.

Aggregate-level `net_*` and `gross_*` fields are sums of the per-strike rows. `gross_*` aggregates are always ≥ 0 by construction; `net_*` aggregates can be signed.

**Near-term aggregation.** The aggregator computes the near-term block from `near_term_expiration` + `near_term_strikes`, not from `all_expirations`. If the parsed near-term expiration yields `t_years <= 0`, or if that front-month slice has no usable strikes after IV filtering, `near_term = None`. `per_strike` is populated with `PerStrikeAggregate` rows so the ETF valuator can extract the top-3 by `|net_gex_strike|`.

**Broad aggregation.** Contingent Stage 3 broad GEX combines the authoritative front-month slice with the additional expirations in `all_expirations`, accumulates the *gamma* sums only (broad VEX/CEX not emitted in this phase), and records both `expirations_used` and `expirations_total_considered`. The rendered/prompted broad line is explicitly described as a **single-rate approximation** because it reuses the one `DGS3MO` snapshot rather than maturity-matched discounting.

**Degenerate-input handling.** All BSM helpers return `0.0` when σ, t, or S is non-positive. `aggregate` returns an `AggregateResult` with `near_term = None` and `broad = None` when no expirations contain usable strikes. Never panics, never returns `Err`.

### GEX summary mapping in ETF valuation

The ETF valuation flow in `crates/scorpio-core/src/valuation/etf/premium_discount.rs` maps the math layer's `AggregateResult` into the state-layer `GexSummary` shape directly, rather than introducing a separate `valuation/etf/gex.rs` wrapper module:

```rust
// crates/scorpio-core/src/valuation/etf/premium_discount.rs

pub fn compute_gex_summary(
    snapshot: &OptionsSnapshot,
    r: f64,
    q: f64,
    as_of: chrono::NaiveDate,
) -> Option<GexSummary> {
    let agg = indicators::gex::aggregate(indicators::gex::AggregateInputs {
        spot: snapshot.spot_price,
        r,
        q,
        as_of,
        near_term_expiration: &snapshot.near_term_expiration,
        near_term_strikes: &snapshot.near_term_strikes,
        expirations: &snapshot.all_expirations,
        atm_iv_fallback: snapshot.atm_iv,
    });

    let near = agg.near_term?;

    // Diagnostic warnings — never user-visible, never block emission.
    if agg.iv_fallback_count > agg.strikes_used / 2 {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            iv_fallback_count = agg.iv_fallback_count,
            strikes_used = agg.strikes_used,
            "GEX computed with majority ATM-IV fallbacks — gamma skew may be understated"
        );
    }

    // Top-3 strikes by |net_gex|. Per-strike rows already carry the
    // OI-multiplied, sign-converted, USD-scaled exposure from the
    // aggregator — no further math here, just sort and truncate.
    let mut walls: Vec<StrikeGex> = near.per_strike.iter()
        .map(|p| StrikeGex {
            strike: p.strike,
            net_gex_usd_per_1pct_move: p.net_gex_usd_per_1pct_move,
        })
        .collect();
    walls.sort_by(|a, b| b.net_gex_usd_per_1pct_move.abs()
        .partial_cmp(&a.net_gex_usd_per_1pct_move.abs())
        .unwrap_or(std::cmp::Ordering::Equal));
    walls.truncate(3);

    let call_put_oi_ratio = if snapshot.put_call_oi_ratio > 0.0 {
        1.0 / snapshot.put_call_oi_ratio
    } else {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            "put_call_oi_ratio is zero — call_put_oi_ratio omitted (set to 0.0)"
        );
        0.0
    };

    Some(GexSummary {
        net_gex_usd_per_1pct_move: near.net_gex_usd_per_1pct_move,
        gross_gex_usd_per_1pct_move: near.gross_gex_usd_per_1pct_move,
        call_put_oi_ratio,
        max_pain_strike: snapshot.max_pain_strike,
        near_term_expiration: near.expiration,
        strikes: walls,
        broad: agg.broad.map(|b| BroadGex {
            net_gex_usd_per_1pct_move: b.net_gex_usd_per_1pct_move,
            gross_gex_usd_per_1pct_move: b.gross_gex_usd_per_1pct_move,
            expirations_used: b.expirations_used,
            expirations_total_considered: b.expirations_total_considered,
        }),
        vex_summary: Some(VexSummary {
            net_vex_usd_per_volpt: near.net_vex_usd_per_volpt,
            gross_vex_usd_per_volpt: near.gross_vex_usd_per_volpt,
        }),
        cex_summary: Some(CexSummary {
            net_cex_usd_per_day: near.net_cex_usd_per_day,
            gross_cex_usd_per_day: near.gross_cex_usd_per_day,
        }),
    })
}
```

Returns `None` when the declared near-term slice cannot produce a usable aggregate. The valuator treats this as **dealer positioning unavailable from the fetched options snapshot**, distinct from the earlier no-snapshot path.

### Options-chain hydration in `AnalystSyncTask`

`AnalystSyncTask` does not grow a new direct `OptionsToolContext` dependency. Instead, the ETF valuation path reads the persisted `TechnicalOptionsContext` already stored on technical state after the technical analysis path materializes options data:

```rust
// crates/scorpio-core/src/workflow/tasks/analyst.rs (sketch)

if pack_id == PackId::EtfBaseline {
    // Phase 1 ETF hydration (quote, fund_info, holdings, benchmark OHLCV) unchanged.

    // Phase 2: read the persisted technical options context that already
    // survives the pipeline on `state.technical_indicators.options_context`.
    let etf_options = match state
        .technical_indicators
        .as_ref()
        .and_then(|technical| technical.options_context.as_ref())
    {
        Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(snap),
        }) => Some(snap),
        Some(TechnicalOptionsContext::Available { outcome: other }) => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                outcome = %other,
                symbol = %symbol,
                "ETF options chain unavailable — GEX/VEX/CEX skipped"
            );
            None
        }
        Some(TechnicalOptionsContext::FetchFailed { reason }) => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                symbol = %symbol,
                fetch_reason = %reason,
                "ETF options fetch failed before valuation — GEX/VEX/CEX skipped"
            );
            None
        }
        None => None,
    };

    valuation_inputs.etf_options = etf_options;
    valuation_inputs.etf_risk_free_rate = state.etf_risk_free_rate;
}
```

No new fetcher is wired; Phase 2 reuses the existing options fetch that already populates `TechnicalOptionsContext` in state. The ETF and equity flows still share one fetch per cycle.

### Leverage-warning injection

```rust
// crates/scorpio-core/src/analysis_packs/etf/baseline.rs

const ETF_LEVERAGE_WARNING: &str = include_str!("prompts/etf_leverage_warning.md");

const LEVERAGE_TOLERANCE: f64 = 1e-6;

/// Append a leverage-warning suffix when the ETF
/// has a non-unit leverage factor. Borrowing fast-path when no warning is
/// needed; owned allocation only on the leveraged-ETF branch.
pub fn append_leverage_warning_if_needed(
    rendered: String,
    leverage_factor: Option<f64>,
) -> String {
    match leverage_factor {
        Some(f) if (f - 1.0).abs() > LEVERAGE_TOLERANCE => {
            format!("{}\n\n---\n\n{}", rendered, ETF_LEVERAGE_WARNING)
        }
        _ => rendered,
    }
}
```

The integration points are explicit rather than renderer-generic:

- `crates/scorpio-core/src/agents/risk/common.rs::render_risk_system_prompt` applies `append_leverage_warning_if_needed(...)` after `{ticker}` / `{analysis_emphasis}` substitution for the Conservative-risk and Neutral-risk bundle slots.
- `crates/scorpio-core/src/agents/auditor/prompt.rs::build_system_prompt` applies the same helper after `{ticker}` substitution for the auditor slot.

For non-ETF runs the helper is not invoked — the prompts substitute unchanged.

The leverage factor source is `EtfValuation.leverage_factor` (already populated by the Phase 1 `EtfPremiumDiscountValuator::assess` from `FundInfo.leverage_factor`). When the field is `None` or `1.0`, the warning is suppressed.

Trader, Aggressive-risk, Fund-manager, and the four analyst prompts do not receive the warning — the dual-risk audit contract only requires it on the explicitly listed slots.

### Report rendering — DEALER POSITIONING block

`crates/scorpio-reporters/src/terminal/etf.rs` extends `render_etf_panel` with a new sub-section. Stage 2 keeps the block compact by leading with a plain-English summary line and treating VEX/CEX as secondary Stage 3 sensitivity detail. Layout when the full Stage 3 surface populates:

```
  ─── DEALER POSITIONING ──────────────────────────────────────────────
  Near-term  (2026-05-23)
    Summary         Dealer hedging likely dampens near-term moves; gamma walls cluster near $615-$625
    Net GEX/1%      +$2.84B    Gross GEX/1%    $7.12B
    Call/Put OI      1.31      Max-pain        $620
    Gamma walls    +$1.20B @ $625, -$0.84B @ $615, +$0.62B @ $630
    Secondary sensitivities
      Net VEX/volpt -$1.20B    Gross VEX       $4.10B
      Net CEX/day   +$0.45B    Gross CEX       $2.30B

  All expirations  (5 used)
    Net GEX/1%      +$8.40B    Gross GEX/1%    $22.1B
```

Per-line visibility rules:

- The entire block is hidden when `options_gex.is_none()`.
- The `Summary ...` line is required whenever the block renders; it is the mainstream-reader headline for the section.
- The Stage 2 validation slice requires only the summary + near-term GEX/call-put/max-pain/gamma-wall core. `Secondary sensitivities` and `All expirations` are Stage 3 additions.
- The `Secondary sensitivities` sub-block is hidden when `vex_summary` and `cex_summary` are absent.
- The `Gamma walls …` line is hidden when `strikes.is_empty()`.
- The `All expirations (N used)` sub-block is hidden when `broad.is_none()`.
- When `options_context` never produced `OptionsOutcome::Snapshot(_)`, the DATA AVAILABILITY section surfaces `⚠ Dealer positioning skipped — no options chain snapshot available`.
- When a snapshot exists but near-term aggregation is unusable, the DATA AVAILABILITY section surfaces `⚠ Dealer positioning skipped — options snapshot present but no usable near-term dealer-positioning aggregate`.
- When the block is present but sub-lines are unavailable, the report prints compact partial-data notes instead of silently omitting context. If both gamma walls and broad GEX are unavailable, the note is combined into one line: `Dealer positioning partial — gamma walls and broad GEX unavailable`.
- The broad line uses `All expirations` only when `expirations_used == expirations_total_considered`; otherwise it renders as `Partial expirations` with a note indicating how many expirations were usable.

The Phase 1 reserved block (`─── DEALER GAMMA (near-term YYYY-MM-DD) ───` from the parent design's wide-terminal example) is replaced by the new `DEALER POSITIONING` layout. Phase 1 never actually emitted the reserved block (`options_gex` was always `None`), so this is the block's first real implementation — not a re-render of an existing layout.

Narrow-terminal fallback follows the same convention as Phase 1: fields stack vertically, plain ASCII labels, no decorative Unicode. The order is summary first, then near-term GEX core metrics, then gamma walls, then secondary sensitivities if present, then the partial-data note if needed, then the `All expirations` broad-GEX approximation line.

When `state.etf_risk_free_rate_source == Some(EtfRiskFreeRateSource::FallbackConst)`, the report header surfaces an additional warning line under the Analysis Pack row:

```
  Analysis Pack    ETF Baseline
  ⚠ Risk-free rate fallback — FRED DGS3MO unavailable; using 0.045 const
```

### Prompt updates — `etf_tracking_options_focus.md`

The Phase 1 file ships with placeholder language explicitly marking the options-chain branch as "deferred to Phase 2." Phase 2 replaces that section with a concrete contract the updated prompt text must satisfy:

- When `options_gex` is present, the prompt must instruct the technical analyst to treat dealer positioning as a **secondary ETF baseline overlay** on top of premium/discount, composition, and tracking evidence.
- The prompt must require the analyst to open the dealer-positioning discussion with one plain-English takeaway sentence for a mainstream ETF reader; raw VEX/CEX detail stays secondary when surfaced.
- The prompt must require separate treatment of:
  - near-term `net_gex_usd_per_1pct_move`
  - broad `broad.net_gex_usd_per_1pct_move` when present, explicitly labeled as an all-expirations approximation
  - `vex_summary.net_vex_usd_per_volpt` when present, as a conditional sensitivity to absolute IV moves
  - `cex_summary.net_cex_usd_per_day` when present, as a conditional sensitivity to one day of time decay
  - gamma walls from `strikes`
  - `call_put_oi_ratio` and `max_pain_strike` as supporting, not primary, evidence
- The prompt must instruct the analyst to name partial availability explicitly:
  - if `broad` is `None`, say broad dealer positioning is unavailable while near-term positioning is present
  - if `broad` is present but `expirations_used < expirations_total_considered`, label it as `Partial expirations` broad dealer positioning and mention both counts
  - if `strikes` is empty, say gamma walls are unavailable
  - if both `broad` and `strikes` are unavailable, say both are unavailable in one combined sentence
  - if `options_gex` is absent because no snapshot exists, say dealer-positioning signals are unavailable because no options chain snapshot was available
  - if `options_gex` is absent because the snapshot was unusable, say dealer-positioning signals are unavailable because the fetched options snapshot did not contain a usable near-term aggregate
- The prompt must preserve the existing `{ticker}` placeholder contract and continue to pass `validate_active_pack_completeness` for all topology shapes.

The manifest `etf_baseline_prompt_bundle` is unchanged — the same `compose_etf_section(technical_analyst.md, &[etf_tracking_options_focus.md])` call picks up the updated bytes.

## Failure modes & data availability

Additive to the Phase 1 failure-modes table:

| Condition                                                       | Detection                                                       | Behaviour                                                                                                                                                                                         |
|-----------------------------------------------------------------|-----------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| FRED `DGS3MO` returns empty or errors                           | `Result<Option<f64>, TradingError>` is `Ok(None)` or `Err(_)`   | `state.etf_risk_free_rate = None`; `state.etf_risk_free_rate_source = Some(FallbackConst)`; valuator falls back to const `0.045`; report header warns.                                            |
| Options chain unavailable (any non-`Snapshot` outcome)          | `TechnicalOptionsContext != Available { outcome: Snapshot(_) }` | `valuation_inputs.etf_options = None`; `options_gex = None`; `flags.options_chain_present = false`; DATA AVAILABILITY shows `⚠ Dealer positioning skipped — no options chain snapshot available`. |
| `aggregate` produces no near-term aggregate                     | `AggregateResult.near_term.is_none()`                           | `compute_gex_summary` returns `None`; DATA AVAILABILITY shows `⚠ Dealer positioning skipped — options snapshot present but no usable near-term dealer-positioning aggregate`.                     |
| Declared near-term expiration is same-day / degenerate          | Parsed `t_years <= 0` for `near_term_expiration`                | The front-month near-term slice is treated as unusable for dealer positioning; no silent rollover to a later expiration.                                                                          |
| Per-strike IV is `None` on both call and put + no ATM fallback  | All three vol sources are missing                               | Row skipped (`strikes_total++`, `strikes_used` unchanged).                                                                                                                                        |
| Majority of strikes used the ATM fallback                       | `iv_fallback_count > strikes_used / 2`                          | `warn!` emitted with `iv_fallback_count` / `strikes_used`; GEX still emitted; no user-visible degradation.                                                                                        |
| `OptionsSnapshot.put_call_oi_ratio == 0.0`                      | Division guard                                                  | `call_put_oi_ratio` set to `0.0` with a `warn!` log.                                                                                                                                              |
| ETF has `leverage_factor != 1.0`                                | `EtfValuation.leverage_factor`                                  | Renderer appends `etf_leverage_warning.md` to Conservative + Neutral + Auditor prompts.                                                                                                           |
| Broad GEX unavailable but near-term dealer positioning exists   | `options_gex.is_some()` and `broad.is_none()`                   | DEALER POSITIONING renders the near-term block plus a partial-data note saying broad dealer positioning is unavailable.                                                                           |
| Gamma walls unavailable but near-term dealer positioning exists | `options_gex.is_some()` and `strikes.is_empty()`                | DEALER POSITIONING renders the near-term block plus a partial-data note saying gamma walls are unavailable.                                                                                       |
| Broad GEX uses only partial expiration coverage                 | `expirations_used < expirations_total_considered`               | DEALER POSITIONING renders `Partial expirations` and states broad GEX was computed from a subset of listed expirations.                                                                           |

## Test plan

| Layer                            | Location                                                                                                             | Coverage                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
|----------------------------------|----------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| BSM math                         | `crates/scorpio-core/src/indicators/gex.rs#tests`                                                                    | `bsm_gamma`, `bsm_vanna`, `bsm_charm_call`, `bsm_charm_put` against analytical reference values within `1e-6`; degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`; ATM gamma > OTM gamma sanity case; Vanna sign matches call/put symmetry expectation.                                                                                                                                                                                                                            |
| Chain aggregation                | `crates/scorpio-core/src/indicators/gex.rs#tests`                                                                    | SqueezeMetrics sign convention (call OI +, put OI −) for GEX / VEX / CEX; IV-fallback path increments counter; empty `expirations` → both `near_term` and `broad` are `None`; mixed-IV multi-expiration broad aggregation sums correctly.                                                                                                                                                                                                                                            |
| GEX summary mapping              | `crates/scorpio-core/src/valuation/etf/premium_discount.rs#tests`                                                    | `compute_gex_summary` returns `None` when near-term aggregate missing; gamma-wall sort/truncate to top-3 by `\|net_gex\|`; zero `put_call_oi_ratio` guard sets `call_put_oi_ratio = 0.0` with warning; field plumbing from snapshot to summary verified, including broad coverage counts.                                                                                                                                                                                            |
| Valuator integration             | `crates/scorpio-core/src/valuation/etf/premium_discount.rs#tests`                                                    | Phase-1 cases unchanged; Stage 2 cases cover near-term GEX + gamma walls; Stage 3 cases extend that to `broad`, `vex_summary`, and `cex_summary`; `etf_risk_free_rate` falls back to const when `None`; flags reflect chain presence.                                                                                                                                                                                                                                                |
| OptionsSnapshot serde            | `crates/scorpio-core/src/data/traits/options.rs#tests`                                                               | `OptionsSnapshot` carries `all_expirations` through the live handoff but strips it before durable state/snapshot serialization; tests cover populated in-memory rows plus the boundary cleanup that leaves persisted/reloaded snapshots without the non-front-month strike payload; `ExpirationStrikes` remains valid for the in-run aggregator contract.                                                                                                                            |
| TradingState serde               | `crates/scorpio-core/tests/state_roundtrip.rs` (extend)                                                              | `etf_risk_free_rate` + `etf_risk_free_rate_source` round-trip with `#[serde(default)]`; legacy Phase 1 snapshots without those fields deserialize unchanged; `GexSummary` with additive `strikes` / `broad` / `vex_summary` / `cex_summary` round-trips, including `BroadGex.expirations_total_considered`; Phase 1 snapshots with `options_gex: None` still deserialize unchanged.                                                                                                  |
| Routing flags / persisted source | `crates/scorpio-core/tests/workflow_pipeline_structure.rs` + `crates/scorpio-core/tests/state_roundtrip.rs` (extend) | `workflow/builder.rs` threads `FredClient` into `PreflightTask`; preflight fetches DGS3MO only when resolved pack is `EtfBaseline`; leaves `RoutingFlags` unchanged; persists `etf_risk_free_rate_source = FredDgs3Mo` on success and `FallbackConst` on fallback; non-ETF runs skip the FRED call entirely.                                                                                                                                                                         |
| ETF input hydration              | `crates/scorpio-core/src/workflow/tasks/analyst.rs#tests` (extend)                                                   | Persisted `TechnicalOptionsContext::Available { outcome: Snapshot(_) }` → `etf_options: Some(_)`; every other `TechnicalOptionsContext` variant → `None` + `warn!` log captured; Stage 3 live-run coverage exercises `all_expirations` through valuation and then verifies the cleanup step that strips it before `serialize_state_to_context(...)`, while snapshot reloads rely on persisted `options_gex.broad`; `etf_risk_free_rate` is plumbed from state into valuation inputs. |
| Leverage warning                 | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` + prompt-builder tests (extend)                         | `append_leverage_warning_if_needed` appends the suffix for `2.0` / `-1.0` / `3.0` and leaves the rendered prompt unchanged for `None` / `1.0`; `render_risk_system_prompt` and `auditor::build_system_prompt` inject it only for Conservative / Neutral / Auditor; other ETF roles remain untouched.                                                                                                                                                                                 |
| Tracking-prompt update           | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` (extend)                                                | Updated `etf_tracking_options_focus.md` still passes `validate_active_pack_completeness` for all four topology shapes; placeholder text no longer present; prompt contract includes baseline-overlay framing, the plain-English takeaway requirement, partial-expiration broad-GEX wording, and distinct no-snapshot vs unusable-snapshot absence branches.                                                                                                                          |
| Report rendering                 | `crates/scorpio-reporters/tests/terminal.rs` (extend)                                                                | DEALER POSITIONING block renders when `options_gex.is_some()`; the summary line is present whenever the block renders; partial-data note renders for missing `broad`, missing `strikes`, and the combined missing state; fallback copy distinguishes no snapshot from unusable snapshot; broad coverage label switches between `All expirations` and `Partial expirations`; VEX/CEX remain secondary detail; equity reports unaffected.                                              |
| Smoke (manual, FRED)             | `crates/scorpio-core/examples/fred_live_test.rs` (extend)                                                            | Add DGS3MO assertion alongside the existing FEDFUNDS / CPI cases; verify the raw FRED observation is in a plausible percent range before preflight converts it to a decimal fraction; runs only when `SCORPIO_FRED_API_KEY` is set.                                                                                                                                                                                                                                                  |
| Smoke (manual, yfin)             | `crates/scorpio-core/examples/yfinance_options_chain_live_test.rs` (new)                                             | `OptionsProvider::fetch_snapshot("SPY", today)` returns `Snapshot(_)`; the live in-memory `all_expirations.len() >= 2`; each expiration has non-empty `strikes`; no `all_expirations` entry shares `near_term_expiration`; bogus ticker → non-`Snapshot` outcome, no panic.                                                                                                                                                                                                          |
| Smoke (manual, e2e)              | `crates/scorpio-core/examples/etf_options_gex_live_test.rs` (new)                                                    | Full Stage 3 path: `AnalysisRuntime::run("SPY")` produces `ScenarioValuation::Etf(_)` with `options_gex: Some(g)`, `g.broad: Some(_)`, populated `g.vex_summary`, populated `g.cex_summary`, `g.strikes.len() == 3`; `state.etf_risk_free_rate_source == Some(FredDgs3Mo)` on a successful fetch path.                                                                                                                                                                               |

Live smokes are NOT in CI — invoke via `cargo run -p scorpio-core --example <name>` per the existing convention.

The smoke-coverage rule is a standing requirement: any new external fetch path added during Phase 2 implementation (or any later phase) gets its own `examples/*_live_test.rs` file. The spec lists the three expected smokes above; implementation may add more.

## Data integrity follow-up (post-Phase 2 implementation)

Three Phase 1-era data-plumbing gaps surfaced during Phase 2 validation runs, leaving the trust-signal renderer printing `NAV ✗ Bid/Ask ✗ Holdings ✗ Benchmark ✗` for representative ETFs. Each gap was fixed alongside Phase 2 completion. The fixes are independent of every Phase 2 architectural decision above — they touch fetcher and resolver layers that Phase 1 introduced but did not exercise end-to-end on real ETF tickers.

| Symptom (renderer output)                                        | Root cause                                                                                                                                                  | Fix                                                                                                                                                                                                                                                       |
|------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `NAV unavailable` + `Bid/Ask unavailable`                        | `yfinance-rs` 0.7 `Quote`/`Info` types do not expose `navPrice`, `bid`, or `ask`; `EtfQuote.{nav, bid, ask}` always resolved to `None`.                     | Direct Yahoo `v10/quoteSummary` HTTP fetch with manual cookie + crumb auth, populating all three fields in one call.                                                                                                                                      |
| `Tracking error skipped — benchmark not resolved`                | `FundInfo.stated_benchmark` is rarely populated upstream for ETFs; N-PORT-P also lacks a benchmark field for most tickers.                                  | Static `ETF → ^index` lookup (`^GSPC`, `^NDX`, `^DJI`, `^RUT`, `^MID`, `^SP600`, `^SOX`) consulted as a third fallback after the upstream sources in `resolve_benchmark_symbol`.                                                                          |
| `Holdings unavailable — N-PORT-P data missing or too stale`      | `resolve_fund_cik` only consulted `/files/company_tickers.json`, which lists operating companies (10-K/10-Q filers). ETFs file as fund series elsewhere.    | Add `/files/company_tickers_mf.json` as a lazy-loaded second source; `resolve_fund_cik` tries the equity map first, then the MF map. SOXX/SPY/QQQ etc. now resolve to a CIK and the N-PORT-P fetch proceeds.                                              |

### NAV / bid / ask backfill via Yahoo `quoteSummary`

**Files:**

- **New:** `crates/scorpio-core/src/data/yfinance/summary.rs` — pure-HTTP `SummaryHttp` client, ~300 lines incl. tests. Performs the cookie/crumb dance (`GET fc.yahoo.com` → extract `Set-Cookie` → `GET getcrumb` → reuse on subsequent requests), then hits `query2.finance.yahoo.com/v10/finance/quoteSummary/{symbol}?modules=summaryDetail&crumb=…`. A schema-validating parser extracts `navPrice` / `bid` / `ask` from the `summaryDetail` block. Cookie + crumb are cached in an `Arc<RwLock<AuthState>>`; a 401/403 response clears both and triggers one-shot retry.
- **Modify:** `crates/scorpio-core/src/data/yfinance/client.rs` — `YfSession` gains a `summary: SummaryHttp` field with a `pub(super) fn summary(&self) -> &SummaryHttp` accessor.
- **Modify:** `crates/scorpio-core/src/data/yfinance/etf.rs::get_quote` — after the existing `Ticker::quote()` + `Ticker::info()` calls, additionally invokes `self.session.summary().fetch(symbol)` under the same rate limiter. Any failure leaves `EtfQuote.nav/bid/ask` as `None`; the trust-signal renderer's existing fail-soft path remains unchanged.

**Why manual cookie handling:** workspace `reqwest` is configured without the `cookies` feature. Rather than expand the workspace dependency surface, this module extracts the first `Set-Cookie` header value and attaches it as a `Cookie` header on follow-on requests — the same degree of state the `yfinance-rs` auth flow needs internally. The pattern mirrors `yfinance-rs/src/core/client/auth.rs` step-for-step.

### Static ETF → benchmark lookup

**Files:**

- **New:** `crates/scorpio-core/src/data/etf_benchmarks.rs` — pure-function `resolve(etf_symbol: &str) -> Option<&'static str>` keyed off a `match` over uppercase-normalized tickers. Returns the Yahoo Finance index ticker (`^GSPC`, `^NDX`, …) that `PriceHistoryProvider` can already fetch via the existing OHLCV path.
- **Modify:** `crates/scorpio-core/src/workflow/tasks/analyst.rs::resolve_benchmark_symbol` — added `etf_symbol: &str` as the first parameter and appended a third `.or_else()` clause consulting the static lookup. The existing priority chain becomes: yfinance `FundInfo.stated_benchmark` → SEC N-PORT `stated_benchmark` → static map. Both call sites were updated to pass the ETF symbol.

**Coverage scope.** ~15 mappings covering ~95% of liquid US ETF volume: S&P 500 family (SPY/IVV/VOO/SPLG), Nasdaq-100 (QQQ/QQQM), Nasdaq Composite (ONEQ), Dow (DIA), Russell 2000 (IWM/VTWO), S&P MidCap 400 (IJH/MDY), S&P SmallCap 600 (IJR/SLY), PHLX Semiconductor (SMH/SOXX/SOXL/SOXS), and proxy mappings VTI/ITOT → `^GSPC`.

**Intentional omissions.** ETFs tracking indices that Yahoo Finance does not publish as a free symbol — MSCI EAFE/EM (EFA, VEA, VWO, EEM), Bloomberg Aggregate (AGG, BND), Treasury indices (TLT, IEF), commodities (GLD, SLV), S&P GICS sector sub-indices (XLF, XLK, XLE, …), and unconstrained funds (ARKK) — are deliberately not mapped. Mapping them to a near-neighbor proxy would silently corrupt tracking-error against the stated benchmark. These ETFs continue to surface `Tracking error skipped — benchmark not resolved` until a different benchmark data source is added in a follow-up.

### SEC `company_tickers_mf.json` fallback for ETF CIK resolution

**Files:**

- **Modify:** `crates/scorpio-core/src/data/sec_edgar/mod.rs`:
  - New constant `COMPANY_TICKERS_MF_PATH = "/files/company_tickers_mf.json"`.
  - New deserializer `MfTickersResponse { fields: Vec<String>, data: Vec<(u32, String, String, String)> }` modeling the SEC column-oriented response shape `{ "fields": ["cik", "seriesId", "classId", "symbol"], "data": [[1100663, "S000004354", "C000012084", "SOXX"], …] }`.
  - New parser `parse_company_tickers_mf` — validates `fields[0] == "cik"` and `fields[3] == "symbol"` so a silent SEC column reorder fails loudly rather than emitting wrong CIKs.
  - New cache field `cik_mf_cache: Arc<RwLock<Option<HashMap<String, u32>>>>` — independent lazy load.
  - New public method `lookup_cik_mf(ticker) -> Result<Option<u32>, TradingError>` — mirrors the existing `lookup_cik` behavior (shared circuit breaker, shared rate limiter, same fail-soft contract).
  - Updated `resolve_fund_cik` — tries `lookup_cik` first, then falls back to `lookup_cik_mf`. Returns the zero-padded 10-digit CIK from whichever map hits.

**Why this matters.** Phase 1's holdings path looked up CIKs only in the equity map. SPY (CIK 884394, series S000003474), QQQ, IWM, SOXX (CIK 1100663, series S000004354) all live in the MF map, so the lookup always returned `Ok(None)` and the entire N-PORT-P fetch was silently skipped — producing the `Holdings unavailable — N-PORT-P data missing or too stale` warning regardless of whether SEC actually had a recent N-PORT for the fund. With this fallback, the existing 180-day staleness threshold and N-PORT-P parser apply unchanged; only the CIK gate moves out of the way.

**Downstream effect on trader confidence.** With holdings now populated, the valuation context delivered to the trader agent no longer renders `EtfComposition` as `null`. The trader prompt's existing instruction — `"Treat any analyst input rendered as null or a null research consensus as missing upstream context. Explicitly acknowledge the material data gap in rationale and calibrate confidence conservatively."` — stops triggering on the holdings field, and confidence normalizes without a prompt change.

### Smoke test consolidation

The four pre-existing ETF live smokes (`etf_quote_live_test.rs`, `etf_options_gex_live_test.rs`, `etf_pack_live_test.rs`) plus the new `etf_data_gap_live_test.rs` were merged into a single sectioned smoke at `crates/scorpio-core/examples/etf_live_test.rs`. The merged binary runs four sequential sections behind clear `§N` banners and treats each section's failure as fail-soft (one section's failure is reported to stderr but does not abort the others). The Stage 4 `dealer_positioning_smoke` retains its `Result`-based contract — `assert!` calls were converted to a `require()` helper to preserve assertion intent without panicking the whole run.

| Section               | Surface covered                                                                                           |
|-----------------------|-----------------------------------------------------------------------------------------------------------|
| §1 ETF surface        | `get_profile` / `get_quote` / `get_fund_info` / `get_distribution_yield_ttm` over SPY/QQQ/TQQQ/AAPL/BOGUS |
| §2 Data-gap fill      | NAV + bid + ask + benchmark resolution across SPY/QQQ/IWM/VTI/SMH/SOXX/AAPL/XYZ123_BOGUS                  |
| §3 Pack routing       | `classify_runtime_pack` over SPY/AAPL/BOGUS                                                               |
| §4 Dealer positioning | Full Stage 3 GEX path on SPY (optional `SCORPIO_FRED_API_KEY`)                                            |

Run: `cargo run -p scorpio-core --example etf_live_test` (`SCORPIO_FRED_API_KEY=…` prefix for §4's preferred rate source).

### Additive test coverage

| Layer                    | Location                                                          | Coverage                                                                                                                                                                                                                                                                                                                                                                                                                                          |
|--------------------------|-------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Yahoo summary parser     | `crates/scorpio-core/src/data/yfinance/summary.rs#tests`          | Full payload roundtrip; partial payload missing `navPrice`; empty `result[]`; null `result`; missing `summaryDetail`; garbage/empty body returns `None`.                                                                                                                                                                                                                                                                                          |
| Static benchmark lookup  | `crates/scorpio-core/src/data/etf_benchmarks.rs#tests`            | Each mapped family resolves to the documented Yahoo ticker (`^GSPC`, `^NDX`, `^DJI`, `^RUT`, `^MID`, `^SP600`, `^SOX`); case insensitivity + trim; proxy mappings (VTI/ITOT → `^GSPC`); unmapped tickers return `None`; blank/whitespace returns `None`.                                                                                                                                                                                          |
| `resolve_benchmark_symbol` fallback | `crates/scorpio-core/src/workflow/tasks/analyst.rs#tests` | Existing fund-info/N-PORT priority preserved; new fallback path: when both upstream sources are silent, the static lookup keyed by ETF symbol resolves the benchmark; unmapped symbol with all sources silent returns `None`.                                                                                                                                                                                                                     |
| SEC MF parser            | `crates/scorpio-core/src/data/sec_edgar/mod.rs#tests`             | Real SOXX/SPY rows extract the right CIK; case normalization; column-reorder schema check fires (header validation prevents silent corruption); type-mismatch column move (e.g. cik no longer at position 0) fails via serde; malformed/empty JSON returns `Err`.                                                                                                                                                                                 |
| `resolve_fund_cik` MF fallback | `crates/scorpio-core/src/data/sec_edgar/mod.rs#tests` (mocked HTTP) | Equity map misses → MF map hits → SOXX resolves to `"0001100663"`; equity map hits → MF endpoint is never queried; both maps miss → `None`; preloaded MF cache short-circuits without HTTP.                                                                                                                                                                                                                                                       |

## Out of scope

- **iNAV / real-time NAV** — yfinance only provides end-of-prior-day NAV (carried from Phase 1).
- **Higher-order Greeks beyond Vanna and Charm** — Vomma, Speed, Color, Zomma, Veta are not emitted. Vanna and Charm are the tier practitioners cite alongside GEX; further-order Greeks rarely surface in dealer-flow analysis.
- **Per-strike Vanna / Charm exposure series** — only `GexSummary.strikes` (gamma walls) is emitted as a per-strike series. Net/gross aggregates are sufficient for prompt-level VEX/CEX reasoning; per-strike Vanna/Charm series would inflate state without a clear LLM use case yet.
- **Persisted per-expiration strike rows** — `OptionsSnapshot.all_expirations` is derive-only at the durable boundary; the live run may carry it through analyst sync to compute broad GEX, but persisted snapshots keep only bounded `BroadGex` counts, not the full non-front-month chain payload.
- **Time-series GEX / historical dealer positioning** — `GexSummary` is a single point-in-time snapshot per run. No historical series, no longitudinal trend.
- **Per-pack risk-free rate sources** — FRED `DGS3MO` is the single series. No DGS1MO / DGS6MO / DGS1 / per-expiration discounting. Acceptable because gamma is weakly sensitive to `r` for near-term expirations.
- **GEX in non-USD denominations** — output is always USD; ETFs traded in non-USD venues are out of scope.
- **Deterministic fund-manager veto on dealer-positioning extremes** — GEX/VEX/CEX stay LLM-visible evidence; no `gex_pinning_extreme` analogue to Phase 1's `tracking_failure` / `extreme_premium` / `leverage_decay` triggers.
- **N-PORT cache and storage consolidation** — durability work for SEC holdings fetches, SQLite layout changes, and any cache-path/config migration are deferred to a later design pass.
- **Non-Yahoo benchmark resolution for international / bond / sector ETFs** — the static ETF→benchmark map added in the data-integrity follow-up intentionally omits MSCI EAFE/EM, Bloomberg Aggregate, Treasury indices, commodities, S&P GICS sector sub-indices, and unconstrained funds because Yahoo Finance does not publish their actual benchmarks as a free index symbol. Resolving these would require a different benchmark price-history source and is deferred.

## Open questions

None for the design itself. Implementation-time questions deferred to the writing-plans phase:

- Exact threshold for the "majority IV fallback" `warn!` log — `> strikes_used / 2` is the proposed cutoff but may be tightened or loosened once we see real chain-sparsity rates on common ETFs.
- Whether `BroadGex` should also carry `net_vex_usd_per_volpt` and `net_cex_usd_per_day` across all expirations — Phase 2 emits broad GEX only; broad VEX/CEX deferred unless prompt analysis shows the LLM citing them. Adding them later is additive (new `#[serde(default)]` fields on `BroadGex`).
- Whether `etf_options_gex_live_test.rs` should also assert a non-zero `vex_summary.net_vex_usd_per_volpt` — exact magnitude depends on the live chain at run time, so the initial assertion can stop at `options_gex.is_some()` plus finite/populated VEX fields; magnitude thresholds may be added once we have a baseline.

## References

- [`2026-05-21-etf-baseline-pack-design.md`](./2026-05-21-etf-baseline-pack-design.md) — Phase 1 parent design; Phase 2 reuses every Phase 1 architectural decision verbatim.
- [`2026-04-28-shared-options-evidence-design.md`](./2026-04-28-shared-options-evidence-design.md) — `OptionsSnapshot` / `OptionsProvider` contract that Phase 2 extends with `all_expirations`.
- [`2026-04-20-fund-manager-dual-risk-escalation-design.md`](./2026-04-20-fund-manager-dual-risk-escalation-design.md) — dual-risk audit contract that Phase 2 leaves untouched (no new deterministic GEX trigger).
- [`2026-04-25-prompt-bundle-centralization-design.md`](./2026-04-25-prompt-bundle-centralization-design.md) — `PromptBundle` substitution path that Phase 2's leverage-warning injection hooks into.
- [`2026-05-16-transcript-local-cache-design.md`](./2026-05-16-transcript-local-cache-design.md) — prior cache design context; any ETF/N-PORT durability follow-up should build on it in a separate design pass.
- `CLAUDE.md` — `Pack-owned prompts (centralized)`, `TradingState schema evolution`, error handling pattern, warning-log discipline.

## Phase 3 scope (deferred)

Items explicitly out of scope for Phase 2 that might constitute a hypothetical Phase 3 — listed here only to signal that the boundary was considered, not committed:

1. Broad VEX / broad CEX (aggregated across all expirations, not just front-month).
2. Per-strike Vanna / Charm series alongside the existing `GexSummary.strikes` gamma-wall series.
3. Time-series dealer positioning (longitudinal GEX trend across daily snapshots).
4. Higher-order Greeks (Vomma, Speed, Color, Zomma, Veta).
5. N-PORT cache and any storage-layout consolidation follow-up.
