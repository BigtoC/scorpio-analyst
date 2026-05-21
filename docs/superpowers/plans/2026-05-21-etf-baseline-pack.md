# ETF Baseline Pack — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a first-class `EtfBaseline` analysis pack so `scorpio analyze SPY` produces ETF-native analysis (premium/discount, composition when fresh N-PORT exists, source-provided-benchmark tracking, ETF report panel, routing-fallback warnings) instead of short-circuiting on `Profile::Fund`.

**Architecture:** Add a new pack manifest and prompt bundle that lives alongside the existing equity baseline, extract 9 cross-cutting equity prompts into `analysis_packs/common/prompts/` so both packs share them by composition, add ETF-specific data adapters (yfinance quote/fund-info + SEC EDGAR N-PORT-P), an `EtfPremiumDiscountValuator`, and a `ScenarioValuation::Etf(...)` output variant. Routing classifies `Profile::Fund` symbols with a supported `fund_kind` to `PackId::EtfBaseline`; all other cases fall back to `PackId::Baseline` with a visible warning. Phase 2 (dealer GEX, options-heavy prompts) is **deferred**.

**Tech Stack:** Rust 1.93 (edition 2024), `yfinance-rs` 0.7, existing `SecEdgarClient` (extended), `rig-core` 0.32 prompt bundle, `graph-flow` 0.5 topology, `serde`/`schemars` for state additive variants, `tokio` for I/O.

**Spec:** `docs/superpowers/specs/2026-05-21-etf-baseline-pack-design.md`

---

## Conventions used in this plan

- Every file path is rooted at the repo root unless absolute.
- Each task ends with a `cargo test` (or `cargo check` on pure plumbing) before committing.
- Commits use the existing convention: `feat(etf): …`, `refactor(prompts): …`, `feat(data): …`, `test(etf): …`. (Recent commits in this repo follow this style; mirror it.)
- All new state types derive `Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq`. **Do not** add `#[serde(deny_unknown_fields)]` on anything reachable from `TradingState` (per CLAUDE.md).
- ETF-specific bundle additions to `TradingState` carry `#[serde(default)]`.
- Live smoke tests live under `crates/scorpio-core/examples/` and are not run in CI.

---

## Task index

1. **State variants** — `EtfValuation`, `ScenarioValuation::Etf`, `EtfDataAvailability`, support types in `state/derived.rs`
2. **PackId + ValuatorId + ValuationAssessment** — additive variants
3. **Common-prompts extraction (Tier 1)** — move 6 verbatim-shared equity prompts into `analysis_packs/common/prompts/`, update equity include paths
4. **Common-prompts extraction (Tier 2)** — move 3 composition-base prompts (news, technical, auditor) into common pool
5. **ETF prompt assets** — write all new ETF-specific prompt `.md` files and ETF delta deltas
6. **ETF pack manifest + builder** — `analysis_packs/etf/baseline.rs` + `mod.rs` + composition helpers + registry wiring
7. **`ValuationInputs` extension + ETF input carriers** — extend the shared `ValuationInputs<'a>` carrier; add `EtfQuote`, `FundInfo`, `NPortHoldings` placeholder structs that downstream tasks fill
8. **yfinance ETF data methods** — `get_quote`, `get_fund_info`, `get_distribution_yield_ttm` + `EtfQuote`/`FundInfo` types
9. **SEC EDGAR N-PORT-P client** — `resolve_fund_cik`, `fetch_latest_nport_p`, new `nport.rs` XBRL parser
10. **ETF valuator** — `valuation/etf/{premium_discount.rs, category_norms.rs, tracking_error.rs}` + registry registration
11. **Runtime pack classifier + builder wiring** — `classify_runtime_pack`, plumb `SecEdgarClient` into `AnalystSyncTask`
12. **Preflight: record routing reason + warning metadata** — write fallback reason into context, surface in state
13. **`AnalystSyncTask` ETF input hydration** — fetch quote/fund-info/CIK/N-PORT/benchmark OHLCV when `pack == EtfBaseline`; call ETF valuator
14. **Report rendering — ETF panel** — `terminal/etf.rs`, `valuation.rs` dispatch, routing-fallback header warning
15. **Routing + topology integration tests** — extend `workflow_pipeline_structure.rs`
16. **Prompt-bundle regression gate** — ensure equity bytes unchanged after shared-prompt extraction; add EtfBaseline coverage
17. **State serde round-trip** — ETF variant compat, old snapshot decode
18. **Live smoke examples** — `etf_quote_live_test.rs`, `nport_live_test.rs`, `etf_pack_live_test.rs`

---

## Task 1: Add ETF state types and `ScenarioValuation::Etf` variant

**Files:**
- Modify: `crates/scorpio-core/src/state/derived.rs`
- Modify: `crates/scorpio-core/src/state/mod.rs` (re-export new types)

This is purely additive — no behaviour changes. The new variant defaults are inert until the valuator lands.

- [ ] **Step 1: Add the new struct definitions to `state/derived.rs`**

Append the following block immediately above the existing `# ─── Tests ───` divider (around line 200), keeping all existing types untouched:

```rust
// ─── ETF valuation types (Phase 1) ────────────────────────────────────────────

/// Premium-band classification anchored to category norms.
///
/// Populated by [`EtfValuation`]. `Unknown` means the band could not be
/// computed (NAV missing, category unknown, or both).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PremiumBand {
    Normal,
    Elevated,
    Extreme,
    Unknown,
}

/// Single holding row inside [`EtfComposition`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HoldingWeight {
    pub cusip: Option<String>,
    pub ticker: Option<String>,
    pub name: String,
    pub weight_pct: f64,
    #[serde(default)]
    pub value_usd: Option<f64>,
}

/// Single sector row inside [`EtfComposition`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SectorWeight {
    pub sector: String,
    pub weight_pct: f64,
}

/// Quote + premium snapshot. `premium_pct` is `None` when NAV is unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PremiumSnapshot {
    pub nav: Option<f64>,
    pub market_price: f64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub premium_pct: Option<f64>,
    pub category_band: PremiumBand,
    pub bid_ask_spread_pct: Option<f64>,
    pub as_of: chrono::DateTime<chrono::Utc>,
}

/// Composition + cost snapshot derived from N-PORT-P + fund metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EtfComposition {
    pub top_holdings: Vec<HoldingWeight>,
    pub top10_concentration_pct: f64,
    pub sector_weights: Vec<SectorWeight>,
    #[serde(default)]
    pub expense_ratio_pct: Option<f64>,
    #[serde(default)]
    pub aum_usd: Option<f64>,
    #[serde(default)]
    pub fund_family: Option<String>,
    #[serde(default)]
    pub distribution_yield_ttm_pct: Option<f64>,
    pub holdings_filing_date: chrono::NaiveDate,
    pub holdings_age_days: u32,
}

/// Tracking error vs a source-provided benchmark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackingError {
    pub benchmark_symbol: String,
    pub te_pct_90d: f64,
    pub te_pct_1y: f64,
    pub sample_days: u32,
}

/// Phase 2 placeholder (declared now so the variant signature is stable; the
/// `EtfValuation.options_gex` field stays `None` in Phase 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GexSummary {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub call_put_oi_ratio: f64,
    pub max_pain_strike: f64,
    pub near_term_expiration: chrono::NaiveDate,
}

/// Per-signal availability flags. Every flag defaults to `false`; the
/// valuator flips them on as each input is observed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EtfDataAvailability {
    #[serde(default)]
    pub nav_available: bool,
    #[serde(default)]
    pub bid_ask_available: bool,
    #[serde(default)]
    pub holdings_present: bool,
    #[serde(default)]
    pub holdings_fresh: bool,
    #[serde(default)]
    pub benchmark_resolved: bool,
    #[serde(default)]
    pub options_chain_present: bool,
    #[serde(default)]
    pub expense_ratio_available: bool,
}

/// Aggregate ETF valuation output. Carried by [`ScenarioValuation::Etf`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EtfValuation {
    pub premium: PremiumSnapshot,
    #[serde(default)]
    pub composition: Option<EtfComposition>,
    #[serde(default)]
    pub tracking: Option<TrackingError>,
    /// Phase 2 — always `None` in Phase 1.
    #[serde(default)]
    pub options_gex: Option<GexSummary>,
    #[serde(default)]
    pub category: Option<String>,
    /// `1.0` for plain ETFs; `2.0`, `3.0`, `-1.0`, `-2.0`, `-3.0` for
    /// leveraged/inverse products. `None` when not declared by the source.
    #[serde(default)]
    pub leverage_factor: Option<f64>,
    #[serde(default)]
    pub flags: EtfDataAvailability,
}
```

- [ ] **Step 2: Add the new `ScenarioValuation::Etf` variant**

Edit the `ScenarioValuation` enum in the same file. Insert the new variant **between** `CorporateEquity(...)` and `NotAssessed { reason }` so old `not_assessed` snapshots continue to deserialize unchanged:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioValuation {
    /// Deterministic corporate equity valuation computed from financial statements.
    #[schemars(
        description = "Deterministic corporate equity valuation computed from financial statements"
    )]
    CorporateEquity(CorporateEquityValuation),

    /// ETF-native valuation: premium/discount band + composition + tracking.
    /// Phase 1 always omits `options_gex` (`None`).
    #[schemars(description = "ETF-native valuation: premium/discount band + composition + tracking")]
    Etf(EtfValuation),

    /// Valuation was not assessed; includes the reason why.
    #[schemars(
        description = "Valuation was not assessed; reason explains why (e.g. fund-style asset or insufficient corporate inputs)"
    )]
    NotAssessed { reason: String },
}
```

- [ ] **Step 3: Re-export the new types from `state/mod.rs`**

Open `crates/scorpio-core/src/state/mod.rs` and extend the existing `pub use derived::{...}` (or per-type) re-exports so external callers can write `use scorpio_core::state::{EtfValuation, PremiumSnapshot, ...}`. Add:

```rust
pub use derived::{
    EtfComposition, EtfDataAvailability, EtfValuation, GexSummary, HoldingWeight,
    PremiumBand, PremiumSnapshot, SectorWeight, TrackingError,
};
```

(Match the existing `pub use derived::{...}` style — open the file and merge into the existing line.)

- [ ] **Step 4: Add unit tests for the additive variant**

Append to the `mod tests` block at the bottom of `state/derived.rs`:

```rust
#[test]
fn scenario_valuation_etf_variant_roundtrips_json() {
    let val = ScenarioValuation::Etf(EtfValuation {
        premium: PremiumSnapshot {
            nav: Some(621.18),
            market_price: 621.40,
            bid: Some(621.39),
            ask: Some(621.41),
            premium_pct: Some(0.04),
            category_band: PremiumBand::Normal,
            bid_ask_spread_pct: Some(0.003),
            as_of: chrono::Utc::now(),
        },
        composition: None,
        tracking: None,
        options_gex: None,
        category: Some("Large Blend".to_owned()),
        leverage_factor: Some(1.0),
        flags: EtfDataAvailability::default(),
    });
    let json = serde_json::to_string(&val).expect("serialize");
    let back: ScenarioValuation = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(val, back);
}

#[test]
fn scenario_valuation_etf_serializes_with_snake_case_key() {
    let val = ScenarioValuation::Etf(EtfValuation {
        premium: PremiumSnapshot {
            nav: None,
            market_price: 100.0,
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
    });
    let json = serde_json::to_string(&val).expect("serialize");
    assert!(json.contains("\"etf\""), "expected 'etf' tag, got: {json}");
}

#[test]
fn legacy_not_assessed_snapshot_still_deserializes_after_etf_variant_added() {
    // Snapshot taken before ScenarioValuation::Etf existed.
    let json = r#"{"not_assessed":{"reason":"fund_style_asset"}}"#;
    let back: ScenarioValuation = serde_json::from_str(json).expect("deserialize");
    assert!(matches!(back, ScenarioValuation::NotAssessed { .. }));
}

#[test]
fn etf_data_availability_defaults_to_all_false() {
    let flags = EtfDataAvailability::default();
    assert!(!flags.nav_available);
    assert!(!flags.bid_ask_available);
    assert!(!flags.holdings_present);
    assert!(!flags.holdings_fresh);
    assert!(!flags.benchmark_resolved);
    assert!(!flags.options_chain_present);
    assert!(!flags.expense_ratio_available);
}
```

- [ ] **Step 5: Run tests + commit**

```bash
cargo test -p scorpio-core --lib state::derived -- --nocapture
cargo fmt
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/state/derived.rs crates/scorpio-core/src/state/mod.rs
git commit -m "feat(state): add ScenarioValuation::Etf variant and ETF valuation types"
```

Expected: all new tests pass; pre-existing 16 `state::derived::tests` still pass unchanged.

---

## Task 2: Add `PackId::EtfBaseline`, `ValuatorId::EtfPremiumDiscount`, `ValuationAssessment::Etf`

**Files:**
- Modify: `crates/scorpio-core/src/analysis_packs/manifest/pack_id.rs`
- Modify: `crates/scorpio-core/src/analysis_packs/manifest/strategy.rs`
- Modify: `crates/scorpio-core/src/analysis_packs/manifest/schema.rs` (extend `resolve_valuation` match)
- Modify: `crates/scorpio-core/src/valuation/mod.rs`

- [ ] **Step 1: Extend `PackId` to include the ETF baseline**

Edit `analysis_packs/manifest/pack_id.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PackId {
    Baseline,
    /// ETF-native pack — premium/discount band + composition + tracking.
    EtfBaseline,
    CryptoDigitalAsset,
}

impl PackId {
    pub fn as_str(self) -> &'static str {
        match self {
            PackId::Baseline => "baseline",
            PackId::EtfBaseline => "etf_baseline",
            PackId::CryptoDigitalAsset => "crypto_digital_asset",
        }
    }
}

impl std::str::FromStr for PackId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // `EtfBaseline` is NOT user-selectable via config in this slice — runtime
        // routing picks it automatically based on Profile + fund metadata.
        match s.trim().to_ascii_lowercase().as_str() {
            "baseline" => Ok(PackId::Baseline),
            unknown => Err(format!(
                "unknown analysis pack: \"{unknown}\" (available: baseline)"
            )),
        }
    }
}
```

- [ ] **Step 2: Extend `ValuationAssessment`**

Edit `analysis_packs/manifest/strategy.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuationAssessment {
    /// Full deterministic valuation (DCF, multiples) for corporate equities.
    Full,
    /// ETF-native valuation (premium/discount + composition + tracking).
    Etf,
    /// Valuation not assessed — explicit fallback for indices, unknown shapes, etc.
    NotAssessed,
}
```

- [ ] **Step 3: Extend `resolve_valuation` to honour Fund → Etf when the pack opts in**

Edit `analysis_packs/manifest/schema.rs`. Update the body of `resolve_valuation` so that a pack whose `default_valuation == Etf` returns `Etf` for `AssetShape::Fund`. The signature stays the same:

```rust
pub fn resolve_valuation(&self, shape: &AssetShape) -> ValuationAssessment {
    match shape {
        AssetShape::CorporateEquity => match self.default_valuation {
            ValuationAssessment::Full | ValuationAssessment::Etf => ValuationAssessment::Full,
            ValuationAssessment::NotAssessed => ValuationAssessment::NotAssessed,
        },
        AssetShape::Fund => match self.default_valuation {
            ValuationAssessment::Etf => ValuationAssessment::Etf,
            ValuationAssessment::Full | ValuationAssessment::NotAssessed => {
                ValuationAssessment::NotAssessed
            }
        },
        AssetShape::Unknown
        | AssetShape::NativeChainAsset
        | AssetShape::Erc20Token
        | AssetShape::Stablecoin
        | AssetShape::LpToken => ValuationAssessment::NotAssessed,
    }
}
```

The existing equity-baseline test (`baseline_pack_etf_gets_not_assessed`) keeps passing — baseline ships `default_valuation = Full`, so Fund still resolves to `NotAssessed` for it.

- [ ] **Step 4: Add `ValuatorId::EtfPremiumDiscount`**

Edit `valuation/mod.rs`. Add the variant to the existing `#[non_exhaustive]` enum (additive, safe). Position it next to `EquityDefault`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ValuatorId {
    EquityDefault,
    /// ETF premium/discount + composition + tracking valuator.
    EtfPremiumDiscount,
    CryptoTokenomics,
    CryptoNetworkValue,
}
```

- [ ] **Step 5: Sanity tests**

Append to `pack_id.rs` tests (create `#[cfg(test)] mod tests` block if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etf_baseline_has_canonical_str() {
        assert_eq!(PackId::EtfBaseline.as_str(), "etf_baseline");
    }

    #[test]
    fn etf_baseline_not_selectable_via_from_str() {
        // ETF routing is automatic — the CLI must not let users force it.
        let err = "etf_baseline".parse::<PackId>().expect_err("must not parse");
        assert!(err.contains("unknown analysis pack"));
    }

    #[test]
    fn baseline_still_parses() {
        assert_eq!("baseline".parse::<PackId>().unwrap(), PackId::Baseline);
    }
}
```

- [ ] **Step 6: Build, run all pack tests, commit**

```bash
cargo test -p scorpio-core --lib analysis_packs
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/analysis_packs/manifest/ crates/scorpio-core/src/valuation/mod.rs
git commit -m "feat(packs): add EtfBaseline PackId, ValuationAssessment::Etf, ValuatorId::EtfPremiumDiscount"
```

Expected: all existing `analysis_packs::manifest::tests` pass; baseline pack still resolves `Fund → NotAssessed`.

---

## Task 3: Promote 6 Tier-1 verbatim equity prompts to `analysis_packs/common/prompts/`

**Files:**
- Create: `crates/scorpio-core/src/analysis_packs/common/mod.rs`
- Create: `crates/scorpio-core/src/analysis_packs/common/prompts/{analyst_runtime_contract,theme_h_sourcing_and_untrusted,debate_moderator,risk_moderator,bullish_researcher,bearish_researcher}.md` (moved, not retyped)
- Modify: `crates/scorpio-core/src/analysis_packs/mod.rs` (declare new `common` submodule)
- Modify: `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` (update `include_str!` paths)

Tier 1 = bit-for-bit identical content used by both packs. The byte-content guard test in Task 16 will confirm equity bytes are unchanged.

- [ ] **Step 1: Create the new module skeleton**

```bash
mkdir -p crates/scorpio-core/src/analysis_packs/common/prompts
```

Write `crates/scorpio-core/src/analysis_packs/common/mod.rs`:

```rust
//! Cross-pack prompt assets shared between equity and ETF packs.
//!
//! Each prompt is included via `include_str!("prompts/<name>.md")` by the
//! pack manifests; this module owns only the directory. Adding a file here
//! does not register it anywhere — the consuming pack chooses to pull it in.
```

- [ ] **Step 2: Move the 6 Tier-1 files into the common directory**

```bash
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/analyst_runtime_contract.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/theme_h_sourcing_and_untrusted.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/debate_moderator.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/risk_moderator.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/bullish_researcher.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/bearish_researcher.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
```

- [ ] **Step 3: Declare the new module**

Edit `crates/scorpio-core/src/analysis_packs/mod.rs` and add `mod common;` next to the other `mod` declarations (it has no public surface — pack manifests pull files via `include_str!`).

- [ ] **Step 4: Update the include paths in `equity/baseline.rs`**

Edit `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` and rewrite the six affected `include_str!` paths (lines 19–22 and 76–84 in the current file) so they point at the common directory. Replace this block at the top of the file:

```rust
const ANALYST_RUNTIME_CONTRACT: &str = include_str!("prompts/analyst_runtime_contract.md");
const THEME_C_MANAGEMENT_RED_FLAGS: &str = include_str!("prompts/theme_c_management_red_flags.md");
const THEME_H_SOURCING_AND_UNTRUSTED: &str =
    include_str!("prompts/theme_h_sourcing_and_untrusted.md");
```

with:

```rust
const ANALYST_RUNTIME_CONTRACT: &str =
    include_str!("../common/prompts/analyst_runtime_contract.md");
const THEME_C_MANAGEMENT_RED_FLAGS: &str = include_str!("prompts/theme_c_management_red_flags.md");
const THEME_H_SOURCING_AND_UNTRUSTED: &str =
    include_str!("../common/prompts/theme_h_sourcing_and_untrusted.md");
```

Then in `baseline_prompt_bundle()` rewrite the four remaining moved-file references (debate_moderator, risk_moderator, bullish_researcher, bearish_researcher):

```rust
bullish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
    "../common/prompts/bullish_researcher.md"
))),
bearish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
    "../common/prompts/bearish_researcher.md"
))),
debate_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
    "../common/prompts/debate_moderator.md"
))),
// ... and similarly for risk_moderator:
risk_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
    "../common/prompts/risk_moderator.md"
))),
```

- [ ] **Step 5: Build + run existing equity tests**

```bash
cargo test -p scorpio-core --lib analysis_packs::equity
cargo test -p scorpio-core --test prompt_bundle_regression_gate
cargo clippy -p scorpio-core --all-targets -- -D warnings
```

Expected: all existing equity-baseline tests pass — the renamed files produce byte-identical content via `include_str!`. If `prompt_bundle_regression_gate` has cached fixture bytes that no longer match because of trailing-newline drift, that means a `git mv` accidentally re-encoded the file. Investigate; do not regenerate fixtures yet — the regression gate is meant to catch exactly this.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/analysis_packs/common/ crates/scorpio-core/src/analysis_packs/equity/baseline.rs crates/scorpio-core/src/analysis_packs/mod.rs crates/scorpio-core/src/analysis_packs/equity/prompts/
git commit -m "refactor(prompts): extract 6 Tier-1 cross-cutting equity prompts to common/"
```

---

## Task 4: Promote 3 Tier-2 composition-base prompts to `common/`

**Files:**
- Move (git mv): `news_analyst.md`, `technical_analyst.md`, `auditor.md` from `equity/prompts/` to `common/prompts/`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` (4 `include_str!` paths)

Tier 2 prompts are byte-identical between equity and ETF today; the ETF pack will later compose against them with small deltas. The equity baseline keeps composing them via `with_analyst_runtime_contract_sections` exactly as before.

- [ ] **Step 1: Move the files**

```bash
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/news_analyst.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/technical_analyst.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
git mv crates/scorpio-core/src/analysis_packs/equity/prompts/auditor.md \
       crates/scorpio-core/src/analysis_packs/common/prompts/
```

- [ ] **Step 2: Update equity include paths**

Edit `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` and rewrite the four affected `include_str!` calls inside `baseline_prompt_bundle()`:

```rust
news_analyst: with_analyst_runtime_contract_sections(
    include_str!("../common/prompts/news_analyst.md"),
    &[THEME_C_MANAGEMENT_RED_FLAGS, theme_h_summary.as_str()],
),
technical_analyst: with_analyst_runtime_contract_sections(
    include_str!("../common/prompts/technical_analyst.md"),
    &[theme_h_summary.as_str()],
),
// ...
auditor: Cow::Borrowed(trim_trailing_newline(include_str!(
    "../common/prompts/auditor.md"
))),
```

- [ ] **Step 3: Build + test**

```bash
cargo test -p scorpio-core --lib analysis_packs::equity
cargo test -p scorpio-core --test prompt_bundle_regression_gate
```

Expected: PASS. Same byte-identity contract as Task 3.

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-core/src/analysis_packs/
git commit -m "refactor(prompts): extract 3 Tier-2 composition-base prompts to common/"
```

---

## Task 5: Write all new ETF-specific prompt assets

**Files:**
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_runtime_contract.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_failure_modes.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_leverage_warning.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/composition_analyst.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/flow_premium_analyst.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_macro_sector_focus.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_landmines.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/trader.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/aggressive_risk.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/conservative_risk.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/neutral_risk.md`
- Create: `crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md`

Each new prompt must preserve the standard placeholders the renderer relies on (`{ticker}`, `{current_date}`, `{analysis_emphasis}`). The baseline equity asset test (`baseline_pack_populates_prompt_bundle_slots_with_runtime_placeholders` in `equity/baseline.rs`) checks that every analyst/researcher/risk slot keeps `{ticker}` and `{current_date}` — write the ETF equivalents accordingly.

- [ ] **Step 1: Bootstrap directories**

```bash
mkdir -p crates/scorpio-core/src/analysis_packs/etf/prompts
```

- [ ] **Step 2: Write `etf_runtime_contract.md`**

Shared scaffolding section appended to every ETF analyst slot. Echoes the discipline of `analyst_runtime_contract.md` but tailored to ETF evidence.

```markdown
## ETF runtime contract

You are analysing an exchange-traded fund. The instrument is a basket — its
value depends on the underlying holdings, the creation/redemption mechanism,
and the management overlay (expense ratio, securities lending, sampling).
Reason about the **wrapper**, not just the price line.

- Quote AS-OF the timestamp present in `EtfQuote.as_of` (UTC). Do NOT
  re-anchor to "today".
- Treat NAV as end-of-prior-session unless explicitly stated otherwise.
  Premium/discount is `(market_price - nav) / nav * 100`, not relative to
  intraday iNAV (intraday NAV is out of scope this run).
- If `flags.holdings_fresh = false`, qualify any composition statement with
  the staleness window. If `flags.holdings_present = false`, do NOT invent
  holdings — say composition is unavailable.
- Tracking error is the rolling stdev of (`etf_return - benchmark_return`)
  annualised over the sample window. Do NOT extrapolate from a single day's
  drift.
```

- [ ] **Step 3: Write `etf_failure_modes.md`**

Cross-cutting failure-mode reminder injected into every ETF analyst + risk slot.

```markdown
## ETF failure modes to weigh

- **AP arbitrage breakdown** — when authorised-participant flow halts (high
  premiums/discounts persist), the wrapper decouples from NAV. Flag it
  whenever `premium_pct` magnitude exceeds the category band's "Extreme"
  threshold.
- **Composition staleness** — N-PORT-P filings have a 60-day legal lag.
  Holdings older than 90 days may not reflect current exposure.
- **Tracking drift vs index** — non-trivial tracking error can come from
  sampling, securities-lending offsets, or fees. Treat persistent drift as
  a structural cost, not noise.
- **Leverage decay** — daily-reset leveraged/inverse products drift from
  their stated multiple over multi-day horizons. Holding-period risk.
- **Distribution mechanics** — large quarterly distributions reset NAV;
  reading the premium across a distribution date without adjustment is a
  false signal.
```

- [ ] **Step 4: Write `etf_leverage_warning.md`**

Conditionally injected at substitution time when `leverage_factor != 1.0`. Lives as a static file so the manifest stays leverage-agnostic.

```markdown
## ⚠ Leveraged / inverse product warning

The selected ETF carries a non-1x leverage factor (`{leverage_factor}x`).
Daily-reset products are designed for single-session exposure; multi-day
returns DRIFT from the stated multiple under volatility. Conservative and
Neutral risk agents MUST treat any holding horizon >1 trading day as a
deterministic-fallback "extreme risk" condition unless explicit hedging
context is supplied in the trader proposal.
```

- [ ] **Step 5: Write `composition_analyst.md`** (Tier 3, slot `fundamental_analyst`)

```markdown
# ETF Composition Analyst

You are the composition specialist for `{ticker}`. The current date is
`{current_date}`. Your job is to reason about the basket: holdings
concentration, sector tilt vs the stated benchmark, expense drag, AUM
solvency, and distribution behaviour.

{analysis_emphasis}

## Required outputs

1. **Top-10 concentration**: cite the percentage from `EtfComposition.top10_concentration_pct`. Compare against a generic "broad index"
   reference of ~25% for US large-cap diversifieds; flag tilts >35%.
2. **Sector tilt summary**: identify the two largest over-/under-weights vs
   the broad market in plain language.
3. **Cost profile**: state `expense_ratio_pct` and `distribution_yield_ttm_pct` if available; flag expense ratios >0.50%
   for index-tracking products.
4. **Staleness audit**: if `flags.holdings_fresh = false`, lead the summary
   with the staleness window (`holdings_age_days`) before any composition
   claim.

If `composition` is `None`, do NOT invent holdings. State explicitly that
N-PORT-P data is unavailable and explain what that means for the analysis.
```

- [ ] **Step 6: Write `flow_premium_analyst.md`** (Tier 3, slot `sentiment_analyst`)

```markdown
# ETF Flow & Premium Analyst

You are the flow and premium specialist for `{ticker}`. The current date is
`{current_date}`. Your job is to read AP arbitrage health from premium
band, bid/ask spread, and AUM/volume context.

{analysis_emphasis}

## Required outputs

1. **Premium band classification**: cite `category_band` (`Normal` /
   `Elevated` / `Extreme` / `Unknown`) and the raw `premium_pct`.
2. **Bid/ask spread reading**: cite `bid_ask_spread_pct`. Spreads >0.05% in
   high-volume large-cap ETFs signal stress; >0.50% in any product is a
   liquidity red flag.
3. **Distribution context**: if a recent distribution is present in the
   evidence, explain how it would affect a naive premium reading taken
   across the ex-date.

Do NOT speculate about fund flows beyond what the evidence supports. If
NAV is unavailable (`flags.nav_available = false`), state that premium
analysis is impossible this run and stop.
```

- [ ] **Step 7: Write `etf_macro_sector_focus.md`** (Tier 2 delta, composes with `common/prompts/news_analyst.md`)

```markdown
## ETF macro & sector lens

Beyond company-specific catalysts, prioritise:

- Sector-level macro tailwinds/headwinds matching the ETF's largest
  sectoral exposure (cite the sector from `EtfComposition.sector_weights`).
- Index-level events (rebalances, methodology changes, IPO inclusions).
- Regulatory shifts affecting the wrapper class (e.g. 1940-Act ETF rules,
  derivatives caps for active ETFs).

Ignore single-name news unless it concerns a top-5 holding by weight.
```

- [ ] **Step 8: Write `etf_tracking_options_focus.md`** (Tier 2 delta, composes with `common/prompts/technical_analyst.md`)

```markdown
## ETF tracking & options lens

In addition to standard technicals:

- **Tracking error** — if `TrackingError` is present, cite
  `te_pct_90d` and `te_pct_1y`. >0.20% annualised on a vanilla index-tracker
  is structurally costly; >1.0% suggests active management or sampling
  mismatch.
- **Options context** — Phase 1 omits the options chain. Treat options
  liquidity as out-of-scope unless explicit `options_context` is supplied
  in evidence. (Phase 2 will add dealer-gamma analysis.)
```

- [ ] **Step 9: Write `etf_landmines.md`** (Tier 2 delta, composes with `common/prompts/auditor.md`)

```markdown
## ETF-specific audit landmines

- The proposal cites holdings older than 90 days without flagging staleness.
- The proposal asserts a tracking-error magnitude without citing the sample
  window or annualisation.
- The proposal applies a leveraged ETF (`leverage_factor != 1.0`) without
  acknowledging multi-day decay.
- The proposal infers fund flows from premium magnitude alone.
- The proposal treats a post-distribution premium reading as if no
  ex-dividend adjustment had occurred.
```

- [ ] **Step 10: Write `trader.md`** (Tier 3, ETF-specific trader)

```markdown
# Trader — ETF Baseline

You synthesise the analyst debate for `{ticker}` on `{current_date}` into a
structured `TradeProposal`. The evidence is ETF-native: premium band,
composition, tracking, distribution.

{analysis_emphasis}

## Anchors

- The premium band is the primary signal: Normal → mean-reverting setups
  argue against extreme conviction either way; Elevated → asymmetric
  caution on the high-premium side; Extreme → escalate `risk_tier` and
  surface AP-arbitrage-breakdown as the central thesis if relevant.
- Tracking error >0.20% annualised on a passive product reduces conviction
  on price-action-driven theses (the wrapper is not a clean expression of
  the index).

## Constraints

- Never propose holding a leveraged/inverse product for >1 trading day
  without an explicit hedging or rebalance plan in `rationale`.
- If `composition` is unavailable, do NOT assert sector or factor exposure.
- Cite the `as_of` timestamp from the premium snapshot when discussing
  current pricing.
```

- [ ] **Step 11: Write `conservative_risk.md`** (Tier 3, ETF-specific)

```markdown
# Conservative Risk — ETF Baseline

You assess the trader's ETF proposal for `{ticker}` on `{current_date}`
through a capital-preservation lens.

{analysis_emphasis}

## Deterministic-flag triggers

You MUST surface one of these condition tags as the leading line of your
output when the corresponding evidence is present (per the fund-manager
dual-risk contract):

- `extreme_premium` — `premium_band == Extreme`.
- `tracking_failure` — `te_pct_90d > 1.0` or `te_pct_1y > 0.50` on a passive product.
- `leverage_decay` — `leverage_factor != 1.0` AND the proposal holds >1 day.
- `stale_holdings` — `flags.holdings_fresh = false` AND the proposal cites
  composition specifically.

If none apply, lead with the bullet `no_deterministic_flag` and proceed to
the qualitative assessment.
```

- [ ] **Step 12: Write `neutral_risk.md`** (Tier 3, ETF-specific)

```markdown
# Neutral Risk — ETF Baseline

You assess the trader's ETF proposal for `{ticker}` on `{current_date}`
through a balanced lens. You also enforce the deterministic-flag triggers
from the conservative agent: if your independent reading of the evidence
trips any of `extreme_premium`, `tracking_failure`, `leverage_decay`, or
`stale_holdings`, surface that tag as the leading line.

{analysis_emphasis}

## Required qualitative pass

Discuss:
- Whether the premium-band classification is anchored to the right
  category norm (small-cap, sector, EM, etc.).
- Whether composition concentration alone explains the proposal's risk
  framing.
- Whether the proposal's holding horizon is compatible with the wrapper's
  rebalance cadence (daily-reset leverage vs multi-day hold).
```

- [ ] **Step 13: Write `aggressive_risk.md`** (Tier 3, ETF-specific)

```markdown
# Aggressive Risk — ETF Baseline

You argue the upside framing of the trader's ETF proposal for `{ticker}`
on `{current_date}`.

{analysis_emphasis}

## Anchors

- A Normal-band premium with a tight bid/ask spread argues for higher
  conviction on directional theses — the wrapper is doing its job.
- Composition concentration in the top-10 above 30% is a feature, not a
  bug, when the thesis is explicitly factor- or theme-driven.
- Tracking error inside category norms (e.g. <0.10% for US large-cap
  index trackers) does NOT diminish a price-action-driven thesis.
```

- [ ] **Step 14: Write `fund_manager.md`** (Tier 3, ETF dual-risk semantics)

```markdown
# Fund Manager — ETF Baseline

You make the final approve/reject call on the ETF trade proposal for
`{ticker}` on `{current_date}`.

{analysis_emphasis}

## Dual-risk audit (first-line invariant)

Per the existing fund-manager dual-risk contract: the first line of your
output MUST be one of `dual_risk_violation: <tag>` or
`dual_risk_clear`.

A `dual_risk_violation` is triggered when BOTH the conservative and
neutral risk agents flag the same condition tag from
`{extreme_premium, tracking_failure, leverage_decay, stale_holdings}`.
When triggered, you MUST `decision: Rejected`.

Otherwise, weigh the analyst, debate, and risk-stage output normally.

## ETF-specific decision considerations

- Bias against approving a leveraged/inverse product proposal with a
  stated holding period >1 trading day.
- If `composition` is `None` AND the proposal's thesis depends on sector
  exposure, reject and ask for re-analysis when N-PORT data refreshes.
```

- [ ] **Step 15: Sanity check the placeholders before continuing**

```bash
grep -L "{ticker}" crates/scorpio-core/src/analysis_packs/etf/prompts/composition_analyst.md crates/scorpio-core/src/analysis_packs/etf/prompts/flow_premium_analyst.md crates/scorpio-core/src/analysis_packs/etf/prompts/trader.md crates/scorpio-core/src/analysis_packs/etf/prompts/{aggressive,conservative,neutral}_risk.md crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md
# Expected: empty (every analyst/role file contains {ticker})

grep -L "{current_date}" crates/scorpio-core/src/analysis_packs/etf/prompts/composition_analyst.md crates/scorpio-core/src/analysis_packs/etf/prompts/flow_premium_analyst.md crates/scorpio-core/src/analysis_packs/etf/prompts/trader.md crates/scorpio-core/src/analysis_packs/etf/prompts/{aggressive,conservative,neutral}_risk.md crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md
# Expected: empty
```

- [ ] **Step 16: Commit**

```bash
git add crates/scorpio-core/src/analysis_packs/etf/prompts/
git commit -m "feat(etf): add ETF-specific prompt assets (analysts, risk, trader, fund manager)"
```

---

## Task 6: ETF pack manifest, builder, and registry wiring

**Files:**
- Create: `crates/scorpio-core/src/analysis_packs/etf/mod.rs`
- Create: `crates/scorpio-core/src/analysis_packs/etf/baseline.rs`
- Modify: `crates/scorpio-core/src/analysis_packs/mod.rs` (declare `mod etf;`)
- Modify: `crates/scorpio-core/src/analysis_packs/registry.rs` (register `PackId::EtfBaseline → etf::etf_baseline_pack()`, add to `REGISTERED_PACKS`)

- [ ] **Step 1: Create the module skeleton**

Write `crates/scorpio-core/src/analysis_packs/etf/mod.rs`:

```rust
//! ETF baseline pack — premium/discount + composition + tracking.
//!
//! Phase 1: yfinance quote + fund info + SEC EDGAR N-PORT-P + source-provided
//! benchmark OHLCV. Phase 2 (dealer GEX) is deferred.

mod baseline;

pub use baseline::etf_baseline_pack;
```

- [ ] **Step 2: Write `etf/baseline.rs`**

Mirror the structure of `equity/baseline.rs` exactly. The composition helpers (`compose_etf_analyst`, `compose_etf_section`, `compose_etf_risk`) are local to this module — they share the underlying `compose_prompt_sections` primitive by inlining the same logic. (Do not pull the equity helpers into a shared crate-level module — keeps pack manifests self-contained.)

```rust
//! ETF baseline pack manifest + prompt composition.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::{prompts::PromptBundle, state::AssetShape, valuation::ValuatorId};

use super::super::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};

// ─── ETF scaffolding ──────────────────────────────────────────────────────────

const ETF_RUNTIME_CONTRACT: &str = include_str!("prompts/etf_runtime_contract.md");
const ETF_FAILURE_MODES: &str = include_str!("prompts/etf_failure_modes.md");

// Shared common-pool scaffolding reused from the common directory.
const COMMON_ANALYST_CONTRACT: &str =
    include_str!("../common/prompts/analyst_runtime_contract.md");

fn trim_trailing_newline(content: &str) -> &str {
    content.strip_suffix('\n').unwrap_or(content)
}

fn compose_prompt_sections(raw: &str, sections: &[&str]) -> String {
    let mut composed = trim_trailing_newline(raw).to_owned();
    for section in sections {
        composed.push_str("\n\n");
        composed.push_str(trim_trailing_newline(section));
    }
    composed
}

/// Compose a fully ETF-native analyst slot: raw prompt + common contract +
/// ETF runtime contract + ETF failure modes.
fn compose_etf_analyst(raw: &'static str) -> Cow<'static, str> {
    Cow::Owned(compose_prompt_sections(
        raw,
        &[COMMON_ANALYST_CONTRACT, ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES],
    ))
}

/// Compose a Tier-2 reuse: common-pool prompt verbatim + small ETF deltas.
fn compose_etf_section(raw: &'static str, deltas: &[&str]) -> Cow<'static, str> {
    let mut sections = vec![ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES];
    sections.extend_from_slice(deltas);
    Cow::Owned(compose_prompt_sections(raw, &sections))
}

/// Compose a risk-agent slot: ETF-specific raw prompt + scaffolding.
fn compose_etf_risk(raw: &'static str) -> Cow<'static, str> {
    Cow::Owned(compose_prompt_sections(
        raw,
        &[ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES],
    ))
}

fn etf_baseline_prompt_bundle() -> PromptBundle {
    PromptBundle {
        // Tier 3 — fully new ETF analysts.
        fundamental_analyst: compose_etf_analyst(include_str!("prompts/composition_analyst.md")),
        sentiment_analyst: compose_etf_analyst(include_str!("prompts/flow_premium_analyst.md")),

        // Tier 2 — common-pool prompt + ETF delta.
        news_analyst: compose_etf_section(
            include_str!("../common/prompts/news_analyst.md"),
            &[include_str!("prompts/etf_macro_sector_focus.md")],
        ),
        technical_analyst: compose_etf_section(
            include_str!("../common/prompts/technical_analyst.md"),
            &[include_str!("prompts/etf_tracking_options_focus.md")],
        ),
        auditor: compose_etf_section(
            include_str!("../common/prompts/auditor.md"),
            &[include_str!("prompts/etf_landmines.md")],
        ),

        // Tier 1 — verbatim reuse from common pool (no composition wrapping).
        bullish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/bullish_researcher.md"
        ))),
        bearish_researcher: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/bearish_researcher.md"
        ))),
        debate_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/debate_moderator.md"
        ))),
        risk_moderator: Cow::Borrowed(trim_trailing_newline(include_str!(
            "../common/prompts/risk_moderator.md"
        ))),

        // Tier 3 — fully new ETF roles (trader, risk, fund manager).
        trader: compose_etf_section(include_str!("prompts/trader.md"), &[]),
        aggressive_risk: compose_etf_risk(include_str!("prompts/aggressive_risk.md")),
        conservative_risk: compose_etf_risk(include_str!("prompts/conservative_risk.md")),
        neutral_risk: compose_etf_risk(include_str!("prompts/neutral_risk.md")),
        fund_manager: compose_etf_section(include_str!("prompts/fund_manager.md"), &[]),
    }
}

/// Build the ETF baseline pack manifest.
pub fn etf_baseline_pack() -> AnalysisPackManifest {
    AnalysisPackManifest {
        id: PackId::EtfBaseline,
        name: "ETF Baseline".to_owned(),
        description: "Phase 1 ETF-native analysis: premium/discount band, \
                       composition/sector tilt when fresh N-PORT data is available, \
                       and tracking error vs a source-provided benchmark. \
                       Sources: yfinance + SEC EDGAR N-PORT-P (free tier)."
            .to_owned(),
        required_inputs: vec![
            "fundamentals".to_owned(), // → Composition & Costs
            "sentiment".to_owned(),    // → Flow & Premium
            "news".to_owned(),         // → Macro & Sector
            "technical".to_owned(),    // → Tracking
        ],
        enrichment_intent: EnrichmentIntent {
            transcripts: false,
            consensus_estimates: false,
            event_news: true,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Premium/discount band classification anchors the assessment. \
                            Weight composition and tracking equally; flag leverage decay \
                            and AP arbitrage breakdown explicitly."
            .to_owned(),
        report_strategy_label: "ETF Baseline".to_owned(),
        default_valuation: ValuationAssessment::Etf,
        prompt_bundle: etf_baseline_prompt_bundle(),
        valuator_selection: {
            let mut m = HashMap::new();
            m.insert(AssetShape::Fund, ValuatorId::EtfPremiumDiscount);
            m
        },
        auditor_enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis_packs::resolve_pack;

    #[test]
    fn etf_baseline_pack_validates_successfully() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert!(pack.validate().is_ok(), "validation failed: {:?}", pack.validate());
    }

    #[test]
    fn etf_baseline_pack_has_correct_id() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(pack.id, PackId::EtfBaseline);
    }

    #[test]
    fn etf_baseline_required_inputs_drive_four_analyst_slots() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.required_inputs,
            vec!["fundamentals", "sentiment", "news", "technical"]
        );
    }

    #[test]
    fn etf_baseline_default_valuation_is_etf() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(pack.default_valuation, ValuationAssessment::Etf);
    }

    #[test]
    fn etf_baseline_fund_shape_resolves_to_etf_valuation() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::Fund),
            ValuationAssessment::Etf
        );
    }

    #[test]
    fn etf_baseline_corporate_equity_falls_through_to_full_per_resolve_rule() {
        // Sanity: the ETF pack doesn't list CorporateEquity in its
        // valuator_selection, but resolve_valuation maps it to Full when
        // default_valuation = Etf per the schema rule from Task 2.
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.resolve_valuation(&AssetShape::CorporateEquity),
            ValuationAssessment::Full
        );
    }

    #[test]
    fn etf_baseline_valuator_selection_maps_fund_shape() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert_eq!(
            pack.valuator_selection.get(&AssetShape::Fund).copied(),
            Some(ValuatorId::EtfPremiumDiscount)
        );
    }

    #[test]
    fn etf_baseline_populates_every_prompt_slot_with_runtime_placeholders() {
        let pack = resolve_pack(PackId::EtfBaseline);
        let slots = [
            ("fundamental_analyst", pack.prompt_bundle.fundamental_analyst.as_ref()),
            ("sentiment_analyst", pack.prompt_bundle.sentiment_analyst.as_ref()),
            ("news_analyst", pack.prompt_bundle.news_analyst.as_ref()),
            ("technical_analyst", pack.prompt_bundle.technical_analyst.as_ref()),
            ("trader", pack.prompt_bundle.trader.as_ref()),
            ("aggressive_risk", pack.prompt_bundle.aggressive_risk.as_ref()),
            ("conservative_risk", pack.prompt_bundle.conservative_risk.as_ref()),
            ("neutral_risk", pack.prompt_bundle.neutral_risk.as_ref()),
            ("fund_manager", pack.prompt_bundle.fund_manager.as_ref()),
        ];
        for (label, template) in slots {
            assert!(!template.is_empty(), "{label} must not be empty");
            assert!(template.contains("{ticker}"), "{label} must contain {{ticker}}");
            assert!(template.contains("{current_date}"), "{label} must contain {{current_date}}");
        }
    }

    #[test]
    fn etf_baseline_auditor_slot_is_non_empty() {
        let pack = resolve_pack(PackId::EtfBaseline);
        assert!(pack.auditor_enabled);
        assert!(!pack.prompt_bundle.auditor.is_empty());
    }
}
```

- [ ] **Step 3: Wire into the pack module + registry**

Edit `crates/scorpio-core/src/analysis_packs/mod.rs` and add `mod etf;` alongside the existing `mod` lines.

Edit `crates/scorpio-core/src/analysis_packs/registry.rs`:

```rust
use super::{AnalysisPackManifest, PackId, crypto, equity, etf, resolve_runtime_policy_for_manifest};

#[must_use]
pub fn resolve_pack(id: PackId) -> AnalysisPackManifest {
    match id {
        PackId::Baseline => equity::baseline_pack(),
        PackId::EtfBaseline => etf::etf_baseline_pack(),
        PackId::CryptoDigitalAsset => crypto::digital_asset_pack(),
    }
}

const REGISTERED_PACKS: &[PackId] = &[
    PackId::Baseline,
    PackId::EtfBaseline,
    PackId::CryptoDigitalAsset,
];
```

- [ ] **Step 4: Update the existing registry test that enumerates packs**

In `analysis_packs/registry.rs::tests`, extend `registered_packs_match_resolve_pack_arms` to assert `PackId::EtfBaseline` is contained, and confirm `pack_diagnostics_returns_empty_today` still holds (the ETF pack must be complete under the fully-enabled topology).

- [ ] **Step 5: Build, test, commit**

```bash
cargo test -p scorpio-core --lib analysis_packs
cargo test -p scorpio-core --test prompt_bundle_regression_gate
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/analysis_packs/
git commit -m "feat(etf): add ETF baseline pack manifest and registry wiring"
```

Expected: every ETF pack test from Step 2 passes, `pack_diagnostics()` stays empty, baseline tests still pass.

---

## Task 7: Extend `ValuationInputs` carrier with ETF fields

**Files:**
- Modify: `crates/scorpio-core/src/valuation/mod.rs`
- Create: `crates/scorpio-core/src/data/yfinance/etf.rs` — defines `EtfQuote`, `FundInfo` types (just types; methods land in Task 8)
- Create: `crates/scorpio-core/src/data/sec_edgar/nport_types.rs` — defines `NPortHoldings` (just types; methods land in Task 9)

Defining the types first lets every downstream task compile against a stable interface.

- [ ] **Step 1: Create `data/yfinance/etf.rs` with placeholder types**

```rust
//! ETF-specific yfinance types — quote, fund info, leverage detection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// ETF quote snapshot — extends the regular quote with NAV and bid/ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EtfQuote {
    pub symbol: String,
    pub regular_market_price: f64,
    pub previous_close: Option<f64>,
    pub nav: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub market_cap: Option<f64>,
    pub day_volume: Option<u64>,
    pub currency: Option<String>,
    pub as_of: DateTime<Utc>,
}

/// Fund-level metadata pulled from yfinance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundInfo {
    pub symbol: String,
    pub category: Option<String>,
    pub fund_family: Option<String>,
    pub expense_ratio: Option<f64>,
    pub total_assets: Option<f64>,
    /// `Some(1.0)` for plain ETFs; `Some(2.0)`, `Some(3.0)`, `Some(-1.0)`,
    /// etc. for leveraged/inverse. `None` when undetermined.
    pub leverage_factor: Option<f64>,
    /// e.g. "etf", "mutual_fund". Lowercased.
    pub fund_kind: Option<String>,
    /// Stated benchmark symbol or index name when present in fund metadata.
    pub stated_benchmark: Option<String>,
}

/// Subset of supported ETF kinds. Used by [`is_supported_etf_kind`] in
/// runtime classification.
#[must_use]
pub fn is_supported_etf_kind(kind: &str) -> bool {
    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "etf" | "exchange-traded fund" | "exchangetradedfund"
    )
}
```

Then add `pub mod etf;` to `crates/scorpio-core/src/data/yfinance/mod.rs` and re-export `EtfQuote`, `FundInfo`, `is_supported_etf_kind`.

- [ ] **Step 2: Create `data/sec_edgar/nport_types.rs`**

Refactor `sec_edgar.rs` into a directory only if you must — for this slice keep the existing flat `sec_edgar.rs` and create a sibling file `sec_edgar_nport.rs` instead. Path:

```
crates/scorpio-core/src/data/sec_edgar_nport.rs
```

Content:

```rust
//! Lightweight types for SEC N-PORT-P holdings (placeholder).
//!
//! The XBRL parser lands in Task 9; this file declares the shape so
//! downstream code can compile.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortHoldingRow {
    pub cusip: Option<String>,
    pub ticker: Option<String>,
    pub name: String,
    pub weight_pct: f64,
    pub value_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortSectorRow {
    pub sector: String,
    pub weight_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortHoldings {
    pub filing_date: NaiveDate,
    pub holdings: Vec<NPortHoldingRow>,
    pub sector_breakdown: Vec<NPortSectorRow>,
    pub stated_benchmark: Option<String>,
}
```

Register in `data/mod.rs`: add `pub mod sec_edgar_nport;` and re-export `NPortHoldings`.

- [ ] **Step 3: Extend `ValuationInputs<'a>`**

Edit `crates/scorpio-core/src/valuation/mod.rs`. Add the new optional fields **after** the existing equity inputs:

```rust
pub struct ValuationInputs<'a> {
    // Equity inputs (unchanged)
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
    pub etf_benchmark_ohlcv: Option<&'a [crate::data::yfinance::Candle]>,
}
```

- [ ] **Step 4: Fix the one existing call site**

`crates/scorpio-core/src/workflow/tasks/analyst.rs` constructs `ValuationInputs` inside `derive_runtime_valuation` (around line 738). Add the four new ETF fields as `None`:

```rust
valuator.assess(
    crate::valuation::ValuationInputs {
        profile: valuation_inputs.profile.clone(),
        cashflow: valuation_inputs.cashflow.as_deref(),
        balance: valuation_inputs.balance.as_deref(),
        income: valuation_inputs.income.as_deref(),
        shares: valuation_inputs.shares.as_deref(),
        earnings_trend: valuation_inputs.trend.as_deref(),
        current_price,
        etf_quote: None,
        etf_fund_info: None,
        etf_holdings: None,
        etf_benchmark_ohlcv: None,
    },
    &provisional.asset_shape,
)
```

Also fix the test in `crates/scorpio-core/src/valuation/equity/default.rs` (the `assess_with_no_inputs_matches_derive_valuation_not_assessed_path` test constructs `ValuationInputs { ... }` literal):

```rust
let inputs = ValuationInputs {
    profile: None,
    cashflow: None,
    balance: None,
    income: None,
    shares: None,
    earnings_trend: None,
    current_price: None,
    etf_quote: None,
    etf_fund_info: None,
    etf_holdings: None,
    etf_benchmark_ohlcv: None,
};
```

- [ ] **Step 5: Build + commit**

```bash
cargo check -p scorpio-core
cargo test -p scorpio-core --lib valuation
git add crates/scorpio-core/src/valuation/ crates/scorpio-core/src/data/yfinance/ crates/scorpio-core/src/data/sec_edgar_nport.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/workflow/tasks/analyst.rs
git commit -m "feat(valuation): extend ValuationInputs with optional ETF fields"
```

---

## Task 8: yfinance ETF data methods (`get_quote`, `get_fund_info`, `get_distribution_yield_ttm`)

**Files:**
- Modify: `crates/scorpio-core/src/data/yfinance/etf.rs` — add `impl YFinanceClient` block with the three methods

Each method returns `Option<T>` and degrades fail-soft per the existing yfinance pattern (`get_quarterly_cashflow`, etc.).

- [ ] **Step 1: Add `get_quote`**

Append to `data/yfinance/etf.rs`:

```rust
use chrono::Utc;
use tracing::warn;
use yfinance_rs::ticker::Ticker;

use super::ohlcv::YFinanceClient;

impl YFinanceClient {
    /// Fetch a quote + NAV snapshot for an ETF symbol.
    ///
    /// Fail-soft: returns `None` on network failure, missing payload, or any
    /// upstream error. The caller decides whether to abort.
    pub async fn get_quote(&self, symbol: &str) -> Option<EtfQuote> {
        let ticker = match self
            .session
            .with_rate_limit(async {
                Ok::<_, yfinance_rs::YfError>(Ticker::new(self.session.client(), symbol))
            })
            .await
        {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                warn!(error = %e, symbol, "failed to construct ticker for ETF quote");
                return None;
            }
            None => return None,
        };

        let quote = self
            .session
            .with_rate_limit(ticker.quote())
            .await
            .map(|r| r.ok())
            .flatten()?;

        let info = self
            .session
            .with_rate_limit(ticker.info())
            .await
            .map(|r| r.ok())
            .flatten();

        Some(EtfQuote {
            symbol: symbol.to_uppercase(),
            regular_market_price: quote.regular_market_price.unwrap_or(0.0),
            previous_close: quote.regular_market_previous_close,
            nav: info.as_ref().and_then(|i| i.nav_price),
            bid: quote.bid,
            ask: quote.ask,
            market_cap: quote.market_cap,
            day_volume: quote.regular_market_volume.map(|v| v as u64),
            currency: quote.currency.clone(),
            as_of: Utc::now(),
        })
    }
}
```

Note: the `yfinance_rs` 0.7 surface for `Ticker::quote()` / `Ticker::info()` may differ from this sketch — when implementing, open `~/.cargo/registry/src/.../yfinance-rs-0.7.*/src/ticker.rs` (the crate is already in `Cargo.lock`) and read the actual return types. Field names like `nav_price`, `regular_market_volume`, etc. may need adjustment. The plan's contract is the **return type** (`Option<EtfQuote>`); how its fields are populated is a small implementation detail.

- [ ] **Step 2: Add `get_fund_info`**

```rust
impl YFinanceClient {
    /// Fetch ETF-level metadata (category, expense ratio, leverage, fund kind).
    pub async fn get_fund_info(&self, symbol: &str) -> Option<FundInfo> {
        let ticker = Ticker::new(self.session.client(), symbol);

        let info = self
            .session
            .with_rate_limit(ticker.info())
            .await
            .and_then(|r| r.ok())?;

        let category = info.category.clone();
        let fund_family = info.fund_family.clone();
        let expense_ratio = info.net_expense_ratio.or(info.gross_expense_ratio);
        let total_assets = info.total_assets;
        let stated_benchmark = info.benchmark.clone();
        let fund_kind = info.quote_type.as_ref().map(|s| s.to_ascii_lowercase());
        let leverage_factor = derive_leverage_factor(info.fund_name.as_deref(), &category);

        Some(FundInfo {
            symbol: symbol.to_uppercase(),
            category,
            fund_family,
            expense_ratio,
            total_assets,
            leverage_factor,
            fund_kind,
            stated_benchmark,
        })
    }
}

/// Heuristic leverage detection from fund name and category.
/// Returns `Some(1.0)` for a plain ETF when neither the name nor the
/// category names a leverage multiplier; returns `Some(2.0)`/`Some(3.0)`/etc.
/// when a known multiplier prefix is present. Defaults to `Some(1.0)`.
fn derive_leverage_factor(fund_name: Option<&str>, category: &Option<String>) -> Option<f64> {
    let haystack = format!(
        "{} {}",
        fund_name.unwrap_or(""),
        category.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();
    if haystack.contains("3x") || haystack.contains("ultra pro") {
        Some(3.0)
    } else if haystack.contains("2x") || haystack.contains("ultra") {
        Some(2.0)
    } else if haystack.contains("inverse") || haystack.contains("-1x") || haystack.contains("short") {
        Some(-1.0)
    } else {
        Some(1.0)
    }
}
```

Again, the `yfinance_rs::Info` field surface is the runtime spec — implement against the real fields after opening the crate locally.

- [ ] **Step 3: Add `get_distribution_yield_ttm`**

```rust
impl YFinanceClient {
    /// Compute trailing-twelve-month distribution yield as
    /// `(sum of last 12 months of distributions) / current_price`.
    /// Returns `None` when no distribution history is available.
    pub async fn get_distribution_yield_ttm(&self, symbol: &str) -> Option<f64> {
        let ticker = Ticker::new(self.session.client(), symbol);
        let distributions = self
            .session
            .with_rate_limit(ticker.dividends(None, None))
            .await
            .and_then(|r| r.ok())?;
        let now = Utc::now();
        let cutoff = now - chrono::Duration::days(365);
        let ttm_sum: f64 = distributions
            .iter()
            .filter(|d| d.date >= cutoff)
            .map(|d| d.amount.amount().to_f64().unwrap_or(0.0))
            .sum();
        if ttm_sum <= 0.0 {
            return None;
        }
        let quote = self.get_quote(symbol).await?;
        if quote.regular_market_price <= 0.0 {
            return None;
        }
        Some(ttm_sum / quote.regular_market_price * 100.0)
    }
}
```

Add `use num_traits::ToPrimitive as _;` to the top of the file if `to_f64` requires it (it does for `rust_decimal` types).

- [ ] **Step 4: Add unit tests with stubbed responses**

Append to the same file. Mirror the stubbed-financials pattern in `data/yfinance/financials.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_supported_etf_kind_matches_known_variants() {
        assert!(is_supported_etf_kind("etf"));
        assert!(is_supported_etf_kind("ETF"));
        assert!(is_supported_etf_kind("Exchange-Traded Fund"));
        assert!(!is_supported_etf_kind("mutual_fund"));
        assert!(!is_supported_etf_kind(""));
    }

    #[test]
    fn derive_leverage_factor_detects_3x() {
        assert_eq!(derive_leverage_factor(Some("ProShares Ultra Pro QQQ"), &None), Some(3.0));
    }

    #[test]
    fn derive_leverage_factor_detects_2x() {
        assert_eq!(derive_leverage_factor(Some("ProShares Ultra QQQ"), &None), Some(2.0));
    }

    #[test]
    fn derive_leverage_factor_detects_inverse() {
        assert_eq!(
            derive_leverage_factor(Some("ProShares Short S&P 500"), &None),
            Some(-1.0)
        );
    }

    #[test]
    fn derive_leverage_factor_defaults_to_1x() {
        assert_eq!(derive_leverage_factor(Some("SPDR S&P 500 ETF Trust"), &None), Some(1.0));
        assert_eq!(derive_leverage_factor(None, &Some("Large Blend".to_owned())), Some(1.0));
    }
}
```

The `get_quote` / `get_fund_info` integration tests live in the live smoke example (Task 18); CI tests cover only the pure helpers above.

- [ ] **Step 5: Build + commit**

```bash
cargo test -p scorpio-core --lib data::yfinance::etf
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/data/yfinance/etf.rs crates/scorpio-core/src/data/yfinance/mod.rs
git commit -m "feat(data): add yfinance ETF methods (get_quote, get_fund_info, get_distribution_yield_ttm)"
```

---

## Task 9: SEC EDGAR N-PORT-P client (`resolve_fund_cik`, `fetch_latest_nport_p`)

**Files:**
- Modify: `crates/scorpio-core/src/data/sec_edgar.rs` (add two methods)
- Create: `crates/scorpio-core/src/data/sec_edgar/nport.rs` (XBRL parser)

The existing `SecEdgarClient::lookup_cik` is already the implementation of "resolve a ticker → CIK". The plan calls for a thin wrapper named `resolve_fund_cik` that returns the zero-padded `Option<String>` form expected by `fetch_latest_nport_p`.

- [ ] **Step 1: Create the directory + parser module**

```bash
mkdir -p crates/scorpio-core/src/data/sec_edgar
```

Move the existing flat `sec_edgar.rs` into the directory: `git mv crates/scorpio-core/src/data/sec_edgar.rs crates/scorpio-core/src/data/sec_edgar/mod.rs`. Then create `crates/scorpio-core/src/data/sec_edgar/nport.rs` next to it:

```rust
//! SEC N-PORT-P parser.
//!
//! Parses the XBRL "primary document" of an N-PORT-P filing into the
//! [`NPortHoldings`] shape. Fail-soft: returns `Ok(None)` for any
//! schema-mismatch or partial-data condition; only returns `Err` for
//! transport-level errors at the caller.

use chrono::NaiveDate;
use serde::Deserialize;

use crate::data::sec_edgar_nport::{NPortHoldings, NPortHoldingRow, NPortSectorRow};

/// Try to parse an N-PORT-P primary XBRL document.
///
/// The N-PORT-P schema groups holdings under `<invstOrSecs>` (invested
/// securities). Sector breakdowns are reported via `<isPrtfRiskMetric>` tags
/// in some filings and as a derived sum-by-industry in others; this parser
/// supports both shapes and falls back to an empty sector vec when neither
/// is present.
pub fn parse_nport_p(xml: &str, filing_date: NaiveDate) -> Option<NPortHoldings> {
    // Implementation: use `quick-xml` for streaming or `roxmltree` for DOM-style.
    // For Phase 1 keep it minimal:
    //   1. Walk the document tree once, extract every <invstOrSec> child.
    //   2. For each child, pull <name>, <cusip>, <pctVal>, <valUSD>, <issuerType>.
    //   3. Sum top-level value to compute weight_pct when valUSD is present.
    //   4. Group by <issuerType> or <industryGroup> for sector breakdown.
    //   5. Return None if `holdings` ends up empty.
    //
    // The full parser body is implementation-time work; the contract is:
    //   - Never panics.
    //   - Returns `None` on any structurally-invalid document.
    //   - On success, weights are normalised to sum ~= 100.0% (±2.0 tolerance).
    None // <-- placeholder; implement during Task 9 Step 2.
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn parse_nport_p_returns_none_for_empty_input() {
        let result = parse_nport_p("", NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert!(result.is_none());
    }

    #[test]
    fn parse_nport_p_returns_none_for_garbage_xml() {
        let result = parse_nport_p("not xml", NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert!(result.is_none());
    }

    // Real fixture-driven tests land alongside the implementation in Step 2.
}
```

- [ ] **Step 2: Implement the XBRL parser**

Add `quick-xml = "0.36"` (or `roxmltree = "0.20"`) to `[workspace.dependencies]` in the root `Cargo.toml`, then to `scorpio-core/Cargo.toml`:

```toml
quick-xml.workspace = true
```

Replace the placeholder body of `parse_nport_p` with a real implementation that walks `<invstOrSec>` elements:

```rust
use quick_xml::events::Event;
use quick_xml::reader::Reader;

pub fn parse_nport_p(xml: &str, filing_date: NaiveDate) -> Option<NPortHoldings> {
    if xml.trim().is_empty() {
        return None;
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut holdings: Vec<NPortHoldingRow> = Vec::new();
    let mut sector_totals: std::collections::HashMap<String, f64> = Default::default();
    let mut stated_benchmark: Option<String> = None;

    let mut current: Option<PartialHolding> = None;
    let mut current_text: Vec<u8> = Vec::new();
    let mut path: Vec<Vec<u8>> = Vec::new();

    loop {
        match reader.read_event() {
            Err(_) => return None,
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_vec();
                path.push(name.clone());
                if name == b"invstOrSec" {
                    current = Some(PartialHolding::default());
                }
            }
            Ok(Event::Text(t)) => {
                current_text = t.into_inner().to_vec();
            }
            Ok(Event::End(e)) => {
                let name = e.name().as_ref().to_vec();
                if let Some(partial) = current.as_mut() {
                    fill_field(partial, &name, &current_text, &mut sector_totals);
                }
                if name == b"invstOrSec" {
                    if let Some(p) = current.take() {
                        if let Some(row) = p.into_row() {
                            holdings.push(row);
                        }
                    }
                }
                if name == b"benchmarkName" || name == b"indxName" {
                    let txt = String::from_utf8_lossy(&current_text).trim().to_owned();
                    if !txt.is_empty() {
                        stated_benchmark = Some(txt);
                    }
                }
                path.pop();
            }
            _ => {}
        }
    }

    if holdings.is_empty() {
        return None;
    }

    // Normalise weights: if `weight_pct` isn't filled but `value_usd` is,
    // recompute weights from the value totals.
    let total_value: f64 = holdings.iter().filter_map(|h| h.value_usd).sum();
    if total_value > 0.0 {
        for h in holdings.iter_mut() {
            if h.weight_pct == 0.0 {
                if let Some(v) = h.value_usd {
                    h.weight_pct = v / total_value * 100.0;
                }
            }
        }
    }

    let sector_breakdown: Vec<NPortSectorRow> = sector_totals
        .into_iter()
        .map(|(sector, weight_pct)| NPortSectorRow { sector, weight_pct })
        .collect();

    Some(NPortHoldings {
        filing_date,
        holdings,
        sector_breakdown,
        stated_benchmark,
    })
}

#[derive(Default)]
struct PartialHolding {
    name: Option<String>,
    cusip: Option<String>,
    ticker: Option<String>,
    weight_pct: f64,
    value_usd: Option<f64>,
}

impl PartialHolding {
    fn into_row(self) -> Option<NPortHoldingRow> {
        let name = self.name?;
        Some(NPortHoldingRow {
            cusip: self.cusip,
            ticker: self.ticker,
            name,
            weight_pct: self.weight_pct,
            value_usd: self.value_usd,
        })
    }
}

fn fill_field(
    partial: &mut PartialHolding,
    tag: &[u8],
    text: &[u8],
    sector_totals: &mut std::collections::HashMap<String, f64>,
) {
    let txt = String::from_utf8_lossy(text).trim().to_owned();
    match tag {
        b"name" => partial.name = Some(txt),
        b"cusip" => partial.cusip = Some(txt),
        b"pctVal" => {
            if let Ok(v) = txt.parse::<f64>() {
                partial.weight_pct = v;
            }
        }
        b"valUSD" => partial.value_usd = txt.parse::<f64>().ok(),
        b"issuerType" | b"industryGroup" => {
            let weight = partial.weight_pct;
            *sector_totals.entry(txt).or_insert(0.0) += weight;
        }
        _ => {}
    }
}
```

Add a fixture-driven test that exercises a known good payload. Place a small fixture under `crates/scorpio-core/tests/fixtures/nport/spy_2026_04_30_excerpt.xml` (≤2 KB, hand-trimmed N-PORT-P excerpt with 3 holdings) and add:

```rust
#[test]
fn parse_nport_p_extracts_three_holdings_from_fixture() {
    let xml = include_str!("../../../tests/fixtures/nport/spy_2026_04_30_excerpt.xml");
    let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 4, 30).unwrap())
        .expect("fixture should parse");
    assert_eq!(result.holdings.len(), 3);
    assert!(result.holdings.iter().any(|h| h.ticker.as_deref() == Some("AAPL")));
}
```

- [ ] **Step 3: Add `SecEdgarClient::resolve_fund_cik` and `fetch_latest_nport_p`**

In `crates/scorpio-core/src/data/sec_edgar/mod.rs`, append:

```rust
use chrono::Utc;

use crate::data::sec_edgar_nport::NPortHoldings;
use super::sec_edgar::nport::parse_nport_p; // adjust path post-move; see Step 1.

impl SecEdgarClient {
    /// Resolve a fund ticker to a zero-padded 10-digit CIK string.
    ///
    /// Wraps [`Self::lookup_cik`] and applies the standard EDGAR zero-padding.
    pub async fn resolve_fund_cik(&self, ticker: &str) -> Option<String> {
        match self.lookup_cik(ticker).await {
            Ok(Some(cik)) => Some(format!("{cik:010}")),
            _ => None,
        }
    }

    /// Fetch the most recent N-PORT-P filing for a fund CIK and parse it.
    ///
    /// Returns `None` when:
    /// - No N-PORT-P filing exists within `max_age_days`.
    /// - The filing cannot be downloaded or parsed.
    ///
    /// Fail-soft: never returns an error.
    pub async fn fetch_latest_nport_p(
        &self,
        cik: &str,
        max_age_days: u32,
    ) -> Option<NPortHoldings> {
        let parsed_cik: u32 = cik.trim_start_matches('0').parse().ok()?;
        let today = Utc::now().date_naive();
        let earliest = today - chrono::Duration::days(max_age_days as i64);
        let filings = self
            .fetch_recent_filings(parsed_cik, &["NPORT-P"], &earliest.to_string(), &today.to_string())
            .await
            .ok()?;
        let latest = filings.first()?;
        let filing_date = chrono::NaiveDate::parse_from_str(&latest.filing_date, "%Y-%m-%d").ok()?;
        let xml = self.fetch_document_text(&latest.primary_doc_url).await?;
        parse_nport_p(&xml, filing_date)
    }

    /// Fetch a raw filing document body, fail-soft.
    async fn fetch_document_text(&self, url: &str) -> Option<String> {
        self.limiter.acquire().await;
        match self.http.get(url).await {
            Ok((200, body)) => Some(body),
            Ok((status, _)) => {
                tracing::warn!(kind = "catalyst_fetch_failed", url, status, "non-200 N-PORT-P fetch");
                None
            }
            Err(e) => {
                tracing::warn!(kind = "catalyst_fetch_failed", url, error = %e, "transport error");
                None
            }
        }
    }
}
```

- [ ] **Step 4: Add tests covering the new surface**

Append to `sec_edgar/mod.rs::tests`:

```rust
#[tokio::test]
async fn resolve_fund_cik_returns_padded_string_when_lookup_succeeds() {
    let mut mock = MockEdgarHttp::new();
    mock.expect_get()
        .returning(|_| Ok((200, r#"{"0":{"cik_str":12345,"ticker":"SPY","title":"SPDR"}}"#.to_owned())));
    let client = SecEdgarClient::with_http(Arc::new(mock), SharedRateLimiter::disabled("test"));
    let cik = client.resolve_fund_cik("SPY").await;
    assert_eq!(cik.as_deref(), Some("0000012345"));
}

#[tokio::test]
async fn resolve_fund_cik_returns_none_for_unknown_ticker() {
    let mut mock = MockEdgarHttp::new();
    mock.expect_get()
        .returning(|_| Ok((200, r#"{}"#.to_owned())));
    let client = SecEdgarClient::with_http(Arc::new(mock), SharedRateLimiter::disabled("test"));
    let cik = client.resolve_fund_cik("BOGUS").await;
    assert_eq!(cik, None);
}

#[tokio::test]
async fn fetch_latest_nport_p_returns_none_when_no_filings_in_window() {
    let mut mock = MockEdgarHttp::new();
    mock.expect_get()
        .returning(|_| Ok((200, r#"{"filings":{"recent":{"form":[],"accessionNumber":[],"filingDate":[],"primaryDocument":[],"items":[]}}}"#.to_owned())));
    let client = SecEdgarClient::with_http(Arc::new(mock), SharedRateLimiter::disabled("test"));
    let result = client.fetch_latest_nport_p("0000012345", 90).await;
    assert!(result.is_none());
}
```

- [ ] **Step 5: Build + commit**

```bash
cargo test -p scorpio-core --lib data::sec_edgar
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/data/sec_edgar/ crates/scorpio-core/Cargo.toml Cargo.toml Cargo.lock crates/scorpio-core/tests/fixtures/nport/
git commit -m "feat(data): add SEC EDGAR N-PORT-P fetch + XBRL parser"
```

---

## Task 10: ETF valuator (`EtfPremiumDiscountValuator`)

**Files:**
- Create: `crates/scorpio-core/src/valuation/etf/mod.rs`
- Create: `crates/scorpio-core/src/valuation/etf/premium_discount.rs`
- Create: `crates/scorpio-core/src/valuation/etf/category_norms.rs`
- Create: `crates/scorpio-core/src/valuation/etf/tracking_error.rs`
- Modify: `crates/scorpio-core/src/valuation/mod.rs` (re-export)
- Modify: `crates/scorpio-core/src/valuation/registry.rs` (add `etf_baseline` factory)

- [ ] **Step 1: Create `category_norms.rs` — band thresholds by category**

```rust
//! Premium-band category norms.
//!
//! Phase 1 ships a hardcoded lookup table derived from the spec's
//! `etf_premium_reference.md`. Future revisions may load these from disk.

use crate::state::PremiumBand;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CategoryBand {
    pub elevated_pct: f64,
    pub extreme_pct: f64,
}

const DEFAULT_BAND: CategoryBand = CategoryBand { elevated_pct: 0.10, extreme_pct: 0.50 };

pub(crate) fn band_for_category(category: Option<&str>) -> CategoryBand {
    let Some(category) = category else {
        return DEFAULT_BAND;
    };
    match category.trim().to_ascii_lowercase().as_str() {
        "large blend" | "large growth" | "large value" => {
            CategoryBand { elevated_pct: 0.05, extreme_pct: 0.20 }
        }
        "small blend" | "small growth" | "small value" | "mid-cap blend" => {
            CategoryBand { elevated_pct: 0.15, extreme_pct: 0.50 }
        }
        "diversified emerging mkts" | "foreign large blend" => {
            CategoryBand { elevated_pct: 0.25, extreme_pct: 1.00 }
        }
        "long government" | "intermediate-term bond" | "high yield bond" => {
            CategoryBand { elevated_pct: 0.20, extreme_pct: 1.00 }
        }
        _ => DEFAULT_BAND,
    }
}

pub(crate) fn classify_band(premium_pct: Option<f64>, band: CategoryBand) -> PremiumBand {
    let Some(p) = premium_pct else {
        return PremiumBand::Unknown;
    };
    let mag = p.abs();
    if mag >= band.extreme_pct {
        PremiumBand::Extreme
    } else if mag >= band.elevated_pct {
        PremiumBand::Elevated
    } else {
        PremiumBand::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_lookup_defaults_when_category_unknown() {
        let b = band_for_category(Some("Thematic Bobsled ETF"));
        assert_eq!(b.elevated_pct, DEFAULT_BAND.elevated_pct);
    }

    #[test]
    fn band_lookup_handles_large_blend() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(b.extreme_pct, 0.20);
    }

    #[test]
    fn classify_band_returns_unknown_when_premium_missing() {
        assert_eq!(classify_band(None, DEFAULT_BAND), PremiumBand::Unknown);
    }

    #[test]
    fn classify_band_returns_normal_inside_elevated_threshold() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(0.02), b), PremiumBand::Normal);
    }

    #[test]
    fn classify_band_returns_elevated_above_elevated_threshold() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(0.08), b), PremiumBand::Elevated);
    }

    #[test]
    fn classify_band_returns_extreme_above_extreme_threshold() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(0.25), b), PremiumBand::Extreme);
    }

    #[test]
    fn classify_band_handles_negative_premium_symmetrically() {
        let b = band_for_category(Some("Large Blend"));
        assert_eq!(classify_band(Some(-0.25), b), PremiumBand::Extreme);
    }
}
```

- [ ] **Step 2: Create `tracking_error.rs`**

```rust
//! ETF vs benchmark tracking error.

use crate::data::yfinance::Candle;
use crate::state::TrackingError;

/// Annualised tracking error over a window of `n` calendar days.
/// Returns `None` when fewer than 30 overlapping samples are present.
pub(crate) fn compute_tracking_error(
    etf_ohlcv: &[Candle],
    benchmark_ohlcv: &[Candle],
    benchmark_symbol: &str,
) -> Option<TrackingError> {
    let etf_returns = daily_returns(etf_ohlcv);
    let bench_returns = daily_returns(benchmark_ohlcv);
    let aligned: Vec<(f64, f64)> = etf_returns
        .iter()
        .zip(bench_returns.iter())
        .map(|(&a, &b)| (a, b))
        .collect();
    if aligned.len() < 30 {
        return None;
    }
    let te_90 = stdev_of_diff(&aligned, 63);
    let te_1y = stdev_of_diff(&aligned, aligned.len().min(252));
    Some(TrackingError {
        benchmark_symbol: benchmark_symbol.to_owned(),
        te_pct_90d: annualise(te_90),
        te_pct_1y: annualise(te_1y),
        sample_days: aligned.len() as u32,
    })
}

fn daily_returns(candles: &[Candle]) -> Vec<f64> {
    candles
        .windows(2)
        .map(|w| (w[1].close - w[0].close) / w[0].close)
        .collect()
}

fn stdev_of_diff(pairs: &[(f64, f64)], window: usize) -> f64 {
    let window = window.min(pairs.len());
    if window < 2 {
        return 0.0;
    }
    let diffs: Vec<f64> = pairs.iter().rev().take(window).map(|(a, b)| a - b).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let var = diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (diffs.len() - 1) as f64;
    var.sqrt()
}

fn annualise(daily_stdev: f64) -> f64 {
    daily_stdev * (252_f64).sqrt() * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn synth_candle(close: f64) -> Candle {
        Candle {
            timestamp: chrono::Utc.timestamp_opt(0, 0).unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: 0,
        }
    }

    #[test]
    fn compute_tracking_error_returns_none_for_short_series() {
        let etf: Vec<Candle> = (0..10).map(|i| synth_candle(100.0 + i as f64)).collect();
        let bench: Vec<Candle> = (0..10).map(|i| synth_candle(100.0 + i as f64)).collect();
        assert!(compute_tracking_error(&etf, &bench, "^GSPC").is_none());
    }

    #[test]
    fn compute_tracking_error_returns_zero_when_series_identical() {
        let etf: Vec<Candle> = (0..100).map(|i| synth_candle(100.0 + i as f64)).collect();
        let bench: Vec<Candle> = (0..100).map(|i| synth_candle(100.0 + i as f64)).collect();
        let te = compute_tracking_error(&etf, &bench, "^GSPC").expect("expected Some");
        assert!(te.te_pct_90d.abs() < 1e-9);
    }
}
```

Note: the `Candle` field names must match the real type in `data/yfinance/ohlcv.rs` (probably `timestamp`, `open`, `high`, `low`, `close`, `volume`); when implementing, open that file and confirm. Adjust the test stub accordingly.

- [ ] **Step 3: Create `premium_discount.rs`**

```rust
//! Premium/discount valuator entry point.

use std::sync::Arc;

use crate::data::yfinance::etf::{EtfQuote, FundInfo};
use crate::data::sec_edgar_nport::NPortHoldings;
use crate::state::{
    AssetShape, DerivedValuation, EtfComposition, EtfDataAvailability, EtfValuation,
    HoldingWeight, PremiumBand, PremiumSnapshot, ScenarioValuation, SectorWeight,
};
use crate::valuation::{ValuationInputs, ValuationReport, Valuator, ValuatorId};

use super::category_norms::{band_for_category, classify_band};
use super::tracking_error::compute_tracking_error;

pub struct EtfPremiumDiscountValuator;

impl Valuator for EtfPremiumDiscountValuator {
    fn id(&self) -> ValuatorId {
        ValuatorId::EtfPremiumDiscount
    }

    fn assess(&self, inputs: ValuationInputs<'_>, shape: &AssetShape) -> ValuationReport {
        if !matches!(shape, AssetShape::Fund) {
            return DerivedValuation {
                asset_shape: shape.clone(),
                scenario: ScenarioValuation::NotAssessed {
                    reason: "etf_valuator_wrong_shape".to_owned(),
                },
            };
        }

        let mut flags = EtfDataAvailability::default();

        let Some(snapshot) = build_premium_snapshot(inputs.etf_quote, inputs.etf_fund_info, &mut flags) else {
            return DerivedValuation {
                asset_shape: shape.clone(),
                scenario: ScenarioValuation::NotAssessed {
                    reason: "etf_quote_unavailable".to_owned(),
                },
            };
        };

        let composition = inputs
            .etf_holdings
            .and_then(|h| build_composition(h, inputs.etf_fund_info, &mut flags));

        let tracking = match (inputs.etf_fund_info, inputs.etf_benchmark_ohlcv) {
            (Some(info), Some(bench)) if info.stated_benchmark.is_some() => {
                let symbol = info.stated_benchmark.as_deref().unwrap_or("^GSPC");
                // ETF OHLCV is also Some in practice (analyst already fetched it);
                // for the Phase 1 valuator we accept that only the benchmark is
                // strictly required here — the ETF series is loaded by the analyst
                // sync stage and merged in as part of inputs.etf_benchmark_ohlcv
                // when fully wired in Task 13. Phase 1 returns None if ETF ohlcv
                // is unavailable to the valuator.
                let etf_ohlcv: &[crate::data::yfinance::Candle] = &[];
                let te = compute_tracking_error(etf_ohlcv, bench, symbol);
                if te.is_some() {
                    flags.benchmark_resolved = true;
                }
                te
            }
            _ => None,
        };

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
    }
}

fn build_premium_snapshot(
    quote: Option<&EtfQuote>,
    fund_info: Option<&FundInfo>,
    flags: &mut EtfDataAvailability,
) -> Option<PremiumSnapshot> {
    let quote = quote?;
    let market_price = quote.regular_market_price;
    if market_price <= 0.0 {
        return None;
    }
    flags.nav_available = quote.nav.is_some();
    flags.bid_ask_available = quote.bid.is_some() && quote.ask.is_some();
    flags.expense_ratio_available = fund_info.and_then(|f| f.expense_ratio).is_some();
    let premium_pct = quote
        .nav
        .filter(|&nav| nav > 0.0)
        .map(|nav| (market_price - nav) / nav * 100.0);
    let spread = match (quote.bid, quote.ask) {
        (Some(b), Some(a)) if a > 0.0 => Some((a - b) / a * 100.0),
        _ => None,
    };
    let band_cfg = band_for_category(fund_info.and_then(|f| f.category.as_deref()));
    let band = classify_band(premium_pct, band_cfg);
    Some(PremiumSnapshot {
        nav: quote.nav,
        market_price,
        bid: quote.bid,
        ask: quote.ask,
        premium_pct,
        category_band: band,
        bid_ask_spread_pct: spread,
        as_of: quote.as_of,
    })
}

fn build_composition(
    nport: &NPortHoldings,
    fund_info: Option<&FundInfo>,
    flags: &mut EtfDataAvailability,
) -> Option<EtfComposition> {
    flags.holdings_present = !nport.holdings.is_empty();
    let today = chrono::Utc::now().date_naive();
    let age_days = (today - nport.filing_date).num_days().max(0) as u32;
    flags.holdings_fresh = age_days <= 90;
    if age_days > 180 {
        return None;
    }
    let mut sorted: Vec<&_> = nport.holdings.iter().collect();
    sorted.sort_by(|a, b| b.weight_pct.partial_cmp(&a.weight_pct).unwrap_or(std::cmp::Ordering::Equal));
    let top10: Vec<HoldingWeight> = sorted
        .iter()
        .take(10)
        .map(|row| HoldingWeight {
            cusip: row.cusip.clone(),
            ticker: row.ticker.clone(),
            name: row.name.clone(),
            weight_pct: row.weight_pct,
            value_usd: row.value_usd,
        })
        .collect();
    let top10_concentration_pct = top10.iter().map(|h| h.weight_pct).sum();
    let sector_weights: Vec<SectorWeight> = nport
        .sector_breakdown
        .iter()
        .map(|s| SectorWeight { sector: s.sector.clone(), weight_pct: s.weight_pct })
        .collect();
    Some(EtfComposition {
        top_holdings: top10,
        top10_concentration_pct,
        sector_weights,
        expense_ratio_pct: fund_info.and_then(|f| f.expense_ratio),
        aum_usd: fund_info.and_then(|f| f.total_assets),
        fund_family: fund_info.and_then(|f| f.fund_family.clone()),
        distribution_yield_ttm_pct: None, // filled by analyst sync from yfinance::get_distribution_yield_ttm
        holdings_filing_date: nport.filing_date,
        holdings_age_days: age_days,
    })
}
```

- [ ] **Step 4: Create `valuation/etf/mod.rs`**

```rust
//! ETF valuators.
//!
//! Phase 1: [`EtfPremiumDiscountValuator`] composes premium/discount band,
//! composition, and tracking error.

pub mod category_norms;
pub mod premium_discount;
pub mod tracking_error;

pub use premium_discount::EtfPremiumDiscountValuator;
```

- [ ] **Step 5: Register valuator in `valuation/mod.rs` + `valuation/registry.rs`**

Edit `valuation/mod.rs`:

```rust
pub mod equity;
pub mod etf;
pub mod registry;

pub use equity::EquityDefaultValuator;
pub use etf::EtfPremiumDiscountValuator;
pub use registry::ValuatorRegistry;
```

Edit `valuation/registry.rs`:

```rust
impl ValuatorRegistry {
    /// ETF-baseline registry — registers the ETF premium/discount valuator
    /// in addition to the equity default. Phase 1.
    #[must_use]
    pub fn etf_baseline() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(EquityDefaultValuator));
        reg.register(Arc::new(EtfPremiumDiscountValuator));
        reg
    }
}
```

Add the import: `use super::{EquityDefaultValuator, EtfPremiumDiscountValuator, Valuator, ValuatorId};`.

- [ ] **Step 6: Unit tests for the valuator**

Append to `premium_discount.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};

    fn quote_with(market_price: f64, nav: Option<f64>) -> EtfQuote {
        EtfQuote {
            symbol: "SPY".into(),
            regular_market_price: market_price,
            previous_close: None,
            nav,
            bid: Some(market_price - 0.01),
            ask: Some(market_price + 0.01),
            market_cap: None,
            day_volume: None,
            currency: Some("USD".into()),
            as_of: Utc::now(),
        }
    }

    fn fund_info_with(category: Option<&str>, leverage: Option<f64>) -> FundInfo {
        FundInfo {
            symbol: "SPY".into(),
            category: category.map(str::to_owned),
            fund_family: None,
            expense_ratio: Some(0.09),
            total_assets: None,
            leverage_factor: leverage,
            fund_kind: Some("etf".into()),
            stated_benchmark: Some("^GSPC".into()),
        }
    }

    fn inputs_with(quote: Option<EtfQuote>, info: Option<FundInfo>) -> (Option<EtfQuote>, Option<FundInfo>) {
        (quote, info)
    }

    #[test]
    fn assess_returns_not_assessed_when_quote_absent() {
        let (q, i) = inputs_with(None, Some(fund_info_with(Some("Large Blend"), Some(1.0))));
        let result = EtfPremiumDiscountValuator.assess(
            ValuationInputs {
                profile: None,
                cashflow: None,
                balance: None,
                income: None,
                shares: None,
                earnings_trend: None,
                current_price: None,
                etf_quote: q.as_ref(),
                etf_fund_info: i.as_ref(),
                etf_holdings: None,
                etf_benchmark_ohlcv: None,
            },
            &AssetShape::Fund,
        );
        assert!(matches!(
            result.scenario,
            ScenarioValuation::NotAssessed { ref reason } if reason == "etf_quote_unavailable"
        ));
    }

    #[test]
    fn assess_emits_unknown_band_when_nav_missing() {
        let q = quote_with(621.40, None);
        let i = fund_info_with(Some("Large Blend"), Some(1.0));
        let result = EtfPremiumDiscountValuator.assess(
            ValuationInputs {
                profile: None,
                cashflow: None, balance: None, income: None, shares: None,
                earnings_trend: None, current_price: None,
                etf_quote: Some(&q),
                etf_fund_info: Some(&i),
                etf_holdings: None,
                etf_benchmark_ohlcv: None,
            },
            &AssetShape::Fund,
        );
        let etf = match result.scenario {
            ScenarioValuation::Etf(e) => e,
            other => panic!("expected Etf variant, got {other:?}"),
        };
        assert!(!etf.flags.nav_available);
        assert!(etf.premium.premium_pct.is_none());
        assert_eq!(etf.premium.category_band, PremiumBand::Unknown);
    }

    #[test]
    fn assess_classifies_normal_band_at_005_premium() {
        let q = quote_with(621.40, Some(621.18));
        let i = fund_info_with(Some("Large Blend"), Some(1.0));
        let result = EtfPremiumDiscountValuator.assess(
            ValuationInputs {
                profile: None,
                cashflow: None, balance: None, income: None, shares: None,
                earnings_trend: None, current_price: None,
                etf_quote: Some(&q),
                etf_fund_info: Some(&i),
                etf_holdings: None,
                etf_benchmark_ohlcv: None,
            },
            &AssetShape::Fund,
        );
        let etf = match result.scenario {
            ScenarioValuation::Etf(e) => e,
            other => panic!("{:?}", other),
        };
        // 0.04% < 0.05% Large-Blend elevated threshold → Normal.
        assert_eq!(etf.premium.category_band, PremiumBand::Normal);
        assert!(etf.flags.nav_available);
        assert!(etf.flags.bid_ask_available);
    }

    #[test]
    fn assess_leverage_factor_passes_through() {
        let q = quote_with(50.0, Some(50.0));
        let i = fund_info_with(Some("Trading--Leveraged Equity"), Some(3.0));
        let result = EtfPremiumDiscountValuator.assess(
            ValuationInputs {
                profile: None,
                cashflow: None, balance: None, income: None, shares: None,
                earnings_trend: None, current_price: None,
                etf_quote: Some(&q),
                etf_fund_info: Some(&i),
                etf_holdings: None,
                etf_benchmark_ohlcv: None,
            },
            &AssetShape::Fund,
        );
        match result.scenario {
            ScenarioValuation::Etf(e) => assert_eq!(e.leverage_factor, Some(3.0)),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn assess_rejects_wrong_shape_with_specific_reason() {
        let q = quote_with(100.0, Some(100.0));
        let result = EtfPremiumDiscountValuator.assess(
            ValuationInputs {
                profile: None,
                cashflow: None, balance: None, income: None, shares: None,
                earnings_trend: None, current_price: None,
                etf_quote: Some(&q),
                etf_fund_info: None,
                etf_holdings: None,
                etf_benchmark_ohlcv: None,
            },
            &AssetShape::CorporateEquity,
        );
        assert!(matches!(
            result.scenario,
            ScenarioValuation::NotAssessed { ref reason } if reason == "etf_valuator_wrong_shape"
        ));
    }
}
```

- [ ] **Step 7: Build + commit**

```bash
cargo test -p scorpio-core --lib valuation::etf
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/valuation/
git commit -m "feat(etf): add EtfPremiumDiscountValuator + category norms + tracking error math"
```

---

## Task 11: Runtime pack classifier + builder wiring

**Files:**
- Create: `crates/scorpio-core/src/workflow/pack_classifier.rs`
- Modify: `crates/scorpio-core/src/workflow/mod.rs` (re-export)
- Modify: `crates/scorpio-core/src/workflow/builder.rs` (call classifier before building graph; wire `SecEdgarClient` into `AnalystSyncTask`)
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs` (carry `SecEdgarClient` through pipeline construction)
- Modify: `crates/scorpio-core/src/workflow/tasks/common.rs` (new context keys for routing reason)

The classifier itself is pure and synchronous given the data — the I/O (`get_profile`, `get_fund_info`) happens in `TradingPipeline::new` before calling the builder, so the builder gets a ready-made `RuntimePackSelection`.

- [ ] **Step 1: Create `pack_classifier.rs`**

```rust
//! Runtime pack classification.
//!
//! Decides which analysis pack a given symbol routes to, based on
//! yfinance Profile + fund metadata. Pure function: inputs in, decision out.

use crate::analysis_packs::PackId;
use crate::data::yfinance::etf::{is_supported_etf_kind, FundInfo};
use yfinance_rs::profile::Profile;

/// Outcome of runtime classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimePackSelection {
    /// Use the baseline equity pack. `reason` is recorded in routing metadata.
    Baseline { reason: &'static str },
    /// Use the ETF baseline pack.
    EtfBaseline,
}

impl RuntimePackSelection {
    pub fn pack_id(&self) -> PackId {
        match self {
            RuntimePackSelection::Baseline { .. } => PackId::Baseline,
            RuntimePackSelection::EtfBaseline => PackId::EtfBaseline,
        }
    }

    pub fn fallback_reason(&self) -> Option<&'static str> {
        match self {
            RuntimePackSelection::Baseline { reason } => Some(reason),
            RuntimePackSelection::EtfBaseline => None,
        }
    }
}

pub fn classify_runtime_pack(
    profile: Option<&Profile>,
    fund_info: Option<&FundInfo>,
) -> RuntimePackSelection {
    match profile {
        Some(Profile::Fund(_)) => match fund_info.and_then(|info| info.fund_kind.as_deref()) {
            Some(kind) if is_supported_etf_kind(kind) => RuntimePackSelection::EtfBaseline,
            _ => RuntimePackSelection::Baseline {
                reason: "unsupported_fund_shape",
            },
        },
        Some(Profile::Company(_)) => RuntimePackSelection::Baseline { reason: "corporate_equity" },
        None => RuntimePackSelection::Baseline { reason: "profile_lookup_unavailable" },
        // yfinance_rs::Profile is `#[non_exhaustive]` in the upstream crate.
        // Future variants fall back to baseline so the pipeline doesn't break.
        _ => RuntimePackSelection::Baseline { reason: "unknown_profile_shape" },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::yfinance::etf::FundInfo;

    fn fund_info(kind: Option<&str>) -> FundInfo {
        FundInfo {
            symbol: "SPY".into(),
            category: None,
            fund_family: None,
            expense_ratio: None,
            total_assets: None,
            leverage_factor: None,
            fund_kind: kind.map(str::to_owned),
            stated_benchmark: None,
        }
    }

    // We can't easily construct a yfinance_rs::Profile::Fund(_) without
    // hitting yfinance internals; these tests cover the None-input branches.

    #[test]
    fn no_profile_falls_back_with_lookup_unavailable_reason() {
        let result = classify_runtime_pack(None, None);
        assert_eq!(result, RuntimePackSelection::Baseline {
            reason: "profile_lookup_unavailable",
        });
    }

    #[test]
    fn no_profile_with_fund_info_still_falls_back() {
        // Defence-in-depth: even if some upstream gave us fund_info without
        // a profile, we don't override the safety fallback.
        let info = fund_info(Some("etf"));
        let result = classify_runtime_pack(None, Some(&info));
        assert_eq!(result, RuntimePackSelection::Baseline {
            reason: "profile_lookup_unavailable",
        });
    }
}
```

(Profile::Fund variant tests live in the live smoke example, since constructing one requires hitting yfinance internals.)

- [ ] **Step 2: Re-export from `workflow/mod.rs`**

```rust
pub mod pack_classifier;
pub use pack_classifier::{classify_runtime_pack, RuntimePackSelection};
```

- [ ] **Step 3: Wire `SecEdgarClient` into the pipeline**

Open `crates/scorpio-core/src/workflow/pipeline/runtime.rs` and locate the construction of `AnalystSyncTask`. Add a `SecEdgarClient` parameter to the constructor chain:

```rust
// In TradingPipeline::new and build_graph helpers, propagate sec_edgar_client.
// AnalystSyncTask::with_yfinance_and_edgar(...) gains the new dependency.
```

Modify the analyst sync constructor in `crates/scorpio-core/src/workflow/tasks/analyst.rs`:

```rust
impl AnalystSyncTask {
    #[must_use]
    pub fn with_yfinance_and_edgar(
        snapshot_store: Arc<SnapshotStore>,
        yfinance: YFinanceClient,
        sec_edgar: Arc<crate::data::sec_edgar::SecEdgarClient>,
        valuation_fetch_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            snapshot_store,
            yfinance,
            sec_edgar: Some(sec_edgar),
            valuation_fetch_timeout,
        })
    }
}
```

And add the field:

```rust
pub struct AnalystSyncTask {
    snapshot_store: Arc<SnapshotStore>,
    yfinance: YFinanceClient,
    sec_edgar: Option<Arc<crate::data::sec_edgar::SecEdgarClient>>,
    valuation_fetch_timeout: Duration,
}
```

Update `AnalystSyncTask::new` and `AnalystSyncTask::with_yfinance` to set `sec_edgar: None` (callers that don't supply one keep working — Task 13 actually consumes this field).

- [ ] **Step 4: Update `build_graph_from_pack` to thread the new `SecEdgarClient`**

In `crates/scorpio-core/src/workflow/builder.rs`, extend `PipelineDeps`:

```rust
pub struct PipelineDeps {
    pub config: Config,
    pub finnhub: FinnhubClient,
    pub fred: FredClient,
    pub yfinance: YFinanceClient,
    pub sec_edgar: Arc<crate::data::sec_edgar::SecEdgarClient>,
    pub snapshot_store: SnapshotStore,
    pub quick_handle: CompletionModelHandle,
    pub deep_handle: CompletionModelHandle,
}
```

In `build_graph_from_pack`, swap the `AnalystSyncTask::with_yfinance` call for the new constructor:

```rust
let analyst_sync = AnalystSyncTask::with_yfinance_and_edgar(
    Arc::clone(&snapshot_store),
    yfinance.clone(),
    Arc::clone(&sec_edgar),
    Duration::from_secs(config.llm.valuation_fetch_timeout_secs),
);
```

Update every existing caller in `pipeline/runtime.rs` to construct a `SecEdgarClient` (the existing `Tier1CatalystProvider` already builds one — share it via `Arc`).

- [ ] **Step 5: Routing context keys**

In `crates/scorpio-core/src/workflow/tasks/common.rs`, add:

```rust
/// Set by preflight after runtime pack classification. Values: `"etf_baseline"`,
/// `"baseline"`.
pub const KEY_RUNTIME_PACK_ROUTE: &str = "routing.pack";

/// Set by preflight when classification fell back to baseline. Values: the
/// `&'static str` reason from `RuntimePackSelection::Baseline { reason }`.
/// Absent when the run used the ETF baseline pack.
pub const KEY_ROUTING_FALLBACK_REASON: &str = "routing.fallback_reason";
```

- [ ] **Step 6: Build (no test required yet — wiring lands in next tasks)**

```bash
cargo check -p scorpio-core
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/workflow/
git commit -m "feat(workflow): add pack classifier + plumb SecEdgarClient through pipeline"
```

---

## Task 12: Preflight records routing reason + warning metadata

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/preflight.rs`
- Modify: `crates/scorpio-core/src/state/trading_state.rs` (add `etf_routing_fallback_reason` field)

The current `PreflightTask::with_runtime_policy` receives an already-resolved policy. After Task 11 the *pack selection* is the caller's concern (it depends on async I/O the builder shouldn't do). So preflight's job here is purely to surface routing metadata into state + context.

- [ ] **Step 1: Add field to `TradingState`**

In `crates/scorpio-core/src/state/trading_state.rs`, add to the `TradingState` struct:

```rust
/// Reason for runtime pack-routing fallback, set when the active pack is
/// not the symbol's expected pack. `None` when routing matched. Surfaced in
/// the final report header.
#[serde(default)]
pub etf_routing_fallback_reason: Option<String>,
```

Mirror the same field in `TradingStateWire` and the `From<TradingStateWire>` impl.

- [ ] **Step 2: Add constructor that carries routing reason**

In `preflight.rs`, add:

```rust
impl PreflightTask {
    /// Like [`Self::with_runtime_policy`] but also propagates a
    /// routing-fallback reason captured upstream by the runtime classifier.
    pub fn with_runtime_policy_and_routing(
        enrichment: crate::config::DataEnrichmentConfig,
        transcripts_enabled: bool,
        snapshot_store: Arc<SnapshotStore>,
        runtime_policy: RuntimePolicy,
        routing_fallback_reason: Option<&'static str>,
    ) -> Self {
        Self {
            enrichment,
            transcripts_enabled,
            snapshot_store,
            runtime_policy: Ok(runtime_policy),
            routing_fallback_reason: routing_fallback_reason.map(str::to_owned),
        }
    }
}
```

Add the field on the struct:

```rust
pub struct PreflightTask {
    enrichment: crate::config::DataEnrichmentConfig,
    transcripts_enabled: bool,
    snapshot_store: Arc<SnapshotStore>,
    runtime_policy: Result<RuntimePolicy, String>,
    routing_fallback_reason: Option<String>,
}
```

Initialize the new field on the existing `new`, `with_pack`, `with_runtime_policy` constructors as `None`.

- [ ] **Step 3: Write routing context keys + state field inside `run`**

In `PreflightTask::run`, after the existing `state.analysis_pack_name = Some(runtime_policy.pack_id.to_string());` line, add:

```rust
state.etf_routing_fallback_reason = self.routing_fallback_reason.clone();

context
    .set(
        super::common::KEY_RUNTIME_PACK_ROUTE,
        runtime_policy.pack_id.as_str().to_owned(),
    )
    .await;
if let Some(reason) = self.routing_fallback_reason.as_deref() {
    context
        .set(super::common::KEY_ROUTING_FALLBACK_REASON, reason.to_owned())
        .await;
}
```

- [ ] **Step 4: Tests**

Append to `preflight.rs::tests`:

```rust
#[tokio::test]
async fn preflight_records_routing_fallback_reason_in_state_and_context() {
    let (store, _dir) = test_store().await;
    let state = TradingState::new("AAPL", "2026-01-15");
    let ctx = Context::new();
    serialize_state_to_context(&state, &ctx).await.expect("state ser");

    let task = PreflightTask::with_runtime_policy_and_routing(
        DataEnrichmentConfig::default(),
        false,
        store,
        crate::analysis_packs::resolve_runtime_policy("baseline").expect("baseline"),
        Some("profile_lookup_unavailable"),
    );
    task.run(ctx.clone()).await.expect("ok");

    let route: String = ctx.get(crate::workflow::tasks::common::KEY_RUNTIME_PACK_ROUTE).await.unwrap();
    assert_eq!(route, "baseline");
    let reason: String = ctx
        .get(crate::workflow::tasks::common::KEY_ROUTING_FALLBACK_REASON)
        .await
        .expect("reason should be set");
    assert_eq!(reason, "profile_lookup_unavailable");

    let after = deserialize_state_from_context(&ctx).await.expect("deser");
    assert_eq!(after.etf_routing_fallback_reason.as_deref(), Some("profile_lookup_unavailable"));
}

#[tokio::test]
async fn preflight_does_not_set_fallback_reason_for_matched_routing() {
    let ctx = run_preflight("AAPL", DataEnrichmentConfig::default())
        .await
        .expect("ok");
    let reason: Option<String> = ctx
        .get(crate::workflow::tasks::common::KEY_ROUTING_FALLBACK_REASON)
        .await;
    assert!(reason.is_none());
}
```

- [ ] **Step 5: Build + commit**

```bash
cargo test -p scorpio-core --lib workflow::tasks::preflight
cargo test -p scorpio-core --test state_roundtrip
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/workflow/tasks/preflight.rs crates/scorpio-core/src/state/trading_state.rs
git commit -m "feat(workflow): preflight records ETF routing fallback reason in state + context"
```

---

## Task 13: `AnalystSyncTask` ETF input hydration

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`

When the active pack is `EtfBaseline`, fetch ETF quote, fund info, fund CIK, N-PORT-P, distribution yield, and benchmark OHLCV (when source-provided benchmark is present). Stuff these into `ValuationInputs` and call the ETF valuator.

- [ ] **Step 1: Extend the local `ValuationInputs` struct**

`workflow/tasks/analyst.rs` already declares a private `struct ValuationInputs` (around line 572) that mirrors the `valuation::ValuationInputs` fields. Add ETF storage:

```rust
#[derive(Debug)]
struct ValuationInputs {
    profile: Option<Profile>,
    cashflow: Option<Vec<CashflowRow>>,
    balance: Option<Vec<BalanceSheetRow>>,
    income: Option<Vec<IncomeStatementRow>>,
    shares: Option<Vec<ShareCount>>,
    trend: Option<Vec<EarningsTrendRow>>,
    // ETF inputs — populated only when pack == EtfBaseline.
    etf_quote: Option<crate::data::yfinance::etf::EtfQuote>,
    etf_fund_info: Option<crate::data::yfinance::etf::FundInfo>,
    etf_holdings: Option<crate::data::sec_edgar_nport::NPortHoldings>,
    etf_benchmark_ohlcv: Option<Vec<crate::data::yfinance::Candle>>,
    /// Cached TTM distribution yield (from yfinance), used to fill
    /// `EtfComposition.distribution_yield_ttm_pct` after the valuator returns.
    etf_distribution_yield_ttm_pct: Option<f64>,
}
```

- [ ] **Step 2: Extend `fetch_valuation_inputs` to branch on active pack**

```rust
async fn fetch_valuation_inputs(
    yfinance: &YFinanceClient,
    sec_edgar: Option<&Arc<crate::data::sec_edgar::SecEdgarClient>>,
    pack_id: crate::analysis_packs::PackId,
    symbol: &str,
    fetch_timeout: Duration,
) -> ValuationInputs {
    // Existing equity fetches.
    let (profile, cashflow, balance, income, shares, trend) = tokio::join!(
        fetch_with_timeout(symbol, "profile", fetch_timeout, yfinance.get_profile(symbol)),
        fetch_with_timeout(symbol, "quarterly_cashflow", fetch_timeout, yfinance.get_quarterly_cashflow(symbol)),
        fetch_with_timeout(symbol, "quarterly_balance_sheet", fetch_timeout, yfinance.get_quarterly_balance_sheet(symbol)),
        fetch_with_timeout(symbol, "quarterly_income_stmt", fetch_timeout, yfinance.get_quarterly_income_stmt(symbol)),
        fetch_with_timeout(symbol, "quarterly_shares", fetch_timeout, yfinance.get_quarterly_shares(symbol)),
        fetch_with_timeout(symbol, "earnings_trend", fetch_timeout, yfinance.get_earnings_trend(symbol)),
    );

    let mut etf_quote = None;
    let mut etf_fund_info = None;
    let mut etf_holdings = None;
    let mut etf_benchmark_ohlcv = None;
    let mut etf_distribution_yield_ttm_pct = None;

    if pack_id == crate::analysis_packs::PackId::EtfBaseline {
        let (quote_opt, info_opt, yld_opt) = tokio::join!(
            fetch_with_timeout(symbol, "etf_quote", fetch_timeout, yfinance.get_quote(symbol)),
            fetch_with_timeout(symbol, "etf_fund_info", fetch_timeout, yfinance.get_fund_info(symbol)),
            fetch_with_timeout(symbol, "etf_dist_yield", fetch_timeout, yfinance.get_distribution_yield_ttm(symbol)),
        );
        etf_quote = quote_opt;
        etf_fund_info = info_opt;
        etf_distribution_yield_ttm_pct = yld_opt;

        if let Some(edgar) = sec_edgar {
            if let Some(cik) = fetch_with_timeout(symbol, "fund_cik", fetch_timeout, edgar.resolve_fund_cik(symbol)).await {
                etf_holdings = fetch_with_timeout(symbol, "nport_holdings", fetch_timeout, edgar.fetch_latest_nport_p(&cik, 180)).await;
            }
        }

        if let Some(bench) = etf_fund_info.as_ref().and_then(|i| i.stated_benchmark.clone()) {
            etf_benchmark_ohlcv = fetch_with_timeout(
                symbol,
                "etf_benchmark_ohlcv",
                fetch_timeout,
                yfinance.get_ohlcv(&bench, "1y"),
            )
            .await;
        }
    }

    ValuationInputs {
        profile, cashflow, balance, income, shares, trend,
        etf_quote, etf_fund_info, etf_holdings, etf_benchmark_ohlcv,
        etf_distribution_yield_ttm_pct,
    }
}
```

If `yfinance::get_ohlcv` doesn't exist with that exact signature (check `data/yfinance/ohlcv.rs`), use the existing equivalent (`fetch_ohlcv` / `get_history` / etc.) and adjust accordingly.

- [ ] **Step 3: Pass `sec_edgar` and `pack_id` into the fetcher call site**

Inside `AnalystSyncTask::run`, locate the existing `fetch_valuation_inputs` call and update:

```rust
let pack_id = state
    .analysis_runtime_policy
    .as_ref()
    .map(|p| p.pack_id)
    .unwrap_or(crate::analysis_packs::PackId::Baseline);
let valuation_inputs = fetch_valuation_inputs(
    &self.yfinance,
    self.sec_edgar.as_ref(),
    pack_id,
    &symbol,
    self.valuation_fetch_timeout,
).await;
```

- [ ] **Step 4: Route the ETF valuator through `derive_runtime_valuation`**

The existing `derive_runtime_valuation` already consults `policy.valuator_selection` and calls `ValuatorRegistry::equity_baseline()`. Replace that with a registry that knows the ETF valuator too:

```rust
let registry = match state.analysis_runtime_policy.as_ref().map(|p| p.pack_id) {
    Some(crate::analysis_packs::PackId::EtfBaseline) => ValuatorRegistry::etf_baseline(),
    _ => ValuatorRegistry::equity_baseline(),
};
```

And populate the ETF fields on the inner `ValuationInputs` literal:

```rust
valuator.assess(
    crate::valuation::ValuationInputs {
        profile: valuation_inputs.profile.clone(),
        cashflow: valuation_inputs.cashflow.as_deref(),
        balance: valuation_inputs.balance.as_deref(),
        income: valuation_inputs.income.as_deref(),
        shares: valuation_inputs.shares.as_deref(),
        earnings_trend: valuation_inputs.trend.as_deref(),
        current_price,
        etf_quote: valuation_inputs.etf_quote.as_ref(),
        etf_fund_info: valuation_inputs.etf_fund_info.as_ref(),
        etf_holdings: valuation_inputs.etf_holdings.as_ref(),
        etf_benchmark_ohlcv: valuation_inputs.etf_benchmark_ohlcv.as_deref(),
    },
    &provisional.asset_shape,
)
```

- [ ] **Step 5: Post-process distribution yield into the composition**

After `state.set_derived_valuation(...)`, fill in the distribution yield (which the valuator left as `None`):

```rust
if let Some(yld) = valuation_inputs.etf_distribution_yield_ttm_pct {
    if let Some(dv) = state.derived_valuation_mut() {
        if let crate::state::ScenarioValuation::Etf(etf) = &mut dv.scenario {
            if let Some(comp) = etf.composition.as_mut() {
                comp.distribution_yield_ttm_pct = Some(yld);
            }
        }
    }
}
```

Add a `derived_valuation_mut(&mut self) -> Option<&mut DerivedValuation>` accessor on `EquityState` / `TradingState` if not already present (mirror the read-side `derived_valuation` getter). Single line addition; check `state/equity.rs` for the existing accessor pattern.

- [ ] **Step 6: Sanity test (logic-only)**

Append to the `mod tests` block in `analyst.rs`:

```rust
#[test]
fn etf_routing_selects_etf_baseline_registry() {
    let pack_id = crate::analysis_packs::PackId::EtfBaseline;
    let registry = match pack_id {
        crate::analysis_packs::PackId::EtfBaseline => {
            crate::valuation::ValuatorRegistry::etf_baseline()
        }
        _ => crate::valuation::ValuatorRegistry::equity_baseline(),
    };
    assert!(registry.get(crate::valuation::ValuatorId::EtfPremiumDiscount).is_some());
}

#[test]
fn baseline_routing_falls_back_to_equity_registry_without_etf_valuator() {
    let pack_id = crate::analysis_packs::PackId::Baseline;
    let registry = match pack_id {
        crate::analysis_packs::PackId::EtfBaseline => {
            crate::valuation::ValuatorRegistry::etf_baseline()
        }
        _ => crate::valuation::ValuatorRegistry::equity_baseline(),
    };
    assert!(registry.get(crate::valuation::ValuatorId::EtfPremiumDiscount).is_none());
}
```

- [ ] **Step 7: Build + commit**

```bash
cargo test -p scorpio-core --lib workflow::tasks::analyst
cargo clippy -p scorpio-core --all-targets -- -D warnings
git add crates/scorpio-core/src/workflow/tasks/analyst.rs crates/scorpio-core/src/state/
git commit -m "feat(etf): hydrate ETF valuation inputs in AnalystSyncTask when pack == EtfBaseline"
```

---

## Task 14: Report rendering — ETF panel + routing header

**Files:**
- Create: `crates/scorpio-reporters/src/terminal/etf.rs`
- Modify: `crates/scorpio-reporters/src/terminal/valuation.rs` (dispatch `ScenarioValuation::Etf`)
- Modify: `crates/scorpio-reporters/src/terminal/final_report.rs` (insert routing-fallback warning into the header)
- Modify: `crates/scorpio-reporters/src/terminal/mod.rs` (declare `mod etf;`)

- [ ] **Step 1: Create `etf.rs` panel renderer**

```rust
//! ETF Valuation Snapshot panel renderer.

use std::fmt::Write;

use scorpio_core::state::{
    EtfComposition, EtfValuation, PremiumBand, ScenarioValuation, TradingState, TrackingError,
};

pub(crate) fn render_etf_panel(out: &mut String, state: &TradingState) {
    super::final_report::section_header(out, "ETF Valuation Snapshot");

    let Some(dv) = state.derived_valuation() else {
        let _ = writeln!(out, "Not computed for this run.");
        return;
    };

    let etf = match &dv.scenario {
        ScenarioValuation::Etf(e) => e,
        ScenarioValuation::NotAssessed { reason } => {
            let _ = writeln!(out, "ETF valuation    Not assessed");
            let _ = writeln!(out, "Reason           {reason}");
            return;
        }
        other => {
            let _ = writeln!(out, "Unexpected valuation variant for ETF panel: {other:?}");
            return;
        }
    };

    render_premium_block(out, etf, state);
    if let Some(comp) = etf.composition.as_ref() {
        render_composition_block(out, comp);
    } else {
        let _ = writeln!(out, "⚠ Holdings unavailable — N-PORT-P data missing or too stale");
    }
    if let Some(tr) = etf.tracking.as_ref() {
        render_tracking_block(out, tr);
    } else {
        let _ = writeln!(out, "⚠ Tracking error skipped — benchmark not resolved");
    }
    render_availability(out, etf);
}

fn render_premium_block(out: &mut String, etf: &EtfValuation, state: &TradingState) {
    let _ = writeln!(out, "Analysis Pack    ETF Baseline");
    let _ = writeln!(out, "Symbol           {}", state.asset_symbol);
    if let Some(cat) = etf.category.as_deref() {
        let _ = writeln!(out, "Category         {cat}");
    }
    let _ = writeln!(out, "Market Price     ${:.2}", etf.premium.market_price);
    match etf.premium.nav {
        Some(nav) => {
            let _ = writeln!(out, "NAV              ${nav:.2}   (as of {})", etf.premium.as_of.format("%H:%M UTC"));
        }
        None => {
            let _ = writeln!(out, "NAV              unavailable");
        }
    }
    match etf.premium.premium_pct {
        Some(p) => {
            let _ = writeln!(out, "Premium          {p:+.2}%   Band  {}", band_label(etf.premium.category_band));
        }
        None => {
            let _ = writeln!(out, "Premium          unavailable   Band  Unknown");
            let _ = writeln!(out, "⚠ Premium band unavailable — NAV missing from ETF quote payload");
        }
    }
    match (etf.premium.bid, etf.premium.ask, etf.premium.bid_ask_spread_pct) {
        (Some(b), Some(a), Some(s)) => {
            let _ = writeln!(out, "Bid/Ask          ${b:.2}/${a:.2}   Spread {s:.3}%");
        }
        _ => {
            let _ = writeln!(out, "Bid/Ask          unavailable   Spread unavailable");
            let _ = writeln!(out, "⚠ Noise-floor check skipped — bid/ask unavailable");
        }
    }
    if let Some(lev) = etf.leverage_factor.filter(|&l| (l - 1.0).abs() > f64::EPSILON) {
        let _ = writeln!(out, "Leverage         {lev:.1}x");
    }
}

fn render_composition_block(out: &mut String, comp: &EtfComposition) {
    let _ = writeln!(
        out,
        "─── COMPOSITION  (filing {}, {} days old) ────────",
        comp.holdings_filing_date, comp.holdings_age_days,
    );
    let _ = writeln!(out, "Top-10 weight    {:.1}%", comp.top10_concentration_pct);
    if !comp.top_holdings.is_empty() {
        let names: Vec<String> = comp
            .top_holdings
            .iter()
            .take(5)
            .map(|h| format!("#{} {}  {:.1}%", "_", h.ticker.as_deref().unwrap_or(&h.name), h.weight_pct))
            .collect();
        let _ = writeln!(out, "{}", names.join(" │ "));
    }
    if !comp.holdings_age_days > 90 {
        let _ = writeln!(out, "⚠ Holdings staleness — {} days old", comp.holdings_age_days);
    }
}

fn render_tracking_block(out: &mut String, tr: &TrackingError) {
    let _ = writeln!(
        out,
        "─── TRACKING vs {} ───────────────",
        tr.benchmark_symbol
    );
    let _ = writeln!(
        out,
        "90d TE: {:.2}% annualised   |   1y TE: {:.2}% annualised  (n={} days)",
        tr.te_pct_90d, tr.te_pct_1y, tr.sample_days
    );
}

fn render_availability(out: &mut String, etf: &EtfValuation) {
    let _ = writeln!(out, "─── DATA AVAILABILITY ────────────");
    let _ = writeln!(out, "NAV: {}  Bid/Ask: {}  Holdings: {}  Benchmark: {}",
        flag_check(etf.flags.nav_available),
        flag_check(etf.flags.bid_ask_available),
        flag_check(etf.flags.holdings_present),
        flag_check(etf.flags.benchmark_resolved),
    );
}

fn flag_check(b: bool) -> char {
    if b { '✓' } else { '✗' }
}

fn band_label(band: PremiumBand) -> &'static str {
    match band {
        PremiumBand::Normal => "▲ Normal",
        PremiumBand::Elevated => "▲ Elevated",
        PremiumBand::Extreme => "▲ Extreme",
        PremiumBand::Unknown => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, HoldingWeight, PremiumSnapshot, TradingState,
    };

    fn etf_state_with(etf: EtfValuation) -> TradingState {
        let mut state = TradingState::new("SPY", "2026-05-21");
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::Etf(etf),
        });
        state
    }

    fn minimal_etf() -> EtfValuation {
        EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(621.18),
                market_price: 621.40,
                bid: Some(621.39),
                ask: Some(621.41),
                premium_pct: Some(0.04),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: None,
            category: Some("Large Blend".into()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability {
                nav_available: true,
                bid_ask_available: true,
                ..EtfDataAvailability::default()
            },
        }
    }

    #[test]
    fn renders_etf_panel_with_full_premium_snapshot() {
        let state = etf_state_with(minimal_etf());
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("ETF Valuation Snapshot"));
        assert!(out.contains("Market Price"));
        assert!(out.contains("Premium"));
        assert!(out.contains("Band  ▲ Normal"));
    }

    #[test]
    fn renders_not_assessed_when_quote_unavailable() {
        let mut state = TradingState::new("BOGUS", "2026-05-21");
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::NotAssessed { reason: "etf_quote_unavailable".into() },
        });
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("Not assessed"));
        assert!(out.contains("etf_quote_unavailable"));
    }

    #[test]
    fn renders_holdings_unavailable_warning_when_composition_missing() {
        let state = etf_state_with(minimal_etf());
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("Holdings unavailable"));
    }

    #[test]
    fn renders_premium_unknown_when_nav_missing() {
        let mut etf = minimal_etf();
        etf.premium.nav = None;
        etf.premium.premium_pct = None;
        etf.premium.category_band = PremiumBand::Unknown;
        etf.flags.nav_available = false;
        let state = etf_state_with(etf);
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("NAV              unavailable"));
        assert!(out.contains("Premium band unavailable"));
    }
}
```

- [ ] **Step 2: Dispatch in `valuation.rs`**

Edit `crates/scorpio-reporters/src/terminal/valuation.rs::write_valuation_body`:

```rust
fn write_valuation_body(out: &mut String, dv: &DerivedValuation, state: &TradingState) {
    let _ = writeln!(out, "Asset shape: {}", asset_shape_label(&dv.asset_shape));
    match &dv.scenario {
        ScenarioValuation::NotAssessed { reason } => {
            let _ = writeln!(out, "Valuation: not assessed for this asset shape.");
            let _ = writeln!(out, "Reason: {reason}");
        }
        ScenarioValuation::CorporateEquity(equity) => {
            let _ = writeln!(out, "Valuation model: Corporate Equity");
            write_equity_metrics(out, equity);
        }
        ScenarioValuation::Etf(_) => {
            super::etf::render_etf_panel(out, state);
        }
    }
}
```

Update the caller `write_scenario_valuation` to pass `state` through:

```rust
match state.derived_valuation() {
    None => { let _ = writeln!(out, "Not computed for this run."); }
    Some(dv) => write_valuation_body(out, dv, state),
}
```

- [ ] **Step 3: Add routing-fallback warning to the header**

Locate `final_report.rs` and find where the analysis pack label is rendered. Add (or extend) the rendering to print a warning when `state.etf_routing_fallback_reason` is `Some`:

```rust
if let Some(reason) = state.etf_routing_fallback_reason.as_deref() {
    let _ = writeln!(out, "⚠ ETF routing fallback — {} ; baseline pack used for this run", reason.replace('_', " "));
}
```

Run the existing reporter tests:

```bash
cargo test -p scorpio-reporters
```

- [ ] **Step 4: Register the new submodule**

In `crates/scorpio-reporters/src/terminal/mod.rs`, add `pub(crate) mod etf;` next to the existing `pub(crate) mod valuation;`.

- [ ] **Step 5: Build + commit**

```bash
cargo test -p scorpio-reporters
cargo clippy -p scorpio-reporters --all-targets -- -D warnings
git add crates/scorpio-reporters/src/terminal/
git commit -m "feat(reporters): add ETF Valuation Snapshot panel + routing-fallback header warning"
```

---

## Task 15: Routing + topology integration tests

**Files:**
- Modify: `crates/scorpio-core/tests/workflow_pipeline_structure.rs`

The existing test asserts the baseline graph topology. We need three new tests: ETF symbol → EtfBaseline pack drives topology; unsupported fund shape → Baseline with `unsupported_fund_shape` reason; `None` profile → Baseline with `profile_lookup_unavailable` reason.

- [ ] **Step 1: Read the existing test file to understand the helper pattern**

```bash
grep -n "fn " /Users/bigtochan/Documents/dev/BigtoC/scorpio-analyst/crates/scorpio-core/tests/workflow_pipeline_structure.rs | head -20
```

It likely uses `crate::workflow::build_graph_from_pack(...)` and asserts task IDs. Mirror that style.

- [ ] **Step 2: Add classifier integration tests**

```rust
#[test]
fn classify_with_no_profile_falls_back_to_baseline_with_reason() {
    use scorpio_core::workflow::pack_classifier::{classify_runtime_pack, RuntimePackSelection};
    let result = classify_runtime_pack(None, None);
    assert_eq!(
        result,
        RuntimePackSelection::Baseline {
            reason: "profile_lookup_unavailable"
        }
    );
}

#[test]
fn etf_baseline_pack_drives_same_four_analyst_topology() {
    use scorpio_core::analysis_packs::{PackId, resolve_pack};
    let pack = resolve_pack(PackId::EtfBaseline);
    assert_eq!(pack.required_inputs.len(), 4);
    assert_eq!(
        pack.required_inputs,
        vec!["fundamentals", "sentiment", "news", "technical"]
    );
}

#[test]
fn etf_baseline_routes_fund_shape_to_etf_valuator() {
    use scorpio_core::analysis_packs::{PackId, resolve_pack};
    use scorpio_core::state::AssetShape;
    use scorpio_core::valuation::ValuatorId;
    let pack = resolve_pack(PackId::EtfBaseline);
    assert_eq!(
        pack.valuator_selection.get(&AssetShape::Fund).copied(),
        Some(ValuatorId::EtfPremiumDiscount)
    );
}
```

- [ ] **Step 3: Build + commit**

```bash
cargo test -p scorpio-core --test workflow_pipeline_structure
git add crates/scorpio-core/tests/workflow_pipeline_structure.rs
git commit -m "test(etf): cover ETF pack routing topology + classifier fallback"
```

---

## Task 16: Prompt-bundle regression gate

**Files:**
- Modify: `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`
- Possibly modify: existing fixture `.txt` files under `tests/fixtures/prompt_bundle/` IF (and only if) the byte-content was unintentionally altered by the `git mv` operations in Tasks 3–4.

The point of this task is twofold: (a) prove the equity baseline prompt bytes are byte-identical after the moves; (b) extend the regression gate to also assert ETF baseline completeness under all four topology shapes.

- [ ] **Step 1: Run the gate**

```bash
cargo test -p scorpio-core --test prompt_bundle_regression_gate -- --nocapture
```

If it passes — Tasks 3 and 4 didn't alter bytes; proceed to Step 2. If it fails, the failing diff will show which prompt drifted. Investigate (probably a trailing newline got dropped in a `git mv`); fix the source `.md` file, do NOT regenerate the fixture.

- [ ] **Step 2: Add ETF completeness coverage**

Append a new test:

```rust
#[test]
fn etf_baseline_passes_completeness_under_all_topology_shapes() {
    use scorpio_core::analysis_packs::{
        validate_active_pack_completeness, PackId, resolve_runtime_policy,
    };
    use scorpio_core::workflow::build_run_topology;

    let policy = resolve_runtime_policy_for_pack_id(PackId::EtfBaseline)
        .expect("ETF baseline must resolve");

    let shapes = [
        (1, 1, true, "full"),
        (0, 1, true, "no_debate"),
        (1, 0, true, "no_risk"),
        (0, 0, true, "no_debate_no_risk"),
    ];
    for (max_debate, max_risk, auditor, label) in shapes {
        let topology =
            build_run_topology(&policy.required_inputs, max_debate, max_risk, auditor);
        let res = validate_active_pack_completeness(&policy, &topology);
        assert!(res.is_ok(), "completeness failed in shape '{label}': {res:?}");
    }
}

// Helper for the assertion — the public API takes the string form.
fn resolve_runtime_policy_for_pack_id(id: scorpio_core::analysis_packs::PackId) -> Result<scorpio_core::analysis_packs::RuntimePolicy, String> {
    let manifest = scorpio_core::analysis_packs::resolve_pack(id);
    // RuntimePolicy::from_manifest is a private fn — use the equivalent
    // public surface; replace this with whatever public path is exposed.
    // If no public path exists, just call resolve_runtime_policy("baseline")
    // for baseline and add a feature-gated test path for EtfBaseline.
    todo!("see Step 3 — bridge through pack_diagnostics or expose a test-only helper")
}
```

- [ ] **Step 3: Decide how to address the test-only resolve hole**

The public surface `resolve_runtime_policy` only accepts user-selectable strings, and ETF isn't selectable. Two options — pick one:

**Option A (preferred):** Add a `#[cfg(any(test, feature = "test-helpers"))] pub use selection::resolve_runtime_policy_for_manifest;` line in `analysis_packs/mod.rs`. Then in the test:

```rust
let manifest = scorpio_core::analysis_packs::resolve_pack(PackId::EtfBaseline);
let policy = scorpio_core::analysis_packs::resolve_runtime_policy_for_manifest(&manifest)
    .expect("ETF baseline manifest must resolve");
```

**Option B:** Add `PackId::EtfBaseline → Ok(PackId::EtfBaseline)` to `FromStr` gated behind a test cfg.

Go with Option A; it's smaller and doesn't affect the user-facing surface.

- [ ] **Step 4: Build + commit**

```bash
cargo test -p scorpio-core --test prompt_bundle_regression_gate
git add crates/scorpio-core/tests/prompt_bundle_regression_gate.rs crates/scorpio-core/src/analysis_packs/mod.rs
git commit -m "test(etf): extend prompt-bundle regression gate for EtfBaseline completeness"
```

---

## Task 17: State serde round-trip tests

**Files:**
- Modify: `crates/scorpio-core/tests/state_roundtrip.rs`

- [ ] **Step 1: Add ETF variant + legacy-snapshot tests**

```rust
#[test]
fn trading_state_with_etf_variant_roundtrips() {
    use chrono::Utc;
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
        PremiumSnapshot, ScenarioValuation, TradingState,
    };

    let mut state = TradingState::new("SPY", "2026-05-21");
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(621.18),
                market_price: 621.40,
                bid: Some(621.39),
                ask: Some(621.41),
                premium_pct: Some(0.04),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: Utc::now(),
            },
            composition: None,
            tracking: None,
            options_gex: None,
            category: Some("Large Blend".into()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });
    let json = serde_json::to_string(&state).expect("ser");
    let back: TradingState = serde_json::from_str(&json).expect("deser");
    assert_eq!(state, back);
}

#[test]
fn legacy_snapshot_without_etf_routing_fallback_field_still_loads() {
    // Snapshot taken before `etf_routing_fallback_reason` was added.
    // The serde derive emits the field; we omit it here to simulate a
    // pre-feature snapshot. Build a minimal JSON manually.
    let json = r#"{
        "execution_id": "00000000-0000-0000-0000-000000000000",
        "asset_symbol": "AAPL",
        "target_date": "2026-05-21",
        "current_price": null,
        "data_coverage": null,
        "provenance_summary": null,
        "debate_history": [],
        "consensus_summary": null,
        "trader_proposal": null,
        "risk_discussion_history": [],
        "aggressive_risk_report": null,
        "neutral_risk_report": null,
        "conservative_risk_report": null,
        "final_execution_status": null,
        "token_usage": {"phase_usages":[]}
    }"#;
    let back: scorpio_core::state::TradingState =
        serde_json::from_str(json).expect("legacy snapshot must deserialize");
    assert_eq!(back.asset_symbol, "AAPL");
    assert!(back.etf_routing_fallback_reason.is_none());
}

#[test]
fn legacy_corporate_equity_snapshot_unchanged_after_etf_variant_added() {
    let json = r#"{"corporate_equity":{"dcf":null,"ev_ebitda":null,"forward_pe":null,"peg":null}}"#;
    let back: scorpio_core::state::ScenarioValuation =
        serde_json::from_str(json).expect("legacy variant must still parse");
    assert!(matches!(
        back,
        scorpio_core::state::ScenarioValuation::CorporateEquity(_)
    ));
}
```

(Adjust the minimal JSON to match the actual `TokenUsageTracker` default shape — open `state/token_usage.rs` if it doesn't deserialize with `phase_usages: []`.)

- [ ] **Step 2: Build + commit**

```bash
cargo test -p scorpio-core --test state_roundtrip
git add crates/scorpio-core/tests/state_roundtrip.rs
git commit -m "test(state): ETF variant + legacy-snapshot compat round-trip"
```

---

## Task 18: Live smoke examples

**Files:**
- Create: `crates/scorpio-core/examples/etf_quote_live_test.rs`
- Create: `crates/scorpio-core/examples/nport_live_test.rs`
- Create: `crates/scorpio-core/examples/etf_pack_live_test.rs`

These are run by hand (`cargo run -p scorpio-core --example etf_quote_live_test`), NOT in CI.

- [ ] **Step 1: `etf_quote_live_test.rs`**

```rust
//! Manual live smoke: yfinance ETF surface methods.
//! Run with: `cargo run -p scorpio-core --example etf_quote_live_test`.

use scorpio_core::data::yfinance::etf::is_supported_etf_kind;
use scorpio_core::data::YFinanceClient;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    let client = YFinanceClient::default();

    for symbol in ["SPY", "QQQ", "TQQQ", "AAPL", "BOGUS_TICKER_DOES_NOT_EXIST"] {
        let quote = client.get_quote(symbol).await;
        let info = client.get_fund_info(symbol).await;
        let yld = client.get_distribution_yield_ttm(symbol).await;
        let profile = client.get_profile(symbol).await;
        println!("\n=== {symbol} ===");
        println!("profile: {profile:?}");
        println!("quote: {quote:?}");
        println!("info: {info:?}");
        println!("dist_yld_ttm: {yld:?}");
        println!("is_etf_kind: {}", info.as_ref().and_then(|i| i.fund_kind.as_deref()).map(is_supported_etf_kind).unwrap_or(false));
    }
}
```

- [ ] **Step 2: `nport_live_test.rs`**

```rust
//! Manual live smoke: SEC EDGAR N-PORT-P fetch.
//! Run with: `cargo run -p scorpio-core --example nport_live_test`.

use std::sync::Arc;

use scorpio_core::data::sec_edgar::SecEdgarClient;
use scorpio_core::rate_limit::SharedRateLimiter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    let limiter = SharedRateLimiter::disabled("sec_edgar_smoke");
    let edgar = Arc::new(SecEdgarClient::new(limiter).expect("client"));

    for symbol in ["SPY", "QQQ", "BOGUS_FUND"] {
        let cik = edgar.resolve_fund_cik(symbol).await;
        println!("\n=== {symbol} ===");
        println!("CIK: {cik:?}");
        if let Some(cik) = cik {
            let holdings = edgar.fetch_latest_nport_p(&cik, 180).await;
            match holdings {
                Some(h) => {
                    println!(
                        "filing_date={} holdings_count={} sectors={}",
                        h.filing_date,
                        h.holdings.len(),
                        h.sector_breakdown.len()
                    );
                    if let Some(h0) = h.holdings.first() {
                        println!("first holding: {} ({:.2}%)", h0.name, h0.weight_pct);
                    }
                }
                None => println!("no N-PORT-P available within 180 days"),
            }
        }
    }
}
```

- [ ] **Step 3: `etf_pack_live_test.rs`**

```rust
//! Manual live smoke: end-to-end runtime classification + pack routing.
//! Run with: `cargo run -p scorpio-core --example etf_pack_live_test`.

use scorpio_core::data::YFinanceClient;
use scorpio_core::workflow::pack_classifier::{classify_runtime_pack, RuntimePackSelection};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    let yf = YFinanceClient::default();

    for symbol in ["SPY", "AAPL", "BOGUS"] {
        let profile = yf.get_profile(symbol).await;
        let fund_info = yf.get_fund_info(symbol).await;
        let result = classify_runtime_pack(profile.as_ref(), fund_info.as_ref());
        println!(
            "{symbol:<8} → pack={:?} fallback={:?}",
            result.pack_id(),
            result.fallback_reason()
        );
        match (symbol, &result) {
            ("SPY", RuntimePackSelection::EtfBaseline) => {}
            ("AAPL", RuntimePackSelection::Baseline { reason }) if *reason == "corporate_equity" => {}
            ("BOGUS", RuntimePackSelection::Baseline { reason }) if *reason == "profile_lookup_unavailable" => {}
            (s, r) => eprintln!("⚠ unexpected routing for {s}: {r:?}"),
        }
    }
}
```

- [ ] **Step 4: Verify examples compile**

```bash
cargo build -p scorpio-core --examples
```

(Don't run them in CI — they hit the network.)

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/examples/
git commit -m "test(etf): add live smoke examples (yfinance quote, EDGAR N-PORT, end-to-end routing)"
```

---

## Final-step checklist

After every task ships, run the full workspace gates:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
cargo build -p scorpio-core --examples
```

If any of these fail in CI before merge, fix the failing crate before touching anything else — do not paper over a clippy warning with `#[allow]` unless the reason is documented in code.

**Manual end-to-end check** (post-merge):

```bash
SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze SPY
```

Expected: the rendered report header shows `Analysis Pack    ETF Baseline`, the ETF Valuation Snapshot panel renders, and no routing-fallback warning appears.

```bash
SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze AAPL
```

Expected: report shows `Analysis Pack    Balanced Institutional`, no ETF panel, no fallback warning.

```bash
SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze BOGUS_TICKER_XYZ
```

Expected: classification falls back to baseline; if pre-symbol-resolution checks pass (or the test symbol is one that resolves syntactically but yfinance can't profile), the header warns `ETF routing fallback — profile lookup unavailable; baseline pack used for this run`.
