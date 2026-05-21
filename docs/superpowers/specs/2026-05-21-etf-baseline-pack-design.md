# EtfBaseline Analysis Pack Design

**Date:** 2026-05-21
**Status:** Draft — awaiting user review

## Goal

Add a first-class `EtfBaseline` analysis pack so that `scorpio analyze SPY` produces ETF-native analysis (premium/discount, holdings composition, sector tilt, tracking error, dealer GEX) instead of the current `valuation_not_assessed` short-circuit. Pack selection happens automatically in preflight based on `yfinance::get_profile(symbol)`; users keep the same `scorpio analyze <SYMBOL>` CLI surface.

## Problem

The current pipeline detects ETFs but cannot analyze them:

- [`state/valuation_derive.rs:46`](../../../crates/scorpio-core/src/state/valuation_derive.rs) returns `NotAssessed { reason: "fund_style_asset" }` whenever `Profile::Fund` is matched.
- [`analysis_packs/equity/baseline.rs:138`](../../../crates/scorpio-core/src/analysis_packs/equity/baseline.rs) maps only `AssetShape::CorporateEquity → ValuatorId::EquityDefault`; `AssetShape::Fund` has no valuator entry.
- No ETF-specific data adapter (NAV, holdings, fund metadata), no ETF-specific prompts, no ETF report panel.
- `analyst_role_for_input()` in `workflow/topology.rs` only maps the four equity input strings to `Role` variants, so any new pack must reuse those four analyst slots.

This design preserves all existing equity behaviour byte-for-byte and adds a parallel ETF pipeline that:

- Reuses every existing infrastructure layer (graph-flow topology, `Role` enum, `PromptBundle`, `Valuator` trait, `ScenarioValuation` enum) by extension, not replacement.
- Uses **only free-tier data sources** (yfinance public endpoints via `yfinance-rs 0.7.2`, SEC EDGAR via the existing `SecEdgarClient`).
- Emits structured availability flags so missing-data cases are visibly degraded rather than silently mis-analysed.

## Decisions

| Decision                     | Choice                                                                                                                                                                            | Rationale                                                                                                                                                                                                                                           |
|------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Signal scope                 | Full skill parity including GEX                                                                                                                                                   | Match `himself65/finance-skills/etf-premium` methodology end-to-end so this pack is competitive with the reference skill on free-tier data.                                                                                                         |
| Analyst slot mapping         | Subject-aligned: Fundamental→Composition, Sentiment→Flow/Premium, News→Macro/Sector, Technical→Tracking/Options                                                                   | Each slot has one sharp purpose; prompts are ETF-native, not "equity prompts with appendix".                                                                                                                                                        |
| Failure policy               | Tiered degradation with per-signal flags                                                                                                                                          | NAV+OHLCV are critical (abort otherwise); holdings, benchmark, options chain degrade silently with `EtfDataAvailability` flag → false. Matches existing 1-analyst-fail-OK contract.                                                                 |
| Prompt strategy              | Per-slot prompts compose shared scaffolding (mirrors `with_analyst_runtime_contract_sections` in equity baseline)                                                                 | Eliminates duplication; same pattern the codebase already uses.                                                                                                                                                                                     |
| Prompt reuse                 | Promote 9 cross-cutting equity prompts to `analysis_packs/common/prompts/` (6 Tier 1 verbatim + 3 Tier 2 with ETF composition deltas); ETF pack composes from common + ETF deltas | ~30% reduction in new prompts; equity pack also benefits from cleaner location; zero cross-pack include paths.                                                                                                                                      |
| Holdings staleness threshold | 90 days                                                                                                                                                                           | Matches worst-case SEC N-PORT-P delay (60-day filing window). >90 days → `holdings_fresh=false`, prompt sees stale flag; >180 days → skip composition signal.                                                                                       |
| CLI behaviour                | Auto-route only; no `--pack` override flag                                                                                                                                        | Smaller surface; profile-based routing is deterministic.                                                                                                                                                                                            |
| Routing fallback             | `get_profile → None` falls back to `PackId::Baseline`                                                                                                                             | `get_profile` already returns `Option<Profile>` (None on any failure). Falling back is safer than aborting; equity valuator returns `NotAssessed` if fundamentals are missing, so an ETF accidentally routed to Baseline still degrades gracefully. |
| Report layout                | New ETF Valuation Snapshot panel; shared report scaffolding                                                                                                                       | Pattern matches today's report — just a different valuation panel.                                                                                                                                                                                  |
| Valuation policy variant     | Add `ValuationAssessment::Etf` (third variant alongside `Full`, `NotAssessed`)                                                                                                    | Mirrors `ScenarioValuation::Etf` on output; avoids semantic abuse of `Full` (which implies DCF).                                                                                                                                                    |
| Output variant               | Add `ScenarioValuation::Etf(EtfValuation)` to existing enum                                                                                                                       | Serde-compatible additive variant; old snapshots round-trip unchanged.                                                                                                                                                                              |
| Inputs carrier               | Extend `ValuationInputs<'a>` with optional ETF fields                                                                                                                             | Existing valuators ignore the new fields; trait signature unchanged.                                                                                                                                                                                |

## Architecture

### Routing dispatch (preflight)

```rust
// crates/scorpio-core/src/workflow/tasks/preflight.rs — one new helper
fn pack_id_for_profile(profile: Option<&Profile>) -> PackId {
    match profile {
        Some(Profile::Fund(_))    => PackId::EtfBaseline,
        Some(Profile::Company(_)) => PackId::Baseline,
        None                      => PackId::Baseline,   // fail-soft
    }
}
```

`PreflightTask` adds one call: `let profile = yfinance.get_profile(&symbol).await;` immediately after symbol validation, then `let active_pack = pack_id_for_profile(profile.as_ref());`. A `tracing::info!` emits `routing.pack`, `routing.profile_present`, and `routing.fallback` for log-side observability. Everything downstream (`validate_active_pack_completeness`, `build_run_topology`, `KEY_RUNTIME_POLICY`/`KEY_ROUTING_FLAGS` writes) consumes the resolved pack as today.

### Component layout

```
crates/scorpio-core/src/
├── analysis_packs/
│   ├── manifest/pack_id.rs           (+) PackId::EtfBaseline
│   ├── common/                       (NEW)
│   │   └── prompts/                  9 files promoted from equity/
│   │       ├── analyst_runtime_contract.md      Tier 1 (generic evidence discipline)
│   │       ├── theme_h_sourcing_and_untrusted.md Tier 1 (sourcing + injection defense)
│   │       ├── debate_moderator.md              Tier 1 (falsifiability framework)
│   │       ├── risk_moderator.md                Tier 1 (synthesis structure)
│   │       ├── bullish_researcher.md            Tier 1 (~90% structural)
│   │       ├── bearish_researcher.md            Tier 1 (~90% structural)
│   │       ├── news_analyst.md                  Tier 2 (generic news structure; ETF composes a delta)
│   │       ├── technical_analyst.md             Tier 2 (RSI/MACD/Bollinger apply to any tradeable)
│   │       └── auditor.md                       Tier 2 (audit structure; ETF composes landmines)
│   ├── equity/
│   │   ├── baseline.rs               (UPDATE: 9 include_str! paths → ../common/prompts/)
│   │   └── prompts/                  (stays: 8 equity-specific prompts)
│   │       ├── theme_c_management_red_flags.md  (skipped by ETF pack)
│   │       ├── fundamental_analyst.md
│   │       ├── sentiment_analyst.md
│   │       ├── trader.md
│   │       ├── aggressive_risk.md
│   │       ├── conservative_risk.md
│   │       ├── neutral_risk.md
│   │       └── fund_manager.md
│   └── etf/                          (NEW)
│       ├── mod.rs
│       ├── baseline.rs               etf_baseline_pack() builder + prompt composition
│       └── prompts/
│           ├── etf_runtime_contract.md       shared scaffolding
│           ├── etf_failure_modes.md          shared scaffolding
│           ├── etf_leverage_warning.md       conditional scaffolding
│           ├── composition_analyst.md        Tier 3 (new)
│           ├── flow_premium_analyst.md       Tier 3 (new)
│           ├── etf_macro_sector_focus.md     Tier 2 delta (composes with news_analyst.md)
│           ├── etf_tracking_options_focus.md Tier 2 delta (composes with technical_analyst.md)
│           ├── etf_landmines.md              Tier 2 delta (composes with auditor.md)
│           ├── trader.md                     Tier 3 (new — premium-band anchored)
│           ├── aggressive_risk.md            Tier 3 (new)
│           ├── conservative_risk.md          Tier 3 (new — deterministic ETF triggers)
│           ├── neutral_risk.md               Tier 3 (new)
│           └── fund_manager.md               Tier 3 (new — ETF dual-risk semantics)
│
├── data/
│   ├── yfinance/
│   │   ├── quote.rs                  (NEW) wraps Ticker::quote() + Ticker::info()
│   │   └── financials.rs             (+) get_distribution_yield_ttm()
│   └── sec_edgar/
│       └── nport.rs                  (NEW) N-PORT-P XBRL parser
│
├── valuation/
│   ├── mod.rs                        (+) ValuatorId::EtfPremiumDiscount
│   └── etf/                          (NEW)
│       ├── mod.rs
│       ├── premium_discount.rs       EtfPremiumDiscountValuator impl
│       ├── category_norms.rs         const lookup table (etf_premium_reference.md)
│       └── tracking_error.rs         ETF vs benchmark deviation math
│
├── indicators/
│   ├── gex.rs                        (NEW) Black-Scholes gamma + GEX aggregation
│   └── etf_benchmarks.rs             (NEW) static top-50 ETF→benchmark map + category fallback
│
└── state/
    └── derived.rs                    (+) ScenarioValuation::Etf(EtfValuation)
                                      (+) EtfValuation, PremiumSnapshot, EtfComposition,
                                          TrackingError, GexSummary, EtfDataAvailability
                                      (+) ScenarioValuation enum gains one variant

crates/scorpio-cli/src/report/
└── etf.rs                            (NEW) render_etf_panel() — shares header/risk scaffolding
```

### State schema additions

```rust
// analysis_packs/manifest/strategy.rs — policy enum gains one variant
pub enum ValuationAssessment {
    Full,         // existing — DCF + multiples
    Etf,          // NEW    — premium/discount + composition + tracking + GEX
    NotAssessed,  // existing — fallback
}

// valuation/mod.rs — valuator id gains one variant (#[non_exhaustive], safe to extend)
pub enum ValuatorId {
    EquityDefault,
    EtfPremiumDiscount,   // NEW
    CryptoTokenomics,
    CryptoNetworkValue,
}

// state/derived.rs — output enum gains one variant (additive, serde-compatible)
pub enum ScenarioValuation {
    CorporateEquity(CorporateEquityValuation),  // existing
    Etf(EtfValuation),                          // NEW
    NotAssessed { reason: String },             // existing
}

pub struct EtfValuation {
    pub premium: PremiumSnapshot,
    pub composition: Option<EtfComposition>,
    pub tracking: Option<TrackingError>,
    pub options_gex: Option<GexSummary>,
    pub category: Option<String>,
    pub leverage_factor: Option<f64>,           // 1.0 / 2.0 / -1.0 / 3.0 etc.
    pub flags: EtfDataAvailability,
}

pub struct PremiumSnapshot {
    pub nav: Option<f64>,
    pub market_price: f64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub premium_pct: Option<f64>,
    pub category_band: PremiumBand,             // Normal | Elevated | Extreme | Unknown
    pub bid_ask_spread_pct: Option<f64>,
    pub as_of: DateTime<Utc>,
}

pub struct EtfComposition {
    pub top_holdings: Vec<HoldingWeight>,       // top 10 from N-PORT
    pub top10_concentration_pct: f64,
    pub sector_weights: Vec<SectorWeight>,
    pub expense_ratio_pct: Option<f64>,
    pub aum_usd: Option<f64>,
    pub fund_family: Option<String>,
    pub distribution_yield_ttm_pct: Option<f64>,
    pub holdings_filing_date: chrono::NaiveDate,
    pub holdings_age_days: u32,
}

pub struct TrackingError {
    pub benchmark_symbol: String,
    pub te_pct_90d: f64,
    pub te_pct_1y: f64,
    pub sample_days: u32,
}

pub struct GexSummary {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub call_put_oi_ratio: f64,
    pub max_pain_strike: f64,
    pub near_term_expiration: chrono::NaiveDate,
}

#[derive(Default)]
pub struct EtfDataAvailability {
    pub nav_available: bool,
    pub bid_ask_available: bool,
    pub holdings_present: bool,
    pub holdings_fresh: bool,                   // < 90 days
    pub benchmark_resolved: bool,
    pub options_chain_present: bool,
    pub expense_ratio_available: bool,
}
```

All new state types derive `Serialize`, `Deserialize`, `JsonSchema`, `Debug`, `Clone`, `PartialEq`. Per CLAUDE.md:

- No `#[serde(deny_unknown_fields)]` on any of these (they're reachable from `TradingState`).
- A new `TradingState` field `pub etf_data_availability: Option<EtfDataAvailability>` carries `#[serde(default)]`.
- Adding `ScenarioValuation::Etf` is an additive enum variant — old snapshots with `CorporateEquity` or `NotAssessed` deserialize unchanged.

### Data adapter additions

| Source | New method | Returns | Fail-soft |
|---|---|---|---|
| yfinance | `YFinanceClient::get_quote(&str) -> Option<EtfQuote>` | `nav, regular_market_price, previous_close, bid, ask, market_cap, day_volume, currency` | Yes — None on transport error |
| yfinance | `YFinanceClient::get_fund_info(&str) -> Option<FundInfo>` | `category, fund_family, expense_ratio, total_assets, leverage_factor, fund_kind, stated_benchmark` | Yes |
| yfinance | `YFinanceClient::get_distribution_yield_ttm(&str) -> Option<f64>` | TTM sum of distributions / current price | Yes |
| SEC EDGAR | `SecEdgarClient::fetch_latest_nport_p(cik: &str, max_age_days: u32) -> Option<NPortHoldings>` | `{filing_date, holdings: Vec<{cusip, ticker?, name, weight_pct, value_usd}>, sector_breakdown, stated_benchmark}` | Yes |
| Benchmark resolver | `etf_benchmarks::resolve(ticker, category) -> Option<&'static str>` | Top-50 hardcoded map + category fallback (e.g. "Large Blend" → "^GSPC") | Yes |
| GEX math | `indicators::gex::compute_gex(chain, spot) -> GexSummary` | Aggregated BSM gamma per strike | N/A (pure function) |

### `ValuationInputs` extension

The trait signature stays the same; the carrier struct grows optional ETF fields, populated by the analyst sync stage only when the active pack is `EtfBaseline`.

```rust
pub struct ValuationInputs<'a> {
    // existing equity inputs (unchanged)
    pub profile: Option<yfinance_rs::profile::Profile>,
    pub cashflow: Option<&'a [CashflowRow]>,
    pub balance: Option<&'a [BalanceSheetRow]>,
    pub income: Option<&'a [IncomeStatementRow]>,
    pub shares: Option<&'a [ShareCount]>,
    pub earnings_trend: Option<&'a [EarningsTrendRow]>,
    pub current_price: Option<f64>,

    // new ETF inputs (None when pack != EtfBaseline)
    pub etf_quote:           Option<&'a EtfQuote>,
    pub etf_fund_info:       Option<&'a FundInfo>,
    pub etf_holdings:        Option<&'a NPortHoldings>,
    pub etf_options:         Option<&'a OptionsSnapshot>,
    pub etf_benchmark_ohlcv: Option<&'a [Candle]>,
}
```

### Valuator implementation

```rust
impl Valuator for EtfPremiumDiscountValuator {
    fn id(&self) -> ValuatorId { ValuatorId::EtfPremiumDiscount }

    fn assess(&self, inputs: ValuationInputs<'_>, shape: &AssetShape) -> ValuationReport {
        if !matches!(shape, AssetShape::Fund) {
            return DerivedValuation::not_assessed("etf_valuator_wrong_shape");
        }

        // 1. Premium snapshot — minimum bar for any output
        let mut flags = EtfDataAvailability::default();
        let snapshot = match build_premium_snapshot(inputs.etf_quote, &mut flags) {
            Some(s) => s,
            None => return DerivedValuation::not_assessed("etf_quote_unavailable"),
        };

        // 2. Composition — soft-fail
        let composition = inputs.etf_holdings
            .map(|h| build_composition(h, inputs.etf_fund_info, &mut flags));

        // 3. Tracking error — depends on benchmark resolution + OHLCV
        let tracking = match (inputs.etf_fund_info, inputs.etf_benchmark_ohlcv) {
            (Some(info), Some(bm)) => compute_tracking_error(info, bm, &mut flags),
            _ => None,
        };

        // 4. GEX — needs options chain
        let options_gex = inputs.etf_options
            .map(|chain| compute_gex_summary(chain, snapshot.market_price, &mut flags));

        let category = inputs.etf_fund_info.and_then(|f| f.category.clone());
        let leverage_factor = inputs.etf_fund_info.and_then(|f| f.leverage_factor);

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
    }
}
```

Contract: never panics, never returns `Err`. Every input failure becomes a `None` sub-struct + `false` flag. If `etf_quote` itself is unavailable, the entire valuation returns `NotAssessed { reason: "etf_quote_unavailable" }`.

### Pack manifest

```rust
pub fn etf_baseline_pack() -> AnalysisPackManifest {
    AnalysisPackManifest {
        id: PackId::EtfBaseline,
        name: "ETF Baseline".to_owned(),
        description: "ETF-native analysis: premium/discount band, holdings concentration, \
                      sector tilt, tracking error vs benchmark, and dealer GEX where available. \
                      Sources: yfinance (free) + SEC EDGAR N-PORT-P (free).".to_owned(),
        required_inputs: vec![
            "fundamentals".to_owned(),   // → Composition & Costs prompt
            "sentiment".to_owned(),      // → Flow & Premium prompt
            "news".to_owned(),           // → Macro & Sector Catalysts prompt
            "technical".to_owned(),      // → Tracking + Options/GEX prompt
        ],
        enrichment_intent: EnrichmentIntent {
            transcripts: false,           // ETFs don't have earnings calls
            consensus_estimates: false,   // no analyst EPS estimates for ETFs
            event_news: true,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Premium/discount band classification anchors the assessment. \
                            Weight composition concentration and tracking error equally; \
                            flag leverage decay and AP arbitrage breakdown explicitly.".to_owned(),
        report_strategy_label: "ETF Baseline".to_owned(),
        default_valuation: ValuationAssessment::Etf,
        prompt_bundle: etf_baseline_prompt_bundle(),
        valuator_selection: HashMap::from([
            (AssetShape::Fund, ValuatorId::EtfPremiumDiscount),
        ]),
        auditor_enabled: true,
    }
}
```

`analysis_emphasis` is ≤256 ASCII chars per the preflight sanitization rule in CLAUDE.md.

### Prompt composition

Mirrors the existing `with_analyst_runtime_contract_sections` pattern in `equity/baseline.rs`. New helpers in `etf/baseline.rs`:

```rust
// crates/scorpio-core/src/analysis_packs/etf/baseline.rs

const ETF_RUNTIME_CONTRACT: &str = include_str!("prompts/etf_runtime_contract.md");
const ETF_FAILURE_MODES:    &str = include_str!("prompts/etf_failure_modes.md");
const ETF_LEVERAGE_WARNING: &str = include_str!("prompts/etf_leverage_warning.md");

const COMMON_ANALYST_CONTRACT: &str = include_str!("../common/prompts/analyst_runtime_contract.md");
const COMMON_SOURCING_GUARD:   &str = include_str!("../common/prompts/theme_h_sourcing_and_untrusted.md");

// Three composition helpers — all wrap the same `compose_sections` primitive
// from the existing equity/baseline.rs pattern. They differ only in which
// scaffolding sections they append.

/// Used for ETF analyst slots that need full ETF-native framing.
fn compose_etf_analyst(raw: &'static str) -> Cow<'static, str> {
    compose_sections(raw, &[COMMON_ANALYST_CONTRACT, ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES])
}

/// Used for slots that reuse a common-pool prompt verbatim plus a small ETF delta.
fn compose_etf_section(raw: &'static str, deltas: &[&str]) -> Cow<'static, str> {
    let mut sections = vec![ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES];
    sections.extend_from_slice(deltas);
    compose_sections(raw, &sections)
}

/// Used for the three risk agents. Mirrors compose_etf_section but always
/// includes ETF_FAILURE_MODES; the renderer can conditionally inject
/// ETF_LEVERAGE_WARNING at substitution time when leverage_factor != 1.0.
fn compose_etf_risk(raw: &'static str) -> Cow<'static, str> {
    compose_sections(raw, &[ETF_RUNTIME_CONTRACT, ETF_FAILURE_MODES])
}

fn etf_baseline_prompt_bundle() -> PromptBundle {
    PromptBundle {
        // Tier 3 — fully new ETF prompts (analysts)
        fundamental_analyst: compose_etf_analyst(include_str!("prompts/composition_analyst.md")),
        sentiment_analyst:   compose_etf_analyst(include_str!("prompts/flow_premium_analyst.md")),

        // Tier 2 — reuse common-pool prompts via composition with small ETF deltas
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

        // Tier 1 — verbatim reuse from common pool (no composition)
        bullish_researcher: Cow::Borrowed(include_str!("../common/prompts/bullish_researcher.md")),
        bearish_researcher: Cow::Borrowed(include_str!("../common/prompts/bearish_researcher.md")),
        debate_moderator:   Cow::Borrowed(include_str!("../common/prompts/debate_moderator.md")),
        risk_moderator:     Cow::Borrowed(include_str!("../common/prompts/risk_moderator.md")),

        // Tier 3 — fully new ETF prompts (trader, risk, fund manager)
        trader:            compose_etf_section(include_str!("prompts/trader.md"), &[]),
        aggressive_risk:   compose_etf_risk(include_str!("prompts/aggressive_risk.md")),
        conservative_risk: compose_etf_risk(include_str!("prompts/conservative_risk.md")),
        neutral_risk:      compose_etf_risk(include_str!("prompts/neutral_risk.md")),
        fund_manager:      compose_etf_section(include_str!("prompts/fund_manager.md"), &[]),
    }
}
```

No cross-pack includes — every external reference is into `analysis_packs/common/prompts/`. The equity pack picks up the same nine common files via its own `include_str!("../common/prompts/X.md")` paths, so neither pack reaches into the other's directory.

The leverage warning is conditionally injected by the renderer at substitution time when `state.etf_data_availability.leverage_factor != 1.0`; the static manifest stays leverage-agnostic.

### Report rendering

A new `crates/scorpio-cli/src/report/etf.rs` exports `render_etf_panel(state: &TradingState) -> String`. The existing report driver detects `matches!(scenario, ScenarioValuation::Etf(_))` and calls the new renderer in place of the DCF/multiples panel. Header, analyst summaries, debate, risk, fund-manager scaffolding are unchanged. Example output:

```
═══ ETF VALUATION SNAPSHOT ═══════════════════════════════════════════
  Symbol           SPY           Category      Large Blend
  Market Price     $621.40       NAV           $621.18   (as of 21:00 UTC)
  Premium          +0.04%        Band          ▲ Normal  (US Large-Cap: ±0.01–0.05%)
  Bid/Ask          $621.39/$621.41   Spread    0.003%   (noise floor)
  Expense Ratio    0.0945%       AUM           $612.3B
  Distribution     1.21% TTM     Leverage      1.0x

  ─── COMPOSITION  (filing 2026-04-30, 21 days old) ────────────────────
  Top-10 weight    27.3%
  #1 AAPL  6.8% │ #2 MSFT  6.2% │ #3 NVDA  5.9% │ #4 AMZN  3.4% │ #5 META  2.7%
  Sector tilt: Tech 30.4% (+2.1pp vs broad), Financials 13.1% (-0.4pp), …

  ─── TRACKING vs ^GSPC ────────────────────────────────────────────────
  90d TE: 0.04% annualized   |   1y TE: 0.09% annualized  (n=251 days)

  ─── DEALER GAMMA  (near-term 2026-05-23) ─────────────────────────────
  Net GEX per 1%   +$2.84B    Gross GEX     $7.12B
  Call/Put OI      1.31       Max-pain      $620

  ─── DATA AVAILABILITY ────────────────────────────────────────────────
  ✓ NAV  ✓ Bid/Ask  ✓ Holdings (fresh)  ✓ Benchmark  ✓ Options chain
═══════════════════════════════════════════════════════════════════════
```

When flags are false, the corresponding sub-section is omitted and a `⚠` line surfaces in DATA AVAILABILITY:

```
  ⚠ Holdings unavailable — no N-PORT-P filing within 90 days (CIK 0001234567)
  ⚠ Tracking error skipped — benchmark not resolved for category "Thematic"
  ⚠ Dealer gamma skipped — no options chain available
```

## Failure modes & data availability

| Condition                                          | Detection                                      | Behaviour                                                                                                                                      |
|----------------------------------------------------|------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| `get_profile` fails (network, parse)               | `Option::None` returned                        | Fallback to `PackId::Baseline`; `tracing::info!` emits `routing.fallback=true`.                                                                |
| ETF detected but `Ticker::quote()` returns nothing | `etf_quote.is_none()` in valuator              | Valuator returns `NotAssessed { reason: "etf_quote_unavailable" }`. Pipeline continues; fund manager sees the gap.                             |
| ETF detected, quote OK, no NAV                     | `quote.nav.is_none()`                          | `flags.nav_available = false`; `premium.premium_pct = None`; flow/premium analyst sees the flag in prompt context.                             |
| No bid/ask                                         | `quote.bid.is_none()` or `quote.ask.is_none()` | `flags.bid_ask_available = false`; noise-floor gate cannot fire.                                                                               |
| N-PORT-P missing                                   | EDGAR returns empty                            | `flags.holdings_present = false`, `composition = None`. Composition analyst gets a flag-only prompt context.                                   |
| N-PORT-P > 90 days old                             | `holdings_age_days > 90`                       | `flags.holdings_fresh = false`; composition still rendered, with staleness shown in report.                                                    |
| N-PORT-P > 180 days old                            | `holdings_age_days > 180`                      | Skip composition signal entirely (`composition = None`).                                                                                       |
| Benchmark unresolvable                             | `etf_benchmarks::resolve()` returns None       | `flags.benchmark_resolved = false`; `tracking = None`.                                                                                         |
| Options chain empty                                | yfinance `options()` returns `[]`              | `flags.options_chain_present = false`; `options_gex = None`.                                                                                   |
| Leveraged/inverse ETF                              | `fund_info.leverage_factor != 1.0`             | `etf_leverage_warning.md` injected into Conservative + Neutral risk + Auditor prompts.                                                         |
| Critical-data abort                                | NAV + bid + ask + OHLCV all unavailable        | `ScenarioValuation::NotAssessed { reason: "etf_quote_unavailable" }`. Pipeline runs to completion; report shows availability-flag footer only. |

The fund manager's deterministic-fallback semantics (per `2026-04-20-fund-manager-dual-risk-escalation-design.md`) are preserved — the ETF `conservative_risk.md` prompt teaches the LLM to flag `tracking_failure`, `extreme_premium`, or `leverage_decay`, and the fund manager prompt's first-line audit contract applies the same way.

## Test plan

| Layer               | Location                                                                        | Coverage                                                                                                                                                                   |
|---------------------|---------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Pack manifest       | `crates/scorpio-core/src/analysis_packs/etf/baseline.rs#tests` (mirrors equity) | `etf_baseline_pack().validate().is_ok()`, correct id, required_inputs, prompt slots populated, valuator_selection contains Fund key                                        |
| Valuator            | `crates/scorpio-core/src/valuation/etf/premium_discount.rs#tests`               | Premium math, band classification, bid-ask floor, each flag-false branch returns sane sub-struct, leverage-factor pass-through                                             |
| State serde         | `crates/scorpio-core/tests/state_roundtrip.rs` (extend)                         | `ScenarioValuation::Etf(…)` round-trips JSON; old snapshots without the variant still deserialize; `EtfDataAvailability` defaults work                                     |
| Routing             | `crates/scorpio-core/tests/workflow_pipeline_structure.rs` (extend)             | `Profile::Fund→EtfBaseline`, `Profile::Company→Baseline`, `None→Baseline`; preflight emits expected `tracing` fields                                                       |
| Prompt completeness | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` (extend)           | EtfBaseline manifest passes `validate_active_pack_completeness` for all 4 topology shapes (full / no-debate / no-risk / neither)                                           |
| Live smoke (manual) | `crates/scorpio-core/examples/etf_quote_live_test.rs` (NEW)                     | `get_quote(SPY)`, `get_fund_info(SPY)`, `get_distribution_yield_ttm(SPY)`, `get_profile(SPY)→Fund`, `get_profile(AAPL)→Company`, `get_profile(BOGUS)→None`                 |
| Live smoke (manual) | `crates/scorpio-core/examples/nport_live_test.rs` (NEW)                         | `fetch_latest_nport_p(spy_cik, 90)`, fail-soft `fetch_latest_nport_p(bogus_cik, 90)→None`, staleness flag fires on aged fixture                                            |
| Live smoke (manual) | `crates/scorpio-core/examples/etf_pack_live_test.rs` (NEW)                      | End-to-end: build EtfBaseline manifest, run preflight on SPY → pack=EtfBaseline; preflight on AAPL → pack=Baseline; preflight on bogus → pack=Baseline (fallback verified) |
| Report rendering    | `crates/scorpio-cli/tests/` (extend)                                            | ETF panel renders for `ScenarioValuation::Etf`; missing-data footer reflects flags; equity report unchanged for `Profile::Company`                                         |

Live smoke tests are NOT in CI — they run via `cargo run -p scorpio-core --example <name>` per the existing `<source>_live_test.rs` convention.

## Out of scope

- Real-time NAV (iNAV) — yfinance only provides end-of-prior-day NAV.
- Bond ETF duration / convexity analysis — needs `/etf/bond-profile` (paid Finnhub) or coupon scraping.
- Multi-asset ETF benchmarking — the resolver fallback returns None; report flags it.
- Closed-end fund discount analysis — different mechanic; separate `PackId::ClosedEndFund` if ever needed.
- ETF holdings recursion (ETF-of-ETFs unwrapping) — first-level holdings only.
- Active ETF stock-picking quality vs benchmark — out of scope; tracking error covers index-tracking variant only.
- Future crypto ETF support — explicitly out (would need spot crypto + custody premium analysis).

## Open questions

None for the design itself. Implementation-time questions deferred to the writing-plans phase:

- Exact CIK lookup path for ETFs in `SecEdgarClient` — existing CIK resolver works for stocks; whether ETFs share that resolver or need a `funds-only` filter is an implementation detail.
- Whether to cache N-PORT-P filings in SQLite (existing snapshot DB) or fetch on every run — likely cache with 30-day TTL but exact strategy TBD in plan.
- BSM gamma helper: borrow `kand` crate API or implement standalone — `kand` doesn't currently expose BSM Greeks; likely standalone in `indicators/gex.rs`.

## References

- `himself65/finance-skills/plugins/market-analysis/skills/etf-premium/SKILL.md` — source methodology.
- `himself65/finance-skills/.../references/etf_premium_reference.md` — category benchmark table.
- CLAUDE.md — `Pack-owned prompts (centralized)`, `TradingState schema evolution`, error handling pattern.
- `2026-04-20-fund-manager-dual-risk-escalation-design.md` — fund manager dual-risk contract preserved.
- `2026-04-25-prompt-bundle-centralization-design.md` — `PromptBundle` and prompt slot architecture.
- `2026-04-28-shared-options-evidence-design.md` — `options_context.outcome.kind` semantics reused by ETF tracking/options analyst.
