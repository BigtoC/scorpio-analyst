# EtfBaseline Phase 2 — Dealer Greeks + Cache Consolidation

**Date:** 2026-05-22
**Status:** Draft — awaiting user review
**Parent:** [`2026-05-21-etf-baseline-pack-design.md`](./2026-05-21-etf-baseline-pack-design.md)

## Goal

Land the dealer-positioning signal the Phase 1 design reserved (`EtfValuation.options_gex`) and the durability work Phase 1 deferred. Phase 2 fills in real Black-Scholes-Merton gamma, Vanna, and Charm computation; threads the existing options-chain fetch into the ETF pack; aggregates dealer GEX, VEX, and CEX across the near-term expiration *and* across all listed expirations; surfaces gamma walls; consolidates the local on-disk caches into a single `data_cache.db` with an N-PORT-P cache table; injects the leverage warning into Conservative / Neutral / Auditor prompts at render time; and sources the BSM risk-free rate from FRED instead of a hard-coded constant.

Pack selection, prompt scaffolding, manifest topology, analyst-slot mapping, and the ETF report header are unchanged from Phase 1.

## Problem

Phase 1 shipped premium/discount, composition, and tracking-error analysis for ETFs but left several first-class holes behind explicit feature flags:

- [`state/derived.rs:290`](../../../crates/scorpio-core/src/state/derived.rs) declares `GexSummary` but the field is always `None` because [`valuation/etf/premium_discount.rs:75`](../../../crates/scorpio-core/src/valuation/etf/premium_discount.rs) hardcodes `options_gex: None`.
- `OptionsSnapshot.near_term_strikes` carries front-month data only — sufficient for the equity Technical Analyst path but throws away the per-expiration per-strike rows the yfinance provider already fetches internally to compute the term structure.
- `etf_tracking_options_focus.md` contains placeholder language ("options-chain branch deferred to Phase 2") that the Technical analyst cannot act on.
- `etf_leverage_warning.md` is wired into `etf/baseline.rs` as a constant but no code path injects it; Conservative-risk / Neutral-risk / Auditor prompts treat leveraged ETFs the same as unlevered ones.
- Every ETF analysis run re-fetches the same N-PORT-P filing from SEC EDGAR even though the filing payload is immutable per `(cik, filing_date)`. The Phase 1 design lists this as a resolved open question pointing at a 30-day TTL cache.
- The transcript cache lives at `~/.scorpio-analyst/transcript_cache.db`. Adding a second SQLite store for N-PORT would double the on-disk file count for a related concern.
- BSM gamma is weakly sensitive to the risk-free rate at near-term expirations, but the existing FRED client already exposes `get_series_latest(series_id)` and the codebase has no general "fetch a treasury rate" precedent — leaving the rate hardcoded would diverge from the project's "deterministic inputs flow through state" pattern.

Phase 2 closes all seven gaps in a single design pass while keeping every individual deliverable independently shippable.

## Decisions

| Decision                                  | Choice                                                                                                                                | Rationale                                                                                                                                                                            |
|-------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Spec scope                                | All six Phase 2 items in one design                                                                                                   | They share data paths (options chain, FRED, cache) and prompt updates; one coherent slice avoids design drift across follow-ups.                                                     |
| Dealer-positioning model                  | SqueezeMetrics convention (dealers short calls, long puts) for GEX / VEX / CEX                                                        | Industry-default convention used by SpotGamma and major sell-side desks; "positive GEX = stabilizing flow" interpretation is the practitioner default.                               |
| BSM volatility input                      | Per-strike IV (`NearTermStrike.call_iv` / `put_iv`) with `OptionsSnapshot.atm_iv` fallback                                            | Preserves the skew/smile signal where the chain exposes per-strike IVs; degrades gracefully when individual rows are sparse.                                                         |
| Risk-free rate `r`                        | FRED `DGS3MO` fetched at preflight when pack = `EtfBaseline`; const `0.045` fallback when FRED fails                                  | Aligns with project pattern of deterministic inputs flowing through state. Existing `FredClient::get_series_latest` covers the fetch — no new method needed.                         |
| Dividend yield `q`                        | `EtfComposition.distribution_yield_ttm_pct` when present, else `0.0`                                                                  | Phase 1 already plumbs the TTM distribution yield; reusing it keeps Phase 2's BSM inputs self-contained.                                                                             |
| Sign convention propagation               | Same SqueezeMetrics convention applied to GEX, VEX, and CEX uniformly                                                                 | Avoids per-Greek polarity surprises in prompts/report; "positive = dealer-stabilizing flow" interpretation generalizes.                                                              |
| Greeks scope                              | Gamma + Vanna + Charm only                                                                                                            | The three Greeks practitioners cite alongside dealer flow analysis. Higher-order (Vomma, Speed, Color, Zomma) are out of scope.                                                      |
| Multi-expiration aggregation              | In scope — emit a `broad: Option<BroadGex>` alongside the front-month `net/gross_gex_usd_per_1pct_move`                               | yfinance already fetches all expirations for the term-structure ATM-IV vector; per-strike rows are discarded today. Plumbing them through adds practitioner parity at no fetch cost. |
| Per-strike gamma walls                    | Top-3 strikes by `\|net_gex\|` emitted in `GexSummary.strikes: Vec<StrikeGex>`                                                        | Aggregator already computes per-strike gamma; surfacing the top concentrations lets the LLM cite specific gamma walls. Capping at 3 keeps state and prompts compact.                 |
| Cache layout                              | Rename `transcript_cache.db` → `data_cache.db`; add `nport_cache` table next to the existing `transcript_cache` table in the same DB  | Keeps a single on-disk cache file; new N-PORT cache reuses the same SQLite pool + WAL + migration plumbing.                                                                          |
| Cache TTL                                 | 30 days, on-demand revalidation at read                                                                                               | Matches the N-PORT-P quarterly filing cadence with monthly snapshots; explicit Phase 1 resolved open question.                                                                       |
| Legacy file migration                     | Auto-rename `transcript_cache.db` → `data_cache.db` via `std::fs::rename` on first open of `DataCacheStore`                           | Preserves cached transcripts transparently; logged via `tracing::info!`; rename failure logs `warn!` and falls back to opening the legacy file.                                      |
| Config field rename                       | Hard rename `storage.transcript_cache_db_path` → `storage.data_cache_db_path`. No serde alias.                                        | Avoids carrying a deprecated knob; `serde(default)` on the new field keeps deserialization permissive when the field is absent.                                                      |
| Config self-heal                          | `Config::load_from_user_path` resolves `data_cache_db_path` to `<dirname(snapshot_db_path)>/data_cache.db` when absent + writes back  | Keeps the user's `config.toml` self-documenting; no deserialization error path; resolution uses the snapshot DB's parent dir as the anchor.                                          |
| Leverage warning injection                | Renderer-side at prompt-assembly time: append `etf_leverage_warning.md` to Conservative / Neutral / Auditor system prompts when ETF's `leverage_factor != 1.0` | Manifest stays leverage-agnostic; injection lives next to the existing `{ticker}` / `{analysis_emphasis}` substitution path.                                                         |
| Deterministic GEX trigger                 | None — GEX/VEX/CEX stay LLM-visible evidence, not deterministic fund-manager vetoes                                                   | Phase 1's dual-risk audit contract already covers `tracking_failure`, `extreme_premium`, `leverage_decay`. A GEX magnitude threshold would commit the contract to a single heuristic. |
| Prompt update for tracking/options focus  | Update `etf_tracking_options_focus.md` in place                                                                                       | Phase 1 wired this file in with placeholder language; Phase 2 replaces it with real GEX/VEX/CEX-aware guidance. No manifest churn.                                                   |
| Smoke-test discipline                     | Every new or extended fetch surface gets a `crates/scorpio-core/examples/*.rs` smoke                                                  | Standing convention per `<source>_live_test.rs`; carries forward through any later phases.                                                                                           |
| Warning discipline                        | All fail-soft paths emit `tracing::warn!` with stable target + structured fields; never log payload bytes                             | Matches the snapshot-deserialize warn rule in CLAUDE.md (`error.kind = "deserialize"`, never `serde_json` text).                                                                     |

## Architecture

### Component layout

```
crates/scorpio-core/src/
├── indicators/
│   └── gex.rs                          (NEW) BSM gamma + Vanna + Charm + aggregation helpers
│
├── data/
│   ├── data_cache/                     (NEW DIR — rename of transcript_cache.rs)
│   │   ├── mod.rs                      DataCacheStore (pool, open, migrate, legacy rename)
│   │   ├── transcripts.rs              transcript put/get (verbatim move from transcript_cache.rs)
│   │   └── nport.rs                    put_nport / get_nport / purge_expired_nport
│   └── sec_edgar/
│       └── nport.rs                    (UPDATE) cache-aware fetch wrapper around live parser
│
├── valuation/
│   └── etf/
│       ├── premium_discount.rs         (UPDATE) consume etf_options + etf_risk_free_rate; call compute_gex_summary
│       └── gex.rs                      (NEW) compute_gex_summary — wraps indicators::gex aggregates into GexSummary
│
├── data/traits/options.rs              (UPDATE) OptionsSnapshot gains all_expirations: Vec<ExpirationStrikes>
├── data/yfinance/options.rs            (UPDATE) normalizer plumbs per-expiration strikes into the new field
│
├── workflow/
│   ├── tasks/preflight.rs              (UPDATE) opportunistic DGS3MO fetch when pack = EtfBaseline
│   └── tasks/analyst.rs                (UPDATE) ETF branch reads OptionsOutcome::Snapshot into ValuationInputs.etf_options
│
├── state/derived.rs                    (UPDATE) GexSummary additive fields; new StrikeGex, BroadGex, VexSummary, CexSummary
├── state/mod.rs                        (UPDATE) TradingState gains etf_risk_free_rate: Option<f64> with #[serde(default)]
│
├── analysis_packs/etf/
│   ├── baseline.rs                     (UPDATE) maybe_inject_leverage_warning helper; renderer-substitution hookup
│   └── prompts/
│       └── etf_tracking_options_focus.md (UPDATE) replace placeholder block with real GEX/VEX/CEX guidance
│
├── config.rs                           (UPDATE) StorageConfig: rename transcript_cache_db_path → data_cache_db_path;
│                                                self-heal via Config::resolve_data_cache_path
│
└── app/mod.rs                          (UPDATE) DataCacheStore::from_config replaces TranscriptCacheStore::from_config

crates/scorpio-core/migrations/
├── transcript_cache/                   (REMOVED — replaced by data_cache/)
└── data_cache/                         (NEW)
    ├── 0001_create_transcript_cache.sql  byte-identical move of the existing transcript_cache migration
    └── 0002_create_nport_cache.sql       new table for parsed N-PORT-P payloads

crates/scorpio-core/examples/
├── fred_live_test.rs                   (UPDATE) add DGS3MO assertion alongside FEDFUNDS + CPI
├── yfinance_options_chain_live_test.rs (NEW) OptionsSnapshot.all_expirations smoke
├── nport_cache_live_test.rs            (NEW) cache hit/miss/TTL + legacy-file rename smoke
└── etf_options_gex_live_test.rs        (NEW) end-to-end ETF run with full GexSummary populated

crates/scorpio-reporters/src/terminal/
└── etf.rs                              (UPDATE) DEALER POSITIONING block + per-Greek lines + gamma walls
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

    // Phase 2 additions — all #[serde(default)] so legacy snapshots (Phase 1
    // wrote `options_gex: None`, never `Some(_)`, so this is hypothetical but
    // we keep additive discipline for future-proofing):
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
// crates/scorpio-core/src/state/mod.rs

pub struct TradingState {
    // ... existing fields ...

    /// Risk-free rate (decimal fraction, e.g. 0.0427) sourced from FRED
    /// DGS3MO at preflight when the active pack is `EtfBaseline`. `None`
    /// when pack != EtfBaseline OR when the FRED fetch failed. Consumers
    /// (the ETF valuator) fall back to a const `0.045` in the `None` case.
    #[serde(default)]
    pub etf_risk_free_rate: Option<f64>,
}
```

Per `CLAUDE.md`'s `TradingState` schema evolution rules:

- `etf_risk_free_rate` carries `#[serde(default)]`; old snapshots without the field deserialize unchanged.
- The four new `GexSummary` sub-fields all carry `#[serde(default)]`. Adding fields to an existing struct nested in `ScenarioValuation::Etf` is additive.
- No `THESIS_MEMORY_SCHEMA_VERSION` bump is required (no renames, removals, or type changes).
- No `#[serde(deny_unknown_fields)]` is introduced on any state struct touched here.

### `OptionsSnapshot` extension

```rust
// crates/scorpio-core/src/data/traits/options.rs

pub struct OptionsSnapshot {
    // ... existing fields unchanged ...

    /// Per-expiration per-strike rows for the full listed chain. Populated
    /// whenever the provider successfully fetches expiration data; empty
    /// vec when the provider only produced front-month rows.
    #[serde(default)]
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

`YFinanceOptionsProvider::fetch_snapshot_impl` already iterates expirations to build the term-structure ATM-IV vector. Phase 2 retains the existing iteration but additionally collects each expiration's normalized strike rows into the new field. No additional network calls.

When the provider hits any `OptionsOutcome` other than `Snapshot(_)` (e.g. `SparseChain`, `NoListedInstrument`, `MissingSpot`, `HistoricalRun`), no change — the outcome is propagated as-is.

### `ValuationInputs` extension

```rust
// crates/scorpio-core/src/valuation/mod.rs

pub struct ValuationInputs<'a> {
    // ... existing fields including Phase 1 ETF fields ...

    /// FRED DGS3MO snapshot threaded through from preflight when the
    /// active pack is `EtfBaseline`. `None` when pack != EtfBaseline OR
    /// when FRED was unreachable at preflight time. The ETF valuator
    /// substitutes a const `0.045` when `None`.
    pub etf_risk_free_rate: Option<f64>,
}
```

`AnalystSyncTask::fetch_valuation_inputs` reads `state.etf_risk_free_rate` and copies it into the carrier alongside the Phase 1 ETF fields. Equity valuators ignore the new field.

### FRED preflight integration

`PreflightTask` already records routing pack, profile presence, and fallback metadata for the ETF runtime classification. Phase 2 extends it with one additional opportunistic call:

```rust
// crates/scorpio-core/src/workflow/tasks/preflight.rs (sketch)

if matches!(resolved_pack, PackId::EtfBaseline) {
    let rate = match fred.get_series_latest("DGS3MO").await {
        Ok(Some(pct)) => {
            // FRED returns observations as percent (e.g. "4.27"); convert
            // to decimal fraction.
            let frac = pct / 100.0;
            tracing::info!(target: "scorpio_core::workflow::preflight",
                series = "DGS3MO", rate_pct = pct, "fetched ETF risk-free rate");
            ctx.insert(KEY_ROUTING_FLAGS, |flags| flags.insert("risk_free_source", "fred_dgs3mo"));
            Some(frac)
        }
        Ok(None) => {
            tracing::warn!(target: "scorpio_core::workflow::preflight",
                series = "DGS3MO", "DGS3MO observation empty — falling back to const 0.045");
            ctx.insert(KEY_ROUTING_FLAGS, |flags| flags.insert("risk_free_source", "fallback_const"));
            None
        }
        Err(e) => {
            tracing::warn!(target: "scorpio_core::workflow::preflight",
                series = "DGS3MO", error.kind = "fred_fetch",
                "DGS3MO fetch failed — falling back to const 0.045");
            // error.kind only — never the raw error text per CLAUDE.md
            let _ = e;
            ctx.insert(KEY_ROUTING_FLAGS, |flags| flags.insert("risk_free_source", "fallback_const"));
            None
        }
    };
    state.etf_risk_free_rate = rate;
}
```

The fetch is gated on the resolved pack to avoid burning the FRED rate-limit budget on equity / fallback-to-baseline runs. When the pack is anything other than `EtfBaseline`, `etf_risk_free_rate` stays `None` and no FRED call is made.

`routing.risk_free_source` joins the existing routing-flag fields (`pack`, `profile_present`, `fallback`) and is rendered in the report header right under the pack name when the value is `"fallback_const"`:

```
  Analysis Pack    ETF Baseline
  ⚠ Risk-free rate fallback — FRED DGS3MO unavailable; using 0.045 const
```

When `routing.risk_free_source == "fred_dgs3mo"` no banner is shown (success is the silent default).

### Cache rename — `transcript_cache.db` → `data_cache.db`

**Module rename.** `crates/scorpio-core/src/data/transcript_cache.rs` is reshaped into a directory module:

```
data/data_cache/
├── mod.rs           // DataCacheStore (open/migrate/legacy-rename), shared SqlitePool
├── transcripts.rs   // existing transcript put/get; verbatim move from transcript_cache.rs
└── nport.rs         // new put_nport / get_nport / purge_expired_nport
```

`DataCacheStore` exposes the same public surface as `TranscriptCacheStore` plus new N-PORT methods:

```rust
pub struct DataCacheStore { pool: SqlitePool }

impl DataCacheStore {
    pub async fn new(db_path: Option<&Path>) -> Result<Self, TradingError>;
    pub async fn from_config(config: &Config) -> Result<Self, TradingError>;

    // existing transcript surface (moved from TranscriptCacheStore, signatures unchanged)
    pub async fn put_transcript(&self, symbol: &str, quarter: &str, fetch: &TranscriptFetch) -> Result<(), TradingError>;
    pub async fn get_transcript(&self, symbol: &str, quarter: &str) -> Result<Option<TranscriptFetch>, TradingError>;

    // new N-PORT surface
    pub async fn put_nport(&self, cik: &str, filing_date: NaiveDate, payload: &NPortHoldings) -> Result<(), TradingError>;
    pub async fn get_nport(&self, cik: &str, filing_date: NaiveDate, ttl: Duration) -> Result<Option<NPortHoldings>, TradingError>;
    pub async fn purge_expired_nport(&self, ttl: Duration) -> Result<u64, TradingError>;
}
```

**Migration directory.** `crates/scorpio-core/migrations/transcript_cache/` is renamed to `migrations/data_cache/`. The existing `0001_create_transcript_cache.sql` is moved verbatim (byte-identical) so SQLite's `_sqlx_migrations` table treats it as already-applied for any user with an existing `transcript_cache.db`. The new `0002_create_nport_cache.sql`:

```sql
CREATE TABLE IF NOT EXISTS nport_cache (
    cik          TEXT NOT NULL,
    filing_date  TEXT NOT NULL CHECK (filing_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    payload_json TEXT NOT NULL,
    cached_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (cik, filing_date)
);

CREATE INDEX IF NOT EXISTS idx_nport_cache_cached_at ON nport_cache(cached_at);
```

`sqlx::migrate!("migrations/data_cache")` is the only invocation site; the call moves from `TranscriptCacheStore` to `DataCacheStore::new`.

**Legacy file auto-migration.** On `DataCacheStore::from_config`:

1. Resolve the configured `data_cache_db_path` (already self-healed at config load — see next section).
2. If the resolved path does not exist on disk, check for a sibling `transcript_cache.db` in the same parent directory.
3. If the legacy file exists, attempt `std::fs::rename(legacy, configured)`.
   - On success: `tracing::info!(target: "scorpio_core::data::data_cache", legacy = %legacy, new = %configured, "migrated transcript_cache.db → data_cache.db")`.
   - On error: `tracing::warn!(target: "scorpio_core::data::data_cache", legacy = %legacy, error.kind = "rename", "failed to rename legacy cache — opening legacy path")`, then open the legacy path for reading and continue. The next successful start re-attempts the rename.
4. Proceed with `SqlitePoolOptions::connect(...)` + `sqlx::migrate!`.

**Call-site rename.** `crates/scorpio-core/src/app/mod.rs` replaces:

```rust
let transcript_cache = crate::data::transcript_cache::TranscriptCacheStore::from_config(&cfg).await ...
```

with:

```rust
let data_cache = crate::data::data_cache::DataCacheStore::from_config(&cfg).await ...
```

`AlphaVantageClient::new` keeps its current signature; the parameter type changes from `Option<TranscriptCacheStore>` to `Option<DataCacheStore>`. All transcript-specific call sites move from `.put(...)` / `.get(...)` to `.put_transcript(...)` / `.get_transcript(...)` — the rename is mechanical and the bytes-on-disk for transcript rows are identical.

### Config self-heal for `data_cache_db_path`

```rust
// crates/scorpio-core/src/config.rs (sketch)

pub struct StorageConfig {
    // ... other fields unchanged ...

    /// Local data cache (transcripts + N-PORT-P holdings). Resolved by
    /// `Config::resolve_data_cache_path` after deserialization when absent
    /// from the user's TOML.
    #[serde(default)]
    pub data_cache_db_path: Option<String>,

    // NOTE: `transcript_cache_db_path` has been removed. Since the config
    // crate does not enable `deny_unknown_fields`, an old TOML containing
    // this field deserializes without error — the value is silently dropped.
}

impl Config {
    fn resolve_data_cache_path(&mut self) -> bool {
        if self.storage.data_cache_db_path.is_some() {
            return false;
        }
        let snapshot = crate::config::expand_path(&self.storage.snapshot_db_path);
        let parent = snapshot.parent().unwrap_or_else(|| Path::new("."));
        let resolved = parent.join("data_cache.db");
        self.storage.data_cache_db_path = Some(resolved.to_string_lossy().into_owned());
        true
    }
}
```

`Config::load_from_user_path` calls `resolve_data_cache_path` after deserialization but before env-override injection. When the call returns `true` *and* a user config file exists at `~/.scorpio-analyst/config.toml`, the resolved path is written back to that file via the existing atomic-rewrite helper in `settings.rs` (`NamedTempFile` + `rename`). The rewrite is logged at `info!`:

```
info: added data_cache_db_path = "/home/user/.scorpio-analyst/data_cache.db" to ~/.scorpio-analyst/config.toml
```

Power-users who previously had a custom `transcript_cache_db_path` value lose that customization on the rename — the value is silently dropped at deserialization, and the auto-resolved default is written instead. This is acknowledged in the out-of-scope list; users can re-set `data_cache_db_path` manually after the upgrade if they need a non-default location.

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
/// Surfaced for the gamma-wall extraction step in `valuation/etf/gex.rs`. Only
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

**Near-term aggregation.** The aggregator selects the chronologically nearest entry in `expirations` (by `(exp_date - as_of).num_days() >= 0`). All per-strike contributions for that expiration are summed into the `NearTermAggregate` fields. `per_strike` is populated with `(strike, StrikeGreeks)` tuples so the wrapper in `valuation/etf/gex.rs` can extract the top-3 by `|net_gex_strike|`.

**Broad aggregation.** Iterates every expiration row in `expirations`, accumulates the *gamma* sums only (GEX broad — VEX/CEX broad not emitted in Phase 2). Each expiration contributes via the same per-strike formula but with that expiration's `t_years`. `expirations_used` increments once per expiration that contributes at least one usable strike.

**Degenerate-input handling.** All BSM helpers return `0.0` when σ, t, or S is non-positive. `aggregate` returns an `AggregateResult` with `near_term = None` and `broad = None` when no expirations contain usable strikes. Never panics, never returns `Err`.

### `compute_gex_summary` (`valuation/etf/gex.rs`)

Thin wrapper that maps the math layer's `AggregateResult` into the state-layer `GexSummary` shape and pulls non-aggregate fields directly from `OptionsSnapshot`:

```rust
// crates/scorpio-core/src/valuation/etf/gex.rs

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

Returns `None` when no near-term expiration produces usable strikes (e.g. extremely sparse chain). The valuator treats `None` as "options chain unavailable" and sets `flags.options_chain_present = false`.

### Options-chain hydration in `AnalystSyncTask`

`AnalystSyncTask` already prefetches an `OptionsOutcome` into `OptionsToolContext` during the equity flow. Phase 2 extends the ETF branch to consume the same outcome:

```rust
// crates/scorpio-core/src/workflow/tasks/analyst.rs (sketch)

if pack_id == PackId::EtfBaseline {
    // Phase 1 ETF hydration (quote, fund_info, holdings, benchmark OHLCV) unchanged.

    // Phase 2: thread the prefetched options outcome into the ETF valuation inputs.
    let etf_options = match &options_tool_ctx.outcome {
        Some(OptionsOutcome::Snapshot(snap)) => Some(snap),
        Some(other) => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                outcome = %other,
                symbol = %symbol,
                "ETF options chain unavailable — GEX/VEX/CEX skipped"
            );
            None
        }
        None => None,
    };

    valuation_inputs.etf_options = etf_options;
    valuation_inputs.etf_risk_free_rate = state.etf_risk_free_rate;
}
```

No new fetcher is wired; Phase 2 reuses the existing `YFinanceOptionsProvider` instance already attached to `OptionsToolContext`. The ETF and equity flows share one fetch per cycle.

### N-PORT cache integration

`SecEdgarClient::fetch_latest_nport_p` keeps its existing signature but the implementation grows a cache-aware wrapper:

```rust
// crates/scorpio-core/src/data/sec_edgar/nport.rs (sketch)

const NPORT_CACHE_TTL_DAYS: i64 = 30;

impl SecEdgarClient {
    pub async fn fetch_latest_nport_p(
        &self,
        cik: &str,
        max_age_days: u32,
    ) -> Option<NPortHoldings> {
        let cache_ttl = Duration::from_secs((NPORT_CACHE_TTL_DAYS as u64) * 86400);

        // Step 1: discover latest filing date via EDGAR submissions index.
        let filing_date = self.latest_nport_filing_date(cik, max_age_days).await?;

        // Step 2: try the cache first.
        if let Some(cache) = self.data_cache.as_ref() {
            match cache.get_nport(cik, filing_date, cache_ttl).await {
                Ok(Some(holdings)) => {
                    tracing::debug!(
                        target: "scorpio_core::data::sec_edgar::nport",
                        cik = %cik, filing_date = %filing_date,
                        "N-PORT cache hit"
                    );
                    return Some(holdings);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        target: "scorpio_core::data::data_cache",
                        cik = %cik, filing_date = %filing_date,
                        error.kind = "cache_read",
                        "N-PORT cache miss due to error — falling back to live fetch"
                    );
                    let _ = e;
                }
            }
        }

        // Step 3: live fetch via existing parser.
        let holdings = self.fetch_and_parse_nport_p(cik, filing_date).await?;

        // Step 4: write-through to cache (best effort).
        if let Some(cache) = self.data_cache.as_ref() {
            if let Err(e) = cache.put_nport(cik, filing_date, &holdings).await {
                tracing::warn!(
                    target: "scorpio_core::data::data_cache",
                    cik = %cik, filing_date = %filing_date,
                    error.kind = "cache_write",
                    "N-PORT cache write failed — continuing without cache"
                );
                let _ = e;
            }
        }

        Some(holdings)
    }
}
```

`SecEdgarClient` gains an optional `data_cache: Option<DataCacheStore>` field set at construction time. `AnalysisRuntime::new` passes the same shared `DataCacheStore` instance that `AlphaVantageClient` already consumes. When `data_cache` is `None` (e.g. test harnesses that bypass the runtime), the cache-aware path degrades to a pure live fetch — Phase 2 behavior on a cache-less runtime is identical to Phase 1.

The cache TTL constant (`NPORT_CACHE_TTL_DAYS = 30`) lives in `data/sec_edgar/nport.rs` rather than `config.rs` — there is no env override and no `SCORPIO__DATA__NPORT_CACHE_TTL_DAYS` knob in Phase 2.

### Leverage-warning injection

```rust
// crates/scorpio-core/src/analysis_packs/etf/baseline.rs

const ETF_LEVERAGE_WARNING: &str = include_str!("prompts/etf_leverage_warning.md");

const LEVERAGE_TOLERANCE: f64 = 1e-6;

/// Wrap a system-prompt body in the leverage-warning suffix when the ETF
/// has a non-unit leverage factor. Borrowing fast-path when no warning is
/// needed; owned allocation only on the leveraged-ETF branch.
pub fn maybe_inject_leverage_warning(
    prompt: &str,
    leverage_factor: Option<f64>,
) -> Cow<'_, str> {
    match leverage_factor {
        Some(f) if (f - 1.0).abs() > LEVERAGE_TOLERANCE => {
            Cow::Owned(format!("{}\n\n---\n\n{}", prompt, ETF_LEVERAGE_WARNING))
        }
        _ => Cow::Borrowed(prompt),
    }
}
```

The renderer (the same code path that substitutes `{ticker}` and `{analysis_emphasis}` into prompts at message-assembly time) calls `maybe_inject_leverage_warning` for the Conservative-risk, Neutral-risk, and Auditor system prompts whenever `state.derived_valuation.scenario` is `ScenarioValuation::Etf(_)`. For non-ETF runs the function is never invoked — the prompts substitute unchanged.

The leverage factor source is `EtfValuation.leverage_factor` (already populated by the Phase 1 `EtfPremiumDiscountValuator::assess` from `FundInfo.leverage_factor`). When the field is `None` or `1.0`, the warning is suppressed.

Trader, Aggressive-risk, Fund-manager, and the four analyst prompts do not receive the warning — the dual-risk audit contract only requires it on the explicitly listed slots.

### Report rendering — DEALER POSITIONING block

`crates/scorpio-reporters/src/terminal/etf.rs` extends `render_etf_panel` with a new sub-section. Layout when every Greek populates:

```
  ─── DEALER POSITIONING ──────────────────────────────────────────────
  Near-term  (2026-05-23)
    Net GEX/1%      +$2.84B    Gross GEX/1%    $7.12B
    Net VEX/volpt   -$1.20B    Gross VEX       $4.10B
    Net CEX/day     +$0.45B    Gross CEX       $2.30B
    Call/Put OI      1.31      Max-pain        $620
    Gamma walls    +$1.20B @ $625, -$0.84B @ $615, +$0.62B @ $630

  All expirations  (5 used)
    Net GEX/1%      +$8.40B    Gross GEX/1%    $22.1B
```

Per-line visibility rules:

- The entire block is hidden when `options_gex.is_none()`.
- The `Net VEX/volpt …` line is hidden when `vex_summary.is_none()`.
- The `Net CEX/day …` line is hidden when `cex_summary.is_none()`.
- The `Gamma walls …` line is hidden when `strikes.is_empty()`.
- The `All expirations (N used)` sub-block is hidden when `broad.is_none()`.
- When the entire block is hidden, the DATA AVAILABILITY section surfaces `⚠ Dealer positioning skipped — no options chain available`.

The Phase 1 reserved block (`─── DEALER GAMMA (near-term YYYY-MM-DD) ───` from the parent design's wide-terminal example) is replaced by the new `DEALER POSITIONING` layout. Phase 1 never actually emitted the reserved block (`options_gex` was always `None`), so this is the block's first real implementation — not a re-render of an existing layout.

Narrow-terminal fallback follows the same convention as Phase 1: fields stack vertically, plain ASCII labels, no decorative Unicode.

When runtime selection fell back to `risk_free_source = "fallback_const"`, the report header surfaces an additional warning line under the Analysis Pack row:

```
  Analysis Pack    ETF Baseline
  ⚠ Risk-free rate fallback — FRED DGS3MO unavailable; using 0.045 const
```

### Prompt updates — `etf_tracking_options_focus.md`

The Phase 1 file ships with placeholder language explicitly marking the options-chain branch as "deferred to Phase 2." Phase 2 replaces that section with concrete GEX/VEX/CEX guidance. Outline of the new section (the actual prompt content is finalized at implementation time):

> When `options_gex` is present in the ETF valuation snapshot, integrate dealer-positioning evidence into your assessment:
>
> - **Net GEX per 1%** — positive values indicate dealer-stabilizing hedging flow (dealers buy on dips, sell on rallies). Negative values indicate destabilizing flow (dealers amplify directional moves). Cite `net_gex_usd_per_1pct_move` and `broad.net_gex_usd_per_1pct_move` separately when both are present; large divergence between near-term and broad suggests roll/expiration concentration.
> - **Gamma walls** — `strikes[…].net_gex_usd_per_1pct_move` lists the top-3 strikes by absolute dealer gamma exposure. These act as magnet/repellent levels into expiration. Cite specific strike levels when relevant to the price action.
> - **Net VEX per vol-point** — negative VEX means rising IV amplifies dealer hedging pressure (vol-shock amplifier). Cite when IV regime is elevated or compressing into known catalysts.
> - **Net CEX per day** — sign indicates the direction of dealer charm flow as expiration approaches. Cite during OPEX week or near a clustered expiration.
> - **Call/Put OI ratio + max-pain** — supplementary positioning evidence; cite when the ratio is significantly above/below 1.0 or when max-pain sits visibly away from spot.
>
> When `options_gex` is absent (or any sub-field is `None`), explicitly state which dealer-positioning signal is unavailable; do not infer it from price action alone.

The manifest `etf_baseline_prompt_bundle` is unchanged — the same `compose_etf_section(technical_analyst.md, &[etf_tracking_options_focus.md])` call picks up the updated bytes.

## Failure modes & data availability

Additive to the Phase 1 failure-modes table:

| Condition                                                            | Detection                                                        | Behaviour                                                                                                                                                  |
|----------------------------------------------------------------------|------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------|
| FRED `DGS3MO` returns empty or errors                                | `Result<Option<f64>, TradingError>` is `Ok(None)` or `Err(_)`    | `state.etf_risk_free_rate = None`; valuator falls back to const `0.045`; preflight emits `routing.risk_free_source = "fallback_const"`; report header warns.|
| Options chain unavailable (any non-`Snapshot` outcome)               | `OptionsOutcome != Snapshot(_)`                                  | `valuation_inputs.etf_options = None`; `options_gex = None`; `flags.options_chain_present = false`; DATA AVAILABILITY shows `⚠ Dealer positioning skipped`. |
| `aggregate` produces no near-term aggregate                          | `AggregateResult.near_term.is_none()`                            | `compute_gex_summary` returns `None`; same downstream behavior as "chain unavailable".                                                                     |
| Per-strike IV is `None` on both call and put + no ATM fallback       | All three vol sources are missing                                | Row skipped (`strikes_total++`, `strikes_used` unchanged).                                                                                                 |
| Majority of strikes used the ATM fallback                            | `iv_fallback_count > strikes_used / 2`                           | `warn!` emitted with `iv_fallback_count` / `strikes_used`; GEX still emitted; no user-visible degradation.                                                 |
| `OptionsSnapshot.put_call_oi_ratio == 0.0`                           | Division guard                                                   | `call_put_oi_ratio` set to `0.0` with a `warn!` log.                                                                                                       |
| ETF has `leverage_factor != 1.0`                                     | `EtfValuation.leverage_factor`                                   | Renderer appends `etf_leverage_warning.md` to Conservative + Neutral + Auditor prompts.                                                                    |
| Legacy `transcript_cache.db` exists, new `data_cache.db` absent      | `DataCacheStore::from_config` filesystem probe                   | `std::fs::rename(legacy, new)`; on success `info!` log; on failure `warn!` log and open legacy path for reading.                                            |
| User TOML contains old `transcript_cache_db_path` field              | Deserialization                                                  | Field silently ignored (no `deny_unknown_fields`); resolved default `data_cache_db_path` is auto-written back to user's `config.toml`.                     |
| User TOML lacks `data_cache_db_path` field                           | `Config::resolve_data_cache_path` post-load                      | Resolved to `<dirname(snapshot_db_path)>/data_cache.db`; written back atomically via `NamedTempFile + rename`.                                              |
| Custom `transcript_cache_db_path` value in legacy TOML               | Deserialization (value dropped)                                  | User loses customization; auto-resolved default written. Acknowledged in out-of-scope.                                                                     |
| N-PORT cache row aged past TTL                                       | `cached_at < now - 30d`                                          | `get_nport` returns `None`; live fetch fires; new row written; old row overwritten in place (primary key `(cik, filing_date)`).                            |
| N-PORT cache read fails (SQLite error)                               | `cache.get_nport` returns `Err(_)`                               | `warn!` with `error.kind = "cache_read"`; falls back to live fetch.                                                                                        |
| N-PORT cache write fails (SQLite error)                              | `cache.put_nport` returns `Err(_)`                               | `warn!` with `error.kind = "cache_write"`; valuator proceeds with the live-fetched payload; next run retries the write.                                    |

## Test plan

| Layer                  | Location                                                                          | Coverage                                                                                                                                                                                                                                                  |
|------------------------|-----------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| BSM math               | `crates/scorpio-core/src/indicators/gex.rs#tests`                                 | `bsm_gamma`, `bsm_vanna`, `bsm_charm_call`, `bsm_charm_put` against analytical reference values within `1e-6`; degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`; ATM gamma > OTM gamma sanity case; Vanna sign matches call/put symmetry expectation. |
| Chain aggregation      | `crates/scorpio-core/src/indicators/gex.rs#tests`                                 | SqueezeMetrics sign convention (call OI +, put OI −) for GEX / VEX / CEX; IV-fallback path increments counter; empty `expirations` → both `near_term` and `broad` are `None`; mixed-IV multi-expiration broad aggregation sums correctly.                  |
| GEX summary wrapper    | `crates/scorpio-core/src/valuation/etf/gex.rs#tests`                              | `compute_gex_summary` returns `None` when near-term aggregate missing; gamma-wall sort/truncate to top-3 by `\|net_gex\|`; zero `put_call_oi_ratio` guard sets `call_put_oi_ratio = 0.0` with warning; field plumbing from snapshot to summary verified.   |
| Valuator integration   | `crates/scorpio-core/src/valuation/etf/premium_discount.rs#tests`                 | Phase-1 cases unchanged; new case where `etf_options.is_some()` populates `options_gex` with all four new fields; `etf_risk_free_rate` falls back to const when `None`; flags reflect chain presence.                                                     |
| OptionsSnapshot serde  | `crates/scorpio-core/src/data/traits/options.rs#tests`                            | `OptionsSnapshot` round-trips JSON with `all_expirations` populated *and* omitted (serde default empty vec); `ExpirationStrikes` schema generation works.                                                                                                  |
| TradingState serde     | `crates/scorpio-core/tests/state_roundtrip.rs` (extend)                           | `etf_risk_free_rate: Option<f64>` round-trips with `#[serde(default)]`; legacy snapshots without the field deserialize unchanged; `GexSummary` with the four Phase 2 fields populated round-trips; legacy snapshots with only Phase 1 fields work.       |
| Routing flags          | `crates/scorpio-core/tests/workflow_pipeline_structure.rs` (extend)               | Preflight fetches DGS3MO only when resolved pack is `EtfBaseline`; sets `routing.risk_free_source = "fred_dgs3mo"` on success, `"fallback_const"` on FRED failure; non-ETF runs skip the FRED call entirely.                                              |
| ETF input hydration    | `crates/scorpio-core/src/workflow/tasks/analyst.rs#tests` (extend)                | `OptionsOutcome::Snapshot` → `etf_options: Some(_)`; every other outcome → `None` + `warn!` log captured; `etf_risk_free_rate` is plumbed from state into valuation inputs.                                                                               |
| Cache rename module    | `crates/scorpio-core/src/data/data_cache/tests.rs` (new)                          | Transcript put/get verbatim regression (legacy behavior preserved post-move); N-PORT put/get round-trip; TTL-expired row returns `None`; legacy `transcript_cache.db` auto-renames to `data_cache.db` on first open in a tempdir; rename-failure fall-back. |
| Config self-heal       | `crates/scorpio-core/src/config.rs` (extend tests)                                | `Config::load_from_user_path` resolves missing `data_cache_db_path` to `<snapshot parent>/data_cache.db`; user file is rewritten when the heal fires; existing field is preserved when present; old `transcript_cache_db_path` is silently ignored.       |
| Leverage warning       | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` (extend)             | `maybe_inject_leverage_warning` returns borrowed Cow for `None` / `1.0`; owned Cow with suffix for `2.0` / `-1.0` / `3.0`; only Conservative / Neutral / Auditor receive injection; other ETF roles untouched.                                            |
| Tracking-prompt update | `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` (extend)             | Updated `etf_tracking_options_focus.md` still passes `validate_active_pack_completeness` for all four topology shapes; placeholder text no longer present; new GEX/VEX/CEX language present.                                                              |
| Report rendering       | `crates/scorpio-reporters/tests/terminal.rs` (extend)                             | DEALER POSITIONING block renders when `options_gex.is_some()`; per-line visibility rules hide individual lines for `None` sub-fields; risk-free-rate fallback banner renders when applicable; narrow-terminal fallback layout unchanged; equity reports unaffected. |
| Smoke (manual, FRED)   | `crates/scorpio-core/examples/fred_live_test.rs` (extend)                         | Add DGS3MO assertion alongside the existing FEDFUNDS / CPI cases; verify returned rate is in `(0.0, 0.20)` decimal range; runs only when `SCORPIO_FRED_API_KEY` is set.                                                                                    |
| Smoke (manual, yfin)   | `crates/scorpio-core/examples/yfinance_options_chain_live_test.rs` (new)          | `OptionsProvider::fetch_snapshot("SPY", today)` returns `Snapshot(_)`; `all_expirations.len() >= 2`; each expiration has a non-empty `strikes`; front-month entry matches `near_term_strikes`; bogus ticker → non-`Snapshot` outcome, no panic.           |
| Smoke (manual, cache)  | `crates/scorpio-core/examples/nport_cache_live_test.rs` (new)                     | Resolve SPY CIK → live N-PORT fetch → `put_nport` → `get_nport` within TTL returns same payload → forcibly age the row via UPDATE past 30d → `get_nport` returns `None` + refetch fires. Pre-create `transcript_cache.db` in a tempdir → open `DataCacheStore` → verify rename to `data_cache.db`. |
| Smoke (manual, e2e)    | `crates/scorpio-core/examples/etf_options_gex_live_test.rs` (new)                 | `AnalysisRuntime::run("SPY")` produces `ScenarioValuation::Etf(_)` with `options_gex: Some(g)`, `g.broad: Some(_)`, `g.vex_summary: Some(_)`, `g.cex_summary: Some(_)`, `g.strikes.len() == 3`; risk-free-rate flag = `"fred_dgs3mo"`; cache populated post-run.|

Live smokes are NOT in CI — invoke via `cargo run -p scorpio-core --example <name>` per the existing convention.

The smoke-coverage rule is a standing requirement: any new external fetch path added during Phase 2 implementation (or any later phase) gets its own `examples/*_live_test.rs` file. The spec lists the four expected smokes above; implementation may add more.

## Out of scope

- **iNAV / real-time NAV** — yfinance only provides end-of-prior-day NAV (carried from Phase 1).
- **Higher-order Greeks beyond Vanna and Charm** — Vomma, Speed, Color, Zomma, Veta are not emitted. Vanna and Charm are the tier practitioners cite alongside GEX; further-order Greeks rarely surface in dealer-flow analysis.
- **Per-strike Vanna / Charm exposure series** — only `GexSummary.strikes` (gamma walls) is emitted as a per-strike series. Net/gross aggregates are sufficient for prompt-level VEX/CEX reasoning; per-strike Vanna/Charm series would inflate state without a clear LLM use case yet.
- **Time-series GEX / historical dealer positioning** — `GexSummary` is a single point-in-time snapshot per run. No historical series, no longitudinal trend.
- **Backward-compat shim for the `transcript_cache_db_path` config field** — the field is hard-renamed to `data_cache_db_path`. Old TOML field is silently ignored at deserialization; new field is auto-resolved + auto-written by the config self-heal. No serde alias.
- **Value migration for custom `transcript_cache_db_path` paths** — power-users who customized the old field lose that customization on rename. They can manually set `data_cache_db_path` after the upgrade.
- **Configurable cache TTL** — `NPORT_CACHE_TTL_DAYS = 30` is a const; no env override, no `SCORPIO__DATA__NPORT_CACHE_TTL_DAYS` knob.
- **General-purpose cache table** — `nport_cache` is N-PORT-specific. No generic `cache_blobs` table; future cached payloads get their own typed migration.
- **Cache eviction beyond TTL-on-read** — there is no background sweep; `purge_expired_nport` exists for tests but is not invoked by the runtime. Rows age in place and are overwritten on the next live fetch for the same `(cik, filing_date)`.
- **Per-pack risk-free rate sources** — FRED `DGS3MO` is the single series. No DGS1MO / DGS6MO / DGS1 / per-expiration discounting. Acceptable because gamma is weakly sensitive to `r` for near-term expirations.
- **GEX in non-USD denominations** — output is always USD; ETFs traded in non-USD venues are out of scope.
- **Deterministic fund-manager veto on dealer-positioning extremes** — GEX/VEX/CEX stay LLM-visible evidence; no `gex_pinning_extreme` analogue to Phase 1's `tracking_failure` / `extreme_premium` / `leverage_decay` triggers.

## Open questions

None for the design itself. Implementation-time questions deferred to the writing-plans phase:

- Exact threshold for the "majority IV fallback" `warn!` log — `> strikes_used / 2` is the proposed cutoff but may be tightened or loosened once we see real chain-sparsity rates on common ETFs.
- Whether `BroadGex` should also carry `net_vex_usd_per_volpt` and `net_cex_usd_per_day` across all expirations — Phase 2 emits broad GEX only; broad VEX/CEX deferred unless prompt analysis shows the LLM citing them. Adding them later is additive (new `#[serde(default)]` fields on `BroadGex`).
- Whether `etf_options_gex_live_test.rs` should also assert a non-zero `vex_summary.net_vex_usd_per_volpt` — exact magnitude depends on the live chain at run time, so initial assertion checks `is_some()` only; magnitude thresholds may be added once we have a baseline.

## References

- [`2026-05-21-etf-baseline-pack-design.md`](./2026-05-21-etf-baseline-pack-design.md) — Phase 1 parent design; Phase 2 reuses every Phase 1 architectural decision verbatim.
- [`2026-04-28-shared-options-evidence-design.md`](./2026-04-28-shared-options-evidence-design.md) — `OptionsSnapshot` / `OptionsProvider` contract that Phase 2 extends with `all_expirations`.
- [`2026-04-20-fund-manager-dual-risk-escalation-design.md`](./2026-04-20-fund-manager-dual-risk-escalation-design.md) — dual-risk audit contract that Phase 2 leaves untouched (no new deterministic GEX trigger).
- [`2026-04-25-prompt-bundle-centralization-design.md`](./2026-04-25-prompt-bundle-centralization-design.md) — `PromptBundle` substitution path that Phase 2's leverage-warning injection hooks into.
- [`2026-05-16-transcript-local-cache-design.md`](./2026-05-16-transcript-local-cache-design.md) — `TranscriptCacheStore` design that Phase 2 rewrites into the unified `DataCacheStore`.
- `CLAUDE.md` — `Pack-owned prompts (centralized)`, `TradingState schema evolution`, error handling pattern, warning-log discipline.

## Phase 3 scope (deferred)

Items explicitly out of scope for Phase 2 that might constitute a hypothetical Phase 3 — listed here only to signal that the boundary was considered, not committed:

1. Broad VEX / broad CEX (aggregated across all expirations, not just front-month).
2. Per-strike Vanna / Charm series alongside the existing `GexSummary.strikes` gamma-wall series.
3. Time-series dealer positioning (longitudinal GEX trend across daily snapshots).
4. Higher-order Greeks (Vomma, Speed, Color, Zomma, Veta).
5. Configurable cache TTL + background eviction sweep.
6. Generic `cache_blobs` table consolidation if additional cached payloads emerge.
