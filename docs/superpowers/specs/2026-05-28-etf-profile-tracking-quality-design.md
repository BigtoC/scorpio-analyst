# ETF Profile And Tracking Quality Design

## Status

Approved verbally on 2026-05-28. Written spec is ready for user review before implementation planning.

## Goal

Improve ETF analysis quality by using Alpha Vantage `ETF_PROFILE` as the primary ETF composition/profile source and by displaying the official textual ETF benchmark from SEC DERA risk/return data when available. The user-visible outcome is that SOXX-style reports show the filed benchmark name without computing misleading tracking error from hardcoded or proxy benchmark symbols.

## Root-Cause Verification

The current N-PORT path can make composition freshness look better than it is, but it is not the direct mechanism that shifts the tracking-error return window.

- `fetch_latest_nport_p_for_ticker(symbol, 180)` filters SEC filings by Atom `filing-date`.
- `parse_nport_p(xml, filing_date)` stores that filing date as `NPortHoldings.filing_date`; it does not parse `repPdDate` or `repPdEnd` from the filing body.
- `build_composition` calculates `holdings_age_days` from `NPortHoldings.filing_date`, so it can understate portfolio-age staleness when the report period predates the filing date.
- Live SOXX SEC evidence on 2026-05-28 showed a public N-PORT filing dated `2026-05-28` with portfolio/report date `2026-03-31`, confirming the filing/report-date gap.
- `compute_tracking_error` uses current ETF OHLCV and benchmark OHLCV, aligned by price date. It does not use N-PORT holdings dates.
- N-PORT only affects tracking indirectly by supplying `stated_benchmark`, if present.
- Live SOXX N-PORT evidence did not expose `benchmarkName` or `indxName`; it had `nameDesignatedIndex` / `indexIdentifier` as `N/A`. With current code, SOXX is therefore likely resolved through the static `SOXX -> ^SOX` fallback, not through N-PORT. That hardcoded fallback is wrong for SOXX because the filed benchmark is `NYSE Semiconductor Index`, not the prior PHLX-style `^SOX` proxy.
- SEC `data.sec.gov` APIs do not provide a standalone benchmark API; they provide submissions history and XBRL financial-statement facts. The SEC DERA Mutual Fund Prospectus Risk/Return Summary datasets are a separate official structured source for tagged prospectus risk/return facts, including objective, strategy, performance-table, and index-return facts by series/class.
- Live SOXX DERA risk/return evidence from the 2025 Q3 dataset, accession `0001193125-25-162603`, series `S000004354`, class `C000012084`, identifies the filed textual benchmark as `NYSE Semiconductor Index` in `StrategyNarrativeTextBlock` and an index return dimension `NYSESemiconductorIndex`. The same filing notes that index returns through 2021-06-20 reflected the PHLX Semiconductor Sector Index, then ICE Semiconductor Index, renamed effective 2023-11-03 to NYSE Semiconductor Index.

Working hypothesis: the false `tracking_failure` risk is primarily benchmark resolution and interpretation, not old holdings entering the TE calculation. The N-PORT date bug still matters for composition freshness and for confidence if a future N-PORT does provide benchmark identity.

## Approach

Use Alpha Vantage `ETF_PROFILE` as the primary profile/composition provider. Keep SEC N-PORT as a regulatory holdings fallback and as a possible benchmark-name source, but parse its report date separately from filing date. Add SEC DERA risk/return prospectus data as an official structured benchmark-name source when available. Delete the hardcoded ETF ticker to benchmark symbol resolver. Do not compute tracking error until a future source provides verified benchmark symbol/name resolution and daily benchmark OHLCV history.

Alternatives considered:

- Only add Alpha Vantage composition. This fixes holdings and fee gaps but leaves deterministic tracking false positives in place.
- Scrape raw Yahoo benchmark metadata. `yfinance-rs` v0.8 typed APIs do not expose an ETF benchmark/stated-index field, and raw Yahoo modules are unstable/rate-limited.
- Keep a manually curated ETF ticker to benchmark symbol table. Rejected for current scope because SOXX and SMH show that ETF-ticker mappings easily drift or point to the wrong index family.
- Recommended: Alpha Vantage primary plus SEC DERA official benchmark-name display, with tracking error disabled until daily benchmark history can be resolved from trusted metadata.

## Data Sources

### Alpha Vantage ETF_PROFILE

`AlphaVantageClient` will add a fail-soft ETF profile fetch. The response fields used are:

- `net_assets` -> AUM in USD.
- `net_expense_ratio` -> expense ratio decimal fraction, preserving the current reporter convention that multiplies by 100 for display.
- `portfolio_turnover` -> optional decimal fraction when not `n/a`.
- `dividend_yield` -> distribution yield decimal fraction.
- `inception_date` -> optional fund inception date.
- `leveraged` -> leverage flag; `NO` maps to `Some(1.0)`, while leveraged products keep existing name/category heuristics unless a stronger numeric source is available.
- `sectors[].weight` -> sector weight decimal fraction converted to percentage points for existing `SectorWeight.weight_pct`.
- `holdings[].weight` -> holding weight decimal fraction converted to percentage points for existing `HoldingWeight.weight_pct`.

Provider diagnostics (`Note`, `Information`, `Error Message`) should follow the existing Alpha Vantage transcript style: classify throttling/unavailable/schema/auth failures without logging secrets or unbounded provider text.

### SEC N-PORT

N-PORT remains the regulatory holdings fallback and a best-effort benchmark-name fallback.

Add `report_date: Option<NaiveDate>` to `NPortHoldings` from `repPdDate` or `repPdEnd`. Keep `filing_date` for audit provenance. Composition freshness must use `report_date.unwrap_or(filing_date)`, not filing date alone.

N-PORT benchmark fields remain best-effort. The parser currently captures `benchmarkName` and `indxName`; it should not treat `nameDesignatedIndex=N/A` as a benchmark. If additional official benchmark fields are added later, values like `N/A`, `None`, and blanks must normalize to `None`.

### SEC DERA Risk/Return Summary

The SEC DERA Mutual Fund Prospectus Risk/Return Summary datasets are official quarterly ZIP files with flattened TSV extracts from tagged fund prospectus risk/return exhibits. They are not a low-latency API, but they are structured and series/class-aware.

Use this source as an optional benchmark-name enrichment path, not as a holdings source and not as a daily return-history source:

- Map ticker to SEC identifiers through `company_tickers_mf.json`, yielding `cik`, `series`, and `class`.
- Select the newest locally fetched/available quarterly dataset that contains rows for the series/class.
- Prefer `StrategyNarrativeTextBlock` and `ObjectivePrimaryTextBlock` for textual stated index extraction.
- Use `AvgAnnlRtrPct` rows whose `measure` is an index-member dimension, plus `PerformanceTableTextBlock` / `PerformanceTableMarketIndexChanged`, as corroborating evidence.
- Persist the dataset quarter, filing accession, filing date, and source document period as benchmark metadata age/provenance.

For SOXX, this source identifies `NYSE Semiconductor Index` as the filed textual index. It does not provide a provider ticker such as `^ICESEMIT` and does not provide daily index history. For SMH-style cases, the official textual benchmark may be a proprietary index such as `MVIS US Listed Semiconductor 25 Index`, which likewise should not be inferred from ETF ticker alone.

## State Model

Add source metadata without breaking old snapshots. All new persisted fields need `#[serde(default)]` per `TradingState` schema rules.

Proposed additions:

- `EtfComposition.source: EtfCompositionSource` with values `alpha_vantage_etf_profile`, `sec_nport` and default `sec_nport` for legacy snapshots.
- `EtfComposition.holdings_report_date: Option<NaiveDate>` for true portfolio date when available.
- `EtfComposition.portfolio_turnover_pct: Option<f64>` and `EtfComposition.inception_date: Option<NaiveDate>` for Alpha Vantage profile enrichment.
- `EtfValuation.official_benchmark_name: Option<String>` for official textual benchmark names that do not necessarily map exactly to a market-data symbol.
- `EtfValuation.official_benchmark_source: Option<BenchmarkSource>`, initially `sec_risk_return` or `sec_nport` when present.
- `EtfValuation.official_benchmark_metadata_age_days: Option<u32>` for benchmark-name provenance freshness.
- `EtfValuation.tracking_status: TrackingStatus` so reports can explain why tracking error is unavailable.

`BenchmarkSource` should distinguish `sec_risk_return` and `sec_nport` for current scope. `TrackingStatus` should distinguish at least `not_resolved`, `benchmark_name_only`, and future-ready `computed`.

## Workflow

ETF valuation input hydration will fetch Alpha Vantage ETF profile alongside yfinance quote/fund info, distribution yield, and ETF OHLCV. SEC N-PORT remains a holdings fallback because Alpha Vantage does not provide a regulatory filing/report date. SEC DERA risk/return data is the preferred official SEC benchmark-name source when available.

Composition merge order:

1. Alpha Vantage `ETF_PROFILE` holdings/sectors/profile when present.
2. SEC N-PORT holdings/sectors/profile fallback when Alpha Vantage is unavailable or structurally empty.
3. Existing graceful absence when neither provider yields usable holdings.

Profile enrichment order:

1. Start with yfinance `FundInfo` for fund identity/family/kind.
2. Overlay Alpha Vantage fees/AUM/yield/leverage metadata when present.
3. Overlay SEC DERA risk/return or SEC N-PORT benchmark only through the benchmark resolver, not by mutating `FundInfo` and losing source metadata.

Benchmark-name resolution should return textual metadata, not a market-data symbol:

1. SEC DERA risk/return textual benchmark if present and normalized.
2. SEC N-PORT benchmark if present and normalized.
3. No fallback. If neither source provides a benchmark name, leave it absent.

Delete `crates/scorpio-core/src/data/etf_benchmarks.rs` and remove the call to `crate::data::etf_benchmarks::resolve(etf_symbol)`. Do not fetch benchmark OHLCV based on ETF ticker or textual benchmark name. Tracking computation should be skipped and `tracking_status` should explain `benchmark daily history not resolved`.

## Tracking Interpretation

Tracking error remains annualized standard deviation of daily ETF active returns versus benchmark returns over aligned return samples, but current scope disables it because benchmark daily OHLCV is not reliably resolved.

Prompt and deterministic-trigger behavior changes:

- Remove `tracking_failure` as a deterministic trigger for current scope because tracking error will not be computed.
- Official benchmark name may be cited as reference context only.
- If tracking error appears in old snapshots, prompts must treat it as optional reference, not strong deterministic evidence.
- The technical prompt should distinguish annualized TE volatility, cumulative tracking difference, and fee drag.
- The valuation context should render official benchmark name and tracking status so risk and fund-manager agents can avoid overclaiming.

For SOXX specifically, SEC risk/return data proves the filed textual benchmark is `NYSE Semiconductor Index`. The current implementation should display that name and skip tracking error until a future metadata source resolves the correct benchmark symbol and daily history.

## Reporting

The terminal ETF panel should become source-aware:

- Composition block labels source as `Alpha Vantage ETF_PROFILE` or `SEC N-PORT`.
- SEC composition displays report date and filing date separately when both are available.
- Alpha Vantage composition avoids pretending to have a regulatory filing/report date; it can display provider snapshot/fetch date or omit age if provider as-of is unavailable.
- ETF Valuation Snapshot displays `Official benchmark: <name> (SEC DERA Risk/Return Summary)` when available.
- ETF Valuation Snapshot displays `Tracking error: unavailable - benchmark daily history not resolved` when a benchmark name exists but no verified daily history exists.
- Trust signals should distinguish `Official benchmark: present` from `Tracking error: unavailable` rather than implying benchmark price-series coverage.

## Tests

Add focused tests before implementation changes:

- Alpha Vantage `ETF_PROFILE` fixture parses numeric strings, `n/a`, decimal weights, fees, AUM, yield, inception date, and leveraged flag.
- Alpha Vantage profile converts holding/sector decimal weights to percentage points.
- N-PORT parser captures `repPdDate` / `repPdEnd` and leaves `N/A` designated-index fields out of `stated_benchmark`.
- Composition age uses report date when present and filing date only as fallback.
- Benchmark-name resolver returns source/provenance metadata for SEC DERA and N-PORT cases and returns absent when neither source has a normalized name.
- SEC risk/return fixture for SOXX extracts `NYSE Semiconductor Index` from `S000004354` / `C000012084`.
- SOXX-like N-PORT with no official benchmark no longer falls back to static `^SOX`.
- Benchmark OHLCV is not fetched when only a textual benchmark name is present.
- Tracking error is absent and tracking status explains benchmark daily history is unresolved.
- Valuation prompt renders official benchmark name and states tracking error is unavailable/reference-only.
- Conservative risk prompt no longer includes deterministic `tracking_failure`.
- Terminal reporter renders composition source, official benchmark name, and tracking-unavailable status.

Full verification before completion remains:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

## Out Of Scope

- Replacing yfinance OHLCV history with a new price-history provider.
- Scraping raw Yahoo quoteSummary modules for unofficial benchmark fields.
- Building a full EDGAR search crawler; use known SEC mapping/files and fail softly when datasets are unavailable.
- Proving every ETF benchmark mapping as exact.
- Manually curated ETF-to-benchmark metadata RAG/catalog integration.
- Reworking ETF valuation beyond composition/profile/tracking metadata quality.

## Future Work

TODO: Resolve official benchmark names to verified benchmark symbols and daily OHLCV history through a trusted metadata/catalog source. A future implementation may ingest a manually curated ETF and benchmark data file as RAG, but that is out of scope for this spec.
