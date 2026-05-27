# ETF Baseline Phase 2 — Dealer Greeks + Prompt Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the dealer-positioning signal reserved by Phase 1 (`EtfValuation.options_gex`) through three staged units of work — Stage 1 (near-term GEX core math + state plumbing), Stage 2 (validation slice: live risk-free-rate sourcing from FRED `DGS3MO` with yfinance `^IRX` fallback, leverage-warning injection, technical-prompt rewrite, terminal rendering), and contingent Stage 3 (broad GEX + VEX/CEX surfacing). Stage 2 is the go/no-go gate for Stage 3.

**Architecture:** Pure BSM math (gamma, vanna, charm) in a new `crates/scorpio-core/src/indicators/gex.rs` module; aggregation uses the SqueezeMetrics dealer convention (short calls, long puts) to emit signed net + non-negative gross exposures. The ETF valuator (`valuation/etf/premium_discount.rs`) maps the math layer's `AggregateResult` directly into the additive `GexSummary` state shape — no separate wrapper module. Leverage-warning injection happens at prompt-assembly time inside `agents/risk/common.rs::render_risk_system_prompt` and `agents/auditor/prompt.rs::build_system_prompt` (renderer-side, not in the manifest). Stage 2 adds live/today risk-free-rate sourcing in preflight: fetch FRED `DGS3MO` first, fallback to the most recent yfinance `^IRX` close, and if both fail leave `etf_risk_free_rate = None` so downstream dealer-positioning derivation degrades to unavailable. Stage 3 adds a transient `OptionsSnapshot.all_expirations` field that is carried through the technical → analyst-sync handoff but stripped before durable serialization. All new state fields are additive with `#[serde(default)]`; no `THESIS_MEMORY_SCHEMA_VERSION` bump.

**Tech Stack:** Rust 2024 (rustc 1.93+, edition 2024); `tokio` async runtime; `serde` + `schemars` for state/snapshot serialization; `chrono` for date math; existing `FredClient`, `YFinanceClient`, `SecEdgarClient` providers; `graph-flow` for task orchestration; `rig-core` for agents; `tracing` for structured warning logs; `nextest` for test execution. Reference: [`docs/superpowers/specs/2026-05-22-etf-baseline-phase2-design.md`](../specs/2026-05-22-etf-baseline-phase2-design.md).

**Post-review execution constraints (authoritative):**

- **Stage 1 stops at near-term GEX + gamma walls.** Implement `StrikeGex` and `GexSummary.strikes` in Stage 1. Defer `BroadGex`, `VexSummary`, `CexSummary`, `ExpirationStrikes`, broad aggregation, and stored VEX/CEX summaries until Stage 3. If an embedded Stage 1 snippet below still shows those Stage 3 surfaces, move that code to Tasks 13-16 before implementation.
- **State-derived valuation inputs are assembled at the state-aware seam.** Keep `fetch_valuation_inputs` provider-only unless the plan explicitly changes its signature and all call sites. Stage 1 populates only `etf_options` and `as_of` where `crate::valuation::ValuationInputs` is constructed with access to `TradingState`; Stage 2 Tasks 18-20 add and populate `etf_risk_free_rate` before terminal validation.
- **Stage 2 uses one generic dealer-positioning absence branch.** Stage 2 does not distinguish no options snapshot from an unusable snapshot unless an explicit derivation status is added first. Prompt/reporter copy should say no usable dealer-positioning overlay was available. A split reason can be added in Stage 3 only with a real status field.
- **Prompt contracts must match available context.** The ETF technical prompt may discuss raw `options_context` / `options_summary` snapshot evidence. Do not ask it to cite `EtfValuation.options_gex` unless that derived payload is explicitly threaded into the prompt context. Valuation-aware downstream prompts may cite `options_gex` only after the context payload exists.
- **The Stage 2 gate must evaluate the full reader experience that Stage 2 actually changes.** Validate terminal output plus generated prose surfaces that receive Stage 2 data (raw options/leverage context). If derived `options_gex` is not threaded into prompts, evaluate derived-GEX value in the terminal block only. Stage 3 additions (broad GEX and VEX/CEX) require separate evidence or separate sub-gate approval; Stage 2 success does not automatically justify them.
- **Risk-free rate has no hardcoded fallback.** Delete any `RISK_FREE_RATE_FALLBACK` / `0.045` plan snippets outside pure test fixtures. Stage 2 must fetch FRED `DGS3MO` for live/today ETF runs, fallback to yfinance `^IRX` most recent close if FRED is unavailable, and leave `etf_risk_free_rate = None` if both fail. Dealer-positioning consumers must degrade to `options_gex: None` when a rate is unavailable; they must not substitute a constant.
- **FRED `DGS3MO` and yfinance `^IRX` are date-sensitive.** Stage 2 may fetch latest market rates only for live/today ETF runs. Historical runs must skip latest-rate fetches and degrade dealer-positioning, or add a true as-of-date rate query with persisted observation date before enabling historical GEX.

---

## File Structure

### Stage 1 — Near-term dealer positioning core

- **Create:** `crates/scorpio-core/src/indicators/gex.rs` — pure BSM math + chain aggregation
- **Modify:** `crates/scorpio-core/src/indicators/mod.rs` — declare new module, re-export public surface
- **Modify:** `crates/scorpio-core/src/state/derived.rs` — additive `StrikeGex` type and `GexSummary.strikes` field for Stage 1; defer `BroadGex`, `VexSummary`, `CexSummary`, and related fields until Stage 3
- **Modify:** `crates/scorpio-core/src/valuation/mod.rs` — `ValuationInputs` gains `etf_options` and `as_of` in Stage 1; `etf_risk_free_rate` is added in Stage 2 Task 20
- **Modify:** `crates/scorpio-core/src/valuation/etf/premium_discount.rs` — add `compute_gex_summary` helper, populate `EtfValuation.options_gex`, derive `q` from `EtfComposition.distribution_yield_ttm_pct`
- **Modify:** `crates/scorpio-core/src/workflow/tasks/analyst.rs` — state-aware valuation assembly reads `TechnicalOptionsContext::Available { outcome: Snapshot(_) }` into `ValuationInputs.etf_options`
- **Modify:** `crates/scorpio-core/tests/state_roundtrip.rs` — additive `GexSummary` field serde compat

### Stage 2 — Surfaced validation slice

- **Modify:** `crates/scorpio-core/src/analysis_packs/etf/baseline.rs` — update `append_leverage_warning_if_needed` to use the `---` divider and `1e-6` tolerance per spec; drop `#[allow(dead_code)]`
- **Modify:** `crates/scorpio-core/src/agents/risk/common.rs::render_risk_system_prompt` — inject leverage warning into Conservative + Neutral slots
- **Modify:** `crates/scorpio-core/src/agents/auditor/prompt.rs::build_system_prompt` — inject leverage warning into the auditor slot
- **Modify:** `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md` — replace Phase 1 placeholder with raw-options guidance that does not cite `EtfValuation.options_gex` unless a derived payload is threaded into context
- **Modify:** `crates/scorpio-reporters/src/terminal/etf.rs` — add `render_dealer_positioning_block` (near-term GEX core + summary line + gamma walls + partial-data note) and the risk-free-rate source banner (FRED `DGS3MO`, yfinance `^IRX`, or degraded notice)
- **Modify:** `crates/scorpio-reporters/tests/terminal.rs` — assertions for the new block and the rate-source banner
- **Modify:** `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` — golden-byte coverage for the rewritten focus prompt + leverage-warning suffix
- **Modify:** `crates/scorpio-core/src/state/trading_state.rs` — add `etf_risk_free_rate: Option<f64>`, `etf_risk_free_rate_source: Option<EtfRiskFreeRateSource>`, `EtfRiskFreeRateSource` enum; thread through `TradingStateWire` (Task 18, promoted from Stage 3 to Stage 2)
- **Modify:** `crates/scorpio-core/src/state/mod.rs` — re-export `EtfRiskFreeRateSource` (Task 18)
- **Modify:** `crates/scorpio-core/src/workflow/builder.rs` — thread `FredClient` + `YFinanceClient` into `PreflightTask` constructor (Task 19)
- **Modify:** `crates/scorpio-core/src/workflow/tasks/preflight.rs` — live/today `DGS3MO` fetch, yfinance `^IRX` close fallback, and no-rate degradation when both fail (Task 19)
- **Modify:** `crates/scorpio-core/src/valuation/mod.rs` — add `etf_risk_free_rate: Option<f64>` to `ValuationInputs` (Task 20)
- **Modify:** `crates/scorpio-core/src/valuation/etf/premium_discount.rs` — degrade `options_gex` to `None` when the carrier is unavailable; no hardcoded rate fallback (Task 20)
- **Modify:** `crates/scorpio-core/src/workflow/tasks/analyst.rs` — populate `valuation_inputs.etf_risk_free_rate` from state (Task 20)
- **Modify:** `crates/scorpio-core/tests/state_roundtrip.rs` — roundtrip the new `TradingState` rate-source fields (Task 18)
- **Modify:** `crates/scorpio-core/tests/workflow_pipeline_structure.rs` — verify FRED/yfinance rate-provider threading + live/today gating (Task 19)
- **Modify:** `crates/scorpio-core/examples/yfinance_live_test.rs` — `^IRX` latest-close assertion for the risk-free-rate fallback path (Task 23, optional smoke ahead of the gate)

### ⚠️ Validation Gate (manual user decision) ⚠️

### Stage 3 — Contingent context expansion (only if Stage 2 clears the gate)

- **Modify:** `crates/scorpio-core/src/data/traits/options.rs` — add `OptionsSnapshot.all_expirations: Vec<ExpirationStrikes>` with `#[serde(skip, default)]` and the `ExpirationStrikes` type
- **Modify:** `crates/scorpio-core/src/data/yfinance/options.rs` — populate `all_expirations` from the existing per-expiration iteration
- **Modify:** `crates/scorpio-core/src/indicators/gex.rs` — extend aggregator with broad GEX aggregation and VEX/CEX surfacing
- **Modify:** `crates/scorpio-core/src/state/derived.rs` — add `BroadGex`, `VexSummary`, `CexSummary` types and the `broad`/`vex_summary`/`cex_summary` fields on `GexSummary`
- **Modify:** `crates/scorpio-core/src/valuation/etf/premium_discount.rs::compute_gex_summary` — emit `broad`, `vex_summary`, `cex_summary`
- **Modify:** `crates/scorpio-core/src/workflow/tasks/analyst.rs` — strip `all_expirations` before `serialize_state_to_context`
- **Modify:** `crates/scorpio-reporters/src/terminal/etf.rs` — extend `render_dealer_positioning_block` with the Secondary sensitivities block and `All expirations` / `Partial expirations` sub-block
- **Create:** `crates/scorpio-core/examples/yfinance_options_chain_live_test.rs`
- **Create:** `crates/scorpio-core/examples/etf_options_gex_live_test.rs`
- **Modify:** `crates/scorpio-core/examples/fred_live_test.rs` — DGS3MO assertion
- **Modify:** `crates/scorpio-core/tests/state_roundtrip.rs` — populated Stage 3 `GexSummary` fields roundtrip

---

## Stage 1 — Near-term dealer positioning core

### Task 1: BSM math helpers (gamma, vanna, charm)

**Files:**
- Create: `crates/scorpio-core/src/indicators/gex.rs`
- Modify: `crates/scorpio-core/src/indicators/mod.rs`

- [ ] **Step 1.1: Declare the module**

Open `crates/scorpio-core/src/indicators/mod.rs` and add the `pub(crate) mod gex;` declaration alongside the existing private modules so the ETF valuator can import `crate::indicators::gex`. After editing, the module list at the top of the file should read:

```rust
mod batch;
mod core_math;
pub(crate) mod gex;
mod support_resistance;
mod tools;
mod types;
mod utils;
```

- [ ] **Step 1.2: Write failing unit tests for the BSM helpers**

Create `crates/scorpio-core/src/indicators/gex.rs` with **only the test module first** so the next step fails for a missing implementation. Use this exact content:

```rust
//! BSM Greeks (gamma, vanna, charm) and chain-level aggregation for ETF
//! dealer-positioning analysis. Pure functions only — no I/O, no `unsafe`,
//! no panics. Degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`.

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_inputs() -> BsmInputs {
        BsmInputs {
            spot: 100.0,
            strike: 100.0,
            iv: 0.20,
            r: 0.045,
            q: 0.015,
            t_years: 30.0 / 365.0,
        }
    }

    #[test]
    fn bsm_gamma_matches_analytical_reference() {
        // Analytical Γ at the inputs above (computed offline) ≈ 0.06931.
        let g = bsm_gamma(ref_inputs());
        assert!(
            (g - 0.069_313).abs() < 1e-5,
            "gamma drift: got {g}"
        );
    }

    #[test]
    fn bsm_gamma_returns_zero_for_degenerate_inputs() {
        let mut i = ref_inputs();
        i.iv = 0.0;
        assert_eq!(bsm_gamma(i.clone()), 0.0);
        i = ref_inputs();
        i.t_years = 0.0;
        assert_eq!(bsm_gamma(i.clone()), 0.0);
        i = ref_inputs();
        i.spot = 0.0;
        assert_eq!(bsm_gamma(i), 0.0);
    }

    #[test]
    fn bsm_gamma_at_the_money_exceeds_out_of_the_money() {
        let atm = bsm_gamma(ref_inputs());
        let otm = bsm_gamma(BsmInputs {
            strike: 120.0,
            ..ref_inputs()
        });
        assert!(atm > otm, "ATM gamma must exceed OTM gamma: atm={atm} otm={otm}");
    }

    #[test]
    fn bsm_vanna_call_and_put_share_value() {
        // Vanna is identical for calls and puts (no put_call sign flip in the
        // closed-form result).
        let v = bsm_vanna(ref_inputs());
        assert!(v.is_finite(), "vanna must be finite: {v}");
        // Sanity: vanna of the ATM forward call is negative when r < q is false
        // but our `q=0.015 < r=0.045` keeps it in standard territory; just
        // verify it stays bounded.
        assert!(v.abs() < 1.0, "|vanna| out of range: {v}");
    }

    #[test]
    fn bsm_charm_call_put_parity_gap_matches_dividend_yield() {
        let call = bsm_charm_call(ref_inputs());
        let put = bsm_charm_put(ref_inputs());
        assert!(call.is_finite() && put.is_finite());
        // Charm parity: call charm - put charm = q·e^{-q·t}.
        let expected_gap = ref_inputs().q * (-ref_inputs().q * ref_inputs().t_years).exp();
        assert!(
            ((call - put) - expected_gap).abs() < 1e-9,
            "unexpected charm parity gap: call={call} put={put} expected_gap={expected_gap}"
        );
    }

    #[test]
    fn bsm_vanna_returns_zero_for_degenerate_inputs() {
        let mut i = ref_inputs();
        i.iv = 0.0;
        assert_eq!(bsm_vanna(i), 0.0);
    }

    #[test]
    fn bsm_charm_returns_zero_for_degenerate_inputs() {
        let mut i = ref_inputs();
        i.t_years = 0.0;
        assert_eq!(bsm_charm_call(i.clone()), 0.0);
        assert_eq!(bsm_charm_put(i), 0.0);
    }
}
```

- [ ] **Step 1.3: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core indicators::gex::tests`
Expected: COMPILE FAILURE (`BsmInputs`, `bsm_gamma`, etc. not defined).

- [ ] **Step 1.4: Implement the BSM helpers**

Replace the test-only stub of `crates/scorpio-core/src/indicators/gex.rs` with the production implementation **plus the existing test module**. Use this content (preserve the `#[cfg(test)] mod tests { ... }` from Step 1.2 verbatim — do not rewrite the tests):

```rust
//! BSM Greeks (gamma, vanna, charm) and chain-level aggregation for ETF
//! dealer-positioning analysis. Pure functions only — no I/O, no `unsafe`,
//! no panics. Degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`.

use statrs::distribution::{Continuous, ContinuousCDF, Normal};

/// Common BSM input bundle. All values are positive decimals; `t_years` is
/// the time-to-expiration in calendar years (e.g. 30/365 for a 30-day option).
#[derive(Debug, Clone, Copy)]
pub struct BsmInputs {
    pub spot: f64,
    pub strike: f64,
    pub iv: f64,
    pub r: f64,
    pub q: f64,
    pub t_years: f64,
}

fn standard_normal() -> Normal {
    // mean=0, std_dev=1 — never returns Err for these arguments.
    Normal::new(0.0, 1.0).expect("standard normal must construct")
}

fn d1_d2(inputs: &BsmInputs) -> Option<(f64, f64)> {
    if inputs.iv <= 0.0 || inputs.t_years <= 0.0 || inputs.spot <= 0.0 || inputs.strike <= 0.0 {
        return None;
    }
    let sigma_sqrt_t = inputs.iv * inputs.t_years.sqrt();
    let d1 = ((inputs.spot / inputs.strike).ln()
        + (inputs.r - inputs.q + 0.5 * inputs.iv * inputs.iv) * inputs.t_years)
        / sigma_sqrt_t;
    let d2 = d1 - sigma_sqrt_t;
    Some((d1, d2))
}

/// Black-Scholes-Merton gamma with continuous dividend yield.
///
/// Γ = e^{-q·t} · φ(d1) / (S · σ · √t)
pub fn bsm_gamma(inputs: BsmInputs) -> f64 {
    let Some((d1, _d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let phi_d1 = standard_normal().pdf(d1);
    (-inputs.q * inputs.t_years).exp() * phi_d1
        / (inputs.spot * inputs.iv * inputs.t_years.sqrt())
}

/// Black-Scholes-Merton vanna (call and put have the same vanna).
///
/// Vanna = -e^{-q·t} · φ(d1) · d2 / σ
pub fn bsm_vanna(inputs: BsmInputs) -> f64 {
    let Some((d1, d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let phi_d1 = standard_normal().pdf(d1);
    -(-inputs.q * inputs.t_years).exp() * phi_d1 * d2 / inputs.iv
}

/// Black-Scholes-Merton call charm (∂Δ_call / ∂t, per year).
///
/// Charm_call = q·e^{-q·t}·N(d1)
///            - e^{-q·t}·φ(d1)·[2(r-q)·t - d2·σ·√t] / (2·t·σ·√t)
pub fn bsm_charm_call(inputs: BsmInputs) -> f64 {
    let Some((d1, d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let n = standard_normal();
    let phi_d1 = n.pdf(d1);
    let big_n_d1 = n.cdf(d1);
    let e_qt = (-inputs.q * inputs.t_years).exp();
    let sigma_sqrt_t = inputs.iv * inputs.t_years.sqrt();
    let bracket = 2.0 * (inputs.r - inputs.q) * inputs.t_years - d2 * sigma_sqrt_t;
    let denom = 2.0 * inputs.t_years * sigma_sqrt_t;
    inputs.q * e_qt * big_n_d1 - e_qt * phi_d1 * bracket / denom
}

/// Black-Scholes-Merton put charm.
///
/// Charm_put = -q·e^{-q·t}·N(-d1)
///           - e^{-q·t}·φ(d1)·[2(r-q)·t - d2·σ·√t] / (2·t·σ·√t)
pub fn bsm_charm_put(inputs: BsmInputs) -> f64 {
    let Some((d1, d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let n = standard_normal();
    let phi_d1 = n.pdf(d1);
    let big_n_neg_d1 = n.cdf(-d1);
    let e_qt = (-inputs.q * inputs.t_years).exp();
    let sigma_sqrt_t = inputs.iv * inputs.t_years.sqrt();
    let bracket = 2.0 * (inputs.r - inputs.q) * inputs.t_years - d2 * sigma_sqrt_t;
    let denom = 2.0 * inputs.t_years * sigma_sqrt_t;
    -inputs.q * e_qt * big_n_neg_d1 - e_qt * phi_d1 * bracket / denom
}

#[cfg(test)]
mod tests {
    // ... preserve the test module written in Step 1.2 verbatim ...
}
```

Before saving, verify `statrs` is already a workspace dependency. Run:

```bash
grep -n 'statrs' /Users/bigtochan/Documents/dev/BigtoC/scorpio-analyst/Cargo.toml /Users/bigtochan/Documents/dev/BigtoC/scorpio-analyst/crates/scorpio-core/Cargo.toml
```

If `statrs` is not already present on `scorpio-core`, add it. The crate is small and pure-Rust:

```toml
# crates/scorpio-core/Cargo.toml — under [dependencies]
statrs = "0.18"
```

(Use whichever minor version the workspace already pins if `statrs` is already a transitive dep; otherwise `0.18` is the most recent stable line.)

- [ ] **Step 1.5: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core indicators::gex::tests`
Expected: 7 tests pass.

- [ ] **Step 1.6: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 1.7: Commit**

```bash
git add crates/scorpio-core/src/indicators/gex.rs crates/scorpio-core/src/indicators/mod.rs crates/scorpio-core/Cargo.toml
git commit -m "feat(indicators): add BSM gamma/vanna/charm helpers

Pure functions with degenerate-input zero-return semantics. Used by
ETF Phase 2 dealer-positioning aggregation."
```

---

### Task 2: Per-strike chain aggregation (near-term, SqueezeMetrics sign)

**Files:**
- Modify: `crates/scorpio-core/src/indicators/gex.rs`

> **Post-review override:** Task 2 is Stage 1 and must implement near-term GEX only. If the snippets below include `ExpirationStrikes`, broad aggregation, VEX, or CEX fields, defer those parts to Stage 3 Tasks 13-16 before coding.

- [ ] **Step 2.1: Write failing aggregator tests**

Append the following test cases inside the existing `#[cfg(test)] mod tests { ... }` block at the bottom of `crates/scorpio-core/src/indicators/gex.rs`:

```rust
    use crate::data::traits::options::{IvTermPoint, NearTermStrike, OptionsSnapshot};

    fn snap(near_term_strikes: Vec<NearTermStrike>) -> OptionsSnapshot {
        OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes,
        }
    }

    fn row(strike: f64, call_oi: u64, put_oi: u64) -> NearTermStrike {
        NearTermStrike {
            strike,
            call_iv: Some(0.20),
            put_iv: Some(0.20),
            call_volume: None,
            put_volume: None,
            call_oi: Some(call_oi),
            put_oi: Some(put_oi),
        }
    }

    #[test]
    fn aggregate_returns_none_when_no_strikes() {
        let s = snap(vec![]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            atm_iv_fallback: s.atm_iv,
        });
        assert!(res.near_term.is_none());
        assert_eq!(res.strikes_total, 0);
        assert_eq!(res.strikes_used, 0);
    }

    #[test]
    fn aggregate_signs_dealer_short_calls_long_puts() {
        // Only call OI present at strike — dealers short calls, so net GEX is positive.
        let s = snap(vec![row(100.0, 1_000, 0)]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            atm_iv_fallback: s.atm_iv,
        });
        let near = res.near_term.expect("near-term aggregate must be present");
        assert!(near.net_gex_usd_per_1pct_move > 0.0, "call-only OI must produce positive net GEX");
        assert!(near.gross_gex_usd_per_1pct_move >= near.net_gex_usd_per_1pct_move);

        // Only put OI — net flips negative; gross stays magnitude.
        let s2 = snap(vec![row(100.0, 0, 1_000)]);
        let res2 = aggregate(AggregateInputs {
            spot: s2.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s2.near_term_expiration,
            near_term_strikes: &s2.near_term_strikes,
            atm_iv_fallback: s2.atm_iv,
        });
        let near2 = res2.near_term.expect("put-only aggregate present");
        assert!(near2.net_gex_usd_per_1pct_move < 0.0, "put-only OI must produce negative net GEX");
    }

    #[test]
    fn aggregate_iv_fallback_counter_increments_when_strike_iv_missing() {
        let row_no_iv = NearTermStrike {
            strike: 100.0,
            call_iv: None,
            put_iv: None,
            call_volume: None,
            put_volume: None,
            call_oi: Some(500),
            put_oi: Some(500),
        };
        let s = snap(vec![row_no_iv]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            atm_iv_fallback: s.atm_iv,
        });
        // One call leg + one put leg = two fallbacks for the single row.
        assert_eq!(res.iv_fallback_count, 2);
        assert_eq!(res.strikes_used, 1);
    }

    #[test]
    fn aggregate_skips_row_when_no_iv_anywhere() {
        let bad_row = NearTermStrike {
            strike: 100.0,
            call_iv: None,
            put_iv: None,
            call_volume: None,
            put_volume: None,
            call_oi: Some(500),
            put_oi: Some(500),
        };
        let mut s = snap(vec![bad_row]);
        s.atm_iv = 0.0; // no fallback either
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            atm_iv_fallback: s.atm_iv,
        });
        assert!(res.near_term.is_none());
        assert_eq!(res.strikes_total, 1);
        assert_eq!(res.strikes_used, 0);
    }

    #[test]
    fn aggregate_returns_none_when_expiration_is_today_or_past() {
        let s = snap(vec![row(100.0, 1_000, 1_000)]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            // expiration parses to 2026-06-26 — `as_of` set after → t_years <= 0.
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            atm_iv_fallback: s.atm_iv,
        });
        assert!(res.near_term.is_none(), "same-day expiration must yield None");
    }
```

- [ ] **Step 2.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core indicators::gex::tests`
Expected: COMPILE FAILURE (`aggregate`, `AggregateInputs`, `AggregateResult` not defined).

- [ ] **Step 2.3: Implement the aggregator**

In `crates/scorpio-core/src/indicators/gex.rs`, **above** the `#[cfg(test)]` block, add:

```rust
use crate::data::traits::options::NearTermStrike;

/// Per-strike aggregated GEX exposure (post-OI, post-sign-convention,
/// post-USD-scaling). Only net GEX is emitted per strike — VEX/CEX per-strike
/// rows are explicitly out of scope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerStrikeAggregate {
    pub strike: f64,
    pub net_gex_usd_per_1pct_move: f64,
}

/// Input bundle for near-term chain-level aggregation.
pub struct AggregateInputs<'a> {
    pub spot: f64,
    pub r: f64,
    pub q: f64,
    pub as_of: chrono::NaiveDate,
    pub near_term_expiration: &'a str,
    pub near_term_strikes: &'a [NearTermStrike],
    pub atm_iv_fallback: f64,
}

/// Result bundle covering the near-term front-month aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateResult {
    pub near_term: Option<NearTermAggregate>,
    pub iv_fallback_count: u32,
    pub strikes_total: u32,
    pub strikes_used: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NearTermAggregate {
    pub expiration: chrono::NaiveDate,
    pub per_strike: Vec<PerStrikeAggregate>,
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
}

const CONTRACT_MULTIPLIER: f64 = 100.0;

struct StrikeContribution {
    net_gex: f64,
    gross_gex: f64,
}

fn build_inputs(
    spot: f64,
    strike: f64,
    iv: f64,
    r: f64,
    q: f64,
    t_years: f64,
) -> BsmInputs {
    BsmInputs { spot, strike, iv, r, q, t_years }
}

/// Compute a single strike's signed + magnitude GEX contributions. Returns
/// `None` when the row has no usable IV on either leg and `atm_iv_fallback <= 0.0`
/// — caller then increments `strikes_total` but not `strikes_used`.
fn contribution_for_strike(
    spot: f64,
    r: f64,
    q: f64,
    t_years: f64,
    atm_iv_fallback: f64,
    row: &NearTermStrike,
    iv_fallback_count: &mut u32,
) -> Option<StrikeContribution> {
    let call_iv = row.call_iv.unwrap_or_else(|| {
        *iv_fallback_count = iv_fallback_count.saturating_add(1);
        atm_iv_fallback
    });
    let put_iv = row.put_iv.unwrap_or_else(|| {
        *iv_fallback_count = iv_fallback_count.saturating_add(1);
        atm_iv_fallback
    });
    if call_iv <= 0.0 && put_iv <= 0.0 {
        return None;
    }

    let call_in = build_inputs(spot, row.strike, call_iv, r, q, t_years);
    let put_in = build_inputs(spot, row.strike, put_iv, r, q, t_years);

    let call_oi = row.call_oi.unwrap_or(0) as f64;
    let put_oi = row.put_oi.unwrap_or(0) as f64;

    let gamma_call = bsm_gamma(call_in);
    let gamma_put = bsm_gamma(put_in);

    let spot_sq_pct = spot * spot * 0.01;

    // GEX (gamma always ≥ 0; sign comes entirely from the call/put OI weighting)
    let net_gex = (gamma_call * call_oi - gamma_put * put_oi) * CONTRACT_MULTIPLIER * spot_sq_pct;
    let gross_gex =
        (gamma_call * call_oi + gamma_put * put_oi) * CONTRACT_MULTIPLIER * spot_sq_pct;

    Some(StrikeContribution {
        net_gex,
        gross_gex,
    })
}

fn parse_expiration(expiration: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(expiration, "%Y-%m-%d").ok()
}

fn years_until(expiration: chrono::NaiveDate, as_of: chrono::NaiveDate) -> f64 {
    let days = (expiration - as_of).num_days();
    if days <= 0 {
        0.0
    } else {
        days as f64 / 365.0
    }
}

/// Aggregate per-strike GEX contributions across the near-term chain.
pub fn aggregate(inputs: AggregateInputs<'_>) -> AggregateResult {
    let mut iv_fallback_count: u32 = 0;
    let mut strikes_total: u32 = 0;
    let mut strikes_used: u32 = 0;

    let near_term = match parse_expiration(inputs.near_term_expiration) {
        Some(exp) => {
            let t_years = years_until(exp, inputs.as_of);
            if t_years <= 0.0 || inputs.near_term_strikes.is_empty() {
                None
            } else {
                let mut per_strike: Vec<PerStrikeAggregate> = Vec::new();
                let mut net_gex = 0.0;
                let mut gross_gex = 0.0;

                for row in inputs.near_term_strikes {
                    strikes_total = strikes_total.saturating_add(1);
                    let Some(c) = contribution_for_strike(
                        inputs.spot,
                        inputs.r,
                        inputs.q,
                        t_years,
                        inputs.atm_iv_fallback,
                        row,
                        &mut iv_fallback_count,
                    ) else {
                        continue;
                    };
                    strikes_used = strikes_used.saturating_add(1);
                    per_strike.push(PerStrikeAggregate {
                        strike: row.strike,
                        net_gex_usd_per_1pct_move: c.net_gex,
                    });
                    net_gex += c.net_gex;
                    gross_gex += c.gross_gex;
                }

                if strikes_used == 0 {
                    None
                } else {
                    Some(NearTermAggregate {
                        expiration: exp,
                        per_strike,
                        net_gex_usd_per_1pct_move: net_gex,
                        gross_gex_usd_per_1pct_move: gross_gex,
                    })
                }
            }
        }
        None => None,
    };

    AggregateResult {
        near_term,
        iv_fallback_count,
        strikes_total,
        strikes_used,
    }
}
```

- [ ] **Step 2.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core indicators::gex::tests`
Expected: 12 tests pass (7 from Task 1 + 5 new).

- [ ] **Step 2.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 2.6: Commit**

```bash
git add crates/scorpio-core/src/indicators/gex.rs
git commit -m "feat(indicators): aggregate per-strike GEX with SqueezeMetrics signs

Near-term aggregation reads OptionsSnapshot.near_term_strikes, uses
atm_iv when per-strike IVs are missing, and skips rows when neither call_iv,
put_iv, nor atm_iv is available. Broad aggregation and VEX/CEX surfacing are
deferred to Stage 3."
```

---

### Task 3: Additive state schema — StrikeGex + GexSummary fields

**Files:**
- Modify: `crates/scorpio-core/src/state/derived.rs`

> **Post-review override:** Task 3 should add only `StrikeGex` and `GexSummary.strikes` in Stage 1. Move `BroadGex`, `VexSummary`, `CexSummary`, and their `GexSummary` fields to Stage 3 before implementation.

- [ ] **Step 3.1: Write failing serde round-trip tests**

Append to the existing `#[cfg(test)] mod tests { ... }` block at the bottom of `crates/scorpio-core/src/state/derived.rs`:

```rust
    #[test]
    fn gex_summary_with_strikes_field_roundtrips_json() {
        let val = GexSummary {
            net_gex_usd_per_1pct_move: 1_000_000.0,
            gross_gex_usd_per_1pct_move: 2_000_000.0,
            call_put_oi_ratio: 1.3,
            max_pain_strike: 100.0,
            near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            strikes: vec![
                StrikeGex { strike: 100.0, net_gex_usd_per_1pct_move: 500_000.0 },
                StrikeGex { strike: 105.0, net_gex_usd_per_1pct_move: -250_000.0 },
            ],
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: GexSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn legacy_phase1_gex_summary_without_strikes_still_deserializes() {
        // Phase 1 GexSummary literally never serialized — `options_gex` was
        // always None — but the serde contract for newly added fields must
        // still default cleanly. Verify with a minimal JSON payload missing
        // the Stage 1 field.
        let json = r#"{
            "net_gex_usd_per_1pct_move": 0.0,
            "gross_gex_usd_per_1pct_move": 0.0,
            "call_put_oi_ratio": 0.0,
            "max_pain_strike": 0.0,
            "near_term_expiration": "2026-06-26"
        }"#;
        let back: GexSummary = serde_json::from_str(json).expect("deserialize");
        assert!(back.strikes.is_empty());
    }
```

- [ ] **Step 3.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core state::derived::tests`
Expected: COMPILE FAILURE (`StrikeGex` type not defined; `GexSummary` missing the `strikes` field).

- [ ] **Step 3.3: Add the new types and extend `GexSummary`**

In `crates/scorpio-core/src/state/derived.rs`, locate the existing `GexSummary` definition (currently around line 290) and replace it with the additive version. Also add the new `StrikeGex` sibling struct **immediately after** the updated `GexSummary`. Use this exact content:

```rust
/// Dealer-positioning summary populated by `compute_gex_summary` from a live
/// `OptionsSnapshot`. Phase 1 always emitted `options_gex: None`; Phase 2
/// Stage 1/2 populates the legacy fields plus `strikes`. Stage 3 additionally
/// adds broad GEX and secondary VEX/CEX summaries. The added `strikes` field
/// carries `#[serde(default)]` so legacy snapshots remain readable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GexSummary {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub call_put_oi_ratio: f64,
    pub max_pain_strike: f64,
    pub near_term_expiration: chrono::NaiveDate,

    /// Top-N strikes by `|net_gex_usd_per_1pct_move|` — gamma walls.
    /// Populated by Stage 1/2.
    #[serde(default)]
    pub strikes: Vec<StrikeGex>,
}

/// Single gamma-wall row inside `GexSummary.strikes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StrikeGex {
    pub strike: f64,
    pub net_gex_usd_per_1pct_move: f64,
}
```

- [ ] **Step 3.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core state::derived::tests`
Expected: previously-passing tests still pass; 2 new tests pass.

- [ ] **Step 3.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 3.6: Commit**

```bash
git add crates/scorpio-core/src/state/derived.rs
git commit -m "feat(state): extend GexSummary with gamma-wall strikes

The new strikes field carries #[serde(default)] so legacy Phase 1 snapshots
(which always serialized options_gex: None) deserialize unchanged. Broad GEX
and VEX/CEX summaries are deferred to Stage 3."
```

---

### Task 4: ValuationInputs carrier + `compute_gex_summary` mapping

**Files:**
- Modify: `crates/scorpio-core/src/valuation/mod.rs`
- Modify: `crates/scorpio-core/src/valuation/etf/premium_discount.rs`

- [ ] **Step 4.1: Write a failing valuator integration test**

Append to `crates/scorpio-core/src/valuation/etf/premium_discount.rs` inside its existing `#[cfg(test)] mod tests { ... }` block:

```rust
    use crate::data::traits::options::{IvTermPoint, NearTermStrike, OptionsSnapshot};

    fn sample_options_snapshot() -> OptionsSnapshot {
        OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 0.8, // call-heavy → call_put_oi_ratio = 1.25
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes: vec![
                NearTermStrike {
                    strike: 95.0,
                    call_iv: Some(0.22),
                    put_iv: Some(0.24),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(1_500),
                    put_oi: Some(500),
                },
                NearTermStrike {
                    strike: 100.0,
                    call_iv: Some(0.20),
                    put_iv: Some(0.20),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(3_000),
                    put_oi: Some(2_500),
                },
                NearTermStrike {
                    strike: 105.0,
                    call_iv: Some(0.21),
                    put_iv: Some(0.23),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(800),
                    put_oi: Some(2_000),
                },
                NearTermStrike {
                    strike: 110.0,
                    call_iv: Some(0.25),
                    put_iv: Some(0.27),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(200),
                    put_oi: Some(1_200),
                },
            ],
        }
    }

    #[test]
    fn compute_gex_summary_returns_none_when_expiration_is_unparseable() {
        let mut snap = sample_options_snapshot();
        snap.near_term_expiration = "not-a-date".to_owned();
        let result = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn compute_gex_summary_emits_top_3_strikes_sorted_by_abs_net_gex() {
        let snap = sample_options_snapshot();
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary present");
        assert_eq!(summary.strikes.len(), 3, "must truncate to top-3");
        // Strict ordering: |w0.net| >= |w1.net| >= |w2.net|.
        let w = &summary.strikes;
        assert!(
            w[0].net_gex_usd_per_1pct_move.abs() >= w[1].net_gex_usd_per_1pct_move.abs(),
            "strikes[0] must dominate strikes[1]: {w:?}"
        );
        assert!(
            w[1].net_gex_usd_per_1pct_move.abs() >= w[2].net_gex_usd_per_1pct_move.abs(),
            "strikes[1] must dominate strikes[2]: {w:?}"
        );
    }

    #[test]
    fn compute_gex_summary_inverts_put_call_oi_ratio_correctly() {
        let snap = sample_options_snapshot();
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary present");
        // OptionsSnapshot stores put/call; GexSummary surfaces call/put.
        // 1 / 0.8 = 1.25.
        assert!(
            (summary.call_put_oi_ratio - 1.25).abs() < 1e-9,
            "expected 1.25, got {}",
            summary.call_put_oi_ratio
        );
    }

    #[test]
    fn compute_gex_summary_returns_zero_call_put_when_put_oi_ratio_is_zero() {
        let mut snap = sample_options_snapshot();
        snap.put_call_oi_ratio = 0.0;
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary present");
        assert_eq!(summary.call_put_oi_ratio, 0.0);
    }
```

- [ ] **Step 4.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core valuation::etf::premium_discount::tests`
Expected: COMPILE FAILURE (`compute_gex_summary` not defined).

- [ ] **Step 4.3: Extend `ValuationInputs`**

In `crates/scorpio-core/src/valuation/mod.rs`, append two Stage 1 fields to the `ValuationInputs` struct (preserving existing fields and their doc comments):
```rust
pub struct ValuationInputs<'a> {
    pub profile: Option<yfinance_rs::profile::Profile>,
    pub cashflow: Option<&'a [yfinance_rs::fundamentals::CashflowRow]>,
    pub balance: Option<&'a [yfinance_rs::fundamentals::BalanceSheetRow]>,
    pub income: Option<&'a [yfinance_rs::fundamentals::IncomeStatementRow]>,
    pub shares: Option<&'a [yfinance_rs::fundamentals::ShareCount]>,
    pub earnings_trend: Option<&'a [yfinance_rs::analysis::EarningsTrendRow]>,
    pub current_price: Option<f64>,

    // ETF inputs (None when active pack != EtfBaseline)
    pub etf_quote: Option<&'a crate::data::yfinance::etf::EtfQuote>,
    pub etf_fund_info: Option<&'a crate::data::yfinance::etf::FundInfo>,
    pub etf_holdings: Option<&'a crate::data::sec_edgar_nport::NPortHoldings>,
    pub etf_ohlcv: Option<&'a [crate::data::yfinance::Candle]>,
    pub etf_benchmark_ohlcv: Option<&'a [crate::data::yfinance::Candle]>,

    /// Phase 2 — Live ETF options snapshot threaded through from the persisted
    /// `TechnicalOptionsContext` before valuation runs. `None` when no snapshot
    /// is available or active pack is not `EtfBaseline`.
    pub etf_options: Option<&'a crate::data::traits::options::OptionsSnapshot>,

    /// Phase 2 — Reference date for time-to-expiration math, sourced from
    /// `state.target_date`. Defaulted to `chrono::Utc::now().date_naive()` by
    /// the equity path which does not read it.
    pub as_of: chrono::NaiveDate,
}
```

Then update every existing call site that constructs `ValuationInputs` to populate the new fields:

```bash
grep -rn 'ValuationInputs {' crates/scorpio-core/src/ crates/scorpio-core/tests/
```

For each constructor, set the new fields to:
- `etf_options: None`
- `as_of: chrono::Utc::now().date_naive()` (or a fixed test date for tests)

In production code, `as_of` should come from `state.target_date` parsed via `chrono::NaiveDate::parse_from_str(&state.target_date, "%Y-%m-%d")` with a fallback to `chrono::Utc::now().date_naive()` on parse failure. Apply that pattern only at the state-aware `crate::valuation::ValuationInputs` construction site in `workflow/tasks/analyst.rs` (Task 5 covers the touch).

- [ ] **Step 4.4: Implement `compute_gex_summary`**

In `crates/scorpio-core/src/valuation/etf/premium_discount.rs`, **above the existing test module**, add:

```rust
use crate::data::traits::options::OptionsSnapshot;
use crate::indicators::gex::{self, AggregateInputs};
use crate::state::{GexSummary, StrikeGex};

const MAX_GAMMA_WALLS: usize = 3;

/// Map a live options snapshot into the persistent `GexSummary` shape.
/// Returns `None` when the front-month near-term aggregate is unusable.
///
/// Inputs:
/// - `snapshot`: live front-month chain
/// - `r`: decimal risk-free rate. Tests pass fixed values; production callers
///   must pass a live rate from FRED `DGS3MO` or yfinance `^IRX`.
/// - `q`: decimal dividend yield (caller derives from
///   `EtfComposition.distribution_yield_ttm_pct / 100` when positive)
/// - `as_of`: reference date for time-to-expiration math
pub fn compute_gex_summary(
    snapshot: &OptionsSnapshot,
    r: f64,
    q: f64,
    as_of: chrono::NaiveDate,
) -> Option<GexSummary> {
    let agg = gex::aggregate(AggregateInputs {
        spot: snapshot.spot_price,
        r,
        q,
        as_of,
        near_term_expiration: &snapshot.near_term_expiration,
        near_term_strikes: &snapshot.near_term_strikes,
        atm_iv_fallback: snapshot.atm_iv,
    });

    let near = agg.near_term?;

    if agg.iv_fallback_count > agg.strikes_used.saturating_div(2) {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            iv_fallback_count = agg.iv_fallback_count,
            strikes_used = agg.strikes_used,
            "GEX computed with majority ATM-IV fallbacks — gamma skew may be understated"
        );
    }

    let mut walls: Vec<StrikeGex> = near
        .per_strike
        .iter()
        .map(|p| StrikeGex {
            strike: p.strike,
            net_gex_usd_per_1pct_move: p.net_gex_usd_per_1pct_move,
        })
        .collect();
    walls.sort_by(|a, b| {
        b.net_gex_usd_per_1pct_move
            .abs()
            .partial_cmp(&a.net_gex_usd_per_1pct_move.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    walls.truncate(MAX_GAMMA_WALLS);

    let call_put_oi_ratio = if snapshot.put_call_oi_ratio > 0.0 {
        1.0 / snapshot.put_call_oi_ratio
    } else {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            "put_call_oi_ratio is zero — call_put_oi_ratio set to 0.0"
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
    })
}
```

- [ ] **Step 4.5: Wire `compute_gex_summary` into the valuator**

Replace the placeholder `options_gex: None` at the existing line in
`crates/scorpio-core/src/valuation/etf/premium_discount.rs` (currently
around line 75 inside the `assess` method) with a real call. Locate this
section:

```rust
        let category = inputs.etf_fund_info.and_then(|f| f.category.clone());
        let leverage_factor = inputs.etf_fund_info.and_then(|f| f.leverage_factor);

        DerivedValuation {
            asset_shape: shape.clone(),
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: snapshot,
                composition,
                tracking,
                options_gex: None,
                category,
                leverage_factor,
                flags,
            }),
        }
```

Replace with:

```rust
        let category = inputs.etf_fund_info.and_then(|f| f.category.clone());
        let leverage_factor = inputs.etf_fund_info.and_then(|f| f.leverage_factor);

        // Phase 2 dealer-positioning will be computed in Stage 3 after live
        // risk-free-rate sourcing is available. No hardcoded rate fallback is
        // allowed, so Stage 1/2 keeps the derived overlay absent.
        let r: Option<f64> = None;
        let q = composition
            .as_ref()
            .and_then(|c| c.distribution_yield_ttm_pct)
            .filter(|y| *y > 0.0)
            .map(|y_pct| y_pct / 100.0)
            .unwrap_or(0.0);
        flags.options_chain_present = inputs.etf_options.is_some();
        let options_gex = match (inputs.etf_options, r) {
            (Some(snap), Some(rate)) => compute_gex_summary(snap, rate, q, inputs.as_of),
            (Some(_), None) => {
                tracing::warn!(
                    target: "scorpio_core::valuation::etf::gex",
                    "ETF dealer-positioning skipped — risk-free rate unavailable"
                );
                None
            }
            (None, _) => None,
        };

        DerivedValuation {
            asset_shape: shape.clone(),
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: snapshot,
                composition,
                tracking,
                options_gex,
                category,
                leverage_factor,
                flags,
            }),
        }
```

- [ ] **Step 4.6: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core valuation::etf::premium_discount::tests`
Expected: previously-passing tests still pass; 4 new tests pass.

- [ ] **Step 4.7: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean. If clippy fires on call sites that construct `ValuationInputs`, fix each one to populate `etf_options` and `as_of`.

- [ ] **Step 4.8: Commit**

```bash
git add crates/scorpio-core/src/valuation/mod.rs crates/scorpio-core/src/valuation/etf/premium_discount.rs
git commit -m "feat(valuation): populate EtfValuation.options_gex from live snapshot

Inverts put/call OI to the canonical call/put ratio, sorts gamma walls
by |net_gex|, truncates to top-3, and degrades to options_gex: None until
Stage 2 supplies a live risk-free rate."
```

---

### Task 5: AnalystSyncTask hydration of `etf_options`

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`

- [ ] **Step 5.1: Write a failing hydration test**

Append to the existing `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/workflow/tasks/analyst.rs` (locate the file's existing test module and add at the end):

```rust
    #[test]
    fn etf_valuation_inputs_carry_options_when_technical_context_has_snapshot() {
        use crate::data::traits::options::{
            IvTermPoint, NearTermStrike, OptionsOutcome, OptionsSnapshot,
        };
        use crate::state::{TechnicalData, TechnicalOptionsContext};

        let snap = OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes: vec![NearTermStrike {
                strike: 100.0,
                call_iv: Some(0.20),
                put_iv: Some(0.20),
                call_volume: None,
                put_volume: None,
                call_oi: Some(1_000),
                put_oi: Some(1_000),
            }],
        };

        let mut state = crate::testing::with_baseline_runtime_policy_state(
            "SPY".to_owned(),
            "2026-05-27".to_owned(),
        );
        state.set_technical_indicators(TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "smoke".to_owned(),
            options_summary: None,
            options_context: Some(TechnicalOptionsContext::Available {
                outcome: OptionsOutcome::Snapshot(snap.clone()),
            }),
        });

        let extracted = etf_options_from_state(&state);
        assert!(matches!(extracted, Some(s) if s.spot_price == snap.spot_price));
    }

    #[test]
    fn etf_valuation_inputs_drop_options_when_technical_context_is_fetch_failed() {
        use crate::state::{TechnicalData, TechnicalOptionsContext};

        let mut state = crate::testing::with_baseline_runtime_policy_state(
            "SPY".to_owned(),
            "2026-05-27".to_owned(),
        );
        state.set_technical_indicators(TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "smoke".to_owned(),
            options_summary: None,
            options_context: Some(TechnicalOptionsContext::FetchFailed {
                reason: "connection refused".to_owned(),
            }),
        });

        let extracted = etf_options_from_state(&state);
        assert!(extracted.is_none());
    }
```

If `crate::testing::with_baseline_runtime_policy_state` does not exist yet, use whatever helper builds a baseline-policy `TradingState` in `crates/scorpio-core/src/testing/runtime_policy.rs`. Inspect:

```bash
grep -n 'pub fn' /Users/bigtochan/Documents/dev/BigtoC/scorpio-analyst/crates/scorpio-core/src/testing/runtime_policy.rs
```

and use the matching helper name. If only `with_baseline_runtime_policy(&mut state)` exists, build a fresh `TradingState::new(...)` first and then apply it.

- [ ] **Step 5.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core workflow::tasks::analyst`
Expected: COMPILE FAILURE (`etf_options_from_state` not defined).

- [ ] **Step 5.3: Add the helper and wire it into the state-aware valuator input assembly**

In `crates/scorpio-core/src/workflow/tasks/analyst.rs`, add the helper as a free function at module scope:

```rust
/// Extract the live ETF options snapshot from persisted technical state.
///
/// Returns `Some(&snapshot)` only when `TechnicalOptionsContext::Available`
/// carries an `OptionsOutcome::Snapshot(_)`. Every other variant emits a
/// `tracing::warn!` and returns `None` so the valuator leaves
/// dealer-positioning absent cleanly.
pub(crate) fn etf_options_from_state(
    state: &crate::state::TradingState,
) -> Option<&crate::data::traits::options::OptionsSnapshot> {
    use crate::data::traits::options::OptionsOutcome;
    use crate::state::TechnicalOptionsContext;

    let technical = state.technical_indicators.as_ref()?;
    let options_context = technical.options_context.as_ref()?;
    match options_context {
        TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(snap),
        } => Some(snap),
        TechnicalOptionsContext::Available { outcome: other } => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                outcome = %other,
                symbol = %state.asset_symbol,
                "ETF options chain unavailable — dealer positioning skipped"
            );
            None
        }
        TechnicalOptionsContext::FetchFailed { reason } => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                symbol = %state.asset_symbol,
                fetch_reason = %reason,
                "ETF options fetch failed before valuation — dealer positioning skipped"
            );
            None
        }
    }
}
```

Then locate the state-aware `crate::valuation::ValuationInputs` construction inside `derive_runtime_valuation` (search with `grep -n 'crate::valuation::ValuationInputs {' crates/scorpio-core/src/workflow/tasks/analyst.rs`) and populate the three new fields there. Do **not** thread `TradingState` into `fetch_valuation_inputs`; that function stays provider-fetch-only.

```rust
let as_of = chrono::NaiveDate::parse_from_str(&state.target_date, "%Y-%m-%d")
    .unwrap_or_else(|_| chrono::Utc::now().date_naive());

let valuation_inputs = ValuationInputs {
    // ... existing fields unchanged ...
    etf_options: etf_options_from_state(state),
    as_of,
};
```

Do not add an `etf_risk_free_rate` carrier here in Stage 1. Stage 2 Task 20 adds that `ValuationInputs` field and flips this construction site to read from state after `TradingState` has the durable rate-source fields.

- [ ] **Step 5.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core workflow::tasks::analyst`
Expected: existing tests still pass; both new tests pass.

- [ ] **Step 5.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 5.6: Commit**

```bash
git add crates/scorpio-core/src/workflow/tasks/analyst.rs
git commit -m "feat(workflow): hydrate ETF valuation_inputs.etf_options from technical state

Reads TechnicalOptionsContext::Available { outcome: Snapshot(_) } into
the valuation carrier. All other variants emit a structured warn! and
fall through to absent dealer-positioning."
```

---

### Task 6: TradingState round-trip integration test for `GexSummary`

**Files:**
- Modify: `crates/scorpio-core/tests/state_roundtrip.rs`

- [ ] **Step 6.1: Write a failing roundtrip test**

Append to `crates/scorpio-core/tests/state_roundtrip.rs`:

```rust
#[test]
fn etf_valuation_with_populated_gex_strikes_roundtrips_through_trading_state() {
    use scorpio_core::state::{
        DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary, PremiumBand,
        PremiumSnapshot, ScenarioValuation, StrikeGex, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.equity = None;

    let derived = DerivedValuation {
        asset_shape: scorpio_core::state::AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: Some(620.39),
                ask: Some(620.41),
                premium_pct: Some(0.06),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 1.2e9,
                gross_gex_usd_per_1pct_move: 3.4e9,
                call_put_oi_ratio: 1.25,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
                strikes: vec![
                    StrikeGex { strike: 620.0, net_gex_usd_per_1pct_move: 0.6e9 },
                    StrikeGex { strike: 615.0, net_gex_usd_per_1pct_move: -0.4e9 },
                    StrikeGex { strike: 625.0, net_gex_usd_per_1pct_move: 0.2e9 },
                ],
            }),
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    };
    state.set_derived_valuation(derived);

    let json = serde_json::to_string(&state).expect("serialize");
    let back: TradingState = serde_json::from_str(&json).expect("deserialize");
    match back.derived_valuation.as_ref().map(|d| &d.scenario) {
        Some(ScenarioValuation::Etf(etf)) => {
            let g = etf.options_gex.as_ref().expect("gex");
            assert_eq!(g.strikes.len(), 3);
        }
        other => panic!("expected ETF scenario with gex, got {other:?}"),
    }
}

#[test]
fn legacy_etf_snapshot_without_phase2_gex_strikes_still_deserializes() {
    // Minimal JSON missing strikes — must default.
    let json = r#"{
        "net_gex_usd_per_1pct_move": 100.0,
        "gross_gex_usd_per_1pct_move": 200.0,
        "call_put_oi_ratio": 1.0,
        "max_pain_strike": 100.0,
        "near_term_expiration": "2026-06-26"
    }"#;
    let summary: scorpio_core::state::GexSummary = serde_json::from_str(json)
        .expect("legacy summary must deserialize");
    assert!(summary.strikes.is_empty());
}
```

Check the existing file for whichever helper builds a fresh `TradingState` — if `TradingState::new(symbol, target_date)` is the canonical constructor, use it directly. If there is a richer builder, prefer it.

If `set_derived_valuation` is not the public mutator on `TradingState`, replace with whatever the existing roundtrip tests use (search `grep -n 'derived_valuation' crates/scorpio-core/tests/state_roundtrip.rs` to find the convention).

- [ ] **Step 6.2: Run the tests to verify**

Run: `cargo nextest run -p scorpio-core --test state_roundtrip`
Expected: 2 new tests pass.

- [ ] **Step 6.3: Run the full workspace test suite as a sanity gate**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: no regressions. Investigate any failure immediately — Stage 1 is additive and must not break existing tests.

- [ ] **Step 6.4: Commit**

```bash
git add crates/scorpio-core/tests/state_roundtrip.rs
git commit -m "test(state): roundtrip GexSummary.strikes through TradingState

Locks in the additive-serde contract: legacy snapshots missing the new
GexSummary.strikes field deserialize with defaults; populated snapshots
preserve gamma walls on serialize+deserialize."
```

---

## Stage 2 — Surfaced validation slice

### Task 7: Leverage-warning helper — bump visibility and tighten format

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/etf/baseline.rs`

- [ ] **Step 7.1: Inspect the current helper**

Read `crates/scorpio-core/src/analysis_packs/etf/baseline.rs:71-86`. The existing helper:

```rust
const LEVERAGE_TOLERANCE: f64 = f64::EPSILON; // (uses f64::EPSILON, joins via compose_prompt_sections)

#[allow(dead_code)]
pub(crate) fn append_leverage_warning_if_needed(
    rendered: String,
    leverage_factor: Option<f64>,
) -> String {
    if leverage_factor
        .map(|factor| (factor - 1.0).abs() > f64::EPSILON)
        .unwrap_or(false)
    {
        compose_prompt_sections(&rendered, &[ETF_LEVERAGE_WARNING])
    } else {
        rendered
    }
}
```

Three changes per the spec:
1. Use a `1e-6` tolerance constant (more robust than `f64::EPSILON` against floating-point noise from upstream sources).
2. Insert an explicit `---` divider so the LLM sees a clear delimiter for the warning.
3. Drop `#[allow(dead_code)]` — the helper is now used.

- [ ] **Step 7.2: Write a failing unit test for the new format**

Append to the existing `#[cfg(test)] mod tests { ... }` block at the bottom of `crates/scorpio-core/src/analysis_packs/etf/baseline.rs`:

```rust
    #[test]
    fn append_leverage_warning_uses_divider_when_factor_diverges() {
        let rendered = "BASE PROMPT".to_owned();
        let result = append_leverage_warning_if_needed(rendered, Some(2.0));
        assert!(
            result.starts_with("BASE PROMPT"),
            "rendered base prompt must remain at the head"
        );
        assert!(
            result.contains("\n\n---\n\n"),
            "must insert the explicit --- divider: {result}"
        );
        // The warning content comes from etf_leverage_warning.md; assert it
        // is present without coupling to the file's exact prose.
        assert!(result.len() > "BASE PROMPT".len() + 8);
    }

    #[test]
    fn append_leverage_warning_uses_1e_minus_6_tolerance() {
        // 1.0 + ε (eps ≈ 2e-16) must NOT trigger the warning under the new
        // 1e-6 tolerance, while still triggering for any meaningful leverage.
        let base = "PROMPT".to_owned();
        let untouched = append_leverage_warning_if_needed(base.clone(), Some(1.0 + f64::EPSILON));
        assert_eq!(untouched, base, "EPSILON drift must not trigger warning");

        // Even a 1e-5 drift, well below typical leverage factors, must trigger.
        let with_drift = append_leverage_warning_if_needed(base.clone(), Some(1.0 + 1e-5));
        assert_ne!(with_drift, base, "1e-5 drift must trigger warning");
    }

    #[test]
    fn append_leverage_warning_skips_for_unit_and_none() {
        let base = "PROMPT".to_owned();
        assert_eq!(append_leverage_warning_if_needed(base.clone(), None), base);
        assert_eq!(
            append_leverage_warning_if_needed(base.clone(), Some(1.0)),
            base
        );
    }

    #[test]
    fn append_leverage_warning_triggers_for_leveraged_and_inverse() {
        let base = "PROMPT".to_owned();
        for factor in [2.0, 3.0, -1.0, -2.0, -3.0] {
            let result = append_leverage_warning_if_needed(base.clone(), Some(factor));
            assert!(
                result.len() > base.len(),
                "factor {factor} should append warning"
            );
        }
    }
```

- [ ] **Step 7.3: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core analysis_packs::etf::baseline::tests`
Expected: `append_leverage_warning_uses_divider_when_factor_diverges` fails because the current helper joins via `compose_prompt_sections` (no `---` divider).

- [ ] **Step 7.4: Update the helper to use the divider, `1e-6` tolerance, and `{leverage_factor}` substitution**

`crates/scorpio-core/src/analysis_packs/etf/prompts/etf_leverage_warning.md` contains a `{leverage_factor}` placeholder that today is never substituted (the current helper appends the file verbatim). The updated helper must substitute it, formatting integer factors without a decimal point (e.g. `3x`, `-2x`) and signed-leverage decimals to one place (e.g. `1.5x`).

In `crates/scorpio-core/src/analysis_packs/etf/baseline.rs`, replace the existing `append_leverage_warning_if_needed` function and its tolerance constant with:

```rust
const LEVERAGE_TOLERANCE: f64 = 1e-6;

/// Runtime-only helper invoked after placeholder substitution. Risk and
/// auditor prompts append the leverage warning when `leverage_factor`
/// diverges from 1.0 beyond the tolerance. Substitutes `{leverage_factor}`
/// in the warning body with a human-friendly representation of the factor.
pub(crate) fn append_leverage_warning_if_needed(
    rendered: String,
    leverage_factor: Option<f64>,
) -> String {
    match leverage_factor {
        Some(factor) if (factor - 1.0).abs() > LEVERAGE_TOLERANCE => {
            let warning = trim_trailing_newline(ETF_LEVERAGE_WARNING)
                .replace("{leverage_factor}", &format_leverage_factor(factor));
            format!("{rendered}\n\n---\n\n{warning}")
        }
        _ => rendered,
    }
}

fn format_leverage_factor(factor: f64) -> String {
    if (factor - factor.round()).abs() < LEVERAGE_TOLERANCE {
        format!("{:.0}", factor)
    } else {
        format!("{:.1}", factor)
    }
}
```

The `#[allow(dead_code)]` attribute is dropped — the helper becomes live as soon as Task 8 wires it into `render_risk_system_prompt`.

Add a unit test for the substitution behaviour. Append to the same `#[cfg(test)] mod tests { ... }` block:

```rust
    #[test]
    fn append_leverage_warning_substitutes_leverage_factor_placeholder() {
        let base = "PROMPT".to_owned();
        let triple = append_leverage_warning_if_needed(base.clone(), Some(3.0));
        assert!(triple.contains("3x"), "must substitute 3x: {triple}");
        assert!(!triple.contains("{leverage_factor}"), "placeholder must be gone");

        let inverse = append_leverage_warning_if_needed(base.clone(), Some(-1.0));
        assert!(inverse.contains("-1x"), "must substitute -1x: {inverse}");

        let half = append_leverage_warning_if_needed(base.clone(), Some(1.5));
        assert!(half.contains("1.5x"), "must substitute 1.5x: {half}");
    }
```

- [ ] **Step 7.5: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core analysis_packs::etf::baseline::tests`
Expected: all tests pass, including the 5 new ones.

- [ ] **Step 7.6: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 7.7: Commit**

```bash
git add crates/scorpio-core/src/analysis_packs/etf/baseline.rs
git commit -m "refactor(etf): tighten leverage-warning helper format and tolerance

Uses an explicit '---' divider between the rendered base prompt and the
ETF leverage warning, and raises the leverage-factor tolerance from
f64::EPSILON to 1e-6 to absorb upstream floating-point noise. Drops the
#[allow(dead_code)] guard ahead of wiring into risk+auditor prompts."
```

---

### Task 8: Inject leverage warning into Conservative + Neutral risk prompts

**Files:**
- Modify: `crates/scorpio-core/src/agents/risk/common.rs`

- [ ] **Step 8.1: Write a failing prompt-rendering test**

Locate the existing `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/agents/risk/common.rs` and append:

```rust
    #[test]
    fn render_risk_system_prompt_appends_leverage_warning_for_levered_etf() {
        let mut state = make_state();
        state.asset_symbol = "TQQQ".to_owned();
        // Hydrate ETF baseline pack + an ETF valuation carrying leverage_factor = 3.0.
        let policy = crate::analysis_packs::resolve_runtime_policy("etf_baseline")
            .expect("etf_baseline pack must resolve");
        state.analysis_runtime_policy = Some(policy);

        use crate::state::{
            AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
            PremiumSnapshot, ScenarioValuation,
        };
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: PremiumSnapshot {
                    nav: Some(50.0),
                    market_price: 50.0,
                    bid: None,
                    ask: None,
                    premium_pct: None,
                    category_band: PremiumBand::Unknown,
                    bid_ask_spread_pct: None,
                    as_of: chrono::Utc::now(),
                },
                composition: None,
                tracking: None,
                options_gex: None,
                category: None,
                leverage_factor: Some(3.0),
                flags: EtfDataAvailability::default(),
            }),
        });

        let policy = state.analysis_runtime_policy.as_ref().unwrap();
        let prompt = render_risk_system_prompt(
            policy,
            &state,
            |b| b.conservative_risk.as_ref(),
            true, // apply_leverage_warning — Conservative opts in
        );
        assert!(
            prompt.contains("Daily-reset products"),
            "leveraged ETF prompt must carry the warning body: {prompt}"
        );
        assert!(
            prompt.contains("3x"),
            "leveraged ETF prompt must substitute the factor: {prompt}"
        );
    }

    #[test]
    fn render_risk_system_prompt_omits_leverage_warning_for_unit_factor() {
        let mut state = make_state();
        state.asset_symbol = "SPY".to_owned();
        let policy = crate::analysis_packs::resolve_runtime_policy("etf_baseline")
            .expect("etf_baseline pack must resolve");
        state.analysis_runtime_policy = Some(policy);

        use crate::state::{
            AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
            PremiumSnapshot, ScenarioValuation,
        };
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: PremiumSnapshot {
                    nav: Some(620.0),
                    market_price: 620.0,
                    bid: None,
                    ask: None,
                    premium_pct: None,
                    category_band: PremiumBand::Unknown,
                    bid_ask_spread_pct: None,
                    as_of: chrono::Utc::now(),
                },
                composition: None,
                tracking: None,
                options_gex: None,
                category: None,
                leverage_factor: Some(1.0),
                flags: EtfDataAvailability::default(),
            }),
        });

        let policy = state.analysis_runtime_policy.as_ref().unwrap();
        let conservative = render_risk_system_prompt(
            policy,
            &state,
            |b| b.conservative_risk.as_ref(),
            true,
        );
        let neutral = render_risk_system_prompt(
            policy,
            &state,
            |b| b.neutral_risk.as_ref(),
            true,
        );
        // "Daily-reset products" is exclusive to etf_leverage_warning.md.
        const WARNING_MARKER: &str = "Daily-reset products";
        assert!(
            !conservative.contains(WARNING_MARKER),
            "unit factor must skip warning: {conservative}"
        );
        assert!(
            !neutral.contains(WARNING_MARKER),
            "unit factor must skip warning: {neutral}"
        );
    }
```

The marker `"Daily-reset products"` is unique to `etf_leverage_warning.md`. Confirm with:

```bash
grep -rn "Daily-reset products" crates/scorpio-core/src/analysis_packs/
```

Expected: a single match, in `etf_leverage_warning.md`.

- [ ] **Step 8.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core agents::risk::common::tests`
Expected: `render_risk_system_prompt_appends_leverage_warning_for_levered_etf` fails because the renderer does not yet call the helper.

- [ ] **Step 8.3: Read the ETF valuation off state and apply the helper**

In `crates/scorpio-core/src/agents/risk/common.rs`, replace the existing `render_risk_system_prompt` function with the version below. **The function MUST take a caller-controlled `apply_leverage_warning: bool` flag** so Conservative and Neutral can inject the warning while Aggressive (and the Risk Moderator) opt out — Task 12's contract requires this asymmetry, and `bundle_slot` is a function pointer that cannot be discriminated at runtime.

```rust
pub(crate) fn render_risk_system_prompt(
    policy: &crate::analysis_packs::RuntimePolicy,
    state: &TradingState,
    bundle_slot: fn(&PromptBundle) -> &str,
    apply_leverage_warning: bool,
) -> String {
    let symbol = sanitize_symbol_for_prompt(&state.asset_symbol);
    let target_date = sanitize_date_for_prompt(&state.target_date);
    let template = bundle_slot(&policy.prompt_bundle);

    let rendered = template
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date)
        .replace("{past_memory_str}", "see untrusted user context")
        .replace("{analysis_emphasis}", &analysis_emphasis_for_prompt(state));

    if apply_leverage_warning {
        crate::analysis_packs::etf::append_leverage_warning_if_needed(
            rendered,
            etf_leverage_factor_from_state(state),
        )
    } else {
        rendered
    }
}

/// Extract the ETF leverage factor from `state.derived_valuation` when the
/// scenario is `ScenarioValuation::Etf`. Returns `None` for any non-ETF
/// scenario so non-ETF runs skip the warning entirely.
fn etf_leverage_factor_from_state(state: &TradingState) -> Option<f64> {
    use crate::state::ScenarioValuation;
    match state.derived_valuation().map(|d| &d.scenario)? {
        ScenarioValuation::Etf(etf) => etf.leverage_factor,
        _ => None,
    }
}
```

Thread the new flag through `RiskAgentCore::new` (or whichever constructor each risk agent uses) so each agent's construction site is the single source of truth for whether the warning applies:

- `agents/risk/conservative.rs` passes `apply_leverage_warning: true`
- `agents/risk/neutral.rs` passes `apply_leverage_warning: true`
- `agents/risk/aggressive.rs` passes `apply_leverage_warning: false`
- `agents/risk/moderator.rs` passes `apply_leverage_warning: false`

Then expose the helper from the `analysis_packs::etf` module because `baseline` is private:

```bash
grep -n 'pub mod baseline' crates/scorpio-core/src/analysis_packs/etf/mod.rs
```

Add `pub(crate) use baseline::append_leverage_warning_if_needed;` in `crates/scorpio-core/src/analysis_packs/etf/mod.rs` and `pub(crate) use etf::append_leverage_warning_if_needed;` in `crates/scorpio-core/src/analysis_packs/mod.rs` (so `common.rs` can call `crate::analysis_packs::append_leverage_warning_if_needed(...)`). Test-only callers (e.g. the `testing::prompt_render` shims) must pass the same flag.

- [ ] **Step 8.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core agents::risk::common::tests`
Expected: all tests pass.

- [ ] **Step 8.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 8.6: Commit**

```bash
git add crates/scorpio-core/src/agents/risk/common.rs crates/scorpio-core/src/analysis_packs/etf/mod.rs
git commit -m "feat(risk): inject ETF leverage warning into Conservative+Neutral prompts

Renderer-side append after placeholder substitution. Reads leverage_factor
from state.derived_valuation when the scenario is ScenarioValuation::Etf,
and skips entirely when factor is None or 1.0. Non-ETF runs are unaffected."
```

---

### Task 9: Inject leverage warning into the Auditor prompt

**Files:**
- Modify: `crates/scorpio-core/src/agents/auditor/prompt.rs`

- [ ] **Step 9.1: Write a failing test**

Append to the existing `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/agents/auditor/prompt.rs`:

```rust
    #[test]
    fn auditor_prompt_carries_leverage_warning_for_levered_etf() {
        use crate::state::{
            AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
            PremiumSnapshot, ScenarioValuation, TradingState,
        };

        let mut state = TradingState::new("TQQQ".to_owned(), "2026-05-27".to_owned());
        let policy = crate::analysis_packs::resolve_runtime_policy("etf_baseline")
            .expect("etf_baseline pack must resolve");
        state.analysis_runtime_policy = Some(policy);
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: PremiumSnapshot {
                    nav: Some(50.0),
                    market_price: 50.0,
                    bid: None,
                    ask: None,
                    premium_pct: None,
                    category_band: PremiumBand::Unknown,
                    bid_ask_spread_pct: None,
                    as_of: chrono::Utc::now(),
                },
                composition: None,
                tracking: None,
                options_gex: None,
                category: None,
                leverage_factor: Some(-2.0),
                flags: EtfDataAvailability::default(),
            }),
        });

        let prompt = build_system_prompt(&state).expect("prompt build");
        assert!(
            prompt.contains("Daily-reset products"),
            "auditor prompt must include the leverage warning body: {prompt}"
        );
        assert!(
            prompt.contains("-2x"),
            "auditor prompt must substitute the factor: {prompt}"
        );
    }
```

- [ ] **Step 9.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core agents::auditor::prompt::tests`
Expected: the new test fails.

- [ ] **Step 9.3: Apply the helper in `build_system_prompt`**

Replace the function body in `crates/scorpio-core/src/agents/auditor/prompt.rs`:

```rust
pub(crate) fn build_system_prompt(state: &TradingState) -> Result<String, TradingError> {
    let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
        TradingError::Config(anyhow::anyhow!(
            "auditor prompt: missing runtime policy — preflight must run before auditor"
        ))
    })?;
    if policy.prompt_bundle.auditor.is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "auditor prompt: auditor slot is empty — pack must supply a non-empty auditor prompt \
             when auditor_enabled = true"
        )));
    }
    let symbol = sanitize_symbol_for_prompt(&state.asset_symbol);
    let target_date = sanitize_date_for_prompt(&state.target_date);
    let rendered = policy
        .prompt_bundle
        .auditor
        .as_ref()
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date);
    Ok(
        crate::analysis_packs::etf::append_leverage_warning_if_needed(
            rendered,
            etf_leverage_factor_from_state(state),
        ),
    )
}

fn etf_leverage_factor_from_state(state: &TradingState) -> Option<f64> {
    use crate::state::ScenarioValuation;
    match state.derived_valuation.as_ref()?.scenario {
        ScenarioValuation::Etf(ref etf) => etf.leverage_factor,
        _ => None,
    }
}
```

(If Task 8 chose the `crate::analysis_packs::etf::append_leverage_warning_if_needed` re-export, use that path here too.)

- [ ] **Step 9.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core agents::auditor::prompt::tests`
Expected: all tests pass.

- [ ] **Step 9.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 9.6: Commit**

```bash
git add crates/scorpio-core/src/agents/auditor/prompt.rs
git commit -m "feat(auditor): inject ETF leverage warning into auditor prompt

Same helper path as Conservative/Neutral risk: reads leverage_factor from
state.derived_valuation's ETF scenario and appends the warning suffix
after placeholder substitution when the factor diverges from 1.0."
```

---

### Task 10: Rewrite `etf_tracking_options_focus.md`

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md`

- [ ] **Step 10.1: Replace the placeholder body**

Open `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md` and replace the entire file contents with:

```markdown
## ETF tracking & dealer-positioning lens

In addition to standard technicals:

- **Tracking error** — if `TrackingError` is present, cite `te_pct_90d` and
  `te_pct_1y`. >0.20% annualised on a vanilla index-tracker is structurally
  costly; >1.0% suggests active management or sampling mismatch.

- **Dealer positioning (secondary baseline overlay)** — when `options_gex` is
  available in the prompt context, treat it as a **secondary overlay** on top
  of premium/discount, composition, and tracking evidence. Do not cite
  `options_gex` fields from the technical prompt unless the implementation has
  explicitly threaded that derived payload into the prompt context. When only
  raw `options_context` / `options_summary` is available, discuss only the raw
  snapshot signals present there.

  When derived `options_gex` is available, cite present, decision-relevant
  signals. Do not force named absence callouts for every unavailable sub-signal:

  - **Near-term gamma exposure** — `options_gex.net_gex_usd_per_1pct_move`.
    Positive net means dealer hedging tends to dampen near-term moves;
    negative net means hedging tends to amplify them.
  - **Broad gamma exposure** — `options_gex.broad.net_gex_usd_per_1pct_move`
    when present. Explicitly label this as an all-expirations
    single-rate approximation when present.
    If `options_gex.broad.expirations_used <
    options_gex.broad.expirations_total_considered`, label the broad line as
    `Partial expirations` and mention both counts.
  - **Volatility sensitivity (VEX)** —
    `options_gex.vex_summary.net_vex_usd_per_volpt` when present, framed as a
    **conditional sensitivity to an absolute IV move**, not as a stand-alone
    stabilizing signal.
  - **Time-decay sensitivity (CEX)** —
    `options_gex.cex_summary.net_cex_usd_per_day` when present, framed as a
    **conditional sensitivity to one calendar day of decay**.
  - **Gamma walls** — `options_gex.strikes` (top dealer concentrations by
    `|net_gex|`) when present.
  - **Supporting evidence** — `options_gex.call_put_oi_ratio` and
    `options_gex.max_pain_strike` are **supporting**, not primary, evidence.
    Cite them only after the near-term GEX line.

- **Absence handling** — Stage 2 uses a single generic branch: if no usable
  derived dealer-positioning overlay is available in the prompt context, say
  dealer-positioning signals are unavailable for this run and keep the rest of
  the ETF analysis anchored on premium/discount, composition, and tracking.
  Split no-snapshot vs unusable-snapshot copy only after adding an explicit
  derivation-status field.
```

- [ ] **Step 10.2: Run prompt-bundle structure tests**

Run: `cargo nextest run -p scorpio-core analysis_packs::etf::baseline::tests::etf_baseline_populates_every_prompt_slot_with_runtime_placeholders`
Expected: PASS — the file still contains `{ticker}` and `{current_date}` references via its outer-composition layers, and the ETF technical analyst slot remains non-empty. (The focus document itself does not need to contain those placeholders; the outer composed prompt does.)

- [ ] **Step 10.3: Run the broader prompt-validation gate**

Run: `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`
Expected: snapshot-byte tests will fail because the prompt bytes changed. That is expected; Task 12 refreshes those golden bytes. For now, capture the diff to confirm the change scope matches expectations:

```bash
cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers 2>&1 | head -80
```

- [ ] **Step 10.4: Commit**

```bash
git add crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md
git commit -m "docs(etf): rewrite tracking/options focus prompt for Phase 2 dealer positioning

Phase 1 placeholder language is replaced with secondary-overlay guidance
that matches the data actually available to the technical prompt, avoids
mandatory missing-subsignal bookkeeping, and uses one generic Stage 2
absence branch until an explicit derivation status exists."
```

---

### Task 11: DEALER POSITIONING block in the terminal reporter

**Files:**
- Modify: `crates/scorpio-reporters/src/terminal/etf.rs`
- Modify: `crates/scorpio-reporters/tests/terminal.rs`

- [ ] **Step 11.1: Write failing assertions**

Append to `crates/scorpio-reporters/tests/terminal.rs`:

```rust
#[test]
fn etf_terminal_renders_dealer_positioning_block_when_gex_present() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary,
        PremiumBand, PremiumSnapshot, ScenarioValuation, StrikeGex, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: Some(620.39),
                ask: Some(620.41),
                premium_pct: Some(0.06),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 2.84e9,
                gross_gex_usd_per_1pct_move: 7.12e9,
                call_put_oi_ratio: 1.31,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![
                    StrikeGex { strike: 625.0, net_gex_usd_per_1pct_move: 1.20e9 },
                    StrikeGex { strike: 615.0, net_gex_usd_per_1pct_move: -0.84e9 },
                    StrikeGex { strike: 630.0, net_gex_usd_per_1pct_move: 0.62e9 },
                ],
                broad: None,
                vex_summary: None,
                cex_summary: None,
            }),
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("DEALER POSITIONING"), "header missing: {rendered}");
    assert!(rendered.contains("Near-term"), "near-term subheader missing: {rendered}");
    assert!(rendered.contains("Summary"), "summary line missing: {rendered}");
    assert!(rendered.contains("Gamma walls"), "gamma walls line missing: {rendered}");
    assert!(rendered.contains("Max-pain"), "max-pain line missing: {rendered}");
    // Stage 2 must NOT show secondary sensitivities or all-expirations rows.
    assert!(
        !rendered.contains("Secondary sensitivities"),
        "Stage 2 must omit VEX/CEX block: {rendered}"
    );
    assert!(
        !rendered.contains("All expirations"),
        "Stage 2 must omit broad GEX line: {rendered}"
    );
}

#[test]
fn etf_terminal_hides_dealer_positioning_block_when_gex_absent() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
        PremiumSnapshot, ScenarioValuation, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: None,
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(
        !rendered.contains("DEALER POSITIONING"),
        "block must be hidden when options_gex is None: {rendered}"
    );
}

#[test]
fn etf_terminal_emits_partial_data_note_for_missing_walls() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary,
        PremiumBand, PremiumSnapshot, ScenarioValuation, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 1.0e9,
                gross_gex_usd_per_1pct_move: 2.0e9,
                call_put_oi_ratio: 1.0,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![], // walls unavailable
                broad: None,
                vex_summary: None,
                cex_summary: None,
            }),
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(
        rendered.contains("gamma walls unavailable")
            || rendered.contains("gamma walls and broad GEX unavailable"),
        "missing partial-data note: {rendered}"
    );
}

#[test]
fn etf_terminal_renders_degraded_rate_banner_when_rate_unavailable() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
        PremiumSnapshot, ScenarioValuation, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = None;
    state.etf_risk_free_rate_source = None;
    // ETF scenario must be present for the degraded banner to fire — non-ETF
    // runs have rate fields at default None and must not trigger the warning.
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: None,
                market_price: 620.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: None,
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(
        rendered.contains("⚠ Risk-free rate unavailable")
            && rendered.contains("dealer positioning unavailable"),
        "degraded-rate banner missing: {rendered}"
    );
}

#[test]
fn non_etf_terminal_does_not_show_degraded_rate_banner() {
    // Equity-only state. Both rate fields default to None — preflight only
    // writes them on ETF baseline runs. The banner must stay silent so
    // non-ETF reports aren't polluted with a warning about a rate that has
    // no meaning for equity analyses.
    use scorpio_core::state::TradingState;
    let state = TradingState::new("AAPL".to_owned(), "2026-05-27".to_owned());
    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(
        !rendered.contains("Risk-free rate unavailable"),
        "non-ETF report must not show rate-unavailable banner: {rendered}"
    );
    assert!(
        !rendered.contains("dealer positioning unavailable"),
        "non-ETF report must not advertise dealer-positioning state: {rendered}"
    );
}

#[test]
fn etf_terminal_labels_yfinance_irx_rate_source_without_warning() {
    use scorpio_core::state::{EtfRiskFreeRateSource, TradingState};

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = Some(0.0433);
    state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::YFinanceIrx);

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("Risk-free rate    yfinance ^IRX"));
    assert!(
        !rendered.contains("Risk-free rate unavailable"),
        "^IRX fallback is a live source, not a hardcoded fallback warning: {rendered}"
    );
}

#[test]
fn etf_terminal_labels_fred_dgs3mo_rate_source_without_warning() {
    use scorpio_core::state::{EtfRiskFreeRateSource, TradingState};

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = Some(0.0427);
    state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FredDgs3Mo);

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("Risk-free rate    FRED DGS3MO"));
    assert!(
        !rendered.contains("Risk-free rate unavailable"),
        "FRED success must not show the degraded banner: {rendered}"
    );
}
```

If the public render entry-point is not `scorpio_reporters::terminal::render_final_report`, use the correct path from `crates/scorpio-reporters/src/lib.rs`. Search:

```bash
grep -n 'pub fn render\|pub fn render_terminal\|pub fn render_final_report' crates/scorpio-reporters/src/lib.rs crates/scorpio-reporters/src/terminal/mod.rs
```

and use whichever function returns the rendered string for a `TradingState`.

The banner tests rely on the Stage 2 rate-sourcing work landed in Tasks 18-20. Stage 2 task ordering is therefore Task 18 → Task 19 → Task 20 → Task 11 (this task) → Task 12. Do not attempt this task before `state.etf_risk_free_rate` and `state.etf_risk_free_rate_source` exist on `TradingState`.

- [ ] **Step 11.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-reporters --test terminal`
Expected: all six new tests fail (three for the dealer-positioning block, two for the rate-source banner, one for the negative non-ETF gate; the unavailable case also fails for the same reason).

- [ ] **Step 11.3: Implement `render_dealer_positioning_block`**

In `crates/scorpio-reporters/src/terminal/etf.rs`, locate `render_etf_panel` (around line 46) and the surrounding block helpers. Add a new function:

```rust
use scorpio_core::state::{GexSummary, StrikeGex};

fn render_dealer_positioning_block(out: &mut String, gex: &GexSummary, policy: RenderPolicy) {
    use std::fmt::Write as _;

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  ─── DEALER POSITIONING ──────────────────────────────────────────────"
    );
    let _ = writeln!(out, "  Near-term  ({})", gex.near_term_expiration);

    let summary_line = build_dealer_summary_line(gex);
    let _ = writeln!(out, "    Summary         {summary_line}");
    let _ = writeln!(
        out,
        "    Net GEX/1%      {net}    Gross GEX/1%    {gross}",
        net = format_usd_signed(gex.net_gex_usd_per_1pct_move),
        gross = format_usd_magnitude(gex.gross_gex_usd_per_1pct_move),
    );
    let _ = writeln!(
        out,
        "    Call/Put OI     {cp:.2}      Max-pain        ${mp:.0}",
        cp = gex.call_put_oi_ratio,
        mp = gex.max_pain_strike,
    );

    if !gex.strikes.is_empty() {
        let walls = gex
            .strikes
            .iter()
            .map(format_strike_gex)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "    Gamma walls    {walls}");
    }

    // Partial-data note. Stage 2 has neither broad nor secondary sensitivities,
    // so partial-data lines only fire when strikes is empty. Stage 3 expands
    // this to cover the broad/secondary cases as well.
    let walls_missing = gex.strikes.is_empty();
    let broad_missing = gex.broad.is_none();
    if walls_missing && broad_missing && !is_stage3_full(gex) {
        // Stage 2 only ever has broad = None and may have empty walls; the
        // combined note is reserved for Stage 3 once `broad` becomes populated
        // by default. Stage 2 emits the single-cause line.
        let _ = writeln!(out, "    Dealer positioning partial — gamma walls unavailable");
    } else if walls_missing {
        let _ = writeln!(out, "    Dealer positioning partial — gamma walls unavailable");
    }

    let _ = policy; // policy hook reserved for narrow-terminal variant; unused in Stage 2
}

/// Stage 3 sentinel — returns true once `broad`, `vex_summary`, or
/// `cex_summary` is populated, so the combined-partial copy applies.
fn is_stage3_full(gex: &GexSummary) -> bool {
    gex.broad.is_some() || gex.vex_summary.is_some() || gex.cex_summary.is_some()
}

fn format_strike_gex(s: &StrikeGex) -> String {
    format!(
        "{} @ ${:.0}",
        format_usd_signed(s.net_gex_usd_per_1pct_move),
        s.strike
    )
}

fn format_usd_signed(value: f64) -> String {
    let abs = value.abs();
    let (suffix, scaled) = scale_for_usd(abs);
    let sign = if value >= 0.0 { '+' } else { '-' };
    format!("{sign}${scaled:.2}{suffix}")
}

fn format_usd_magnitude(value: f64) -> String {
    let (suffix, scaled) = scale_for_usd(value.abs());
    format!("${scaled:.2}{suffix}")
}

fn scale_for_usd(value: f64) -> (&'static str, f64) {
    const B: f64 = 1.0e9;
    const M: f64 = 1.0e6;
    const K: f64 = 1.0e3;
    if value >= B {
        ("B", value / B)
    } else if value >= M {
        ("M", value / M)
    } else if value >= K {
        ("K", value / K)
    } else {
        ("", value)
    }
}

fn build_dealer_summary_line(gex: &GexSummary) -> String {
    // Plain-English one-liner aimed at mainstream ETF readers.
    let regime = if gex.net_gex_usd_per_1pct_move > 0.0 {
        "Dealer hedging likely dampens near-term moves"
    } else if gex.net_gex_usd_per_1pct_move < 0.0 {
        "Dealer hedging likely amplifies near-term moves"
    } else {
        "Dealer hedging is roughly neutral on near-term moves"
    };

    if gex.strikes.is_empty() {
        regime.to_owned()
    } else {
        let strikes_sorted: Vec<f64> = {
            let mut s: Vec<f64> = gex.strikes.iter().map(|w| w.strike).collect();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            s
        };
        let lo = strikes_sorted.first().copied().unwrap_or(0.0);
        let hi = strikes_sorted.last().copied().unwrap_or(0.0);
        if (hi - lo).abs() < f64::EPSILON {
            format!("{regime}; gamma walls cluster near ${lo:.0}")
        } else {
            format!("{regime}; gamma walls cluster near ${lo:.0}-${hi:.0}")
        }
    }
}
```

Then call the new helper from `render_etf_panel_with_policy` immediately after the tracking block. Locate the place where `render_tracking_block` is called and add:

```rust
if let Some(gex) = etf.options_gex.as_ref() {
    render_dealer_positioning_block(out, gex, policy);
} else {
    // Surface the absence in DATA AVAILABILITY when an options chain was
    // expected but no usable dealer-positioning overlay was produced.
    if etf.options_gex.is_none() {
        let _ = std::fmt::Write::write_fmt(
            out,
            format_args!(
                "  ⚠ Dealer positioning skipped — no usable options-derived overlay available\n"
            ),
        );
    }
}
```

- [ ] **Step 11.4: Render the risk-free-rate source banner**

The rate-source data is populated in Stage 2 (Tasks 18-20). The validation gate evaluates terminal output, so the banner that names the source must render in Stage 2 as well. Locate the ETF report header (search `grep -n 'Analysis Pack' crates/scorpio-reporters/src/terminal/`), and insert the banner immediately after the `Analysis Pack    ETF Baseline` line:

```rust
use std::fmt::Write as _;

// Both None is the default state for every non-ETF run, so the degraded
// warning must additionally observe an ETF-scenario marker to avoid
// false-positive banners on equity reports (preflight only writes the
// rate fields when `pack == EtfBaseline && today` — see Task 19).
let is_etf_run = state
    .derived_valuation()
    .map(|d| matches!(d.scenario, scorpio_core::state::ScenarioValuation::Etf(_)))
    .unwrap_or(false);

match (state.etf_risk_free_rate, state.etf_risk_free_rate_source) {
    (Some(rate), Some(scorpio_core::state::EtfRiskFreeRateSource::FredDgs3Mo)) => {
        let _ = writeln!(out, "  Risk-free rate    FRED DGS3MO ({:.2}%)", rate * 100.0);
    }
    (Some(rate), Some(scorpio_core::state::EtfRiskFreeRateSource::YFinanceIrx)) => {
        let _ = writeln!(out, "  Risk-free rate    yfinance ^IRX ({:.2}%)", rate * 100.0);
    }
    (None, None) if is_etf_run => {
        let _ = writeln!(
            out,
            "  ⚠ Risk-free rate unavailable — dealer positioning unavailable"
        );
    }
    _ => {}
}
```

Match arms intentionally collapse mismatched `(Some, None)` / `(None, Some)` pairings to no-op; preflight always writes both fields or neither, so any other combination indicates state corruption and is not surfaced. The `(None, None)` arm is gated on the ETF-scenario marker because equity runs default to `(None, None)` and must not surface a warning about a rate that is irrelevant to non-ETF analyses.

- [ ] **Step 11.5: Run the reporter tests**

Run: `cargo nextest run -p scorpio-reporters --test terminal`
Expected: 6 new tests pass (3 dealer-positioning + 2 banner labels + 1 negative banner; the degraded banner test also flips green once Step 11.4 lands).

- [ ] **Step 11.6: Run the full workspace test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: no regressions outside `prompt_bundle_regression_gate` (covered in Task 12).

- [ ] **Step 11.7: Lint and format**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 11.8: Commit**

```bash
git add crates/scorpio-reporters/src/terminal/etf.rs crates/scorpio-reporters/tests/terminal.rs
git commit -m "feat(reporter): Stage 2 dealer-positioning block and risk-free-rate banner

Compact near-term GEX block (plain-English summary, signed net/gross GEX
per 1% move, call/put OI, max-pain strike, top-3 gamma walls) plus the
risk-free-rate source/degraded banner under Analysis Pack. The banner
shows FRED DGS3MO or yfinance ^IRX with the live rate, or a degraded
notice when both sources fail. Stage 3 extends with secondary
sensitivities and all-expirations broad GEX."
```

---

### Task 12: Update the prompt-bundle regression-gate goldens

**Files:**
- Modify: `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`

- [ ] **Step 12.1: Inspect the existing gate**

Read the file:

```bash
head -120 crates/scorpio-core/tests/prompt_bundle_regression_gate.rs
```

The gate compares rendered prompt bytes against either inline expected strings or files under `crates/scorpio-core/tests/fixtures/prompt_bundles/` (look for the actual layout). The current gate is baseline-oriented, so choose one ETF strategy before updating bytes: either make the fixture path pack-aware, or add a separate ETF fixture namespace/test path. Identify the technical-analyst slot for the ETF baseline pack — its bytes changed in Task 10. Identify the three rendered slots that now receive the leverage warning suffix when a leveraged ETF state is provided (Conservative, Neutral, and Auditor).

- [ ] **Step 12.2: Refresh the technical-analyst goldens**

For the ETF baseline pack, regenerate the technical-analyst slot's expected bytes under the chosen ETF fixture strategy. If the gate stores them inline in the test file, copy the new rendered output from a debug print:

```rust
#[test]
fn _dump_etf_technical_analyst() {
    let pack = scorpio_core::analysis_packs::registry::resolve_pack(
        scorpio_core::analysis_packs::PackId::EtfBaseline,
    );
    eprintln!("---BEGIN---\n{}\n---END---", pack.prompt_bundle.technical_analyst);
}
```

Run with `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers _dump_etf_technical_analyst --nocapture` and copy the captured bytes into the gate fixture. Remove the dump test before committing.

If goldens live in `tests/fixtures/prompt_bundles/*.txt`, replace the file contents and let the gate match by file read.

- [ ] **Step 12.3: Add leverage-warning coverage to the gate**

Add a new gate scenario that exercises Conservative + Neutral + Auditor with a leveraged ETF state. Append to `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`:

```rust
#[test]
fn leverage_warning_appears_only_for_conservative_neutral_auditor_when_levered() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
        PremiumSnapshot, ScenarioValuation, TradingState,
    };
    let mut state = TradingState::new("TQQQ".to_owned(), "2026-05-27".to_owned());
    let policy = scorpio_core::analysis_packs::resolve_runtime_policy("etf_baseline")
        .expect("etf_baseline pack must resolve");
    state.analysis_runtime_policy = Some(policy);
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(50.0),
                market_price: 50.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: None,
            category: None,
            leverage_factor: Some(3.0),
            flags: EtfDataAvailability::default(),
        }),
    });
    const MARKER: &str = "Daily-reset products";

    let bundle = &state.analysis_runtime_policy.as_ref().unwrap().prompt_bundle;
    let policy = state.analysis_runtime_policy.as_ref().unwrap();
    // Slots that MUST carry the warning — opt-in via apply_leverage_warning=true:
    let conservative = scorpio_core::agents::risk::common::render_risk_system_prompt(
        policy,
        &state,
        |b: &scorpio_core::prompts::PromptBundle| b.conservative_risk.as_ref(),
        true,
    );
    let neutral = scorpio_core::agents::risk::common::render_risk_system_prompt(
        policy,
        &state,
        |b: &scorpio_core::prompts::PromptBundle| b.neutral_risk.as_ref(),
        true,
    );
    let auditor = scorpio_core::agents::auditor::build_system_prompt(&state)
        .expect("auditor prompt");
    assert!(conservative.contains(MARKER), "conservative must carry warning");
    assert!(neutral.contains(MARKER), "neutral must carry warning");
    assert!(auditor.contains(MARKER), "auditor must carry warning");

    // Slots that MUST NOT carry the warning — opt-out via apply_leverage_warning=false:
    let aggressive = scorpio_core::agents::risk::common::render_risk_system_prompt(
        policy,
        &state,
        |b: &scorpio_core::prompts::PromptBundle| b.aggressive_risk.as_ref(),
        false,
    );
    let trader = bundle.trader.as_ref();
    let fund_manager = bundle.fund_manager.as_ref();
    assert!(!aggressive.contains(MARKER), "aggressive must NOT carry warning");
    // Trader/fund_manager bundles are inspected raw (no leverage helper hook).
    assert!(!trader.contains(MARKER), "trader must NOT carry warning");
    assert!(!fund_manager.contains(MARKER), "fund_manager must NOT carry warning");
}
```

In practice the test is most naturally written via a testing-facade helper (e.g. `scorpio_core::testing::render_levered_etf_risk_prompts_for_gate()` returning a `LeverageWarningProbe` struct) so the integration test doesn't need direct access to `pub(crate)` items. Use that pattern if the underlying functions aren't exposed under `test-helpers`.

If `scorpio_core::agents::auditor::build_system_prompt` is not exported under `test-helpers`, expose the existing private function with `#[cfg(any(test, feature = "test-helpers"))] pub use prompt::build_system_prompt;` in `crates/scorpio-core/src/agents/auditor/mod.rs`.

- [ ] **Step 12.4: Run the regression gate**

Run: `cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers`
Expected: all assertions pass after the goldens are refreshed.

- [ ] **Step 12.5: Run the full workspace test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: green.

- [ ] **Step 12.6: Commit**

```bash
git add crates/scorpio-core/tests/prompt_bundle_regression_gate.rs crates/scorpio-core/src/agents/risk crates/scorpio-core/src/agents/auditor
git commit -m "test(prompts): regression-gate coverage for Phase 2 Stage 2 prompt deltas

Refreshes ETF technical-analyst goldens after etf_tracking_options_focus.md
rewrite. Adds positive+negative leverage-warning coverage across Conservative,
Neutral, Aggressive-negative, Trader/FundManager-negative, and Auditor slots."
```

---

### Required Stage 2 Risk-Free-Rate Work

Complete the detailed Tasks 18-20 before running the Stage 2 validation gate. These tasks are promoted into Stage 2 because production dealer-positioning must not use a hardcoded risk-free-rate fallback.

- Task 18 adds durable `etf_risk_free_rate` / source state.
- Task 19 fetches FRED `DGS3MO`, falls back to yfinance `^IRX`, and degrades to no rate if both fail.
- Task 20 threads the live rate into `ValuationInputs` and leaves `options_gex` absent when no rate is available.

The detailed task bodies remain below to preserve the existing task numbering, but they are no longer contingent Stage 3 work.

---

## ⚠️ Stage 2 Validation Gate ⚠️

**Do not proceed past this point without an explicit proceed/stop decision from the user.**

After Stage 2 ships, run the surfaced overlay against a validation sample that includes liquid positive cases and negative-control ETFs where dealer-positioning is expected to add little value (e.g. SPY, QQQ, TQQQ, IWM, EFA plus at least two lower-options-value mainstream ETFs). For each sample, capture terminal output and the generated prose surfaces that Stage 2 actually changes:

1. **Distinctness** — does the DEALER POSITIONING block deliver a non-redundant risk/liquidity takeaway, or does it merely echo what premium/discount + composition + tracking already say?
2. **Secondary-ness** — does the block stay clearly secondary to the existing ETF anchors, or does it dominate the read?
3. **Mainstream-reader fit** — does the plain-English summary line work without prior options literacy?
4. **Prompt-integrated fit** — do generated outputs that receive Stage 2 data keep dealer-positioning/raw-options/leverage context secondary in the final prose? If derived `options_gex` is not threaded into prompts, evaluate derived-GEX value from the terminal block only.
5. **No-harm on negative controls** — when the overlay is absent or low-value, does the report stay concise instead of surfacing options bookkeeping?

**Decision criteria (recorded by the writing-plans handoff, per the design):**

- **Proceed to Stage 3** — Stage 2 overlay adds a distinct, non-redundant signal in a strong majority of positive/mainstream cases; remains clearly secondary in terminal and generated prose; reads cleanly for a mainstream audience; and causes no clutter/regression in negative-control cases.
- **Stop after Stage 2** — Stage 2 overlay is usually absent, redundant, or makes the report feel options-specialist. Close the Phase 2 plan here; document the stop reason in the implementation notes and revisit dealer positioning only if downstream evidence motivates it.
- **Partial Stage 3** — if only one contingent addition is justified, schedule only that addition. Broad GEX and VEX/CEX require separate evidence; Stage 2 success is not blanket approval for both.

The decision owner is the user. **The executor must request explicit go/no-go before scheduling any Stage 3 task.**

---

## Stage 3 — Contingent context expansion

> Stage 3 work begins only after the Stage 2 validation gate clears with an explicit proceed decision. If the user says stop after Stage 2, archive this plan and skip the contingent tasks below. Tasks 18-20 are listed later for numbering continuity but are required Stage 2 risk-free-rate work, not contingent Stage 3 work.

Stage 3 has independent subtracks. After the validation gate, schedule only the subtracks approved by the user:

- **Subtrack A — Broad GEX:** Tasks 13-15 plus the broad portion of Task 16 and Task 21. Adds transient `all_expirations`, NTM-per-expiration broad aggregation, and all/partial-expiration reporter output.
- **Subtrack B — Secondary VEX/CEX:** VEX/CEX portions of Task 13, Task 16, and Task 21. Adds near-term secondary sensitivity summaries and reporter output.
- **Subtrack C — Live smokes:** Tasks 22, 24, 25 (and Task 23 if it was deferred from Stage 2). Optional manual evidence after whichever Stage 3 subtracks are approved.

Do not implement Subtrack A just because Subtrack B is approved, or vice versa.

> **Tasks 18-20 are not Stage 3 subtracks.** They are required Stage 2 risk-free-rate work that the validation gate depends on. Their bodies live under the Stage 3 heading for numbering continuity only — they must be complete before the gate runs, regardless of the gate outcome.

### Task 13: Aggregator extensions — broad path + VEX/CEX surfacing

**Files:**
- Modify: `crates/scorpio-core/src/indicators/gex.rs`
- Modify: `crates/scorpio-core/src/state/derived.rs`

> **Broad GEX normalization contract:** `OptionsSnapshot.near_term_strikes` is the authoritative front-month slice. `OptionsSnapshot.all_expirations` stores **non-front-month** expirations only, using the same NTM row-normalization as the front-month slice; it is not a full-chain row dump. Broad GEX therefore means "front-month NTM slice plus NTM slices for additional expirations," not every listed strike. Renderers and prompts must label it as an all-expirations single-rate approximation, not as full listed-chain exposure.

- [ ] **Step 13.1: Write failing tests for broad aggregation**

Append to the `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/indicators/gex.rs`:

```rust
    use crate::data::traits::options::ExpirationStrikes;

    fn extra_expiration(date: &str, rows: Vec<NearTermStrike>) -> ExpirationStrikes {
        ExpirationStrikes {
            expiration: date.to_owned(),
            strikes: rows,
        }
    }

    #[test]
    fn aggregate_broad_combines_front_month_with_additional_expirations() {
        let near = vec![row(100.0, 1_000, 1_000)];
        let extras = vec![
            extra_expiration("2026-07-31", vec![row(100.0, 500, 500)]),
            extra_expiration("2026-08-29", vec![row(100.0, 300, 300)]),
        ];
        let res = aggregate(AggregateInputs {
            spot: 100.0,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: "2026-06-26",
            near_term_strikes: &near,
            expirations: &extras,
            atm_iv_fallback: 0.20,
        });
        let broad = res.broad.expect("broad present");
        assert_eq!(broad.expirations_used, 3, "1 front + 2 extra");
        assert_eq!(broad.expirations_total_considered, 3);
        assert!(broad.gross_gex_usd_per_1pct_move > 0.0);
    }

    #[test]
    fn aggregate_broad_reports_partial_coverage_when_some_expirations_unusable() {
        let near = vec![row(100.0, 1_000, 1_000)];
        let extras = vec![
            // Unparseable expiration date — counted as considered but not used.
            extra_expiration("not-a-date", vec![row(100.0, 100, 100)]),
            // Same-day expiration — t_years <= 0, counted but not used.
            extra_expiration("2026-05-27", vec![row(100.0, 200, 200)]),
            extra_expiration("2026-07-31", vec![row(100.0, 500, 500)]),
        ];
        let res = aggregate(AggregateInputs {
            spot: 100.0,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: "2026-06-26",
            near_term_strikes: &near,
            expirations: &extras,
            atm_iv_fallback: 0.20,
        });
        let broad = res.broad.expect("broad present");
        assert_eq!(broad.expirations_used, 2, "front + one valid extra");
        assert_eq!(broad.expirations_total_considered, 4, "front + 3 extras");
    }

    #[test]
    fn aggregate_broad_is_none_when_no_usable_expirations() {
        let near: Vec<NearTermStrike> = vec![];
        let res = aggregate(AggregateInputs {
            spot: 100.0,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: "2026-06-26",
            near_term_strikes: &near,
            expirations: &[],
            atm_iv_fallback: 0.20,
        });
        assert!(res.near_term.is_none());
        assert!(res.broad.is_none());
    }
```

- [ ] **Step 13.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core indicators::gex::tests`
Expected: the three new broad-aggregation tests fail because `aggregate` currently always emits `broad = None`.

- [ ] **Step 13.3: Implement broad aggregation**

First, extend the Stage 1 aggregator types in `crates/scorpio-core/src/indicators/gex.rs`:

```rust
pub struct AggregateInputs<'a> {
    pub spot: f64,
    pub r: f64,
    pub q: f64,
    pub as_of: chrono::NaiveDate,
    pub near_term_expiration: &'a str,
    pub near_term_strikes: &'a [NearTermStrike],
    pub expirations: &'a [crate::data::traits::options::ExpirationStrikes],
    pub atm_iv_fallback: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregateResult {
    pub near_term: Option<NearTermAggregate>,
    pub broad: Option<BroadAggregate>,
    pub iv_fallback_count: u32,
    pub strikes_total: u32,
    pub strikes_used: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BroadAggregate {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub expirations_used: u32,
    pub expirations_total_considered: u32,
}
```

In `crates/scorpio-core/src/indicators/gex.rs`, replace the `// Stage 3 fills this in. Stage 1/2 always produces broad = None.` block in `aggregate` with:

```rust
    // Broad aggregation. The front-month near-term contribution is added in
    // when present; each additional expiration is parsed, time-validated, and
    // aggregated using the same per-strike helper. The single rate `r` is
    // reused across all expirations — the renderer labels this as a
    // single-rate approximation.
    let mut expirations_total_considered: u32 = 0;
    let mut expirations_used: u32 = 0;
    let mut broad_net_gex: f64 = 0.0;
    let mut broad_gross_gex: f64 = 0.0;

    if let Some(ref nt) = near_term {
        expirations_total_considered = expirations_total_considered.saturating_add(1);
        expirations_used = expirations_used.saturating_add(1);
        broad_net_gex += nt.net_gex_usd_per_1pct_move;
        broad_gross_gex += nt.gross_gex_usd_per_1pct_move;
    }

    for extra in inputs.expirations {
        expirations_total_considered = expirations_total_considered.saturating_add(1);
        let Some(exp) = parse_expiration(&extra.expiration) else {
            continue;
        };
        let t_years = years_until(exp, inputs.as_of);
        if t_years <= 0.0 || extra.strikes.is_empty() {
            continue;
        }
        let mut local_net_gex = 0.0;
        let mut local_gross_gex = 0.0;
        let mut row_used = false;
        for row in &extra.strikes {
            // Reuse the per-strike helper but track only GEX for broad.
            // Broad VEX/CEX are out of scope per the spec.
            let Some(c) = contribution_for_strike(
                inputs.spot,
                inputs.r,
                inputs.q,
                t_years,
                inputs.atm_iv_fallback,
                row,
                &mut iv_fallback_count,
            ) else {
                continue;
            };
            local_net_gex += c.net_gex;
            local_gross_gex += c.gross_gex;
            row_used = true;
        }
        if row_used {
            expirations_used = expirations_used.saturating_add(1);
            broad_net_gex += local_net_gex;
            broad_gross_gex += local_gross_gex;
        }
    }

    let broad = if expirations_used > 0 {
        Some(BroadAggregate {
            net_gex_usd_per_1pct_move: broad_net_gex,
            gross_gex_usd_per_1pct_move: broad_gross_gex,
            expirations_used,
            expirations_total_considered,
        })
    } else {
        None
    };
```

Also add the `ExpirationStrikes` type in `crates/scorpio-core/src/data/traits/options.rs` after `NearTermStrike`:

```rust
/// Per-expiration NTM strike rows for non-front-month expirations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExpirationStrikes {
    /// ISO-8601 expiration date.
    pub expiration: String,
    /// NTM per-strike rows for this expiration, normalized with the same helper
    /// used for `OptionsSnapshot.near_term_strikes`.
    pub strikes: Vec<NearTermStrike>,
}
```

Then extend `crates/scorpio-core/src/state/derived.rs` with the Stage 3 durable broad-GEX state type and field:

```rust
pub struct GexSummary {
    // ... existing fields unchanged ...

    /// Broad dealer-positioning aggregate across NTM slices for all listed
    /// expirations. Populated by Stage 3.
    #[serde(default)]
    pub broad: Option<BroadGex>,
}

/// Broad (all-expirations) GEX aggregate. Single-rate approximation — the
/// renderer/prompt always labels this as such.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BroadGex {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub expirations_used: u32,
    #[serde(default)]
    pub expirations_total_considered: u32,
}
```

Add these serde tests to the `#[cfg(test)] mod tests { ... }` block in `derived.rs`:

```rust
    #[test]
    fn broad_gex_with_partial_expiration_coverage_roundtrips() {
        let val = BroadGex {
            net_gex_usd_per_1pct_move: 5_000_000.0,
            gross_gex_usd_per_1pct_move: 9_000_000.0,
            expirations_used: 3,
            expirations_total_considered: 5,
        };
        let json = serde_json::to_string(&val).expect("serialize");
        let back: BroadGex = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(val, back);
    }

    #[test]
    fn legacy_broad_gex_without_total_considered_defaults_to_zero() {
        let json = r#"{
            "net_gex_usd_per_1pct_move": 0.0,
            "gross_gex_usd_per_1pct_move": 0.0,
            "expirations_used": 0
        }"#;
        let back: BroadGex = serde_json::from_str(json).expect("deserialize");
        assert_eq!(back.expirations_total_considered, 0);
    }
```

- [ ] **Step 13.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core indicators::gex::tests`
Expected: all aggregator tests pass.

- [ ] **Step 13.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 13.6: Commit**

```bash
git add crates/scorpio-core/src/indicators/gex.rs crates/scorpio-core/src/data/traits/options.rs crates/scorpio-core/src/state/derived.rs
git commit -m "feat(indicators): aggregate broad GEX across all listed expirations

Single-rate approximation: reuses the per-strike helper for each additional
expiration in `expirations`, increments `expirations_used` only when the
expiration contributed at least one usable row, and tracks
`expirations_total_considered` for the 'Partial expirations' renderer label."
```

---

### Task 14: `OptionsSnapshot.all_expirations` transient field

**Files:**
- Modify: `crates/scorpio-core/src/data/traits/options.rs`

- [ ] **Step 14.1: Write a failing serde-skip test**

Append to whichever test module exists in `crates/scorpio-core/src/data/traits/options.rs`. If there is no test module, add a `#[cfg(test)] mod tests { ... }` block at the bottom with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> OptionsSnapshot {
        OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes: vec![],
            all_expirations: vec![ExpirationStrikes {
                expiration: "2026-07-31".to_owned(),
                strikes: vec![NearTermStrike {
                    strike: 105.0,
                    call_iv: Some(0.21),
                    put_iv: Some(0.22),
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(100),
                    put_oi: Some(100),
                }],
            }],
        }
    }

    #[test]
    fn all_expirations_is_stripped_on_serialization() {
        let snap = sample_snapshot();
        let json = serde_json::to_string(&snap).expect("serialize");
        assert!(
            !json.contains("all_expirations"),
            "all_expirations must not appear in serialized form: {json}"
        );
    }

    #[test]
    fn all_expirations_defaults_to_empty_on_deserialization() {
        let json = r#"{
            "spot_price": 100.0,
            "atm_iv": 0.2,
            "iv_term_structure": [],
            "put_call_volume_ratio": 1.0,
            "put_call_oi_ratio": 1.0,
            "max_pain_strike": 100.0,
            "near_term_expiration": "2026-06-26",
            "near_term_strikes": []
        }"#;
        let snap: OptionsSnapshot = serde_json::from_str(json).expect("deserialize");
        assert!(snap.all_expirations.is_empty());
    }
}
```

- [ ] **Step 14.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core data::traits::options`
Expected: COMPILE FAILURE (`all_expirations` not defined on `OptionsSnapshot`).

- [ ] **Step 14.3: Add the transient field**

In `crates/scorpio-core/src/data/traits/options.rs`, modify the `OptionsSnapshot` struct to add the transient field:

```rust
pub struct OptionsSnapshot {
    // ... existing fields unchanged ...
    pub near_term_strikes: Vec<NearTermStrike>,

    /// Stage 3 only — per-expiration per-strike rows for listed expirations
    /// beyond the authoritative front-month slice already carried in
    /// `near_term_expiration` / `near_term_strikes`.
    ///
    /// **Derive-don't-persist:** this field is populated by the yfinance
    /// provider during a live run so the ETF valuator can compute broad GEX,
    /// then stripped by `AnalystSyncTask` before `serialize_state_to_context`.
    /// Persisted snapshots therefore never contain these rows; the only
    /// durable artifact is `EtfValuation.options_gex.broad`.
    #[serde(skip, default)]
    pub all_expirations: Vec<ExpirationStrikes>,
}
```

(`ExpirationStrikes` was added in Task 13 Step 13.3, so no new type needed.)

- [ ] **Step 14.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core data::traits::options`
Expected: all tests pass.

- [ ] **Step 14.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean. If clippy fires on call sites that build `OptionsSnapshot` (likely in test fixtures across the workspace), add `all_expirations: Vec::new()` to each constructor.

- [ ] **Step 14.6: Commit**

```bash
git add crates/scorpio-core/src/data/traits/options.rs
git commit -m "feat(options): add transient OptionsSnapshot.all_expirations

#[serde(skip, default)] so the field is populated in-memory by the yfinance
provider and consumed by the ETF valuator within a single run, but stripped
from any persisted form. Bounded BroadGex stays the durable artifact."
```

---

### Task 15: Populate `all_expirations` in the yfinance provider

**Files:**
- Modify: `crates/scorpio-core/src/data/yfinance/options.rs`

- [ ] **Step 15.1: Inspect the existing fetch flow**

Read `crates/scorpio-core/src/data/yfinance/options.rs` to locate the loop that iterates listed expirations to compute the term-structure ATM-IV vector. The same loop is the natural place to capture per-expiration strikes.

```bash
grep -n 'iv_term_structure\|near_term_strikes\|expirations\|fetch_snapshot_impl' crates/scorpio-core/src/data/yfinance/options.rs | head -40
```

- [ ] **Step 15.2: Write a failing in-process test**

Append to the test module in `crates/scorpio-core/src/data/yfinance/options.rs` (or create a `#[cfg(test)] mod tests` block if none exists):

```rust
    #[test]
    fn normalized_snapshot_carries_all_expirations_with_distinct_dates() {
        // Use a fixture from the existing yfinance test harness. If the file
        // exposes a helper like `normalize_chain_from_fixture(fixture_id)`,
        // use it. Otherwise, build a synthetic input that exercises the
        // normalizer's expiration loop. See nearby tests for the canonical
        // pattern.
        let snap = build_test_normalized_snapshot();
        assert!(
            !snap.all_expirations.is_empty(),
            "live in-memory snapshot must populate all_expirations"
        );
        for extra in &snap.all_expirations {
            assert_ne!(
                extra.expiration, snap.near_term_expiration,
                "all_expirations must not include the front-month slice"
            );
        }
    }
```

Adapt `build_test_normalized_snapshot()` to whatever helper the file already uses for constructing a `OptionsSnapshot` from fixture bytes. If no such helper exists, mark the assertion behind `#[ignore = "requires live HTTP"]` and instead add a unit test that exercises the per-expiration capture in isolation by extracting the relevant normalization helper.

- [ ] **Step 15.3: Run the test to verify failure**

Run: `cargo nextest run -p scorpio-core data::yfinance::options`
Expected: failure (`all_expirations` empty).

- [ ] **Step 15.4: Update the normalizer**

In `crates/scorpio-core/src/data/yfinance/options.rs`, find the expiration loop and capture per-expiration strikes for non-front-month expirations. Outline:

```rust
let mut all_expirations: Vec<ExpirationStrikes> = Vec::new();
for (idx, exp) in listed_expirations.iter().enumerate() {
    let chain = fetch_chain_for_expiration(exp).await?;
    // existing ATM-IV / term-structure capture …

    if idx != front_month_idx {
        let rows: Vec<NearTermStrike> = normalize_rows(&chain);
        if !rows.is_empty() {
            all_expirations.push(ExpirationStrikes {
                expiration: exp.iso_date_string(),
                strikes: rows,
            });
        }
    }
}

// existing OptionsSnapshot construction — add `all_expirations` to the literal:
OptionsSnapshot {
    // ... existing fields ...
    near_term_strikes: front_month_rows,
    all_expirations,
}
```

The exact extraction depends on the existing normalizer's structure — preserve the existing front-month path verbatim, then add the additional capture as a side branch. No new HTTP calls; the chain for each expiration is already being fetched today.

- [ ] **Step 15.5: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core data::yfinance::options`
Expected: all tests pass.

- [ ] **Step 15.6: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 15.7: Commit**

```bash
git add crates/scorpio-core/src/data/yfinance/options.rs
git commit -m "feat(yfinance): populate OptionsSnapshot.all_expirations

Reuses the existing per-expiration iteration that already builds the
iv_term_structure vector. Per-expiration strike rows are captured for
non-front-month expirations only. No new HTTP calls."
```

---

### Task 16: Extend `compute_gex_summary` to emit `broad`/`vex_summary`/`cex_summary`

**Files:**
- Modify: `crates/scorpio-core/src/valuation/etf/premium_discount.rs`

- [ ] **Step 16.1: Write failing tests**

Append to the existing `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/valuation/etf/premium_discount.rs`:

```rust
    use crate::data::traits::options::ExpirationStrikes;

    #[test]
    fn compute_gex_summary_emits_broad_when_all_expirations_populated() {
        let mut snap = sample_options_snapshot();
        snap.all_expirations = vec![
            ExpirationStrikes {
                expiration: "2026-07-31".to_owned(),
                strikes: snap.near_term_strikes.clone(),
            },
            ExpirationStrikes {
                expiration: "2026-08-29".to_owned(),
                strikes: snap.near_term_strikes.clone(),
            },
        ];
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary");
        let broad = summary.broad.as_ref().expect("broad populated");
        assert_eq!(broad.expirations_used, 3);
        assert_eq!(broad.expirations_total_considered, 3);
    }

    #[test]
    fn compute_gex_summary_emits_vex_and_cex_summaries() {
        let snap = sample_options_snapshot();
        let summary = compute_gex_summary(
            &snap,
            0.045,
            0.015,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
        )
        .expect("summary");
        let v = summary.vex_summary.as_ref().expect("vex");
        let c = summary.cex_summary.as_ref().expect("cex");
        assert!(v.gross_vex_usd_per_volpt >= v.net_vex_usd_per_volpt.abs());
        assert!(c.gross_cex_usd_per_day >= c.net_cex_usd_per_day.abs());
    }
```

- [ ] **Step 16.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core valuation::etf::premium_discount::tests`
Expected: 2 new tests fail.

- [ ] **Step 16.3: Wire `broad`/`vex`/`cex` through `compute_gex_summary`**

First extend the near-term aggregator with VEX/CEX totals in `crates/scorpio-core/src/indicators/gex.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct NearTermAggregate {
    pub expiration: chrono::NaiveDate,
    pub per_strike: Vec<PerStrikeAggregate>,
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub net_vex_usd_per_volpt: f64,
    pub gross_vex_usd_per_volpt: f64,
    pub net_cex_usd_per_day: f64,
    pub gross_cex_usd_per_day: f64,
}

struct StrikeContribution {
    net_gex: f64,
    gross_gex: f64,
    net_vex: f64,
    gross_vex: f64,
    net_cex: f64,
    gross_cex: f64,
}
```

Inside `contribution_for_strike`, compute the extra Greeks after gamma:

```rust
let vanna_call = bsm_vanna(call_in);
let vanna_put = bsm_vanna(put_in);
let charm_call = bsm_charm_call(call_in);
let charm_put = bsm_charm_put(put_in);

let net_vex = (vanna_call * call_oi - vanna_put * put_oi) * CONTRACT_MULTIPLIER * spot;
let gross_vex =
    ((vanna_call * call_oi).abs() + (vanna_put * put_oi).abs()) * CONTRACT_MULTIPLIER * spot;

let net_cex =
    (charm_call * call_oi - charm_put * put_oi) * CONTRACT_MULTIPLIER * spot / 365.0;
let gross_cex = ((charm_call * call_oi).abs() + (charm_put * put_oi).abs())
    * CONTRACT_MULTIPLIER
    * spot
    / 365.0;
```

Accumulate those fields in `aggregate` alongside `net_gex` and `gross_gex`, then set the four new `NearTermAggregate` fields when constructing `Some(NearTermAggregate { ... })`.

Then extend `crates/scorpio-core/src/state/derived.rs` with secondary-sensitivity state:

```rust
pub struct GexSummary {
    // ... existing fields unchanged ...

    /// Secondary sensitivity: dealer exposure to absolute IV moves.
    /// Populated by Stage 3.
    #[serde(default)]
    pub vex_summary: Option<VexSummary>,

    /// Secondary sensitivity: dealer exposure to one day of time decay.
    /// Populated by Stage 3.
    #[serde(default)]
    pub cex_summary: Option<CexSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VexSummary {
    /// Per 1.0 vol-point change (callers typically divide by 100 at display
    /// time to express "per 1% absolute IV move").
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

Replace the construction of `GexSummary` at the bottom of `compute_gex_summary` (in `crates/scorpio-core/src/valuation/etf/premium_discount.rs`) with:

```rust
    let broad = agg.broad.as_ref().map(|b| crate::state::BroadGex {
        net_gex_usd_per_1pct_move: b.net_gex_usd_per_1pct_move,
        gross_gex_usd_per_1pct_move: b.gross_gex_usd_per_1pct_move,
        expirations_used: b.expirations_used,
        expirations_total_considered: b.expirations_total_considered,
    });

    let vex_summary = Some(crate::state::VexSummary {
        net_vex_usd_per_volpt: near.net_vex_usd_per_volpt,
        gross_vex_usd_per_volpt: near.gross_vex_usd_per_volpt,
    });

    let cex_summary = Some(crate::state::CexSummary {
        net_cex_usd_per_day: near.net_cex_usd_per_day,
        gross_cex_usd_per_day: near.gross_cex_usd_per_day,
    });

    Some(GexSummary {
        net_gex_usd_per_1pct_move: near.net_gex_usd_per_1pct_move,
        gross_gex_usd_per_1pct_move: near.gross_gex_usd_per_1pct_move,
        call_put_oi_ratio,
        max_pain_strike: snapshot.max_pain_strike,
        near_term_expiration: near.expiration,
        strikes: walls,
        broad,
        vex_summary,
        cex_summary,
    })
```

Also update the `AggregateInputs` call to pass `&snapshot.all_expirations` instead of the empty slice:

```rust
    let agg = gex::aggregate(AggregateInputs {
        // ...
        expirations: &snapshot.all_expirations,
        // ...
    });
```

- [ ] **Step 16.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core valuation::etf::premium_discount::tests`
Expected: all tests pass.

- [ ] **Step 16.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 16.6: Commit**

```bash
git add crates/scorpio-core/src/indicators/gex.rs crates/scorpio-core/src/state/derived.rs crates/scorpio-core/src/valuation/etf/premium_discount.rs
git commit -m "feat(valuation): emit broad/vex_summary/cex_summary on GexSummary

Stage 3 only — broad GEX comes from the aggregator's broad path over
all_expirations; VEX/CEX summaries surface the near-term aggregator's
existing sums."
```

---

### Task 17: AnalystSyncTask strips `all_expirations` before serialization

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`

- [ ] **Step 17.1: Write a failing test**

Append to the existing `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/workflow/tasks/analyst.rs`:

```rust
    #[test]
    fn strip_all_expirations_clears_transient_field_in_place() {
        use crate::data::traits::options::{
            ExpirationStrikes, IvTermPoint, NearTermStrike, OptionsOutcome, OptionsSnapshot,
        };
        use crate::state::{TechnicalData, TechnicalOptionsContext};

        let snap = OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes: vec![],
            all_expirations: vec![ExpirationStrikes {
                expiration: "2026-07-31".to_owned(),
                strikes: vec![NearTermStrike {
                    strike: 105.0,
                    call_iv: Some(0.21),
                    put_iv: None,
                    call_volume: None,
                    put_volume: None,
                    call_oi: Some(100),
                    put_oi: None,
                }],
            }],
        };

        let mut state = crate::testing::with_baseline_runtime_policy_state(
            "SPY".to_owned(),
            "2026-05-27".to_owned(),
        );
        state.set_technical_indicators(TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "smoke".to_owned(),
            options_summary: None,
            options_context: Some(TechnicalOptionsContext::Available {
                outcome: OptionsOutcome::Snapshot(snap),
            }),
        });

        strip_transient_all_expirations(&mut state);

        let context = state.technical_indicators.as_ref().unwrap().options_context.as_ref().unwrap();
        match context {
            TechnicalOptionsContext::Available {
                outcome: OptionsOutcome::Snapshot(s),
            } => {
                assert!(s.all_expirations.is_empty(), "all_expirations must be cleared in place");
            }
            other => panic!("expected Available+Snapshot, got {other:?}"),
        }
    }
```

- [ ] **Step 17.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core workflow::tasks::analyst`
Expected: COMPILE FAILURE (`strip_transient_all_expirations` not defined).

- [ ] **Step 17.3: Implement the strip helper and wire into the analyst task**

In `crates/scorpio-core/src/workflow/tasks/analyst.rs`, add the helper at module scope:

```rust
/// Stage 3 cleanup: clear the transient `OptionsSnapshot.all_expirations`
/// vector in place so persisted technical snapshots keep only the bounded
/// `EtfValuation.options_gex.broad` summary, not the full non-front-month
/// chain payload.
///
/// Called immediately before `serialize_state_to_context(...)` on the ETF
/// branch; no-op for any other technical context shape.
pub(crate) fn strip_transient_all_expirations(state: &mut crate::state::TradingState) {
    use crate::data::traits::options::OptionsOutcome;
    use crate::state::TechnicalOptionsContext;

    let Some(technical) = state.technical_indicators_mut() else {
        return;
    };
    let Some(options_context) = technical.options_context.as_mut() else {
        return;
    };
    if let TechnicalOptionsContext::Available {
        outcome: OptionsOutcome::Snapshot(snap),
    } = options_context
    {
        snap.all_expirations.clear();
    }
}
```

Add a `pub(crate)` mutable accessor on `TradingState` before using the helper:

```rust
pub(crate) fn technical_indicators_mut(&mut self) -> Option<&mut TechnicalData> {
    self.equity.as_mut()?.technical_indicators.as_mut()
}
```

Search for the existing immutable getter to place it nearby:

```bash
grep -n 'fn technical_indicators\|fn set_technical_indicators' crates/scorpio-core/src/state/trading_state.rs
```

Then call `strip_transient_all_expirations(&mut state)` from the ETF branch of the analyst task, **immediately before** the call to `serialize_state_to_context(...)`. Locate the serialization site:

```bash
grep -n 'serialize_state_to_context' crates/scorpio-core/src/workflow/tasks/analyst.rs
```

Wrap the cleanup behind the `pack_id == PackId::EtfBaseline` guard so other packs are unaffected.

- [ ] **Step 17.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core workflow::tasks::analyst`
Expected: all tests pass.

- [ ] **Step 17.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 17.6: Commit**

```bash
git add crates/scorpio-core/src/workflow/tasks/analyst.rs crates/scorpio-core/src/state/trading_state.rs
git commit -m "feat(workflow): strip transient all_expirations before snapshot serialize

Last ETF-only cleanup step on AnalystSyncTask so persisted technical
snapshots keep only the bounded EtfValuation.options_gex.broad summary.
The live run still derives broad GEX from the in-memory rows."
```

---

### Task 18: `TradingState` gains `etf_risk_free_rate` + source

**Files:**
- Modify: `crates/scorpio-core/src/state/trading_state.rs`
- Modify: `crates/scorpio-core/src/state/mod.rs`

- [ ] **Step 18.1: Write a failing roundtrip test**

Append to `crates/scorpio-core/tests/state_roundtrip.rs`:

```rust
#[test]
fn trading_state_etf_risk_free_rate_fields_roundtrip_with_serde_default() {
    use scorpio_core::state::{EtfRiskFreeRateSource, TradingState};

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = Some(0.0427);
    state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FredDgs3Mo);

    let json = serde_json::to_string(&state).expect("serialize");
    let back: TradingState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.etf_risk_free_rate, Some(0.0427));
    assert_eq!(
        back.etf_risk_free_rate_source,
        Some(EtfRiskFreeRateSource::FredDgs3Mo)
    );
}

#[test]
fn legacy_trading_state_without_etf_risk_free_rate_fields_deserializes() {
    // Snapshot from before Stage 3: TradingStateWire must accept missing fields.
    let json = serde_json::json!({
        "execution_id": "00000000-0000-0000-0000-000000000000",
        "asset_symbol": "SPY",
        "target_date": "2026-05-27"
    });
    let back: scorpio_core::state::TradingState =
        serde_json::from_value(json).expect("legacy snapshot must deserialize");
    assert!(back.etf_risk_free_rate.is_none());
    assert!(back.etf_risk_free_rate_source.is_none());
}
```

- [ ] **Step 18.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core --test state_roundtrip`
Expected: COMPILE FAILURE (`EtfRiskFreeRateSource`, `etf_risk_free_rate` fields not defined).

- [ ] **Step 18.3: Add the enum and `TradingState` fields**

In `crates/scorpio-core/src/state/trading_state.rs`:

1. Add the source enum near the other state-supporting enums (search `grep -n '^pub enum' crates/scorpio-core/src/state/trading_state.rs` for a good neighbour, or add at the bottom of the file):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EtfRiskFreeRateSource {
    FredDgs3Mo,
    YFinanceIrx,
}
```

2. Add the two new fields to `TradingState`:

```rust
pub struct TradingState {
    // ... existing fields ...

    /// Stage 2 — risk-free rate (decimal fraction, e.g. 0.0427) sourced from
    /// FRED DGS3MO at preflight when the active pack is `EtfBaseline`, or from
    /// the most recent yfinance `^IRX` close when FRED is unavailable.
    /// `None` when pack != EtfBaseline OR when both live rate sources fail. The
    /// ETF valuator must degrade dealer-positioning to unavailable when `None`.
    #[serde(default)]
    pub etf_risk_free_rate: Option<f64>,

    /// Stage 2 — Persisted origin of the ETF risk-free-rate input so live
    /// runs and `scorpio report` render the same source/degradation banner from
    /// reloaded snapshots.
    #[serde(default)]
    pub etf_risk_free_rate_source: Option<EtfRiskFreeRateSource>,
}
```

3. Add matching `#[serde(default)]` fields to `TradingStateWire` and propagate them in `impl From<TradingStateWire> for TradingState`. Inspect the wire shape:

```bash
grep -n 'struct TradingStateWire\|impl From<TradingStateWire>' crates/scorpio-core/src/state/trading_state.rs
```

4. Update `TradingState::new` to initialise both fields to `None`.

5. Add `pub use trading_state::EtfRiskFreeRateSource;` to `crates/scorpio-core/src/state/mod.rs` (find the existing `pub use trading_state::...` line).

- [ ] **Step 18.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core --test state_roundtrip`
Expected: both new tests pass; all previously-passing tests still pass.

- [ ] **Step 18.5: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 18.6: Commit**

```bash
git add crates/scorpio-core/src/state/trading_state.rs crates/scorpio-core/src/state/mod.rs crates/scorpio-core/tests/state_roundtrip.rs
git commit -m "feat(state): add etf_risk_free_rate + source on TradingState

Additive: both fields are #[serde(default)] so legacy snapshots without
them deserialize unchanged. The source enum lets scorpio report distinguish
FRED DGS3MO from yfinance ^IRX and no-rate degradation after reload."
```

---

### Task 19: Thread live risk-free-rate providers into `PreflightTask`

**Files:**
- Modify: `crates/scorpio-core/src/workflow/builder.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/preflight.rs`

- [ ] **Step 19.1: Write a failing structure test**

Append to `crates/scorpio-core/tests/workflow_pipeline_structure.rs`:

```rust
#[tokio::test]
async fn preflight_fetches_dgs3mo_for_etf_pack_and_persists_source() {
    use scorpio_core::state::{EtfRiskFreeRateSource, TradingState};
    use scorpio_core::testing::{with_fake_fred_client, with_fake_yfinance_client};

    let fred = with_fake_fred_client(|series| match series {
        "DGS3MO" => Ok(Some(4.27)), // FRED returns percent
        _ => Ok(None),
    });
    let yfinance = with_fake_yfinance_client(|symbol| {
        panic!("^IRX fallback must not be called when FRED succeeds, got symbol={symbol}")
    });

    let mut state = TradingState::new(
        "SPY".to_owned(),
        chrono::Utc::now().date_naive().to_string(),
    );
    scorpio_core::workflow::tasks::preflight::run_for_test(
        &mut state,
        scorpio_core::analysis_packs::PackId::EtfBaseline,
        &fred,
        &yfinance,
    )
    .await
    .expect("preflight must succeed");

    assert_eq!(state.etf_risk_free_rate, Some(0.0427));
    assert_eq!(
        state.etf_risk_free_rate_source,
        Some(EtfRiskFreeRateSource::FredDgs3Mo)
    );
}

#[tokio::test]
async fn preflight_skips_dgs3mo_for_historical_etf_pack() {
    use scorpio_core::state::TradingState;
    use scorpio_core::testing::{with_fake_fred_client, with_fake_yfinance_client};

    let fred = with_fake_fred_client(|series| {
        panic!("historical ETF run must not call latest FRED, but got series={series}")
    });
    let yfinance = with_fake_yfinance_client(|symbol| {
        panic!("historical ETF run must not call latest ^IRX, but got symbol={symbol}")
    });

    let mut state = TradingState::new("SPY".to_owned(), "2026-01-01".to_owned());
    scorpio_core::workflow::tasks::preflight::run_for_test(
        &mut state,
        scorpio_core::analysis_packs::PackId::EtfBaseline,
        &fred,
        &yfinance,
    )
    .await
    .expect("preflight must succeed");

    assert!(state.etf_risk_free_rate.is_none());
    assert!(state.etf_risk_free_rate_source.is_none());
}

#[tokio::test]
async fn preflight_skips_dgs3mo_for_non_etf_pack() {
    use scorpio_core::state::TradingState;
    use scorpio_core::testing::{with_fake_fred_client, with_fake_yfinance_client};

    let fred = with_fake_fred_client(|series| {
        panic!("non-ETF pack must not call FRED, but got series={series}")
    });
    let yfinance = with_fake_yfinance_client(|symbol| {
        panic!("non-ETF pack must not call ^IRX, but got symbol={symbol}")
    });

    let mut state = TradingState::new("AAPL".to_owned(), "2026-05-27".to_owned());
    scorpio_core::workflow::tasks::preflight::run_for_test(
        &mut state,
        scorpio_core::analysis_packs::PackId::Baseline,
        &fred,
        &yfinance,
    )
    .await
    .expect("preflight must succeed");

    assert!(state.etf_risk_free_rate.is_none());
    assert!(state.etf_risk_free_rate_source.is_none());
}

#[tokio::test]
async fn preflight_falls_back_to_yfinance_irx_when_fred_returns_empty() {
    use scorpio_core::state::{EtfRiskFreeRateSource, TradingState};
    use scorpio_core::testing::{with_fake_fred_client, with_fake_yfinance_client};

    let fred = with_fake_fred_client(|_| Ok(None));
    let yfinance = with_fake_yfinance_client(|symbol| match symbol {
        "^IRX" => Ok(Some(4.33)), // ^IRX close is quoted in percent
        _ => Ok(None),
    });

    let mut state = TradingState::new(
        "SPY".to_owned(),
        chrono::Utc::now().date_naive().to_string(),
    );
    scorpio_core::workflow::tasks::preflight::run_for_test(
        &mut state,
        scorpio_core::analysis_packs::PackId::EtfBaseline,
        &fred,
        &yfinance,
    )
    .await
    .expect("preflight must succeed");

    assert_eq!(state.etf_risk_free_rate, Some(0.0433));
    assert_eq!(
        state.etf_risk_free_rate_source,
        Some(EtfRiskFreeRateSource::YFinanceIrx)
    );
}

#[tokio::test]
async fn preflight_degrades_rate_when_fred_and_yfinance_fail() {
    use scorpio_core::state::TradingState;
    use scorpio_core::testing::{with_fake_fred_client, with_fake_yfinance_client};

    let fred = with_fake_fred_client(|_| Ok(None));
    let yfinance = with_fake_yfinance_client(|_| Ok(None));

    let mut state = TradingState::new(
        "SPY".to_owned(),
        chrono::Utc::now().date_naive().to_string(),
    );
    scorpio_core::workflow::tasks::preflight::run_for_test(
        &mut state,
        scorpio_core::analysis_packs::PackId::EtfBaseline,
        &fred,
        &yfinance,
    )
    .await
    .expect("preflight must succeed");

    assert!(state.etf_risk_free_rate.is_none());
    assert!(state.etf_risk_free_rate_source.is_none());
}
```

`with_fake_fred_client`, `with_fake_yfinance_client`, and `preflight::run_for_test` are new test shims that you will add in Step 19.3. Their signatures appear in the assertions above.

- [ ] **Step 19.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core --test workflow_pipeline_structure`
Expected: COMPILE FAILURE — both test helpers and the new code path are missing.

- [ ] **Step 19.3: Thread FRED + yfinance risk-free-rate providers and add test shims**

Open `crates/scorpio-core/src/workflow/builder.rs`. It already constructs `FredClient` and `YFinanceClient` in `PipelineDeps`. Pass both clients into `PreflightTask` by adding constructor parameters:

```rust
// crates/scorpio-core/src/workflow/builder.rs (sketch)
let preflight = PreflightTask::with_runtime_policy_and_rate_clients(
    enrichment.clone(),
    transcripts_enabled,
    snapshot_store.clone(),
    runtime_policy.clone(),
    fred_client.clone(),
    yfinance_client.clone(),
);
```

In `crates/scorpio-core/src/workflow/tasks/preflight.rs`:

1. Add `fred: Arc<dyn FredSeriesClient>` and `yfinance: Arc<dyn RiskFreeRateYFinanceClient>` to the `PreflightTask` struct. Provide a constructor `with_runtime_policy_and_rate_clients(...)` that takes the runtime policy plus both clients.

Use small async traits so tests can inject fakes without wrapping concrete clients:

```rust
#[async_trait::async_trait]
pub(crate) trait FredSeriesClient: Send + Sync {
    async fn get_series_latest(&self, series_id: &str) -> Result<Option<f64>, TradingError>;
}

#[async_trait::async_trait]
impl FredSeriesClient for crate::data::FredClient {
    async fn get_series_latest(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
        crate::data::FredClient::get_series_latest(self, series_id).await
    }
}

#[async_trait::async_trait]
pub(crate) trait RiskFreeRateYFinanceClient: Send + Sync {
    /// Return the latest close as an annualized treasury yield in percent
    /// units. Implemented for `^IRX` today; the trait is rate-specific to
    /// avoid implying that all yfinance OHLCV close values are percent.
    async fn latest_risk_free_rate_pct(
        &self,
        symbol: &str,
    ) -> Result<Option<f64>, TradingError>;
}

#[async_trait::async_trait]
impl RiskFreeRateYFinanceClient for crate::data::YFinanceClient {
    async fn latest_risk_free_rate_pct(
        &self,
        symbol: &str,
    ) -> Result<Option<f64>, TradingError> {
        let today = chrono::Utc::now().date_naive();
        let start = today - chrono::Duration::days(14);
        let candles = self
            .get_ohlcv(symbol, &start.to_string(), &today.to_string())
            .await?;
        Ok(candles.last().map(|c| c.close))
    }
}
```

2. In the `Task::run` body, after the resolved-pack classification but before `serialize_state_to_context`, add the live/today ETF risk-free-rate fetch. FRED is authoritative; yfinance `^IRX` is the only fallback. If both fail, leave both fields `None` so downstream GEX degrades cleanly:

```rust
// `is_today` is anchored to UTC: `chrono::Utc::now().date_naive()` flips at
// 00:00 UTC, so late-evening Pacific/Asia runs may see "tomorrow" before the
// local calendar does. This is acceptable for live-rate gating because both
// FRED and yfinance publish on US trading-session timestamps; it does mean
// off-hours operators outside US/Eastern may observe one extra historical-run
// degradation per day. If we later care about local-time anchoring, source
// the gate from a single `state.run_started_at` instead of `now()`.
let is_today = state.target_date == chrono::Utc::now().date_naive().to_string();
if matches!(resolved_pack, PackId::EtfBaseline) && is_today {
    if let Ok(Some(pct)) = self.fred.get_series_latest("DGS3MO").await {
        state.etf_risk_free_rate = Some(pct / 100.0);
        state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FredDgs3Mo);
    } else if let Ok(Some(pct)) = self.yfinance.latest_risk_free_rate_pct("^IRX").await {
        state.etf_risk_free_rate = Some(pct / 100.0);
        state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::YFinanceIrx);
    } else {
        tracing::warn!(
            target: "scorpio_core::workflow::preflight",
            fred_series = "DGS3MO",
            yfinance_symbol = "^IRX",
            "ETF risk-free rate unavailable — dealer positioning will degrade"
        );
        state.etf_risk_free_rate = None;
        state.etf_risk_free_rate_source = None;
    }
}
```

For historical ETF runs, skip latest-rate fetches so rerunning the same symbol/date remains reproducible. If a later task needs historical risk-free rates, add as-of-date FRED/yfinance queries and persist the observation date with the source metadata.

3. Add a `#[cfg(any(test, feature = "test-helpers"))]` test-shim function `run_for_test(state: &mut TradingState, pack_id: PackId, fred: &dyn FredSeriesClient, yfinance: &dyn RiskFreeRateYFinanceClient)` that exercises the risk-free-rate logic in isolation against injectable clients. Place it next to `PreflightTask::with_runtime_policy_and_rate_clients` in `crates/scorpio-core/src/workflow/tasks/preflight.rs`.

4. Add `with_fake_fred_client(...)` and `with_fake_yfinance_client(...)` to `crates/scorpio-core/src/testing/runtime_policy.rs` (or new files exposed from `mod.rs`). Use function-pointer-backed wrappers that implement the two traits above.

- [ ] **Step 19.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core --test workflow_pipeline_structure`
Expected: all five new tests pass.

- [ ] **Step 19.5: Run the full workspace test suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: green.

- [ ] **Step 19.6: Lint and format**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 19.7: Commit**

```bash
git add crates/scorpio-core/src/workflow/builder.rs crates/scorpio-core/src/workflow/tasks/preflight.rs crates/scorpio-core/src/testing crates/scorpio-core/tests/workflow_pipeline_structure.rs
git commit -m "feat(preflight): fetch live ETF risk-free rate with IRX fallback

Gated on resolved_pack == EtfBaseline and target_date == today so non-ETF
and historical runs do not consume rate-provider quota or leak present-day
rates into old analyses. FRED DGS3MO is authoritative; yfinance ^IRX latest
close is the only fallback. If both fail, the rate remains None and downstream
dealer-positioning degrades to unavailable."
```

---

### Task 20: Read `etf_risk_free_rate` into `ValuationInputs`

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`

- [ ] **Step 20.1: Write a failing test**

Append to the existing `#[cfg(test)] mod tests { ... }` block in `crates/scorpio-core/src/workflow/tasks/analyst.rs`:

```rust
    #[test]
    fn etf_valuation_inputs_thread_etf_risk_free_rate_from_state() {
        let mut state = crate::testing::with_baseline_runtime_policy_state(
            "SPY".to_owned(),
            "2026-05-27".to_owned(),
        );
        state.etf_risk_free_rate = Some(0.0427);

        let inputs_rate = etf_risk_free_rate_from_state(&state);
        assert_eq!(inputs_rate, Some(0.0427));

        state.etf_risk_free_rate = None;
        let inputs_rate_none = etf_risk_free_rate_from_state(&state);
        assert!(inputs_rate_none.is_none());
    }
```

- [ ] **Step 20.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-core workflow::tasks::analyst`
Expected: COMPILE FAILURE (`etf_risk_free_rate_from_state` not defined).

- [ ] **Step 20.3: Add the accessor and enforce no-rate degradation**

In `crates/scorpio-core/src/valuation/mod.rs`, add the Stage 2 risk-free-rate carrier immediately after `etf_options`:

```rust
    /// Phase 2 (Stage 3) — FRED `DGS3MO` snapshot threaded from preflight when
    /// the active pack is `EtfBaseline`, or yfinance `^IRX` when FRED is
    /// unavailable. `None` when pack != EtfBaseline OR when both live rate
    /// sources failed. The ETF valuator must degrade dealer-positioning to
    /// `None`; no hardcoded risk-free-rate fallback is allowed.
    pub etf_risk_free_rate: Option<f64>,
```

In `crates/scorpio-core/src/workflow/tasks/analyst.rs`, add:

```rust
pub(crate) fn etf_risk_free_rate_from_state(state: &crate::state::TradingState) -> Option<f64> {
    state.etf_risk_free_rate
}
```

Then update every existing call site that constructs `ValuationInputs` to populate the new field:

```bash
grep -rn 'ValuationInputs {' crates/scorpio-core/src/ crates/scorpio-core/tests/
```

Use `etf_risk_free_rate: None` for non-state-aware or non-ETF constructors. At the state-aware `crate::valuation::ValuationInputs` construction site modified in Task 5 Step 5.3, add `etf_risk_free_rate: etf_risk_free_rate_from_state(state)`.

Finally ensure `crates/scorpio-core/src/valuation/etf/premium_discount.rs` contains no `RISK_FREE_RATE_FALLBACK`, no `0.045` production fallback, and degrades `options_gex` when the carrier is unavailable:

```rust
let options_gex = match (inputs.etf_options, inputs.etf_risk_free_rate) {
    (Some(snap), Some(r)) => compute_gex_summary(snap, r, q, inputs.as_of),
    (Some(_), None) => {
        tracing::warn!(
            target: "scorpio_core::valuation::etf::gex",
            "ETF dealer-positioning skipped — risk-free rate unavailable"
        );
        None
    }
    (None, _) => None,
};
```

- [ ] **Step 20.4: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core workflow::tasks::analyst`
Expected: all tests pass.

- [ ] **Step 20.5: Lint and format**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 20.6: Commit**

```bash
git add crates/scorpio-core/src/valuation/mod.rs crates/scorpio-core/src/valuation/etf/premium_discount.rs crates/scorpio-core/src/workflow/tasks/analyst.rs
git commit -m "feat(workflow): thread state.etf_risk_free_rate into ValuationInputs

The valuator never substitutes a hardcoded risk-free rate. ETF baseline runs
consume FRED DGS3MO or yfinance ^IRX when preflight succeeded and degrade
dealer-positioning to unavailable when both live sources fail."
```

---

### Task 21: Reporter Stage 3 expansion (secondary sensitivities, broad GEX line)

**Files:**
- Modify: `crates/scorpio-reporters/src/terminal/etf.rs`
- Modify: `crates/scorpio-reporters/tests/terminal.rs`

- [ ] **Step 21.1: Write failing assertions**

Append to `crates/scorpio-reporters/tests/terminal.rs`:

```rust
#[test]
fn etf_terminal_renders_full_dealer_positioning_with_broad_and_secondary() {
    use scorpio_core::state::{
        AssetShape, BroadGex, CexSummary, DerivedValuation, EtfDataAvailability, EtfValuation,
        GexSummary, PremiumBand, PremiumSnapshot, ScenarioValuation, StrikeGex, TradingState,
        VexSummary,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: Some(620.39),
                ask: Some(620.41),
                premium_pct: Some(0.06),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 2.84e9,
                gross_gex_usd_per_1pct_move: 7.12e9,
                call_put_oi_ratio: 1.31,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![StrikeGex {
                    strike: 625.0,
                    net_gex_usd_per_1pct_move: 1.2e9,
                }],
                broad: Some(BroadGex {
                    net_gex_usd_per_1pct_move: 8.4e9,
                    gross_gex_usd_per_1pct_move: 22.1e9,
                    expirations_used: 5,
                    expirations_total_considered: 5,
                }),
                vex_summary: Some(VexSummary {
                    net_vex_usd_per_volpt: -1.2e9,
                    gross_vex_usd_per_volpt: 4.1e9,
                }),
                cex_summary: Some(CexSummary {
                    net_cex_usd_per_day: 0.45e9,
                    gross_cex_usd_per_day: 2.3e9,
                }),
            }),
            category: None,
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("Secondary sensitivities"));
    assert!(rendered.contains("Net VEX/volpt"));
    assert!(rendered.contains("Net CEX/day"));
    assert!(rendered.contains("All expirations  (5 used)"));
}

#[test]
fn etf_terminal_uses_partial_expirations_label_when_not_all_used() {
    use scorpio_core::state::{
        AssetShape, BroadGex, DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary,
        PremiumBand, PremiumSnapshot, ScenarioValuation, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 1.0e9,
                gross_gex_usd_per_1pct_move: 2.0e9,
                call_put_oi_ratio: 1.0,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![],
                broad: Some(BroadGex {
                    net_gex_usd_per_1pct_move: 3.0e9,
                    gross_gex_usd_per_1pct_move: 5.0e9,
                    expirations_used: 3,
                    expirations_total_considered: 5,
                }),
                vex_summary: None,
                cex_summary: None,
            }),
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("Partial expirations"));
    assert!(rendered.contains("3 used of 5"));
}

```

The risk-free-rate banner tests live in Task 11 (Stage 2), not here. Stage 3 only extends the dealer-positioning block with secondary sensitivities and the broad-GEX/partial-expirations sub-block.

- [ ] **Step 21.2: Run the tests to verify failure**

Run: `cargo nextest run -p scorpio-reporters --test terminal`
Expected: two new tests fail (the two added in this step).

- [ ] **Step 21.3: Extend the dealer-positioning block**

In `crates/scorpio-reporters/src/terminal/etf.rs`, extend `render_dealer_positioning_block` to emit the secondary-sensitivities and broad-GEX lines:

```rust
    if let (Some(v), Some(c)) = (gex.vex_summary.as_ref(), gex.cex_summary.as_ref()) {
        let _ = writeln!(out, "    Secondary sensitivities");
        let _ = writeln!(
            out,
            "      Net VEX/volpt {nv}    Gross VEX       {gv}",
            nv = format_usd_signed(v.net_vex_usd_per_volpt),
            gv = format_usd_magnitude(v.gross_vex_usd_per_volpt),
        );
        let _ = writeln!(
            out,
            "      Net CEX/day   {nc}    Gross CEX       {gc}",
            nc = format_usd_signed(c.net_cex_usd_per_day),
            gc = format_usd_magnitude(c.gross_cex_usd_per_day),
        );
    }

    if let Some(broad) = gex.broad.as_ref() {
        let _ = writeln!(out);
        if broad.expirations_used == broad.expirations_total_considered {
            let _ = writeln!(out, "  All expirations  ({} used)", broad.expirations_used);
        } else {
            let _ = writeln!(
                out,
                "  Partial expirations  ({} used of {})",
                broad.expirations_used, broad.expirations_total_considered
            );
        }
        let _ = writeln!(
            out,
            "    Net GEX/1%      {net}    Gross GEX/1%    {gross}",
            net = format_usd_signed(broad.net_gex_usd_per_1pct_move),
            gross = format_usd_magnitude(broad.gross_gex_usd_per_1pct_move),
        );
    }
```

Also update the partial-data branch from Task 11 to cover the combined case per spec:

```rust
    let walls_missing = gex.strikes.is_empty();
    let broad_missing = gex.broad.is_none();
    if walls_missing && broad_missing {
        let _ = writeln!(
            out,
            "    Dealer positioning partial — gamma walls and broad GEX unavailable"
        );
    } else if walls_missing {
        let _ = writeln!(out, "    Dealer positioning partial — gamma walls unavailable");
    } else if broad_missing {
        let _ = writeln!(out, "    Dealer positioning partial — broad GEX unavailable");
    }
```

- [ ] **Step 21.4: Run the reporter tests**

Run: `cargo nextest run -p scorpio-reporters --test terminal`
Expected: all new tests pass plus previously-passing tests still green.

- [ ] **Step 21.5: Lint and format**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 21.6: Commit**

```bash
git add crates/scorpio-reporters/src/terminal/etf.rs crates/scorpio-reporters/tests/terminal.rs
git commit -m "feat(reporter): Stage 3 dealer-positioning expansion

Adds Secondary sensitivities (VEX/CEX) and All expirations / Partial
expirations sub-blocks. The risk-free-rate source banner shipped with
Stage 2 (Task 11)."
```

---

### Task 22: Live FRED smoke — extend `fred_live_test.rs` with DGS3MO

**Files:**
- Modify: `crates/scorpio-core/examples/fred_live_test.rs`

- [ ] **Step 22.1: Locate the existing FEDFUNDS / CPI assertions**

```bash
grep -n 'FEDFUNDS\|CPALTT01\|get_series_latest' crates/scorpio-core/examples/fred_live_test.rs
```

- [ ] **Step 22.2: Add the DGS3MO assertion**

Append a new block near the existing FEDFUNDS/CPI assertions:

```rust
    let dgs3mo = fred
        .get_series_latest("DGS3MO")
        .await
        .expect("DGS3MO fetch")
        .expect("DGS3MO observation present");
    println!("DGS3MO latest observation: {dgs3mo}");
    assert!(
        (0.0..=20.0).contains(&dgs3mo),
        "DGS3MO observation must be in a plausible percent range: {dgs3mo}"
    );
```

- [ ] **Step 22.3: Run the smoke manually**

This is not part of CI. Confirm locally:

```bash
SCORPIO_FRED_API_KEY=... cargo run -p scorpio-core --example fred_live_test
```

Expected: the example prints all three series and exits 0.

- [ ] **Step 22.4: Commit**

```bash
git add crates/scorpio-core/examples/fred_live_test.rs
git commit -m "test(smoke): assert DGS3MO is fetchable in the live FRED smoke"
```

---

### Task 23: Live yfinance smoke — extend `yfinance_live_test.rs` with `^IRX`

**Files:**
- Modify: `crates/scorpio-core/examples/yfinance_live_test.rs`

- [ ] **Step 23.1: Locate the existing OHLCV assertions**

```bash
grep -n 'get_ohlcv\|YFinanceClient\|AAPL\|SPY' crates/scorpio-core/examples/yfinance_live_test.rs
```

- [ ] **Step 23.2: Add the `^IRX` latest-close assertion**

Append a new block near the existing OHLCV smoke assertions:

```rust
    let today = chrono::Utc::now().date_naive();
    let start = today - chrono::Duration::days(14);
    let irx = yf
        .get_ohlcv("^IRX", &start.to_string(), &today.to_string())
        .await
        .expect("^IRX OHLCV fetch");
    let latest_irx_close = irx
        .last()
        .map(|c| c.close)
        .expect("^IRX must return at least one recent daily candle");
    println!("^IRX latest close: {latest_irx_close}");
    assert!(
        (0.0..=20.0).contains(&latest_irx_close),
        "^IRX close must be a plausible annualized percent rate: {latest_irx_close}"
    );
```

Use the existing client variable name in `yfinance_live_test.rs`; if it is not `yf`, replace `yf` in the snippet with the local name already used by the smoke.

- [ ] **Step 23.3: Run locally**

```bash
cargo run -p scorpio-core --example yfinance_live_test
```

Expected: the example prints the existing yfinance smoke output plus `^IRX latest close: ...` and exits 0.

- [ ] **Step 23.4: Commit**

```bash
git add crates/scorpio-core/examples/yfinance_live_test.rs
git commit -m "test(smoke): assert yfinance can fetch ^IRX latest close"
```

---

### Task 24: New live smoke — `yfinance_options_chain_live_test.rs`

**Files:**
- Create: `crates/scorpio-core/examples/yfinance_options_chain_live_test.rs`

- [ ] **Step 24.1: Author the smoke**

Use the existing `yfinance_live_test.rs` and `etf_quote_live_test.rs` as templates. Create `crates/scorpio-core/examples/yfinance_options_chain_live_test.rs`:

```rust
//! Live smoke: yfinance options chain populates `OptionsSnapshot.all_expirations`.
//!
//! Not part of CI. Run with:
//!     cargo run -p scorpio-core --example yfinance_options_chain_live_test

use scorpio_core::data::traits::options::{OptionsOutcome, OptionsProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let symbol = scorpio_core::domain::Symbol::parse("SPY")?;
    let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let client = scorpio_core::data::yfinance::YFinanceClient::new_unrate_limited()?;
    let outcome = client.fetch_snapshot(&symbol, &today).await?;
    let snap = match outcome {
        OptionsOutcome::Snapshot(s) => s,
        other => {
            return Err(format!("expected Snapshot(_), got {other}").into());
        }
    };

    println!("near_term_expiration = {}", snap.near_term_expiration);
    println!("all_expirations count = {}", snap.all_expirations.len());
    assert!(
        snap.all_expirations.len() >= 2,
        "expected ≥ 2 additional expirations"
    );
    for extra in &snap.all_expirations {
        assert_ne!(
            extra.expiration, snap.near_term_expiration,
            "all_expirations must not include the front-month slice"
        );
        assert!(
            !extra.strikes.is_empty(),
            "expiration {} has no strikes",
            extra.expiration
        );
    }

    // Negative sanity: bogus ticker yields a non-Snapshot outcome, no panic.
    let bogus = scorpio_core::domain::Symbol::parse("ZZZZZZZ").unwrap();
    let bogus_outcome = client.fetch_snapshot(&bogus, &today).await?;
    assert!(
        !matches!(bogus_outcome, OptionsOutcome::Snapshot(_)),
        "bogus ticker must not produce a Snapshot"
    );

    println!("OK");
    Ok(())
}
```

If `YFinanceClient::new_unrate_limited` does not exist, use whatever constructor the other smokes use (search `grep -n 'YFinanceClient::' crates/scorpio-core/examples/`).

- [ ] **Step 24.2: Run locally**

```bash
cargo run -p scorpio-core --example yfinance_options_chain_live_test
```

Expected: prints expirations and "OK".

- [ ] **Step 24.3: Commit**

```bash
git add crates/scorpio-core/examples/yfinance_options_chain_live_test.rs
git commit -m "test(smoke): live yfinance options chain populates all_expirations"
```

---

### Task 25: New live smoke — `etf_options_gex_live_test.rs`

**Files:**
- Create: `crates/scorpio-core/examples/etf_options_gex_live_test.rs`

- [ ] **Step 25.1: Author the e2e smoke**

Use `etf_pack_live_test.rs` as the template. Create `crates/scorpio-core/examples/etf_options_gex_live_test.rs`:

```rust
//! Live smoke: full Stage 3 ETF Phase 2 path produces a populated GexSummary.
//!
//! Not part of CI. Run with:
//!     cargo run -p scorpio-core --example etf_options_gex_live_test

use scorpio_core::state::{EtfRiskFreeRateSource, ScenarioValuation};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = scorpio_core::app::AnalysisRuntime::from_env_with_pack("etf_baseline").await?;
    let state = runtime.run("SPY").await?;

    let etf = match state
        .derived_valuation
        .as_ref()
        .map(|d| &d.scenario)
        .ok_or("derived_valuation missing")?
    {
        ScenarioValuation::Etf(etf) => etf,
        other => return Err(format!("expected Etf scenario, got {other:?}").into()),
    };

    let gex = etf
        .options_gex
        .as_ref()
        .ok_or("options_gex missing — Stage 3 must populate it on SPY")?;

    println!("near-term GEX (net) = {}", gex.net_gex_usd_per_1pct_move);
    assert!(gex.net_gex_usd_per_1pct_move.is_finite());
    assert!(gex.broad.is_some(), "broad GEX must populate");
    assert!(gex.vex_summary.is_some(), "vex_summary must populate");
    assert!(gex.cex_summary.is_some(), "cex_summary must populate");
    assert_eq!(gex.strikes.len(), 3, "top-3 walls expected");

    match state.etf_risk_free_rate_source {
        Some(EtfRiskFreeRateSource::FredDgs3Mo) => println!("FRED DGS3MO path"),
        Some(EtfRiskFreeRateSource::YFinanceIrx) => println!("yfinance ^IRX path"),
        None => return Err("risk-free rate source missing; GEX should have degraded".into()),
    }

    println!("OK");
    Ok(())
}
```

If `AnalysisRuntime::from_env_with_pack` is not the canonical constructor, replace with whatever entry point `etf_pack_live_test.rs` uses.

- [ ] **Step 25.2: Run locally**

```bash
SCORPIO_FRED_API_KEY=... cargo run -p scorpio-core --example etf_options_gex_live_test
```

Expected: prints the populated GEX summary and "OK".

- [ ] **Step 25.3: Commit**

```bash
git add crates/scorpio-core/examples/etf_options_gex_live_test.rs
git commit -m "test(smoke): live ETF run populates options_gex with broad/vex/cex/strikes"
```

---

## Self-review checklist

Run through this list before declaring Stage 1, Stage 2, or Stage 3 done:

1. **Spec coverage** — every decision in the spec's decision table maps to at least one task. Stage labels show where each task ships, including the three promoted tasks (18-20) whose bodies still live under the Stage 3 heading for numbering continuity:
   - BSM math (gamma/vanna/charm) → Task 1 (Stage 1)
   - Per-strike aggregation with SqueezeMetrics sign → Task 2 (Stage 1)
   - State schema additions → Task 3 (Stage 1)
   - ValuationInputs carrier + `compute_gex_summary` near-term → Task 4 (Stage 1)
   - Live options hydration on AnalystSyncTask → Task 5 (Stage 1)
   - GEX state roundtrip → Task 6 (Stage 1)
   - Leverage helper format + visibility → Task 7 (Stage 2)
   - Conservative/Neutral injection → Task 8 (Stage 2)
   - Auditor injection → Task 9 (Stage 2)
   - Rewritten focus prompt → Task 10 (Stage 2)
   - Terminal DEALER POSITIONING block + risk-free-rate banner → Task 11 (Stage 2)
   - Prompt-bundle regression-gate refresh → Task 12 (Stage 2)
   - Broad aggregation in aggregator → Task 13 (Stage 3)
   - `all_expirations` transient field → Task 14 (Stage 3)
   - yfinance plumbing → Task 15 (Stage 3)
   - `compute_gex_summary` broad/VEX/CEX → Task 16 (Stage 3)
   - Strip-before-serialize → Task 17 (Stage 3)
   - TradingState risk-free-rate fields → Task 18 (**Stage 2 — promoted**)
   - Preflight FRED DGS3MO + yfinance ^IRX fetch → Task 19 (**Stage 2 — promoted**)
   - ValuationInputs.etf_risk_free_rate hookup → Task 20 (**Stage 2 — promoted**)
   - Reporter Stage 3 expansion (secondary sensitivities + broad-GEX sub-block) → Task 21 (Stage 3)
   - fred_live_test DGS3MO → Task 22 (Stage 3, optional smoke)
   - yfinance_live_test ^IRX → Task 23 (Stage 2 smoke, optional)
   - yfinance_options_chain smoke → Task 24 (Stage 3, optional)
   - e2e etf_options_gex smoke → Task 25 (Stage 3, optional)

2. **Placeholder scan** — no "TBD", no "fill in later", no "add appropriate handling". Each step shows the actual code or actual command.

3. **Type consistency** — `compute_gex_summary` signature is the same across Tasks 4 and 16; Stage 1 `GexSummary` field names match the near-term state struct from Task 3; Stage 3 `BroadGex`, `VexSummary`, `CexSummary`, `AggregateInputs`, `AggregateResult`, `NearTermAggregate`, `BroadAggregate`, and `ExpirationStrikes` field names match consistently across Tasks 13-16.

4. **Validation gate** — Stage 3 tasks start only after the user signals proceed at the gate. If the gate decision is stop, archive the plan and document the decision in `docs/solutions/`.

5. **Compile-step ordering** — each stage's tasks are ordered so the codebase always compiles after each commit. Stage 1: types before consumers. Stage 3: state fields before preflight and reporter. No task creates an orphaned reference that the next task is expected to retire.

---

## Execution handoff

**Plan complete and saved to `docs/superpowers/plans/2026-05-27-etf-baseline-phase2-implementation.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — A fresh subagent runs each task with a two-stage review checkpoint between tasks. Best for this plan because Stage 2 ends at an explicit user gate.

**2. Inline Execution** — Tasks run in the current session via `superpowers:executing-plans` with batch checkpoints.

**Which approach do you want?**
