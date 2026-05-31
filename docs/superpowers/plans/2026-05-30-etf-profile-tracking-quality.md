# ETF Profile Tracking Quality Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve ETF analysis quality by sourcing ETF profile/composition from Alpha Vantage `ETF_PROFILE`, showing official SEC benchmark names when available, and disabling misleading tracking-error computation until verified benchmark daily history exists.

**Architecture:** Add provider-specific ETF profile and SEC risk/return benchmark-name enrichment as typed data inputs, then merge them inside the ETF valuation path without mutating yfinance `FundInfo` into a provenance sink. Keep SEC N-PORT as the regulatory holdings fallback, but parse report date separately from filing date. Remove static ETF-to-benchmark symbol resolution and leave `EtfValuation.tracking` absent with an explicit `TrackingStatus` until a future trusted source resolves benchmark daily OHLCV.

**Tech Stack:** Rust 2024, `tokio`, `serde`/`schemars`, `chrono`, existing `AlphaVantageClient`, `SecEdgarClient`, `YFinanceData`, `EtfPremiumDiscountValuator`, `TradingState`, terminal reporters, and `cargo nextest`.

**Source Spec:** `docs/superpowers/specs/2026-05-28-etf-profile-tracking-quality-design.md`

---

## Scope And Constraints

- Preserve `TradingState` snapshot compatibility: every new field reachable from persisted state carries `#[serde(default)]` unless it is a new standalone type used only in new fields.
- Do not bump `THESIS_MEMORY_SCHEMA_VERSION` for additive struct fields. The previous ETF enum variant bump already moved the active snapshot schema to v4.
- Do not add a new manually curated benchmark-symbol table. `crates/scorpio-core/src/data/etf_benchmarks.rs` is deleted in this plan.
- Do not infer benchmark price tickers from official textual names. A textual benchmark name is display and prompt context only.
- Do not fetch benchmark OHLCV in the current scope. ETF OHLCV can still be fetched for price/technical context, but benchmark OHLCV is left `None`.
- Use local fixtures for Alpha Vantage `ETF_PROFILE`, N-PORT report-date parsing, and SEC risk/return TSV parsing.
- Use the existing mock seams: `YFinanceData` for yfinance return values, `EdgarHttp` for SEC request return values, pure parser tests for JSON/XML/TSV transformation.

---

## File Structure

- Modify: `crates/scorpio-core/src/state/derived.rs` — add composition source, benchmark source, tracking status, official benchmark fields, and Alpha Vantage profile metadata fields.
- Modify: `crates/scorpio-core/src/state/mod.rs` — existing `pub use derived::*` should already expose new state types; update only if implementation creates a separate state module.
- Modify: `crates/scorpio-core/tests/state_roundtrip.rs` — prove old ETF snapshots deserialize with additive defaults and new ETF snapshots round-trip.
- Modify: `crates/scorpio-core/src/data/alpha_vantage.rs` — add fail-soft `ETF_PROFILE` fetch, response parsing, provider diagnostics, and tests.
- Create: `crates/scorpio-core/tests/fixtures/alpha_vantage/soxx_etf_profile.json` — compact ETF profile fixture.
- Modify: `crates/scorpio-core/src/data/sec_edgar_nport.rs` — add `report_date` to `NPortHoldings`.
- Modify: `crates/scorpio-core/src/data/sec_edgar/nport.rs` — parse `repPdDate` / `repPdEnd`, normalize benchmark text, and test `N/A` handling.
- Create: `crates/scorpio-core/src/data/sec_risk_return.rs` — SEC DERA risk/return parser/resolver for official textual benchmark names.
- Modify: `crates/scorpio-core/src/data/sec_edgar/mod.rs` — expose class ID in `MfTickerEntry`, add risk/return client wiring or helper methods that use the existing SEC HTTP seam.
- Modify: `crates/scorpio-core/src/data/mod.rs` — export `sec_risk_return`; remove `etf_benchmarks`.
- Delete: `crates/scorpio-core/src/data/etf_benchmarks.rs` — remove static benchmark fallback.
- Modify: `crates/scorpio-core/src/data/yfinance/etf.rs` — update docs and keep `FundInfo.stated_benchmark` for upstream-provided values only.
- Modify: `crates/scorpio-core/src/valuation/mod.rs` — extend `ValuationInputs` with Alpha Vantage profile and official benchmark metadata; remove benchmark OHLCV carrier.
- Modify: `crates/scorpio-core/src/valuation/etf/premium_discount.rs` — merge Alpha Vantage composition first, N-PORT fallback second, official benchmark metadata, and tracking status.
- Modify: `crates/scorpio-core/src/valuation/etf/mod.rs` — remove `tracking_error` module export only if no internal tests still require it; otherwise keep the pure function unused by runtime.
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs` — fetch Alpha Vantage ETF profile, SEC DERA benchmark metadata, N-PORT fallback, and stop benchmark symbol/OHLCV resolution.
- Modify: `crates/scorpio-core/src/workflow/builder.rs`, `crates/scorpio-core/src/workflow/pipeline/runtime.rs`, `crates/scorpio-core/src/workflow/pipeline/mod.rs`, `crates/scorpio-core/src/app/mod.rs` — thread Alpha Vantage into `AnalystSyncTask` without violating preflight runtime-policy ownership.
- Modify: `crates/scorpio-core/src/agents/shared/valuation_prompt.rs` — render official benchmark and tracking-unavailable status.
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/*.md` — update ETF prompt guidance to remove deterministic `tracking_failure` and frame tracking as unavailable/reference-only.
- Modify: `crates/scorpio-reporters/src/terminal/etf.rs` — source-aware composition, official benchmark display, tracking-unavailable text, trust signals.
- Modify: `crates/scorpio-reporters/tests/terminal.rs` — reporter assertions for source labels, official benchmark, and unavailable tracking status.
- Modify: `crates/scorpio-reporters/src/json.rs`, `crates/scorpio-reporters/tests/json.rs` — bump JSON report schema only if JSON consumers need an explicit artifact version for the additive state fields. If treated as backward-compatible, leave v2 and add no-op rationale in the task notes.
- Modify: `docs/architecture/dependencies.md`, `docs/architecture/equity-analysis-pack.md`, `docs/architecture/config-and-errors.md` — document Alpha Vantage ETF profile, SEC risk/return source, and no benchmark-OHLCV behavior.

---

## Task 1: Add ETF State Metadata

**Files:**
- Modify: `crates/scorpio-core/src/state/derived.rs`
- Modify: `crates/scorpio-core/tests/state_roundtrip.rs`

- [x] **Step 1: Write failing state tests**

Add these tests near the ETF round-trip tests in `crates/scorpio-core/tests/state_roundtrip.rs`:

```rust
#[test]
fn legacy_etf_snapshot_without_profile_quality_fields_deserializes_with_defaults() {
    let json = r#"{
        "etf": {
            "premium": {
                "nav": 100.0,
                "market_price": 100.1,
                "bid": null,
                "ask": null,
                "premium_pct": 0.1,
                "category_band": "normal",
                "bid_ask_spread_pct": null,
                "as_of": "2026-05-28T12:00:00Z"
            },
            "composition": null,
            "tracking": null,
            "options_gex": null,
            "category": "Technology",
            "leverage_factor": 1.0,
            "flags": {}
        }
    }"#;

    let scenario: ScenarioValuation = serde_json::from_str(json).expect("legacy ETF scenario");
    let ScenarioValuation::Etf(etf) = scenario else {
        panic!("expected ETF scenario");
    };

    assert!(etf.official_benchmark_name.is_none());
    assert!(etf.official_benchmark_source.is_none());
    assert_eq!(etf.tracking_status, TrackingStatus::NotResolved);
}

#[test]
fn etf_composition_profile_quality_fields_roundtrip() {
    let comp = EtfComposition {
        source: EtfCompositionSource::AlphaVantageEtfProfile,
        top_holdings: vec![HoldingWeight {
            cusip: None,
            ticker: Some("NVDA".to_owned()),
            name: "NVIDIA Corp".to_owned(),
            weight_pct: 8.4,
            value_usd: None,
        }],
        top10_concentration_pct: 8.4,
        sector_weights: vec![SectorWeight {
            sector: "Semiconductors".to_owned(),
            weight_pct: 78.2,
        }],
        expense_ratio_pct: Some(0.0035),
        aum_usd: Some(12_300_000_000.0),
        fund_family: Some("iShares".to_owned()),
        distribution_yield_ttm_pct: Some(0.0061),
        holdings_filing_date: chrono::NaiveDate::from_ymd_opt(2026, 5, 28).unwrap(),
        holdings_report_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 30).unwrap()),
        holdings_age_days: 0,
        portfolio_turnover_pct: Some(0.24),
        inception_date: Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap()),
    };

    let json = serde_json::to_string(&comp).expect("serialize");
    let back: EtfComposition = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, comp);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p scorpio-core legacy_etf_snapshot_without_profile_quality_fields_deserializes_with_defaults etf_composition_profile_quality_fields_roundtrip --no-fail-fast`

Expected: FAIL with missing `EtfCompositionSource`, `TrackingStatus`, `official_benchmark_name`, `official_benchmark_source`, `holdings_report_date`, `portfolio_turnover_pct`, or `inception_date` symbols/fields.

- [x] **Step 3: Add state enums and fields**

Modify `crates/scorpio-core/src/state/derived.rs` by adding these enums near the existing ETF state types:

```rust
/// Provider that supplied ETF composition/profile rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EtfCompositionSource {
    #[default]
    SecNport,
    AlphaVantageEtfProfile,
}

/// Official textual benchmark-name source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkSource {
    SecRiskReturn,
    SecNport,
}

/// Status for ETF tracking-error computation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackingStatus {
    #[default]
    NotResolved,
    BenchmarkNameOnly,
    Computed,
}
```

Extend `EtfComposition` in the same file:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EtfComposition {
    #[serde(default)]
    pub source: EtfCompositionSource,
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
    #[serde(default)]
    pub holdings_report_date: Option<chrono::NaiveDate>,
    pub holdings_age_days: u32,
    #[serde(default)]
    pub portfolio_turnover_pct: Option<f64>,
    #[serde(default)]
    pub inception_date: Option<chrono::NaiveDate>,
}
```

Extend `EtfValuation` in the same file:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EtfValuation {
    pub premium: PremiumSnapshot,
    #[serde(default)]
    pub composition: Option<EtfComposition>,
    #[serde(default)]
    pub tracking: Option<TrackingError>,
    #[serde(default)]
    pub tracking_status: TrackingStatus,
    #[serde(default)]
    pub official_benchmark_name: Option<String>,
    #[serde(default)]
    pub official_benchmark_source: Option<BenchmarkSource>,
    #[serde(default)]
    pub official_benchmark_metadata_age_days: Option<u32>,
    #[serde(default)]
    pub options_gex: Option<GexSummary>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub leverage_factor: Option<f64>,
    #[serde(default)]
    pub flags: EtfDataAvailability,
}
```

Update every existing `EtfValuation { ... }` construction to set:

```rust
tracking_status: TrackingStatus::NotResolved,
official_benchmark_name: None,
official_benchmark_source: None,
official_benchmark_metadata_age_days: None,
```

Update every existing `EtfComposition { ... }` construction to set:

```rust
source: EtfCompositionSource::SecNport,
holdings_report_date: None,
portfolio_turnover_pct: None,
inception_date: None,
```

- [x] **Step 4: Run state tests to verify they pass**

Run: `cargo nextest run -p scorpio-core legacy_etf_snapshot_without_profile_quality_fields_deserializes_with_defaults etf_composition_profile_quality_fields_roundtrip trading_state_with_etf_variant_roundtrips --no-fail-fast`

Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/state/derived.rs crates/scorpio-core/tests/state_roundtrip.rs
git commit -m "feat(etf): add profile quality metadata to ETF state"
```

---

## Task 2: Add Alpha Vantage ETF Profile Parsing

**Files:**
- Modify: `crates/scorpio-core/src/data/alpha_vantage.rs`
- Create: `crates/scorpio-core/tests/fixtures/alpha_vantage/soxx_etf_profile.json`

- [x] **Step 1: Add the fixture**

Create `crates/scorpio-core/tests/fixtures/alpha_vantage/soxx_etf_profile.json`:

```json
{
  "net_assets": "12300000000",
  "net_expense_ratio": "0.0035",
  "portfolio_turnover": "0.24",
  "dividend_yield": "0.0061",
  "inception_date": "2001-07-10",
  "leveraged": "NO",
  "sectors": [
    { "sector": "Semiconductors", "weight": "0.782" },
    { "sector": "Technology Hardware", "weight": "0.104" }
  ],
  "holdings": [
    { "symbol": "NVDA", "description": "NVIDIA Corp", "weight": "0.084" },
    { "symbol": "AVGO", "description": "Broadcom Inc", "weight": "0.077" },
    { "symbol": "TSM", "description": "Taiwan Semiconductor Manufacturing", "weight": "n/a" }
  ]
}
```

- [x] **Step 2: Write failing parser tests**

Add this test module content inside the existing `#[cfg(test)] mod tests` in `crates/scorpio-core/src/data/alpha_vantage.rs`:

```rust
#[test]
fn parse_etf_profile_converts_decimal_weights_and_profile_fields() {
    let raw = include_str!("../../tests/fixtures/alpha_vantage/soxx_etf_profile.json");
    let profile = AlphaVantageClient::parse_etf_profile_response(raw).expect("parse profile");

    assert_eq!(profile.aum_usd, Some(12_300_000_000.0));
    assert_eq!(profile.expense_ratio_pct, Some(0.0035));
    assert_eq!(profile.portfolio_turnover_pct, Some(0.24));
    assert_eq!(profile.distribution_yield_pct, Some(0.0061));
    assert_eq!(
        profile.inception_date,
        Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap())
    );
    assert_eq!(profile.leverage_factor, Some(1.0));
    assert_eq!(profile.holdings[0].ticker.as_deref(), Some("NVDA"));
    assert!((profile.holdings[0].weight_pct - 8.4).abs() < 1e-9);
    assert_eq!(profile.holdings.len(), 2, "n/a holding weight is skipped");
    assert!((profile.sectors[0].weight_pct - 78.2).abs() < 1e-9);
}

#[test]
fn parse_etf_profile_classifies_provider_diagnostics_without_secret_text() {
    assert_eq!(
        AlphaVantageClient::parse_etf_profile_response(
            r#"{"Note":"Thank you. Standard call frequency is 5 calls per minute."}"#
        ),
        Ok(EtfProfileFetch::Throttled)
    );
    assert_eq!(
        AlphaVantageClient::parse_etf_profile_response(
            r#"{"Information":"This endpoint is not available under your current plan."}"#
        ),
        Ok(EtfProfileFetch::Unavailable)
    );
    let err = AlphaVantageClient::parse_etf_profile_response(
        r#"{"Error Message":"bad api key\nSECRET"}"#,
    )
    .expect_err("error message should be schema violation");
    assert!(!format!("{err}").contains('\n'));
}
```

- [x] **Step 3: Run tests to verify they fail**

Run: `cargo nextest run -p scorpio-core parse_etf_profile_converts_decimal_weights_and_profile_fields parse_etf_profile_classifies_provider_diagnostics_without_secret_text --no-fail-fast`

Expected: FAIL with missing `EtfProfileFetch`, `EtfProfileData`, or `parse_etf_profile_response`.

- [x] **Step 4: Implement ETF profile types and parser**

Add these imports near the top of `crates/scorpio-core/src/data/alpha_vantage.rs`:

```rust
use chrono::NaiveDate;
use crate::state::{HoldingWeight, SectorWeight};
```

Add these types near `AlphaVantageTranscriptResponse`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum EtfProfileFetch {
    Found(EtfProfileData),
    Throttled,
    Unavailable,
    NotAvailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EtfProfileData {
    pub holdings: Vec<HoldingWeight>,
    pub sectors: Vec<SectorWeight>,
    pub aum_usd: Option<f64>,
    pub expense_ratio_pct: Option<f64>,
    pub portfolio_turnover_pct: Option<f64>,
    pub distribution_yield_pct: Option<f64>,
    pub inception_date: Option<NaiveDate>,
    pub leverage_factor: Option<f64>,
}

#[derive(Deserialize)]
struct AlphaVantageEtfProfileResponse {
    net_assets: Option<String>,
    net_expense_ratio: Option<String>,
    portfolio_turnover: Option<String>,
    dividend_yield: Option<String>,
    inception_date: Option<String>,
    leveraged: Option<String>,
    #[serde(default)]
    sectors: Vec<AlphaVantageSectorRow>,
    #[serde(default)]
    holdings: Vec<AlphaVantageHoldingRow>,
    #[serde(rename = "Note")]
    note: Option<String>,
    #[serde(rename = "Information")]
    information: Option<String>,
    #[serde(rename = "Error Message")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct AlphaVantageSectorRow {
    sector: Option<String>,
    weight: Option<String>,
}

#[derive(Deserialize)]
struct AlphaVantageHoldingRow {
    symbol: Option<String>,
    description: Option<String>,
    weight: Option<String>,
}
```

Add these helpers in `impl AlphaVantageClient`:

```rust
fn parse_optional_f64(raw: Option<&str>) -> Option<f64> {
    let value = raw?.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("n/a") {
        return None;
    }
    value.replace(',', "").parse::<f64>().ok()
}

fn parse_optional_date(raw: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw?.trim(), "%Y-%m-%d").ok()
}

fn parse_decimal_weight_pct(raw: Option<&str>) -> Option<f64> {
    Self::parse_optional_f64(raw).map(|value| value * 100.0)
}

fn parse_leverage_factor(raw: Option<&str>) -> Option<f64> {
    match raw?.trim().to_ascii_uppercase().as_str() {
        "NO" => Some(1.0),
        "YES" => None,
        _ => None,
    }
}

pub(crate) fn parse_etf_profile_response(raw: &str) -> Result<EtfProfileFetch, TradingError> {
    let resp: AlphaVantageEtfProfileResponse = serde_json::from_str(raw).map_err(|e| {
        TradingError::SchemaViolation {
            message: format!("Alpha Vantage ETF_PROFILE response deserialization failed: {e}"),
        }
    })?;

    if let Some(msg) = &resp.error_message {
        return Err(TradingError::SchemaViolation {
            message: format!("Alpha Vantage ETF_PROFILE error: {}", Self::truncate_provider_msg(msg)),
        });
    }

    if let Some(body) = resp.note.as_deref().or(resp.information.as_deref()) {
        return Ok(match Self::classify_information(body) {
            TranscriptFetch::Throttled => EtfProfileFetch::Throttled,
            TranscriptFetch::Unavailable => EtfProfileFetch::Unavailable,
            TranscriptFetch::NotPublished | TranscriptFetch::Found(_) => EtfProfileFetch::NotAvailable,
        });
    }

    let holdings = resp
        .holdings
        .into_iter()
        .filter_map(|row| {
            let weight_pct = Self::parse_decimal_weight_pct(row.weight.as_deref())?;
            let name = row.description.or_else(|| row.symbol.clone())?;
            Some(HoldingWeight {
                cusip: None,
                ticker: row.symbol.filter(|s| !s.trim().is_empty()),
                name,
                weight_pct,
                value_usd: None,
            })
        })
        .collect();

    let sectors = resp
        .sectors
        .into_iter()
        .filter_map(|row| {
            Some(SectorWeight {
                sector: row.sector.filter(|s| !s.trim().is_empty())?,
                weight_pct: Self::parse_decimal_weight_pct(row.weight.as_deref())?,
            })
        })
        .collect();

    Ok(EtfProfileFetch::Found(EtfProfileData {
        holdings,
        sectors,
        aum_usd: Self::parse_optional_f64(resp.net_assets.as_deref()),
        expense_ratio_pct: Self::parse_optional_f64(resp.net_expense_ratio.as_deref()),
        portfolio_turnover_pct: Self::parse_optional_f64(resp.portfolio_turnover.as_deref()),
        distribution_yield_pct: Self::parse_optional_f64(resp.dividend_yield.as_deref()),
        inception_date: Self::parse_optional_date(resp.inception_date.as_deref()),
        leverage_factor: Self::parse_leverage_factor(resp.leveraged.as_deref()),
    }))
}
```

- [x] **Step 5: Add fail-soft fetch method**

Add this method to `impl AlphaVantageClient`:

```rust
fn build_etf_profile_url(&self) -> String {
    format!("{}?function=ETF_PROFILE", self.base_url)
}

pub async fn fetch_etf_profile(&self, symbol: &str) -> Result<EtfProfileFetch, TradingError> {
    validate_symbol(symbol)?;
    self.rate_limiter.acquire().await;

    let response = self
        .http
        .get(self.build_etf_profile_url())
        .query(&[("symbol", symbol), ("apikey", self.key.expose_secret())])
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.map_err(|e| {
                TradingError::Config(anyhow::anyhow!("Alpha Vantage ETF_PROFILE body read error: {e}"))
            })?;
            Self::parse_etf_profile_response(&body)
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
            Ok(EtfProfileFetch::Throttled)
        }
        Ok(resp)
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                || resp.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            self.escalate_auth_failure(resp.status());
            Ok(EtfProfileFetch::Unavailable)
        }
        Ok(resp) if resp.status().is_server_error() => Ok(EtfProfileFetch::Unavailable),
        Ok(resp) => Err(TradingError::Config(anyhow::anyhow!(
            "Alpha Vantage ETF_PROFILE HTTP error: {}",
            resp.status()
        ))),
        Err(e) if e.is_timeout() || e.is_connect() => Ok(EtfProfileFetch::Unavailable),
        Err(e) => Err(TradingError::Config(anyhow::anyhow!(
            "Alpha Vantage ETF_PROFILE request error: {}",
            e.without_url()
        ))),
    }
}
```

- [x] **Step 6: Run Alpha Vantage tests**

Run: `cargo nextest run -p scorpio-core parse_etf_profile_converts_decimal_weights_and_profile_fields parse_etf_profile_classifies_provider_diagnostics_without_secret_text --no-fail-fast`

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/scorpio-core/src/data/alpha_vantage.rs crates/scorpio-core/tests/fixtures/alpha_vantage/soxx_etf_profile.json
git commit -m "feat(data): parse Alpha Vantage ETF profiles"
```

---

## Task 3: Fix N-PORT Report Date And Benchmark Normalization

**Files:**
- Modify: `crates/scorpio-core/src/data/sec_edgar_nport.rs`
- Modify: `crates/scorpio-core/src/data/sec_edgar/nport.rs`
- Modify: `crates/scorpio-core/src/valuation/etf/premium_discount.rs`

- [x] **Step 1: Write failing N-PORT tests**

Add these tests to `crates/scorpio-core/src/data/sec_edgar/nport.rs`:

```rust
#[test]
fn parse_nport_p_extracts_report_date_from_rep_pd_date() {
    let xml = r#"
    <edgarSubmission>
      <formData><genInfo><repPdDate>2026-03-31</repPdDate></genInfo></formData>
      <invstOrSec><name>Apple Inc</name><pctVal>5.0</pctVal><issuerType>Technology</issuerType></invstOrSec>
    </edgarSubmission>
    "#;
    let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 5, 28).unwrap())
        .expect("fixture should parse");
    assert_eq!(
        result.report_date,
        Some(NaiveDate::from_ymd_opt(2026, 3, 31).unwrap())
    );
}

#[test]
fn parse_nport_p_ignores_na_designated_index_fields() {
    let xml = r#"
    <edgarSubmission>
      <formData><genInfo><repPdEnd>2026-03-31</repPdEnd></genInfo></formData>
      <nameDesignatedIndex>N/A</nameDesignatedIndex>
      <indexIdentifier>None</indexIdentifier>
      <invstOrSec><name>Apple Inc</name><pctVal>5.0</pctVal><issuerType>Technology</issuerType></invstOrSec>
    </edgarSubmission>
    "#;
    let result = parse_nport_p(xml, NaiveDate::from_ymd_opt(2026, 5, 28).unwrap())
        .expect("fixture should parse");
    assert!(result.stated_benchmark.is_none());
}
```

Add this valuation test to the existing tests in `crates/scorpio-core/src/valuation/etf/premium_discount.rs`:

```rust
#[test]
fn build_composition_uses_report_date_for_age_when_present() {
    let mut flags = EtfDataAvailability::default();
    let today = chrono::Utc::now().date_naive();
    let nport = NPortHoldings {
        filing_date: today,
        report_date: Some(today - chrono::Duration::days(70)),
        holdings: vec![crate::data::sec_edgar_nport::NPortHoldingRow {
            cusip: None,
            ticker: Some("AAPL".to_owned()),
            name: "Apple Inc".to_owned(),
            weight_pct: 5.0,
            value_usd: None,
        }],
        sector_breakdown: vec![],
        stated_benchmark: None,
    };

    let comp = build_composition(&nport, None, &mut flags).expect("composition");
    assert_eq!(comp.holdings_report_date, nport.report_date);
    assert!(comp.holdings_age_days >= 70);
    assert_eq!(flags.holdings_age_band, HoldingsAgeBand::Aging);
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p scorpio-core parse_nport_p_extracts_report_date_from_rep_pd_date parse_nport_p_ignores_na_designated_index_fields build_composition_uses_report_date_for_age_when_present --no-fail-fast`

Expected: FAIL because `NPortHoldings.report_date` is missing and `build_composition` uses filing date only.

- [x] **Step 3: Add `report_date` to N-PORT state**

Modify `crates/scorpio-core/src/data/sec_edgar_nport.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortHoldings {
    pub filing_date: NaiveDate,
    #[serde(default)]
    pub report_date: Option<NaiveDate>,
    pub holdings: Vec<NPortHoldingRow>,
    pub sector_breakdown: Vec<NPortSectorRow>,
    pub stated_benchmark: Option<String>,
}
```

Update every existing `NPortHoldings { ... }` construction to include `report_date: None` unless a test needs a real report date.

- [x] **Step 4: Parse report date and normalize benchmark values**

Modify `crates/scorpio-core/src/data/sec_edgar/nport.rs` so `parse_nport_p` tracks report date:

```rust
let mut report_date: Option<NaiveDate> = None;
```

Inside the `Event::End` branch, add:

```rust
if name == b"repPdDate" || name == b"repPdEnd" {
    let txt = String::from_utf8_lossy(&current_text).trim().to_owned();
    report_date = NaiveDate::parse_from_str(&txt, "%Y-%m-%d").ok();
}

if name == b"benchmarkName" || name == b"indxName" {
    let txt = String::from_utf8_lossy(&current_text).trim().to_owned();
    if let Some(normalized) = normalize_optional_benchmark(&txt) {
        stated_benchmark = Some(normalized);
    }
}
```

Add this helper near `fill_field`:

```rust
pub(crate) fn normalize_optional_benchmark(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "n/a" | "na" | "none" | "null" => None,
        _ => Some(trimmed.to_owned()),
    }
}
```

Include `report_date` in the returned `NPortHoldings`:

```rust
Some(NPortHoldings {
    filing_date,
    report_date,
    holdings,
    sector_breakdown,
    stated_benchmark,
})
```

- [x] **Step 5: Use report date in composition age**

Modify `build_composition` in `crates/scorpio-core/src/valuation/etf/premium_discount.rs`:

```rust
let age_anchor = nport.report_date.unwrap_or(nport.filing_date);
let age_days = (today - age_anchor).num_days().max(0) as u32;
```

Set these fields in `EtfComposition`:

```rust
source: EtfCompositionSource::SecNport,
holdings_report_date: nport.report_date,
portfolio_turnover_pct: None,
inception_date: None,
```

- [x] **Step 6: Run N-PORT and composition tests**

Run: `cargo nextest run -p scorpio-core parse_nport_p_extracts_report_date_from_rep_pd_date parse_nport_p_ignores_na_designated_index_fields build_composition_uses_report_date_for_age_when_present --no-fail-fast`

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/scorpio-core/src/data/sec_edgar_nport.rs crates/scorpio-core/src/data/sec_edgar/nport.rs crates/scorpio-core/src/valuation/etf/premium_discount.rs
git commit -m "fix(etf): use N-PORT report dates for holdings age"
```

---

## Task 4: Add SEC Risk/Return Benchmark Resolver

**Files:**
- Create: `crates/scorpio-core/src/data/sec_risk_return.rs`
- Modify: `crates/scorpio-core/src/data/sec_edgar/mod.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs`
- Create: `crates/scorpio-core/tests/fixtures/sec_risk_return/soxx_rr.tsv`

- [x] **Step 1: Create SOXX SEC risk/return fixture**

Create `crates/scorpio-core/tests/fixtures/sec_risk_return/soxx_rr.tsv`:

```text
adsh	cik	series_id	class_id	filed	period	doc	stmt	line	tag	value
0001193125-25-162603	1100663	S000004354	C000012084	2025-07-18	2025-06-30	soxx-485bpos.htm	RR	1	StrategyNarrativeTextBlock	The Fund seeks to track the investment results of the NYSE Semiconductor Index, which measures the performance of U.S.-listed semiconductor equities.
0001193125-25-162603	1100663	S000004354	C000012084	2025-07-18	2025-06-30	soxx-485bpos.htm	RR	2	ObjectivePrimaryTextBlock	The Fund seeks investment results that correspond generally to the price and yield performance of the NYSE Semiconductor Index.
0001193125-25-162603	1100663	S000004354	C000012084	2025-07-18	2025-06-30	soxx-485bpos.htm	RR	3	AvgAnnlRtrPct	NYSESemiconductorIndex
```

- [x] **Step 2: Write failing parser tests**

Create `crates/scorpio-core/src/data/sec_risk_return.rs` with these tests first:

```rust
use chrono::NaiveDate;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_soxx_benchmark_from_strategy_text() {
        let raw = include_str!("../../tests/fixtures/sec_risk_return/soxx_rr.tsv");
        let benchmark = parse_risk_return_tsv_for_benchmark(
            raw,
            RiskReturnLookup {
                series_id: "S000004354",
                class_id: "C000012084",
            },
            "2025q3",
        )
        .expect("benchmark");

        assert_eq!(benchmark.name, "NYSE Semiconductor Index");
        assert_eq!(benchmark.source, crate::state::BenchmarkSource::SecRiskReturn);
        assert_eq!(benchmark.dataset_quarter, "2025q3");
        assert_eq!(benchmark.accession.as_deref(), Some("0001193125-25-162603"));
        assert_eq!(
            benchmark.filing_date,
            Some(NaiveDate::from_ymd_opt(2025, 7, 18).unwrap())
        );
    }

    #[test]
    fn returns_none_when_series_class_do_not_match() {
        let raw = include_str!("../../tests/fixtures/sec_risk_return/soxx_rr.tsv");
        let benchmark = parse_risk_return_tsv_for_benchmark(
            raw,
            RiskReturnLookup {
                series_id: "S000000000",
                class_id: "C000000000",
            },
            "2025q3",
        );
        assert!(benchmark.is_none());
    }
}
```

- [x] **Step 3: Run tests to verify they fail**

Run: `cargo nextest run -p scorpio-core extracts_soxx_benchmark_from_strategy_text returns_none_when_series_class_do_not_match --no-fail-fast`

Expected: FAIL with missing `RiskReturnLookup`, `parse_risk_return_tsv_for_benchmark`, and `BenchmarkMetadata` implementation.

- [x] **Step 4: Implement parser and metadata type**

Add this implementation above the tests in `crates/scorpio-core/src/data/sec_risk_return.rs`:

```rust
use chrono::NaiveDate;

use crate::state::BenchmarkSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskReturnLookup<'a> {
    pub series_id: &'a str,
    pub class_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkMetadata {
    pub name: String,
    pub source: BenchmarkSource,
    pub dataset_quarter: String,
    pub accession: Option<String>,
    pub filing_date: Option<NaiveDate>,
    pub source_period: Option<NaiveDate>,
}

pub fn parse_risk_return_tsv_for_benchmark(
    raw: &str,
    lookup: RiskReturnLookup<'_>,
    dataset_quarter: &str,
) -> Option<BenchmarkMetadata> {
    let mut lines = raw.lines();
    let header = lines.next()?;
    let columns: Vec<&str> = header.split('\t').collect();
    let idx = |name: &str| columns.iter().position(|column| *column == name);
    let adsh_idx = idx("adsh")?;
    let series_idx = idx("series_id")?;
    let class_idx = idx("class_id")?;
    let filed_idx = idx("filed")?;
    let period_idx = idx("period")?;
    let tag_idx = idx("tag")?;
    let value_idx = idx("value")?;

    lines.filter_map(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.get(series_idx)? != &lookup.series_id || fields.get(class_idx)? != &lookup.class_id {
            return None;
        }

        let tag = *fields.get(tag_idx)?;
        if tag != "StrategyNarrativeTextBlock" && tag != "ObjectivePrimaryTextBlock" {
            return None;
        }

        let name = extract_index_name(fields.get(value_idx)?)?;
        Some(BenchmarkMetadata {
            name,
            source: BenchmarkSource::SecRiskReturn,
            dataset_quarter: dataset_quarter.to_owned(),
            accession: fields.get(adsh_idx).map(|value| (*value).to_owned()),
            filing_date: fields
                .get(filed_idx)
                .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()),
            source_period: fields
                .get(period_idx)
                .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()),
        })
    }).next()
}

// `extract_index_name` over the StrategyNarrativeTextBlock / ObjectivePrimaryTextBlock
// narrative is the AUTHORITATIVE source of the benchmark's spaced name; the structured
// `AvgAnnlRtrPct` index-member token only CORROBORATES it (per spec — the structured row
// carries an unspaced token like `NYSESemiconductorIndex`, not the spaced display name).
// This scan is best-effort: it commits to the first `" index"` occurrence and can
// mis-extract on phrasings like "uses an index sampling strategy to track the CRSP US
// Total Market Index", so treat a low-confidence extraction as `None`. Do NOT special-case
// any single fund's index name; the SOXX fixture exercises this generic path ("track the
// NYSE Semiconductor Index" resolves correctly without a hardcoded marker).
fn extract_index_name(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let suffix = " index";
    let end = lower.find(suffix)? + suffix.len();
    let prefix_start = lower[..end]
        .rfind("the ")
        .map(|pos| pos + "the ".len())
        .unwrap_or(0);
    let candidate = text[prefix_start..end].trim_matches(|c: char| c == ',' || c == '.').trim();
    if candidate.len() >= "S&P 500 Index".len() {
        Some(candidate.to_owned())
    } else {
        None
    }
}
```

`parse_risk_return_tsv_for_benchmark` resolves the benchmark name from `extract_index_name` over the narrative text (the authoritative spaced name) and uses the structured `AvgAnnlRtrPct` index-member token only to corroborate — matching the spec's evidence hierarchy. (The structured token is unspaced, e.g. `NYSESemiconductorIndex`, so it cannot itself satisfy the spaced-name assertion in the Step 2 test.) Add a direct unit test calling `extract_index_name` with non-SOXX inline strategy strings — a well-formed `"...track the <Name> Index..."` case and an ambiguous one — asserting it returns the spaced name or `None`, never a mangled fragment. Inline `&str` inputs need no fixture file.

- [x] **Step 5: Preserve class ID from SEC MF ticker map**

Modify `MfTickerEntry` in `crates/scorpio-core/src/data/sec_edgar/mod.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MfTickerEntry {
    pub cik: u32,
    pub series_id: String,
    pub class_id: String,
}
```

Modify `parse_company_tickers_mf` mapping:

```rust
.map(|(cik, series_id, class_id, symbol)| {
    (
        symbol.to_uppercase(),
        MfTickerEntry {
            cik,
            series_id,
            class_id,
        },
    )
})
```

Update existing tests that construct `MfTickerEntry` so they include `class_id: "C000012084".to_owned()` or the matching fixture value.

In `parse_company_tickers_mf`, extend the existing schema-position guard (which already asserts `fields[0] == "cik"`, `fields[1] == "seriesId"`, and `fields[3] == "symbol"`) to also assert `fields.get(2).map(String::as_str) == Some("classId")`. `class_id` is now load-bearing for the risk/return lookup, so a silent SEC column reorder must fail loudly here rather than mismatching every benchmark downstream.

- [x] **Step 6: Export the module**

Modify `crates/scorpio-core/src/data/mod.rs`:

```rust
pub mod sec_risk_return;
```

- [x] **Step 7: Run SEC parser and MF ticker tests**

Run: `cargo nextest run -p scorpio-core extracts_soxx_benchmark_from_strategy_text returns_none_when_series_class_do_not_match parse_company_tickers_mf_extracts_etf_to_entry_map --no-fail-fast`

Expected: PASS.

- [x] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/data/sec_risk_return.rs crates/scorpio-core/src/data/sec_edgar/mod.rs crates/scorpio-core/src/data/mod.rs crates/scorpio-core/tests/fixtures/sec_risk_return/soxx_rr.tsv
git commit -m "feat(data): extract official ETF benchmarks from SEC risk return data"
```

---

## Task 5: Remove Static Benchmark Resolution And Benchmark OHLCV Fetches

**Files:**
- Delete: `crates/scorpio-core/src/data/etf_benchmarks.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs`
- Modify: `crates/scorpio-core/src/data/yfinance/etf.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`
- Modify: `crates/scorpio-core/src/valuation/mod.rs`
- Modify: `crates/scorpio-core/examples/etf_live_test.rs`

- [x] **Step 1: Write failing workflow tests**

Replace the current static lookup tests in `crates/scorpio-core/src/workflow/tasks/analyst.rs` with:

```rust
#[test]
fn benchmark_name_resolution_does_not_fall_back_to_static_symbol_lookup() {
    let fund_info = FundInfo {
        symbol: "SOXX".into(),
        category: None,
        fund_family: None,
        expense_ratio: None,
        total_assets: None,
        leverage_factor: Some(1.0),
        fund_kind: Some("etf".into()),
        stated_benchmark: None,
    };
    let nport = NPortHoldings {
        filing_date: NaiveDate::from_ymd_opt(2026, 5, 28).unwrap(),
        report_date: Some(NaiveDate::from_ymd_opt(2026, 3, 31).unwrap()),
        holdings: vec![],
        sector_breakdown: vec![],
        stated_benchmark: None,
    };

    assert!(super::resolve_official_benchmark_name(None, Some(&nport)).is_none());
    assert!(fund_info.stated_benchmark.is_none());
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p scorpio-core benchmark_name_resolution_does_not_fall_back_to_static_symbol_lookup --no-fail-fast`

Expected: FAIL because static fallback still resolves SOXX to `^SOX`.

- [x] **Step 3: Remove static module export and file**

Modify `crates/scorpio-core/src/data/mod.rs` by deleting:

```rust
pub mod etf_benchmarks;
```

Delete `crates/scorpio-core/src/data/etf_benchmarks.rs`.

Modify `crates/scorpio-core/src/data/yfinance/etf.rs` module docs by replacing the static lookup paragraph with:

```rust
//! Stated benchmark names are not resolved to market-data symbols in this
//! module. Official textual benchmark metadata is carried separately by the ETF
//! valuation path; benchmark OHLCV is intentionally disabled until a trusted
//! source can resolve daily benchmark history.
```

- [x] **Step 4: Replace benchmark symbol resolver with textual resolver**

In `crates/scorpio-core/src/workflow/tasks/analyst.rs`, replace `resolve_benchmark_symbol` with:

```rust
// Reuse the shared helper rather than redefining identical logic; Task 3 exposes
// `normalize_optional_benchmark` as `pub(crate)` in `sec_edgar/nport.rs` (ensure
// the `nport` module is at least `pub(crate)` so this path resolves).
use crate::data::sec_edgar::nport::normalize_optional_benchmark;

fn resolve_official_benchmark_name(
    risk_return: Option<&crate::data::sec_risk_return::BenchmarkMetadata>,
    nport: Option<&NPortHoldings>,
) -> Option<(String, crate::state::BenchmarkSource, Option<u32>)> {
    if let Some(metadata) = risk_return {
        return Some((metadata.name.clone(), metadata.source, benchmark_metadata_age_days(metadata)));
    }
    nport
        .and_then(|holdings| holdings.stated_benchmark.as_deref())
        .and_then(normalize_optional_benchmark)
        .map(|name| (name, crate::state::BenchmarkSource::SecNport, None))
}

fn benchmark_metadata_age_days(
    metadata: &crate::data::sec_risk_return::BenchmarkMetadata,
) -> Option<u32> {
    let filing_date = metadata.filing_date?;
    Some((chrono::Utc::now().date_naive() - filing_date).num_days().max(0) as u32)
}
```

Remove calls that write fallback symbols into `FundInfo.stated_benchmark` before valuation.

- [x] **Step 5: Stop benchmark OHLCV fetches**

In `fetch_valuation_inputs`, delete the block that calls `fetch_ohlcv_1y(yfinance, &bench)` for `etf_benchmark_ohlcv`, and remove the `etf_benchmark_ohlcv` field from the local `ValuationInputs` struct in this file. Step 6 removes the matching field from the crate-level `crate::valuation::ValuationInputs` and updates its remaining references.

- [x] **Step 6: Remove benchmark OHLCV from valuation carrier**

Remove `etf_benchmark_ohlcv` from every remaining site. Delete the field from `crate::valuation::ValuationInputs` in `crates/scorpio-core/src/valuation/mod.rs`:

```rust
pub etf_benchmark_ohlcv: Option<&'a [crate::data::yfinance::Candle]>,
```

Then update the `derive_runtime_valuation` construction so it no longer supplies `etf_benchmark_ohlcv`; update the crate-level `ValuationInputs` construction in `crates/scorpio-core/src/valuation/equity/default.rs` (the `etf_benchmark_ohlcv: None` line) so it no longer sets the removed field; delete the now-dead `use super::tracking_error::compute_tracking_error;` import in `premium_discount.rs` once its call site is gone; and drop `etf_benchmark_ohlcv: None` from the `empty_inputs()` test helper in `premium_discount.rs` so the existing valuator tests still compile. Leaving any of these behind trips the repo's `-D warnings` gate (unused import) or fails to compile (`no field named etf_benchmark_ohlcv`).

- [x] **Step 7: Update example**

Modify `crates/scorpio-core/examples/etf_live_test.rs` so it no longer imports or prints `etf_benchmarks`. Replace benchmark output with official benchmark fields from `EtfValuation` when present.

- [x] **Step 8: Run no-static-fallback tests**

Run: `cargo nextest run -p scorpio-core benchmark_name_resolution_does_not_fall_back_to_static_symbol_lookup --no-fail-fast`

Expected: PASS.

- [x] **Step 9: Commit**

```bash
git add crates/scorpio-core/src/data/mod.rs crates/scorpio-core/src/data/yfinance/etf.rs crates/scorpio-core/src/workflow/tasks/analyst.rs crates/scorpio-core/src/valuation/mod.rs crates/scorpio-core/examples/etf_live_test.rs
git rm crates/scorpio-core/src/data/etf_benchmarks.rs
git commit -m "fix(etf): remove static benchmark symbol fallback"
```

---

## Task 6: Merge ETF Profile, SEC Benchmark Metadata, And Tracking Status In Valuation

**Files:**
- Modify: `crates/scorpio-core/src/valuation/mod.rs`
- Modify: `crates/scorpio-core/src/valuation/etf/premium_discount.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`
- Modify: `crates/scorpio-core/src/workflow/builder.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs`
- Modify: `crates/scorpio-core/src/workflow/pipeline/mod.rs`
- Modify: `crates/scorpio-core/src/app/mod.rs`

- [x] **Step 1: Write failing valuator tests**

Add tests to `crates/scorpio-core/src/valuation/etf/premium_discount.rs`:

```rust
#[test]
fn etf_valuator_prefers_alpha_vantage_profile_composition() {
    let quote = test_quote();
    let av = crate::data::alpha_vantage::EtfProfileData {
        holdings: vec![HoldingWeight {
            cusip: None,
            ticker: Some("NVDA".to_owned()),
            name: "NVIDIA Corp".to_owned(),
            weight_pct: 8.4,
            value_usd: None,
        }],
        sectors: vec![SectorWeight {
            sector: "Semiconductors".to_owned(),
            weight_pct: 78.2,
        }],
        aum_usd: Some(12_300_000_000.0),
        expense_ratio_pct: Some(0.0035),
        portfolio_turnover_pct: Some(0.24),
        distribution_yield_pct: Some(0.0061),
        inception_date: Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap()),
        leverage_factor: Some(1.0),
    };

    let report = EtfPremiumDiscountValuator.assess(
        crate::valuation::ValuationInputs {
            profile: None,
            cashflow: None,
            balance: None,
            income: None,
            shares: None,
            earnings_trend: None,
            current_price: Some(100.0),
            etf_quote: Some(&quote),
            etf_fund_info: None,
            etf_holdings: None,
            etf_profile: Some(&av),
            etf_official_benchmark: None,
            etf_ohlcv: None,
            etf_options: None,
            etf_risk_free_rate: None,
            etf_distribution_yield_ttm: None,
            as_of: chrono::Utc::now().date_naive(),
        },
        &AssetShape::Fund,
    );

    let ScenarioValuation::Etf(etf) = report.scenario else {
        panic!("expected ETF valuation");
    };
    let comp = etf.composition.expect("composition");
    assert_eq!(comp.source, EtfCompositionSource::AlphaVantageEtfProfile);
    assert_eq!(comp.holdings_report_date, None);
    assert_eq!(comp.portfolio_turnover_pct, Some(0.24));
    assert_eq!(etf.tracking, None);
    assert_eq!(etf.tracking_status, TrackingStatus::NotResolved);
}

#[test]
fn etf_valuator_renders_benchmark_name_only_status_when_official_name_exists() {
    let quote = test_quote();
    let benchmark = crate::data::sec_risk_return::BenchmarkMetadata {
        name: "NYSE Semiconductor Index".to_owned(),
        source: BenchmarkSource::SecRiskReturn,
        dataset_quarter: "2025q3".to_owned(),
        accession: Some("0001193125-25-162603".to_owned()),
        filing_date: Some(chrono::NaiveDate::from_ymd_opt(2025, 7, 18).unwrap()),
        source_period: Some(chrono::NaiveDate::from_ymd_opt(2025, 6, 30).unwrap()),
    };

    let report = EtfPremiumDiscountValuator.assess(
        crate::valuation::ValuationInputs {
            profile: None,
            cashflow: None,
            balance: None,
            income: None,
            shares: None,
            earnings_trend: None,
            current_price: Some(100.0),
            etf_quote: Some(&quote),
            etf_fund_info: None,
            etf_holdings: None,
            etf_profile: None,
            etf_official_benchmark: Some((&benchmark, Some(100))),
            etf_ohlcv: None,
            etf_options: None,
            etf_risk_free_rate: None,
            etf_distribution_yield_ttm: None,
            as_of: chrono::Utc::now().date_naive(),
        },
        &AssetShape::Fund,
    );

    let ScenarioValuation::Etf(etf) = report.scenario else {
        panic!("expected ETF valuation");
    };
    assert_eq!(etf.official_benchmark_name.as_deref(), Some("NYSE Semiconductor Index"));
    assert_eq!(etf.official_benchmark_source, Some(BenchmarkSource::SecRiskReturn));
    assert_eq!(etf.official_benchmark_metadata_age_days, Some(100));
    assert_eq!(etf.tracking_status, TrackingStatus::BenchmarkNameOnly);
    assert!(etf.tracking.is_none());
}
```

- [x] **Step 2: Run valuator tests to verify they fail**

Run: `cargo nextest run -p scorpio-core etf_valuator_prefers_alpha_vantage_profile_composition etf_valuator_renders_benchmark_name_only_status_when_official_name_exists --no-fail-fast`

Expected: FAIL because valuation inputs and merge logic do not yet carry Alpha Vantage profile or official benchmark metadata.

- [x] **Step 3: Extend valuation carrier**

Modify `crates/scorpio-core/src/valuation/mod.rs`:

```rust
pub etf_profile: Option<&'a crate::data::alpha_vantage::EtfProfileData>,
pub etf_official_benchmark:
    Option<(&'a crate::data::sec_risk_return::BenchmarkMetadata, Option<u32>)>,
```

- [x] **Step 4: Implement Alpha Vantage composition builder**

Add this helper in `crates/scorpio-core/src/valuation/etf/premium_discount.rs`:

```rust
fn build_alpha_vantage_composition(
    profile: &crate::data::alpha_vantage::EtfProfileData,
    fund_info: Option<&FundInfo>,
    flags: &mut EtfDataAvailability,
) -> Option<EtfComposition> {
    if profile.holdings.is_empty() && profile.sectors.is_empty() {
        return None;
    }
    flags.holdings_present = !profile.holdings.is_empty();
    flags.holdings_age_band = HoldingsAgeBand::Unknown;

    let mut top_holdings = profile.holdings.clone();
    top_holdings.sort_by(|a, b| {
        b.weight_pct
            .partial_cmp(&a.weight_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_holdings.truncate(10);
    let top10_concentration_pct = top_holdings.iter().map(|h| h.weight_pct).sum();
    let today = chrono::Utc::now().date_naive();

    Some(EtfComposition {
        source: EtfCompositionSource::AlphaVantageEtfProfile,
        top_holdings,
        top10_concentration_pct,
        sector_weights: profile.sectors.clone(),
        expense_ratio_pct: profile.expense_ratio_pct.or_else(|| fund_info.and_then(|f| f.expense_ratio)),
        aum_usd: profile.aum_usd.or_else(|| fund_info.and_then(|f| f.total_assets)),
        fund_family: fund_info.and_then(|f| f.fund_family.clone()),
        distribution_yield_ttm_pct: profile.distribution_yield_pct,
        holdings_filing_date: today,
        holdings_report_date: None,
        holdings_age_days: 0,
        portfolio_turnover_pct: profile.portfolio_turnover_pct,
        inception_date: profile.inception_date,
    })
}
```

- [x] **Step 5: Merge profile and benchmark metadata in valuator**

Modify `EtfPremiumDiscountValuator::assess` composition selection:

```rust
let composition = inputs
    .etf_profile
    .and_then(|profile| build_alpha_vantage_composition(profile, inputs.etf_fund_info, &mut flags))
    .or_else(|| {
        inputs
            .etf_holdings
            .and_then(|h| build_composition(h, inputs.etf_fund_info, &mut flags))
    });
```

Remove the runtime tracking computation block from `assess` and set:

```rust
let (official_benchmark_name, official_benchmark_source, official_benchmark_metadata_age_days) =
    inputs
        .etf_official_benchmark
        .map(|(metadata, age_days)| {
            (
                Some(metadata.name.clone()),
                Some(metadata.source),
                age_days,
            )
        })
        .unwrap_or((None, None, None));
let tracking_status = if official_benchmark_name.is_some() {
    TrackingStatus::BenchmarkNameOnly
} else {
    TrackingStatus::NotResolved
};
let tracking = None;
```

Populate the new `EtfValuation` fields:

```rust
tracking,
tracking_status,
official_benchmark_name,
official_benchmark_source,
official_benchmark_metadata_age_days,
```

- [x] **Step 6: Thread Alpha Vantage into AnalystSyncTask**

Modify `AnalystSyncTask` in `crates/scorpio-core/src/workflow/tasks/analyst.rs` to add:

```rust
alpha_vantage: Option<Arc<crate::data::AlphaVantageClient>>,
```

Update constructors:

```rust
pub fn with_yfinance(
    snapshot_store: Arc<SnapshotStore>,
    yfinance: Arc<dyn YFinanceData>,
    valuation_fetch_timeout: Duration,
) -> Arc<Self> {
    Arc::new(Self {
        snapshot_store,
        yfinance,
        sec_edgar: None,
        alpha_vantage: None,
        valuation_fetch_timeout,
    })
}

pub fn with_yfinance_edgar_and_alpha_vantage(
    snapshot_store: Arc<SnapshotStore>,
    yfinance: Arc<dyn YFinanceData>,
    sec_edgar: Arc<SecEdgarClient>,
    alpha_vantage: Option<Arc<crate::data::AlphaVantageClient>>,
    valuation_fetch_timeout: Duration,
) -> Arc<Self> {
    Arc::new(Self {
        snapshot_store,
        yfinance,
        sec_edgar: Some(sec_edgar),
        alpha_vantage,
        valuation_fetch_timeout,
    })
}
```

Keep `with_yfinance_and_edgar` as a convenience wrapper only if existing tests call it:

```rust
pub fn with_yfinance_and_edgar(
    snapshot_store: Arc<SnapshotStore>,
    yfinance: Arc<dyn YFinanceData>,
    sec_edgar: Arc<SecEdgarClient>,
    valuation_fetch_timeout: Duration,
) -> Arc<Self> {
    Self::with_yfinance_edgar_and_alpha_vantage(
        snapshot_store,
        yfinance,
        sec_edgar,
        None,
        valuation_fetch_timeout,
    )
}
```

- [x] **Step 7: Thread Alpha Vantage through graph construction**

Update `build_graph_from_pack` and `runtime::build_graph` signatures to accept:

```rust
alpha_vantage: Option<Arc<crate::data::AlphaVantageClient>>,
```

Construct `AnalystSyncTask` with:

```rust
let analyst_sync = AnalystSyncTask::with_yfinance_edgar_and_alpha_vantage(
    Arc::clone(&snapshot_store),
    Arc::new(yfinance.clone()),
    sec_edgar,
    alpha_vantage,
    Duration::from_secs(config.llm.valuation_fetch_timeout_secs),
);
```

In `TradingPipeline::try_new`, convert the owned client for graph use before storing the pipeline. Use this shape:

```rust
let alpha_vantage = alpha_vantage.map(Arc::new);
let graph = runtime::build_graph(
    Arc::clone(&config),
    &finnhub,
    &fred,
    &yfinance,
    sec_edgar,
    alpha_vantage.clone(),
    Arc::clone(&snapshot_store),
    &quick_handle,
    &deep_handle,
);
```

Change `TradingPipeline.alpha_vantage` to store `Option<Arc<crate::data::AlphaVantageClient>>`. Update `AnalysisRuntime::new` to pass the unwrapped client through `try_new`; `try_new` owns the conversion.

Changing the `runtime::build_graph` / `build_graph_from_pack` signatures breaks every call site, so update them all (not just `try_new`):

- `TradingPipeline::new` (`pipeline/mod.rs`) — pass `None`.
- `build_graph(&self)` (`pipeline/mod.rs`) — pass `self.alpha_vantage.clone()`.
- `try_new` — pass `alpha_vantage.clone()` (as shown above).
- `runtime::build_graph` — forward the new `alpha_vantage` argument into `crate::workflow::builder::build_graph_from_pack` (it delegates, so the parameter must be threaded through, not left unused).
- `builder.rs` — replace the existing `AnalystSyncTask::with_yfinance_and_edgar(...)` construction with `with_yfinance_edgar_and_alpha_vantage(...)`, threading `alpha_vantage` in.

Every site must compile after the signature change; do not leave the new parameter unused in `runtime::build_graph` or any caller passing the old arity.

- [x] **Step 8: Fetch ETF profile and official benchmark in valuation inputs**

Extend the local `ValuationInputs` struct in `workflow/tasks/analyst.rs`:

```rust
etf_profile: Option<crate::data::alpha_vantage::EtfProfileData>,
etf_official_benchmark: Option<(crate::data::sec_risk_return::BenchmarkMetadata, Option<u32>)>,
```

In `fetch_valuation_inputs`, add parameters:

```rust
alpha_vantage: Option<&Arc<crate::data::AlphaVantageClient>>,
risk_return_lookup: Option<crate::data::sec_risk_return::RiskReturnLookup<'_>>,
```

Inside the live ETF branch, fetch Alpha Vantage profile fail-soft:

```rust
if let Some(av) = alpha_vantage {
    etf_profile = match fetch_with_timeout(
        symbol,
        "alpha_vantage_etf_profile",
        fetch_timeout,
        av.fetch_etf_profile(symbol),
    )
    .await
    {
        Some(Ok(crate::data::alpha_vantage::EtfProfileFetch::Found(profile))) => Some(profile),
        _ => None,
    };
}
```

After N-PORT fetch and SEC risk/return lookup exist, set `etf_official_benchmark` by preferring SEC risk/return metadata over N-PORT textual benchmark. If SEC dataset fetch is not in this task yet, wire the field from a pure helper and leave the provider call in Task 7.

Adding `alpha_vantage` and `risk_return_lookup` to `fetch_valuation_inputs` breaks its existing 7-arg callers — update them in this step: the two existing `fetch_valuation_inputs(...)` test call sites in `analyst.rs` (near the SPY tests) and the production caller in the live ETF branch must each pass `None, None` (or the real values) for the two new parameters.

Add the benchmark-OHLCV-disable assertion here, moved from Task 5 (it could not compile against the Task 5 7-arg signature). The `etf_benchmark_ohlcv` field is removed in Task 5/6, so this version drops the field-based assertion and relies on the single `get_ohlcv` expectation to prove no benchmark-symbol OHLCV is fetched:

```rust
#[tokio::test]
async fn etf_baseline_fetch_does_not_fetch_benchmark_ohlcv_when_only_name_exists() {
    let mut mock = MockYFinanceData::new();
    mock.expect_get_quote().returning(|_| None);
    mock.expect_get_distribution_yield_ttm().returning(|_| None);
    mock.expect_get_ohlcv()
        .times(1)
        .returning(|symbol, _, _| {
            assert_eq!(symbol, "SOXX", "only ETF OHLCV should be fetched");
            Ok(vec![])
        });

    let info = etf_fund_info_snapshot();
    let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let _inputs = fetch_valuation_inputs(
        &mock,
        None,
        None,
        None,
        PackId::EtfBaseline,
        "SOXX",
        &today,
        Duration::from_secs(1),
        Some(&info),
    )
    .await;

    // The single get_ohlcv expectation above (times(1), symbol == "SOXX") proves
    // no benchmark-symbol OHLCV is fetched; the removed etf_benchmark_ohlcv field
    // is no longer asserted.
}
```

Include `etf_baseline_fetch_does_not_fetch_benchmark_ohlcv_when_only_name_exists` in the Step 10 run command.

- [x] **Step 9: Pass new inputs to valuator**

In `derive_runtime_valuation`, pass:

```rust
etf_profile: valuation_inputs.etf_profile.as_ref(),
etf_official_benchmark: valuation_inputs
    .etf_official_benchmark
    .as_ref()
    .map(|(metadata, age_days)| (metadata, *age_days)),
```

- [x] **Step 10: Run valuation and wiring tests**

Run: `cargo nextest run -p scorpio-core etf_valuator_prefers_alpha_vantage_profile_composition etf_valuator_renders_benchmark_name_only_status_when_official_name_exists etf_baseline_fetch_does_not_fetch_benchmark_ohlcv_when_only_name_exists --no-fail-fast`

Expected: PASS.

- [x] **Step 11: Commit**

```bash
git add crates/scorpio-core/src/valuation/mod.rs crates/scorpio-core/src/valuation/etf/premium_discount.rs crates/scorpio-core/src/workflow/tasks/analyst.rs crates/scorpio-core/src/workflow/builder.rs crates/scorpio-core/src/workflow/pipeline/runtime.rs crates/scorpio-core/src/workflow/pipeline/mod.rs crates/scorpio-core/src/app/mod.rs
git commit -m "feat(etf): merge ETF profile and official benchmark metadata"
```

---

## Task 7: Wire SEC Risk/Return Dataset Fetch Fail-Soft

**Files:**
- Modify: `crates/scorpio-core/src/data/sec_risk_return.rs`
- Modify: `crates/scorpio-core/src/data/sec_edgar/mod.rs`
- Modify: `crates/scorpio-core/src/workflow/tasks/analyst.rs`

> **Scope note (round-2 decision):** The *live* SEC DERA risk/return fetch is deferred to a follow-on plan. This task ships the parser, the `risk_return_zip_path` URL helper, and `fetch_risk_return_benchmark_for_ticker` as pure, unit-tested building blocks, but does **not** wire the live fetch into the valuation path — official benchmark names come from the N-PORT `stated_benchmark` fallback (Step 5). The bytes-returning seam, ZIP decode (member-by-header + size cap), and publication-lag-aware quarter selection are out of scope here.

- [x] **Step 1: Write failing SEC lookup tests**

Add this test to `crates/scorpio-core/src/data/sec_edgar/mod.rs`:

```rust
#[tokio::test]
async fn lookup_cik_mf_returns_class_id_for_risk_return_lookup() {
    let mut mock = MockEdgarHttp::new();
    mock.expect_get().returning(|url| {
        assert!(url.ends_with("/files/company_tickers_mf.json"));
        Ok((
            200,
            r#"{
                "fields":["cik","seriesId","classId","symbol"],
                "data":[[1100663,"S000004354","C000012084","SOXX"]]
            }"#
            .to_owned(),
        ))
    });
    let client = SecEdgarClient::with_http(Arc::new(mock), SharedRateLimiter::disabled("test"));

    let entry = client.lookup_cik_mf("SOXX").await.expect("lookup").expect("entry");
    assert_eq!(entry.series_id, "S000004354");
    assert_eq!(entry.class_id, "C000012084");
}
```

Add this test to `crates/scorpio-core/src/data/sec_risk_return.rs`:

```rust
#[test]
fn risk_return_zip_url_uses_official_sec_quarter_path() {
    assert_eq!(
        risk_return_zip_path("2025q3"),
        "/files/dera/data/mutual-fund-prospectus-risk/return-summary-data-sets/2025q3_rr1.zip"
    );
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p scorpio-core lookup_cik_mf_returns_class_id_for_risk_return_lookup risk_return_zip_url_uses_official_sec_quarter_path --no-fail-fast`

Expected: FAIL until `class_id` and URL helper are implemented.

- [x] **Step 3: Implement risk/return quarter URL helper**

Add to `crates/scorpio-core/src/data/sec_risk_return.rs`:

```rust
pub fn risk_return_zip_path(quarter: &str) -> String {
    format!(
        "/files/dera/data/mutual-fund-prospectus-risk/return-summary-data-sets/{}_rr1.zip",
        quarter.to_ascii_lowercase()
    )
}
```

- [x] **Step 4: Add fetch orchestration seam**

Use the existing `SecEdgarClient` HTTP seam rather than a second SEC client. Add a method in `crates/scorpio-core/src/data/sec_edgar/mod.rs`:

```rust
pub async fn fetch_risk_return_benchmark_for_ticker(
    &self,
    ticker: &str,
    dataset_quarter: &str,
) -> Option<crate::data::sec_risk_return::BenchmarkMetadata> {
    let entry = self.lookup_cik_mf(ticker).await.ok().flatten()?;
    let path = crate::data::sec_risk_return::risk_return_zip_path(dataset_quarter);
    let url = format!("{EDGAR_WWW_BASE_URL}{path}");
    self.limiter.acquire().await;
    let (status, body) = self.http.get(&url).await.ok()?;
    if status != 200 {
        return None;
    }
    crate::data::sec_risk_return::parse_risk_return_tsv_for_benchmark(
        &body,
        crate::data::sec_risk_return::RiskReturnLookup {
            series_id: &entry.series_id,
            class_id: &entry.class_id,
        },
        dataset_quarter,
    )
}
```

**Live fetch deferred to a follow-on plan.** The live SEC DERA risk/return path needs machinery that is out of scope here: a bytes-returning `EdgarHttp` seam (the `String`-returning `get` cannot carry ZIP bytes), ZIP member selection *by header* (the archive is named `{quarter}_rr1.zip` but contains several TSVs, none named `_rr1`), a decompressed-size cap (zip-bomb guard), and publication-lag-aware quarter selection. In this plan, `fetch_risk_return_benchmark_for_ticker` and `risk_return_zip_path` are defined and unit-tested as pure building blocks but are **not** wired into the live valuation path; official benchmark names come solely from the N-PORT `stated_benchmark` fallback (Step 5). A follow-on plan adds the byte-fetch + decode seam and wires it ahead of the N-PORT fallback. Keep the pure TSV-parser tests unchanged.

- [x] **Step 5: Wire lookup into ETF valuation input fetch**

In `fetch_valuation_inputs`, after N-PORT fetch, add:

```rust
// Live SEC DERA risk/return fetch is deferred to a follow-on plan (see Step 4).
// Official benchmark names come from the N-PORT stated_benchmark fallback only.
if let Some((name, source, age)) = resolve_official_benchmark_name(None, etf_holdings.as_ref()) {
    etf_official_benchmark = Some((
        crate::data::sec_risk_return::BenchmarkMetadata {
            name,
            source,
            dataset_quarter: String::new(),
            accession: None,
            filing_date: None,
            source_period: None,
        },
        age,
    ));
}
```

(Live quarter selection — `risk_return_quarter` and the prior-quarter fallback — moves to the follow-on plan with the byte-fetch seam. This plan keeps only the pure `risk_return_zip_path` helper and its `risk_return_zip_url_uses_official_sec_quarter_path` test.)

- [x] **Step 6: Run SEC wiring tests**

Run: `cargo nextest run -p scorpio-core lookup_cik_mf_returns_class_id_for_risk_return_lookup risk_return_zip_url_uses_official_sec_quarter_path --no-fail-fast`

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/scorpio-core/src/data/sec_risk_return.rs crates/scorpio-core/src/data/sec_edgar/mod.rs crates/scorpio-core/src/workflow/tasks/analyst.rs
git commit -m "feat(etf): fetch SEC risk return benchmark metadata"
```

---

## Task 8: Update Prompt Context And ETF Prompt Contracts

**Files:**
- Modify: `crates/scorpio-core/src/agents/shared/valuation_prompt.rs`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/trader.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/conservative_risk.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/neutral_risk.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/aggressive_risk.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/etf_runtime_contract.md`
- Modify: `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`

- [x] **Step 1: Write failing valuation prompt test**

Add this test to `crates/scorpio-core/src/agents/shared/valuation_prompt.rs` tests:

```rust
#[test]
fn etf_valuation_context_renders_official_benchmark_and_unavailable_tracking() {
    let mut etf = minimal_etf_valuation();
    etf.official_benchmark_name = Some("NYSE Semiconductor Index".to_owned());
    etf.official_benchmark_source = Some(BenchmarkSource::SecRiskReturn);
    etf.tracking_status = TrackingStatus::BenchmarkNameOnly;

    let context = build_etf_valuation_context(&etf);

    assert!(context.contains("official benchmark: NYSE Semiconductor Index"));
    assert!(context.contains("SEC DERA Risk/Return Summary"));
    assert!(context.contains("tracking error: unavailable"));
    assert!(context.contains("benchmark daily history not resolved"));
}
```

- [x] **Step 2: Run prompt test to verify it fails**

Run: `cargo nextest run -p scorpio-core etf_valuation_context_renders_official_benchmark_and_unavailable_tracking --no-fail-fast`

Expected: FAIL because prompt context does not render official benchmark or tracking status yet.

- [x] **Step 3: Update valuation context rendering**

Modify `build_etf_valuation_context` in `crates/scorpio-core/src/agents/shared/valuation_prompt.rs`:

```rust
if let Some(name) = etf.official_benchmark_name.as_deref() {
    lines.push(format!(
        "  - official benchmark: {} ({})",
        sanitize_prompt_context(name),
        benchmark_source_label(etf.official_benchmark_source),
    ));
}

match (etf.tracking.as_ref(), etf.tracking_status) {
    (Some(tracking), TrackingStatus::Computed) => lines.push(format!(
        "  - tracking error: 90d {:.2}%, 1y {:.2}% vs {}",
        tracking.te_pct_90d,
        tracking.te_pct_1y,
        sanitize_prompt_context(&tracking.benchmark_symbol),
    )),
    (_, TrackingStatus::BenchmarkNameOnly) => lines.push(
        "  - tracking error: unavailable; benchmark daily history not resolved; treat benchmark name as reference context only".to_owned(),
    ),
    _ => lines.push(
        "  - tracking error: unavailable; benchmark daily history not resolved".to_owned(),
    ),
}
```

Add helper:

```rust
fn benchmark_source_label(source: Option<BenchmarkSource>) -> &'static str {
    match source {
        Some(BenchmarkSource::SecRiskReturn) => "SEC DERA Risk/Return Summary",
        Some(BenchmarkSource::SecNport) => "SEC N-PORT",
        None => "unknown source",
    }
}
```

- [x] **Step 4: Update ETF prompt markdown**

Use these replacements:

`crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md`:

```markdown
## ETF tracking & options lens

In addition to standard technicals:

- **Official benchmark name** — when present, cite it as filed reference context.
  Do not infer a benchmark ticker from the textual name.
- **Tracking error** — current scope leaves tracking error unavailable unless a
  future verified daily benchmark-history source is present. Treat any old
  `TrackingError` value as optional reference, not deterministic evidence.
- **Tracking interpretation** — distinguish annualised tracking-error volatility,
  cumulative tracking difference, and fee drag. Do not collapse them into one
  claim.
- **Options context** — use options evidence only when explicit `options_context`
  or `options_gex` is supplied in evidence.
```

`crates/scorpio-core/src/analysis_packs/etf/prompts/conservative_risk.md`: delete the `tracking_failure` deterministic trigger bullet and replace it with:

```markdown
- Tracking error is unavailable in current ETF runs unless verified benchmark
  daily history exists. Do not flag tracking failure from a textual benchmark
  name alone.
```

`crates/scorpio-core/src/analysis_packs/etf/prompts/neutral_risk.md`: replace the leading deterministic tags line with:

```markdown
trips any of `extreme_premium`, `leverage_decay`, or `stale_holdings`, surface
that tag as the leading line. Do not invent `tracking_failure` when tracking
status says benchmark daily history is unresolved.
```

`crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md`: replace any trigger set containing `tracking_failure` with:

```markdown
`{extreme_premium, leverage_decay, stale_holdings}`.
```

Update trader/aggressive/runtime text so tracking-error language says unavailable/reference-only unless `TrackingStatus::Computed` exists.

- [x] **Step 5: Run prompt and regression tests**

Run: `cargo nextest run -p scorpio-core etf_valuation_context_renders_official_benchmark_and_unavailable_tracking prompt_bundle_regression_gate --no-fail-fast`

Expected: PASS after updating prompt golden fixtures if this repo stores prompt bundle snapshots.

- [x] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/agents/shared/valuation_prompt.rs crates/scorpio-core/src/analysis_packs/etf/prompts/etf_tracking_options_focus.md crates/scorpio-core/src/analysis_packs/etf/prompts/trader.md crates/scorpio-core/src/analysis_packs/etf/prompts/conservative_risk.md crates/scorpio-core/src/analysis_packs/etf/prompts/neutral_risk.md crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md crates/scorpio-core/src/analysis_packs/etf/prompts/aggressive_risk.md crates/scorpio-core/src/analysis_packs/etf/prompts/etf_runtime_contract.md crates/scorpio-core/tests/prompt_bundle_regression_gate.rs
git commit -m "fix(etf): make tracking prompts benchmark-name aware"
```

---

## Task 9: Render Source-Aware ETF Report Panel

**Files:**
- Modify: `crates/scorpio-reporters/src/terminal/etf.rs`
- Modify: `crates/scorpio-reporters/tests/terminal.rs`

- [x] **Step 1: Write failing reporter test**

Add this test to `crates/scorpio-reporters/tests/terminal.rs`:

```rust
#[test]
fn etf_terminal_renders_profile_source_official_benchmark_and_unavailable_tracking() {
    use scorpio_core::state::{
        AssetShape, BenchmarkSource, DerivedValuation, EtfComposition, EtfCompositionSource,
        EtfDataAvailability, EtfValuation, HoldingWeight, PremiumBand, PremiumSnapshot,
        ScenarioValuation, SectorWeight, TrackingStatus,
    };

    let mut state = TradingState::new("SOXX".to_owned(), "2026-05-30".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(240.0),
                market_price: 241.0,
                bid: Some(240.9),
                ask: Some(241.1),
                premium_pct: Some(0.42),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.08),
                as_of: chrono::Utc::now(),
            },
            composition: Some(EtfComposition {
                source: EtfCompositionSource::AlphaVantageEtfProfile,
                top_holdings: vec![HoldingWeight {
                    cusip: None,
                    ticker: Some("NVDA".to_owned()),
                    name: "NVIDIA Corp".to_owned(),
                    weight_pct: 8.4,
                    value_usd: None,
                }],
                top10_concentration_pct: 8.4,
                sector_weights: vec![SectorWeight {
                    sector: "Semiconductors".to_owned(),
                    weight_pct: 78.2,
                }],
                expense_ratio_pct: Some(0.0035),
                aum_usd: Some(12_300_000_000.0),
                fund_family: Some("iShares".to_owned()),
                distribution_yield_ttm_pct: Some(0.0061),
                holdings_filing_date: chrono::Utc::now().date_naive(),
                holdings_report_date: None,
                holdings_age_days: 0,
                portfolio_turnover_pct: Some(0.24),
                inception_date: Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap()),
            }),
            tracking: None,
            tracking_status: TrackingStatus::BenchmarkNameOnly,
            official_benchmark_name: Some("NYSE Semiconductor Index".to_owned()),
            official_benchmark_source: Some(BenchmarkSource::SecRiskReturn),
            official_benchmark_metadata_age_days: Some(316),
            options_gex: None,
            category: Some("Technology".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(rendered.contains("Composition source") && rendered.contains("Alpha Vantage ETF_PROFILE"));
    assert!(rendered.contains("Official benchmark") && rendered.contains("NYSE Semiconductor Index"));
    assert!(rendered.contains("SEC DERA Risk/Return Summary"));
    assert!(rendered.contains("Tracking error") && rendered.contains("unavailable"));
    assert!(rendered.contains("benchmark daily history not resolved"));
}
```

- [x] **Step 2: Run reporter test to verify it fails**

Run: `cargo nextest run -p scorpio-reporters etf_terminal_renders_profile_source_official_benchmark_and_unavailable_tracking --no-fail-fast`

Expected: FAIL because current renderer only shows N-PORT filing date and benchmark-resolved trust signal.

- [x] **Step 3: Add source and benchmark labels**

Modify `crates/scorpio-reporters/src/terminal/etf.rs` imports to include:

```rust
BenchmarkSource, EtfCompositionSource, TrackingStatus,
```

Add helpers:

```rust
fn composition_source_label(source: EtfCompositionSource) -> &'static str {
    match source {
        EtfCompositionSource::AlphaVantageEtfProfile => "Alpha Vantage ETF_PROFILE",
        EtfCompositionSource::SecNport => "SEC N-PORT",
    }
}

fn benchmark_source_label(source: BenchmarkSource) -> &'static str {
    match source {
        BenchmarkSource::SecRiskReturn => "SEC DERA Risk/Return Summary",
        BenchmarkSource::SecNport => "SEC N-PORT",
    }
}
```

- [x] **Step 4: Render composition source and date semantics**

Update `render_composition_block`:

```rust
let _ = writeln!(
    out,
    "Composition source  {}",
    composition_source_label(comp.source)
);
match comp.source {
    EtfCompositionSource::AlphaVantageEtfProfile => {
        let _ = writeln!(out, "Provider snapshot  Alpha Vantage latest available profile");
    }
    EtfCompositionSource::SecNport => {
        if let Some(report_date) = comp.holdings_report_date {
            let _ = writeln!(out, "Report date      {report_date}");
        }
        let _ = writeln!(out, "Filing date      {}", comp.holdings_filing_date);
    }
}
```

Keep the existing top holdings and staleness rows after the source/date block.

- [x] **Step 5: Render official benchmark and tracking status**

Add a benchmark block before tracking:

```rust
fn render_official_benchmark_block(out: &mut String, etf: &EtfValuation) {
    if let Some(name) = etf.official_benchmark_name.as_deref() {
        let source = etf
            .official_benchmark_source
            .map(benchmark_source_label)
            .unwrap_or("unknown source");
        let _ = writeln!(out, "Official benchmark {name} ({source})");
    }
}
```

Replace the `None` branch for tracking with:

```rust
fn render_tracking_status(out: &mut String, etf: &EtfValuation, policy: RenderPolicy) {
    match etf.tracking.as_ref() {
        Some(tr) if etf.tracking_status == TrackingStatus::Computed => render_tracking_block(out, tr, policy),
        _ => {
            let _ = writeln!(
                out,
                "{} Tracking error unavailable - benchmark daily history not resolved",
                policy.warn()
            );
        }
    }
}
```

Call:

```rust
render_official_benchmark_block(out, etf);
render_tracking_status(out, etf, policy);
```

- [x] **Step 6: Update trust signals**

Change trust signals from `Benchmark` to separate benchmark-name and tracking-history concepts:

```rust
"NAV: {}  Bid/Ask: {}  Holdings: {}  Official benchmark: {}  Tracking history: {}",
policy.check(etf.flags.nav_available),
policy.check(etf.flags.bid_ask_available),
policy.check(etf.flags.holdings_present),
policy.check(etf.official_benchmark_name.is_some()),
policy.check(etf.tracking_status == TrackingStatus::Computed),
```

- [x] **Step 7: Run reporter tests**

Run: `cargo nextest run -p scorpio-reporters etf_terminal_renders_profile_source_official_benchmark_and_unavailable_tracking --no-fail-fast`

Expected: PASS.

- [x] **Step 8: Commit**

```bash
git add crates/scorpio-reporters/src/terminal/etf.rs crates/scorpio-reporters/tests/terminal.rs
git commit -m "feat(report): render source-aware ETF benchmark status"
```

---

## Task 10: Update Documentation And Schema Notes

**Files:**
- Modify: `docs/architecture/dependencies.md`
- Modify: `docs/architecture/equity-analysis-pack.md`
- Modify: `docs/architecture/config-and-errors.md`
- Modify: `crates/scorpio-reporters/src/json.rs`
- Modify: `crates/scorpio-reporters/tests/json.rs`

- [x] **Step 1: Confirm JSON artifact schema version (stays v2)**

If consumers treat additive ETF fields as backward-compatible, keep `JSON_REPORT_SCHEMA_VERSION` at `2` and add this comment in `crates/scorpio-reporters/src/json.rs` below the v2 note:

```rust
/// Additive ETF profile/benchmark fields remain schema v2 because the envelope
/// shape and existing field meanings are unchanged; old consumers ignore the
/// new nested fields.
```

Keep `JSON_REPORT_SCHEMA_VERSION` at `2`: these ETF fields are additive and do not change the envelope shape or any existing field meaning, so old consumers ignore the new nested fields — the same additive-is-backward-compatible rule the Scope section applies to `THESIS_MEMORY_SCHEMA_VERSION`. Bump the version only when a future change removes or renames a field. Leave `crates/scorpio-reporters/tests/json.rs` asserting `2`.

- [x] **Step 2: Update architecture docs**

In `docs/architecture/dependencies.md`, add Alpha Vantage `ETF_PROFILE` and SEC DERA risk/return datasets to the provider dependency table.

In `docs/architecture/equity-analysis-pack.md`, update ETF valuation sections so they say Alpha Vantage is primary composition/profile source, SEC N-PORT is fallback/regulatory provenance, and tracking error is unavailable until verified benchmark daily history exists.

In `docs/architecture/config-and-errors.md`, add the Alpha Vantage key note that transcripts and ETF profile share `SCORPIO_ALPHA_VANTAGE_API_KEY`, and both fail open when the key is absent or throttled.

- [x] **Step 3: Run doc-adjacent tests**

Run: `cargo nextest run -p scorpio-reporters json_reporter_writes_valid_file_with_correct_schema_version --no-fail-fast`

Expected: PASS with the v2 assertion unchanged.

- [x] **Step 4: Commit**

```bash
git add docs/architecture/dependencies.md docs/architecture/equity-analysis-pack.md docs/architecture/config-and-errors.md crates/scorpio-reporters/src/json.rs crates/scorpio-reporters/tests/json.rs
git commit -m "docs(etf): document profile and benchmark data sources"
```

---

## Task 11: Full Verification

**Files:**
- No code changes expected.

- [x] **Step 1: Run formatter check**

Run: `cargo fmt -- --check`

Expected: PASS. If it fails, run `cargo fmt`, inspect the diff, and repeat `cargo fmt -- --check`.

- [x] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: PASS with zero warnings.

- [x] **Step 3: Run full nextest suite**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`

Expected: PASS. If `protoc` is missing locally, install it with `brew install protobuf` on macOS before rerunning.

- [x] **Step 4: Optional smoke run**

Run: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze SOXX`

Expected: terminal report shows `Official benchmark: NYSE Semiconductor Index (SEC DERA Risk/Return Summary)` when the SEC risk/return fixture/live data path is available, `Tracking error unavailable - benchmark daily history not resolved`, and no static `^SOX` benchmark fallback.

- [x] **Step 5: Commit any verification-only fixes**

```bash
git add <files changed by verification fixes>
git commit -m "fix(etf): satisfy profile tracking verification"
```

Skip this commit if verification produced no changes.

---

## Self-Review

**Spec coverage:** This plan covers Alpha Vantage `ETF_PROFILE` parsing and workflow use, N-PORT report date parsing, SEC DERA benchmark-name parsing, additive ETF state metadata, deletion of the static benchmark resolver, disabled benchmark OHLCV and tracking-error computation, prompt updates, terminal reporting, and verification commands.

**No-placeholder scan:** The plan contains concrete file paths, test snippets, implementation snippets, commands, and expected outcomes. It avoids `TBD`, incomplete sections, and unspecified validation language.

**Type consistency:** The same type names are used throughout: `EtfCompositionSource`, `BenchmarkSource`, `TrackingStatus`, `EtfProfileFetch`, `EtfProfileData`, `BenchmarkMetadata`, and `RiskReturnLookup`. New valuation carrier fields are named `etf_profile` and `etf_official_benchmark` consistently from workflow to valuator.

**Execution note:** The live SEC DERA risk/return fetch is deferred to a follow-on plan. This plan ships the pure TSV parser (`parse_risk_return_tsv_for_benchmark`), the `risk_return_zip_path` URL helper, and `fetch_risk_return_benchmark_for_ticker` as tested building blocks, but does not wire the live fetch into the valuation path — official benchmark names come from the N-PORT `stated_benchmark` fallback. The follow-on plan adds the bytes-returning seam, ZIP decode (member-by-header + size cap), and publication-lag-aware quarter selection, wiring it ahead of the N-PORT fallback.
