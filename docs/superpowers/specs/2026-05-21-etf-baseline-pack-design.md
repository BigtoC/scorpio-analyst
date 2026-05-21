# EtfBaseline Analysis Pack Design

**Date:** 2026-05-21
**Status:** Draft — awaiting user review

## Goal

Add a first-class `EtfBaseline` analysis pack so that `scorpio analyze SPY` stops short-circuiting on fund-shaped assets and produces ETF-native analysis. Phase 1 covers premium/discount, holdings composition and sector tilt when fresh N-PORT data is available, and tracking error when a source-provided benchmark is available; Phase 2 adds dealer GEX. Pack selection stays automatic and uses `yfinance::get_profile(symbol)` plus fund metadata before pack-specific graph wiring; users keep the same `scorpio analyze <SYMBOL>` CLI surface.

## Problem

The current pipeline detects ETFs but cannot analyze them:

- [`state/valuation_derive.rs:46`](../../../crates/scorpio-core/src/state/valuation_derive.rs) returns `NotAssessed { reason: "fund_style_asset" }` whenever `Profile::Fund` is matched.
- [`analysis_packs/equity/baseline.rs:138`](../../../crates/scorpio-core/src/analysis_packs/equity/baseline.rs) maps only `AssetShape::CorporateEquity → ValuatorId::EquityDefault`; `AssetShape::Fund` has no valuator entry.
- No ETF-specific data adapter (NAV, holdings, fund metadata), no ETF-specific prompts, no ETF report panel.
- `analyst_role_for_input()` in `workflow/topology.rs` only maps the four equity input strings to `Role` variants, so any new pack must reuse those four analyst slots.

This design preserves all existing equity behaviour byte-for-byte and adds a parallel ETF pipeline that:

- Reuses every existing infrastructure layer (graph-flow topology, `Role` enum, `PromptBundle`, `Valuator` trait, `ScenarioValuation` enum) by extension, not replacement.
- Uses **only free-tier data sources** (yfinance public endpoints via `yfinance-rs 0.7.2`, SEC EDGAR via the existing `SecEdgarClient`).
- Emits structured availability flags so missing-data cases degrade visibly via warnings and omitted sections rather than being silently mis-analysed.

## Decisions

| Decision                     | Choice                                                                                                                                                                            | Rationale                                                                                                                                                                                                                           |
|------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Signal scope                 | Phase 1 ships premium/discount plus composition/tracking when supported; Phase 2 adds GEX parity                                                                                  | Solves the ETF short-circuit first while keeping the full reference-skill path explicit for follow-up work.                                                                                                                         |
| Analyst slot mapping         | Subject-aligned: Fundamental→Composition, Sentiment→Flow/Premium, News→Macro/Sector, Technical→Tracking/Options                                                                   | Each slot has one sharp purpose; prompts are ETF-native, not "equity prompts with appendix".                                                                                                                                        |
| Failure policy               | Tiered degradation with per-signal flags                                                                                                                                          | Quote availability is the only hard stop; missing NAV, holdings, benchmark, and options data degrade the affected signals via `EtfDataAvailability` flags plus report/prompt warnings. Matches existing 1-analyst-fail-OK contract. |
| Prompt strategy              | Per-slot prompts compose shared scaffolding (mirrors `with_analyst_runtime_contract_sections` in equity baseline)                                                                 | Eliminates duplication; same pattern the codebase already uses.                                                                                                                                                                     |
| Prompt reuse                 | Promote 9 cross-cutting equity prompts to `analysis_packs/common/prompts/` (6 Tier 1 verbatim + 3 Tier 2 with ETF composition deltas); ETF pack composes from common + ETF deltas | ~30% reduction in new prompts; equity pack also benefits from cleaner location; zero cross-pack include paths.                                                                                                                      |
| Holdings staleness threshold | 90 days                                                                                                                                                                           | Matches worst-case SEC N-PORT-P delay (60-day filing window). >90 days → `holdings_fresh=false`, prompt sees stale flag; >180 days → skip composition signal.                                                                       |
| CLI behaviour                | Auto-route only; no `--pack` override flag                                                                                                                                        | Smaller surface; profile-based routing is deterministic.                                                                                                                                                                            |
| Routing fallback             | `get_profile → None` falls back to `PackId::Baseline` with a user-visible warning                                                                                                 | `get_profile` already returns `Option<Profile>` (None on any failure). Falling back is safer than aborting, but the rendered report/header must say the run used baseline routing because ETF detection was unavailable.            |
| Routing visibility           | Always surface the active pack and any fallback reason in the rendered report/header                                                                                              | Auto-routing has no user control surface, so users need to see what actually ran.                                                                                                                                                   |
| Report layout                | New ETF Valuation Snapshot panel; shared report scaffolding                                                                                                                       | Pattern matches today's report — just a different valuation panel.                                                                                                                                                                  |
| Valuation policy variant     | Add `ValuationAssessment::Etf` (third variant alongside `Full`, `NotAssessed`)                                                                                                    | Mirrors `ScenarioValuation::Etf` on output; avoids semantic abuse of `Full` (which implies DCF).                                                                                                                                    |
| Output variant               | Add `ScenarioValuation::Etf(EtfValuation)` to existing enum                                                                                                                       | Serde-compatible additive variant; old snapshots round-trip unchanged.                                                                                                                                                              |
| Inputs carrier               | Extend `ValuationInputs<'a>` with optional ETF fields                                                                                                                             | Existing valuators ignore the new fields; trait signature unchanged.                                                                                                                                                                |

### Delivery phases

- Phase 1 (this slice): runtime ETF classification, ETF valuation inputs, premium/discount, composition when fresh N-PORT data exists, source-provided benchmark tracking, and user-visible ETF report/routing warnings.
- Phase 2 (follow-up once Phase 1 is useful): dealer GEX, options-heavy prompt/report extensions, and any related live-data hardening for that path.

## Architecture

### Routing dispatch (runtime selection + preflight recording)

```rust
// runtime pack selection runs before `build_graph_from_pack(...)`
enum RuntimePackSelection {
    Baseline { reason: &'static str },
    EtfBaseline,
}

fn classify_runtime_pack(
    profile: Option<&Profile>,
    fund_info: Option<&FundInfo>,
) -> RuntimePackSelection {
    match (profile, fund_info.and_then(|info| info.fund_kind.as_deref())) {
        (Some(Profile::Fund(_)), Some(kind)) if is_supported_etf_kind(kind) => {
            RuntimePackSelection::EtfBaseline
        }
        (Some(Profile::Fund(_)), _) => RuntimePackSelection::Baseline {
            reason: "unsupported_fund_shape",
        },
        (Some(Profile::Company(_)), _) => RuntimePackSelection::Baseline {
            reason: "corporate_equity",
        },
        (None, _) => RuntimePackSelection::Baseline {
            reason: "profile_lookup_unavailable",
        },
    }
}
```

`TradingPipeline::new` adds a lightweight runtime-selection step before `build_graph_from_pack(...)` so the selected pack still drives `required_inputs`, analyst fan-out, and runtime-policy wiring. The selector calls `yfinance.get_profile(&symbol)` first and, for `Profile::Fund` symbols, `get_fund_info(&symbol)` to confirm the instrument is an in-scope ETF before handing `PackId::EtfBaseline` to the graph builder. `PreflightTask` remains the first graph task, but it no longer owns pack selection: it records the already-resolved pack, routing reason, and warning metadata into state/context for downstream topology checks, tracing, and user-visible reporting. When runtime selection falls back to `Baseline`, preflight still emits `routing.pack`, `routing.profile_present`, and `routing.fallback`, and the rendered report/header shows the same fallback warning to the user.

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
│   └── gex.rs                        (NEW, Phase 2) Black-Scholes gamma + GEX aggregation
│
├── workflow/
│   ├── builder.rs                    (UPDATE) runtime pack selection resolves before `build_graph_from_pack()`
│   └── tasks/
│       ├── preflight.rs              (UPDATE) records resolved pack + routing warning metadata
│       └── analyst.rs                (UPDATE) owns ETF input hydration; constructor gains `SecEdgarClient`
│
└── state/
    └── derived.rs                    (+) ScenarioValuation::Etf(EtfValuation)
                                       (+) EtfValuation, PremiumSnapshot, EtfComposition,
                                           TrackingError, GexSummary, EtfDataAvailability
                                       (+) ScenarioValuation enum gains one variant

crates/scorpio-reporters/src/terminal/
├── etf.rs                            (NEW) render_etf_panel() — shares header/risk scaffolding
└── valuation.rs                      (UPDATE) dispatches `ScenarioValuation::Etf(_)` to the ETF panel
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

| Source              | New method                                                                                    | Returns                                                                                                                                       | Fail-soft                     |
|---------------------|-----------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------|
| yfinance            | `YFinanceClient::get_quote(&str) -> Option<EtfQuote>`                                         | `nav, regular_market_price, previous_close, bid, ask, market_cap, day_volume, currency`                                                       | Yes — None on transport error |
| yfinance            | `YFinanceClient::get_fund_info(&str) -> Option<FundInfo>`                                     | `category, fund_family, expense_ratio, total_assets, leverage_factor, fund_kind, stated_benchmark`                                            | Yes                           |
| yfinance            | `YFinanceClient::get_distribution_yield_ttm(&str) -> Option<f64>`                             | TTM sum of distributions / current price                                                                                                      | Yes                           |
| SEC EDGAR           | `SecEdgarClient::resolve_fund_cik(&str) -> Option<String>`                                    | ETF CIK resolved through the existing ticker→CIK lookup path already owned by `sec_edgar.rs`; unresolved tickers degrade holdings/composition | Yes                           |
| SEC EDGAR           | `SecEdgarClient::fetch_latest_nport_p(cik: &str, max_age_days: u32) -> Option<NPortHoldings>` | `{filing_date, holdings: Vec<{cusip, ticker?, name, weight_pct, value_usd}>, sector_breakdown, stated_benchmark}`                             | Yes                           |
| Benchmark selection | task-local helper on ETF hydration                                                            | `fund_info.stated_benchmark.or(nport_holdings.stated_benchmark)`                                                                              | Yes                           |
| GEX math            | `indicators::gex::compute_gex(chain, spot) -> GexSummary`                                     | Aggregated BSM gamma per strike (Phase 2)                                                                                                     | N/A (pure function)           |

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

### ETF input hydration ownership

`AnalystSyncTask` stays the owner of deterministic valuation input hydration. Its constructor grows a `SecEdgarClient`, and `fetch_valuation_inputs()` splits into two branches:

- Existing equity fetches remain unconditional.
- ETF-only fetches run when the resolved runtime pack is `EtfBaseline`: quote, fund info, distribution yield, fund CIK resolution, N-PORT holdings, options snapshot, and benchmark OHLCV when a source-provided benchmark symbol is present.

`TradingPipeline::new` / `build_graph_from_pack()` wire the `SecEdgarClient` into `AnalystSyncTask`, while preflight exposes the resolved pack and routing reason the sync stage reads. The hydrated ETF bundle is stored alongside the existing valuation inputs before `valuation_derive`, preserving the current fail-soft contract.

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
        description: "Phase 1: ETF-native analysis via premium/discount band, composition/sector tilt \
                      when fresh N-PORT data is available, and tracking error vs a source-provided \
                      benchmark. Phase 2 adds dealer GEX where available. Sources: yfinance (free) \
                      + SEC EDGAR N-PORT-P (free).".to_owned(),
        required_inputs: vec![
            "fundamentals".to_owned(),   // → Composition & Costs prompt
            "sentiment".to_owned(),      // → Flow & Premium prompt
            "news".to_owned(),           // → Macro & Sector Catalysts prompt
            "technical".to_owned(),      // → Tracking prompt in Phase 1 (+ Options/GEX in Phase 2)
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

The leverage warning is conditionally injected by the renderer at substitution time when the ETF valuation context carries `leverage_factor != 1.0`; the static manifest stays leverage-agnostic.

### Report rendering

A new `crates/scorpio-reporters/src/terminal/etf.rs` exports `render_etf_panel(state: &TradingState) -> String`, and `crates/scorpio-reporters/src/terminal/valuation.rs` dispatches `ScenarioValuation::Etf(_)` to it while `scorpio-cli` keeps delegating to `scorpio_reporters::terminal::render_final_report`. The rendered header always shows `Analysis Pack`, and when runtime selection falls back to `Baseline` it also prints a warning line describing the fallback reason. Header, analyst summaries, debate, risk, and fund-manager scaffolding are unchanged. Wide-terminal example output after Phase 2 (Phase 1 omits the `DEALER GAMMA` sub-section):

```
═══ ETF VALUATION SNAPSHOT ═══════════════════════════════════════════
  Analysis Pack    ETF Baseline
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

When a flag disables a signal, the affected sub-section is omitted and a `⚠` line surfaces in DATA AVAILABILITY. `holdings_fresh=false` is the explicit exception: composition still renders with a staleness warning.

```
  ⚠ Holdings unavailable — no N-PORT-P filing within 90 days (CIK 0001234567)
  ⚠ Tracking error skipped — benchmark not resolved for category "Thematic"
  ⚠ Dealer gamma skipped — no options chain available
```

When runtime selection falls back to `Baseline`, the same warning surfaces in the rendered header:

```
  Analysis Pack    Baseline
  ⚠ ETF routing fallback — profile lookup unavailable; baseline pack used for this run
```

The degraded ETF states are explicit rather than implicit:

```
  Market Price     $621.40       NAV           unavailable
  Premium          unavailable   Band          Unknown
  ⚠ Premium band unavailable — NAV missing from ETF quote payload
```

```
  Bid/Ask          unavailable   Spread        unavailable
  ⚠ Noise-floor check skipped — bid/ask unavailable
```

```
  Analysis Pack    ETF Baseline
  ETF valuation    Not assessed
  Reason           etf_quote_unavailable
  ⚠ ETF quote unavailable — premium snapshot and downstream ETF signals skipped
```

When terminal width is constrained, the renderer falls back to a narrow/plain-text layout that stacks fields vertically, avoids multi-column wrapping, and replaces decorative Unicode with ASCII labels where needed. The wide layout above remains the default when the terminal is wide enough.

## Failure modes & data availability

| Condition                                          | Detection                                                                              | Behaviour                                                                                                                                                   |
|----------------------------------------------------|----------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `get_profile` fails (network, parse)               | `Option::None` returned                                                                | Fallback to `PackId::Baseline`; preflight emits `routing.fallback=true`, and the rendered header warns `ETF routing fallback — profile lookup unavailable`. |
| Fund-shaped symbol is not a supported ETF          | `Profile::Fund` + missing/unsupported `fund_kind`                                      | Fallback to `PackId::Baseline`; rendered header warns `unsupported fund shape for ETF routing`.                                                             |
| ETF detected but `Ticker::quote()` returns nothing | `etf_quote.is_none()` in valuator                                                      | Valuator returns `NotAssessed { reason: "etf_quote_unavailable" }`. Pipeline continues; ETF panel renders the explicit not-assessed variant.                |
| ETF detected, quote OK, no NAV                     | `quote.nav.is_none()`                                                                  | `flags.nav_available = false`; `premium.premium_pct = None`; report shows `NAV unavailable` and band `Unknown`.                                             |
| No bid/ask                                         | `quote.bid.is_none()` or `quote.ask.is_none()`                                         | `flags.bid_ask_available = false`; noise-floor gate cannot fire; report shows `Bid/Ask unavailable`.                                                        |
| N-PORT-P missing                                   | EDGAR returns empty                                                                    | `flags.holdings_present = false`, `composition = None`. Composition is opportunistic; report warns and continues.                                           |
| N-PORT-P > 90 days old                             | `holdings_age_days > 90`                                                               | `flags.holdings_fresh = false`; composition still rendered, with staleness shown in report.                                                                 |
| N-PORT-P > 180 days old                            | `holdings_age_days > 180`                                                              | Skip composition signal entirely (`composition = None`).                                                                                                    |
| Benchmark unavailable from sources                 | `fund_info.stated_benchmark.is_none()` and `nport_holdings.stated_benchmark.is_none()` | `flags.benchmark_resolved = false`; `tracking = None`.                                                                                                      |
| Options chain empty                                | yfinance `options()` returns `[]`                                                      | `flags.options_chain_present = false`; `options_gex = None` (Phase 2 only).                                                                                 |
| Leveraged/inverse ETF                              | `fund_info.leverage_factor != 1.0`                                                     | `etf_leverage_warning.md` injected into Conservative + Neutral risk + Auditor prompts.                                                                      |

The fund manager's deterministic-fallback semantics (per `2026-04-20-fund-manager-dual-risk-escalation-design.md`) are preserved — the ETF `conservative_risk.md` prompt teaches the LLM to flag `tracking_failure`, `extreme_premium`, or `leverage_decay`, and the fund manager prompt's first-line audit contract applies the same way.

## Test plan

| Layer               | Location                                                                        | Coverage                                                                                                                                                                      |
|---------------------|---------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Pack manifest       | `crates/scorpio-core/src/analysis_packs/etf/baseline.rs#tests` (mirrors equity) | `etf_baseline_pack().validate().is_ok()`, correct id, required_inputs, prompt slots populated, valuator_selection contains Fund key                                           |
| Valuator            | `crates/scorpio-core/src/valuation/etf/premium_discount.rs#tests`               | Premium math, band classification, bid-ask floor, each flag-false branch returns sane sub-struct, leverage-factor pass-through                                                |
| State serde         | `crates/scorpio-core/tests/state_roundtrip.rs` (extend)                         | `ScenarioValuation::Etf(…)` round-trips JSON; old snapshots without the variant still deserialize; `EtfDataAvailability` defaults work                                        |
| Routing             | `crates/scorpio-core/tests/workflow_pipeline_structure.rs` (extend)             | ETF-confirmed fund→EtfBaseline, unsupported fund→Baseline + warning, `None→Baseline` + fallback warning; selected pack still drives topology before analyst fan-out           |
| ETF input hydration | `crates/scorpio-core/src/workflow/tasks/analyst.rs#tests` (extend)              | `AnalystSyncTask` fetches quote, fund info, fund CIK, N-PORT, source-provided benchmark, and options opportunistically; each missing branch degrades without aborting         |
| Prompt completeness | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` (extend)           | EtfBaseline manifest passes `validate_active_pack_completeness` for all 4 topology shapes (full / no-debate / no-risk / neither)                                              |
| Equity regression   | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` (extend)           | shared-prompt extraction keeps baseline-equity prompt bytes and manifest behaviour unchanged for the moved prompt files                                                       |
| Live smoke (manual) | `crates/scorpio-core/examples/etf_quote_live_test.rs` (NEW)                     | `get_quote(SPY)`, `get_fund_info(SPY)`, `get_distribution_yield_ttm(SPY)`, `get_profile(SPY)→Fund`, `get_profile(AAPL)→Company`, `get_profile(BOGUS)→None`                    |
| Live smoke (manual) | `crates/scorpio-core/examples/nport_live_test.rs` (NEW)                         | `resolve_fund_cik(spy_ticker)`, `fetch_latest_nport_p(spy_cik, 90)`, fail-soft `fetch_latest_nport_p(bogus_cik, 90)→None`, staleness flag fires on aged fixture               |
| Live smoke (manual) | `crates/scorpio-core/examples/etf_pack_live_test.rs` (NEW)                      | End-to-end: runtime selection on SPY → pack=EtfBaseline; unsupported fund fixture → pack=Baseline + warning; bogus → pack=Baseline (fallback warning verified)                |
| Report rendering    | `crates/scorpio-reporters/tests/terminal.rs` (extend)                           | ETF panel renders for `ScenarioValuation::Etf`; routing banner, degraded-state output, narrow fallback, and missing-data footer all render correctly; equity report unchanged |

Live smoke tests are NOT in CI — they run via `cargo run -p scorpio-core --example <name>` per the existing `<source>_live_test.rs` convention.

## Out of scope

- Real-time NAV (iNAV) — yfinance only provides end-of-prior-day NAV.
- Bond ETF duration / convexity analysis — needs `/etf/bond-profile` (paid Finnhub) or coupon scraping.
- Benchmark inference beyond source-provided benchmark fields — if fund metadata and N-PORT both omit a benchmark, tracking error is skipped and flagged.
- Closed-end fund discount analysis — different mechanic; separate `PackId::ClosedEndFund` if ever needed.
- ETF holdings recursion (ETF-of-ETFs unwrapping) — first-level holdings only.
- Active ETF stock-picking quality vs benchmark — out of scope; tracking error covers index-tracking variant only.
- Future crypto ETF support — explicitly out (would need spot crypto + custody premium analysis).

## Open questions

None for the design itself. Implementation-time questions deferred to the writing-plans phase:

- Whether to cache N-PORT-P filings in SQLite (existing snapshot DB) or fetch on every run — likely cache with 30-day TTL but exact strategy TBD in plan.
- Phase 2 GEX math helper: borrow `kand` crate API or implement standalone — `kand` doesn't currently expose BSM Greeks; likely standalone in `indicators/gex.rs`.

## References

- `himself65/finance-skills/plugins/market-analysis/skills/etf-premium/SKILL.md` — source methodology.
- `himself65/finance-skills/.../references/etf_premium_reference.md` — category benchmark table.
- CLAUDE.md — `Pack-owned prompts (centralized)`, `TradingState schema evolution`, error handling pattern.
- `2026-04-20-fund-manager-dual-risk-escalation-design.md` — fund manager dual-risk contract preserved.
- `2026-04-25-prompt-bundle-centralization-design.md` — `PromptBundle` and prompt slot architecture.
- `2026-04-28-shared-options-evidence-design.md` — `options_context.outcome.kind` semantics reused by ETF tracking/options analyst.

## Phase 2 scope (deferred)

The Phase 2 slice fills in the dealer-gamma signal and lands the durability work the Phase 1 implementation deferred. It runs once Phase 1 is in production and useful. Each item is independently shippable.

1. **`indicators/gex.rs` — Black-Scholes gamma + GEX aggregation (standalone).** Per the resolved open question, `kand` does not currently expose BSM Greeks; this slice implements a small standalone helper that takes an `OptionsSnapshot` plus spot price and returns net/gross dealer gamma per 1% move, call/put OI ratio, and max-pain strike. Pure function — no I/O, no `unsafe`.
2. **Options-chain fetch in `AnalystSyncTask`.** The existing `YFinanceOptionsProvider` already returns an `OptionsSnapshot`; thread it into `ValuationInputs.etf_options` only when `pack_id == EtfBaseline`. Fail-soft on empty chains (`flags.options_chain_present = false`, `options_gex = None`).
3. **`compute_gex_summary` in `valuation/etf/`.** New module that consumes `OptionsSnapshot` + `PremiumSnapshot.market_price` and populates `EtfValuation.options_gex` with the `GexSummary` already declared in Phase 1's state schema. Updates `EtfPremiumDiscountValuator::assess` to call it when `etf_options` is `Some`.
4. **ETF report panel "Dealer Gamma" sub-section.** The Phase 1 wide-terminal layout already reserves the `─── DEALER GAMMA  (near-term …) ───` block; Phase 2 fills it in and threads the conditional `etf_leverage_warning.md` injection through the renderer at substitution time (Phase 1 marks the warning text but does not inject it).
5. **Prompt extensions to `etf_tracking_options_focus.md` (and the technical-analyst composition).** Phase 1 wires the file in as a Tier-2 delta but marks the options-chain branch out-of-scope; Phase 2 swaps the placeholder language for real GEX-aware reasoning so analysts can cite `options_gex.net_gex_usd_per_1pct_move`, the call/put OI ratio, and max-pain strike directly.
6. **Cache N-PORT monthly report.** Persist parsed `NPortHoldings` to the existing snapshot SQLite store keyed on `(cik, filing_date)` with a 30-day TTL (per the resolved open question). The cache replaces the per-run EDGAR `fetch_latest_nport_p` call when fresh, and falls back to a live fetch otherwise. N-PORT-P is filed quarterly with monthly snapshots — a 30-day TTL captures the typical update cadence without forcing redundant fetches for the same filing across consecutive analyses. Lives in `data/sec_edgar/nport_cache.rs`; fail-soft on cache-read errors (treat any failure as a miss).
