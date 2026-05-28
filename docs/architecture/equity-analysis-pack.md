# Equity Analysis Pack — Deep Dive

This document describes how the equity analysis pack works end-to-end: how it is defined, how data is fetched, how analysis and computation happen, how the pipeline flows, and what numbers appear in the final report. Everything below is derived from the source code.

---

## 1. Pack Definition

### What is a pack

A pack is a **declarative data manifest** (`AnalysisPackManifest`), not a trait. It declares *what* analysis to perform without owning execution logic. The execution graph topology is fixed across all packs; packs vary the analyst set, prompt content, enrichment intent, and valuation strategy.

### The `AnalysisPackManifest` struct

Defined in `crates/scorpio-core/src/analysis_packs/manifest/schema.rs`:

| Field                   | Type                              | Purpose                                                                      |
|-------------------------|-----------------------------------|------------------------------------------------------------------------------|
| `id`                    | `PackId`                          | Unique identifier (`Baseline` for equity)                                    |
| `name`                  | `String`                          | Human-readable name: `"Balanced Institutional"`                              |
| `description`           | `String`                          | Strategy description                                                         |
| `required_inputs`       | `Vec<String>`                     | Drives analyst fan-out: `["fundamentals", "sentiment", "news", "technical"]` |
| `enrichment_intent`     | `EnrichmentIntent`                | Which enrichment data to fetch (transcripts, consensus, event news)          |
| `strategy_focus`        | `StrategyFocus`                   | Lens for prompt/report framing: `Balanced`                                   |
| `analysis_emphasis`     | `String`                          | Injected into analysis prompts                                               |
| `report_strategy_label` | `String`                          | Label in report header: `"Balanced Institutional"`                           |
| `default_valuation`     | `ValuationAssessment`             | `Full` for equity                                                            |
| `prompt_bundle`         | `PromptBundle`                    | Per-role system prompts (all 14 agent roles)                                 |
| `valuator_selection`    | `HashMap<AssetShape, ValuatorId>` | `CorporateEquity -> EquityDefault`                                           |
| `auditor_enabled`       | `bool`                            | `true` for equity                                                            |
| `reddit_subreddits`     | `Vec<String>`                     | `["stocks", "investing", "wallstreetbets", "StockMarket", "Daytrading"]`     |

### Where the pack is built

`crates/scorpio-core/src/analysis_packs/equity/baseline.rs` — `baseline_pack()` factory function at line 129.

### Pack registration

`crates/scorpio-core/src/analysis_packs/registry.rs` — `resolve_pack(PackId::Baseline)` calls `equity::baseline_pack()`. Pure compile-time lookup, no I/O.

### Runtime policy resolution

`resolve_runtime_policy("baseline")` converts the manifest into a serializable `RuntimePolicy` (`crates/scorpio-core/src/analysis_packs/selection.rs`). This is the "single resolution boundary" — raw pack structure does not leak past the selection module.

---

## 2. Prompt System

### PromptBundle

The equity baseline pack ships 14 prompt slots, one per agent role. Each slot is a `Cow<'static, str>` loaded at compile time via `include_str!`.

### Equity-specific prompts

Located in `crates/scorpio-core/src/analysis_packs/equity/prompts/`:
- `fundamental_analyst.md`
- `sentiment_analyst.md`
- `trader.md`
- `fund_manager.md`
- `aggressive_risk.md`, `conservative_risk.md`, `neutral_risk.md`
- `theme_c_management_red_flags.md`

### Shared prompts

Located in `crates/scorpio-core/src/analysis_packs/common/prompts/`:
- `news_analyst.md`, `technical_analyst.md`
- `bullish_researcher.md`, `bearish_researcher.md`, `debate_moderator.md`
- `risk_moderator.md`, `auditor.md`
- `analyst_runtime_contract.md` — evidence-discipline rules appended to every analyst prompt
- `theme_h_sourcing_and_untrusted.md` — sourcing rules
- Output contracts: `risk_report_output_contract.md`, `trade_proposal_output_contract.md`, `execution_status_output_contract.md`

### Prompt composition

In `baseline_prompt_bundle()` (baseline.rs line 72):
1. Each analyst prompt is composed from its raw `.md` file + theme sections + the analyst runtime contract.
2. `{ticker}`, `{current_date}`, `{analysis_emphasis}` placeholders are substituted at runtime by `render_analyst_system_prompt()` (in `agents/analyst/equity/common.rs`).
3. Risk agents get the shared `RiskReport` output contract with `{stance}` substituted.
4. The trader gets the `TradeProposal` output contract.
5. The fund manager gets the `ExecutionStatus` output contract.

---

## 3. Data Fetching

### External data sources

| Provider                                 | API         | What it provides                                                                                                                                         | Used by                                                         |
|------------------------------------------|-------------|----------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------|
| **Finnhub**                              | REST API    | Fundamentals (P/E, EPS, revenue, margins, debt, insider transactions), earnings data, company news, market news                                          | Fundamental Analyst, Sentiment Analyst, News Analyst            |
| **Yahoo Finance**                        | yfinance-rs | OHLCV price history, options chain, financial statements (cashflow, balance sheet, income statement, shares outstanding), earnings trend, profile, quote | Technical Analyst, AnalystSyncTask (valuation), pack classifier |
| **FRED** (Federal Reserve Economic Data) | REST API    | Economic indicators (GDP, inflation, employment, rates)                                                                                                  | News Analyst                                                    |
| **Alpha Vantage**                        | REST API    | Earnings-call transcripts                                                                                                                                | Sentiment Analyst, News Analyst (via enrichment layer)          |
| **SEC EDGAR**                            | REST API    | 8-K filings, N-PORT-P holdings (ETF)                                                                                                                     | AnalystSyncTask (enrichment catalysts)                          |
| **Reddit**                               | Reddit API  | Crowd commentary from 5 subreddits                                                                                                                       | Sentiment Analyst (via sentiment sidecar)                       |

### News pre-fetch

Before the analyst fan-out starts, `prefetch_analyst_news()` (in `agents/analyst/mod.rs` line 321) runs three providers concurrently:

```
Finnhub news + Yahoo news + Reddit news
       ↓              ↓           ↓
    ┌──────────────────────────────────┐
    │  Vetted lane: Finnhub + Yahoo    │ → NewsAnalyst
    │  (deduplicated, max 30 articles) │
    ├──────────────────────────────────┤
    │  Sentiment lane: Vetted + Reddit │ → SentimentAnalyst
    │  (Reddit rows preserved distinct)│
    └──────────────────────────────────┘
```

**Deduplication strategy** (`merge_news` at line 243):
1. URL-first: canonical URL after shortener filtering (yhoo.it, bit.ly, t.co, etc.)
2. Title-fallback: exact normalized title (lowercased, trimmed)

**Lane split**: Reddit rows never displace vetted rows from the NewsAnalyst. The SentimentAnalyst sees both vetted + Reddit as distinct sentiment inputs.

### Per-analyst tool binding

Each analyst agent is an LLM with tools bound. The LLM decides when to call tools during inference.

| Analyst         | Tools bound                                                                                                                                                                        | Data fetched                                                                                            |
|-----------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------|
| **Fundamental** | `GetFundamentals`, `GetEarnings` (Finnhub)                                                                                                                                         | P/E, EPS, revenue growth, current ratio, debt-to-equity, gross margin, net income, insider transactions |
| **Sentiment**   | `GetNews` or `GetCachedNews` (Finnhub)                                                                                                                                             | Company news articles for sentiment inference; also receives transcript context for Theme C             |
| **News**        | `GetNews` or `GetCachedNews`, `GetMarketNews`, `GetEconomicIndicators` (Finnhub + FRED)                                                                                            | Company news, broader market news, macro economic indicators                                            |
| **Technical**   | `GetOhlcv`, `CalculateAllIndicators`, `CalculateRsi`, `CalculateMacd`, `CalculateAtr`, `CalculateBollingerBands`, `CalculateIndicatorByName`, `GetOptionsSnapshot` (Yahoo Finance) | OHLCV price history (365 days), technical indicators, options snapshot                                  |

When cached news is available (from pre-fetch), `GetCachedNews` is bound instead of `GetNews`, saving one Finnhub API call.

---

## 4. Analyst Agents

### Concurrency model

`run_analyst_team()` (in `agents/analyst/mod.rs` line 415) spawns all four analysts concurrently via `tokio::spawn`. Each gets its own `CompletionModelHandle` (quick-thinking tier).

### Per-agent flow

Each analyst follows the same pattern:
1. **Construct**: build system prompt from `RuntimePolicy.prompt_bundle` + placeholder substitution
2. **Bind tools**: create tool instances scoped to the symbol
3. **Build agent**: `build_agent_with_tools(handle, system_prompt, tools)`
4. **Run inference**: `run_analyst_inference()` with retry policy, timeout, parse hook, validate hook
5. **Parse output**: deserialize LLM JSON response into typed struct
6. **Validate**: semantic checks (non-empty summary, score ranges, etc.)
7. **Return**: `(TypedData, AgentTokenUsage)`

### Inference routing

`run_analyst_inference()` (in `agents/analyst/equity/common.rs`) routes between:
- **Native-typed output** (OpenAI, Anthropic, Gemini): structured JSON extraction
- **Text-fallback** (OpenRouter, DeepSeek): raw text extraction with JSON parsing

Includes retry with self-correction: if parse fails, the error message is fed back to the LLM for another attempt.

### Degradation policy

- 0 failures: all four fields populated
- 1 failure: three fields populated, one `None`, continues with partial data
- 2+ failures: abort with `TradingError::AnalystError`

### Output types

| Analyst     | Output struct     | Key fields                                                                                                                                                                                                                                 |
|-------------|-------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Fundamental | `FundamentalData` | `revenue_growth_pct`, `pe_ratio`, `eps`, `current_ratio`, `debt_to_equity`, `gross_margin`, `net_income`, `insider_transactions[]`, `summary`                                                                                              |
| Sentiment   | `SentimentData`   | `overall_score` (-1.0 to 1.0), `source_breakdown[]` (per-source score + sample_size), `engagement_peaks[]`, `summary`                                                                                                                      |
| News        | `NewsData`        | `articles[]` (title, source, published_at, relevance_score, snippet, url), `macro_events[]` (event, impact_direction, confidence), `summary`                                                                                               |
| Technical   | `TechnicalData`   | `rsi`, `macd` (macd_line, signal_line, histogram), `atr`, `sma_20`, `sma_50`, `ema_12`, `ema_26`, `bollinger_upper`, `bollinger_lower`, `support_level`, `resistance_level`, `volume_avg`, `options_summary`, `options_context`, `summary` |

---

## 5. Deterministic Valuation

### When it runs

After the analyst fan-out completes, `AnalystSyncTask` (in `workflow/tasks/analyst.rs`) runs. It:
1. Merges analyst results into `TradingState`
2. Fetches financial statement data from Yahoo Finance
3. Calls `derive_valuation()` to compute deterministic metrics
4. Writes `DerivedValuation` to state

### What data is fetched for valuation

`fetch_valuation_inputs()` (analyst.rs line 796) fetches concurrently (with timeout):
- `profile` — company/fund classification
- `quarterly_cashflow` — free cash flow data
- `quarterly_balance_sheet` — cash, debt, shares outstanding
- `quarterly_income_stmt` — operating income (EBITDA proxy)
- `quarterly_shares` — share count history
- `earnings_trend` — forward EPS estimates, growth rates

All fetches degrade gracefully to `None` on network failure.

### How metrics are computed

`derive_valuation()` in `crates/scorpio-core/src/state/valuation_derive.rs`:

#### Asset shape routing

1. `Profile::Company` → `AssetShape::CorporateEquity` → proceed to valuation
2. `Profile::Fund` → `AssetShape::Fund` → `NotAssessed { reason: "fund_style_asset" }`
3. No profile → data-shape detection: if any of cashflow/balance/income/earnings_trend is present → `CorporateEquity`; otherwise → `Unknown`

#### DCF (Discounted Cash Flow)

**Inputs**: trailing 4 quarters of free cash flow + shares outstanding
**Formula**: `intrinsic_value_per_share = (FCF / 0.10) / shares_outstanding`
**Constants**: fixed 10% discount rate
**Guards**: FCF must be positive; shares must be > 0; requires 4 consecutive quarters

#### EV/EBITDA

**Inputs**: balance sheet (shares, cash, long-term debt) + income statement (operating income) + current price
**Formula**:
- `market_cap = shares * price`
- `enterprise_value = market_cap + debt - cash`
- `EBITDA ≈ operating_income` (trailing 4 quarters)
- `ev_ebitda_ratio = EV / EBITDA`
**Guards**: all inputs must be present and positive

#### Forward P/E

**Inputs**: earnings trend (forward EPS estimate) + current price
**Formula**: `forward_pe = price / forward_eps`
**Selection**: picks the first trend row with a non-None, positive `earnings_estimate.avg`, preferring `+1Y` > `0Y` > annual periods
**Guards**: EPS and price must be > 0

#### PEG Ratio

**Inputs**: forward P/E + earnings growth rate
**Formula**: `peg_ratio = forward_pe / (growth_rate * 100)`
**Guards**: forward P/E must exist; growth must be > 0

### Valuation routing via pack

The pack's `valuator_selection` map (`CorporateEquity -> EquityDefault`) determines which valuator handles each asset shape. `EquityDefaultValuator` (in `valuation/equity/default.rs`) is a thin wrapper around `derive_valuation()`.

Shapes not in the map fall through to `NotAssessed { reason: "no_valuator_selected" }`.

---

## 6. Pipeline Execution Flow

### Graph topology

Built by `build_graph_from_pack()` in `workflow/builder.rs`:

```
Preflight
    │
    ▼
AnalystFanOut ───────────────────────────────┐
    │  (4 concurrent tasks)                  │
    │  - FundamentalAnalystTask              │
    │  - SentimentAnalystTask                │
    │  - NewsAnalystTask                     │
    │  - TechnicalAnalystTask                │
    ▼                                        │
AnalystSync                                  │
    │  - Merge results into TradingState     │
    │  - Write evidence records              │
    │  - Derive deterministic valuation      │
    │  - Build data coverage report          │
    │  - Build provenance summary            │
    │                                        │
    ├──[debate enabled?]──► BullishResearcher│
    │                         │              │
    │                         ▼              │
    │                    BearishResearcher   │
    │                         │              │
    │                         ▼              │
    │                    DebateModerator     │
    │                         │              │
    │                    [round < max?]──────┤
    │                         │              │
    │                         ▼              │
    ├──[debate disabled]────►Trader◄─────────┘
    │                         │
    ├──[risk enabled?]──► AggressiveRisk
    │                         │
    │                    ConservativeRisk
    │                         │
    │                    NeutralRisk
    │                         │
    │                    RiskModerator
    │                         │
    │                    [round < max?]──► loop back
    │                         │
    │                         ▼
    ├──[risk disabled]──► FundManager
                              │
                              ▼
                          Auditor
```

### Preflight task

`PreflightTask` (in `workflow/tasks/preflight.rs`):
1. Writes `RuntimePolicy` into graph-flow context (sole writer of `state.analysis_runtime_policy`)
2. Validates pack completeness (all required prompt slots non-empty)
3. Sets `RoutingFlags` controlling debate/risk/auditor stage entry
4. Fetches enrichment data (transcripts, consensus estimates, event news)
5. Fetches market volatility (VIX)
6. Fetches current price

### Analyst fan-out

Each `AnalystTask` (e.g., `FundamentalAnalystTask`):
1. Deserializes `TradingState` from context
2. Reads `RuntimePolicy` from state
3. Constructs the analyst agent with tools
4. Runs LLM inference
5. Writes typed result + token usage to context
6. Sets success/failure flag

### AnalystSync task

After all four analysts complete:
1. Reads results from context
2. Merges into `TradingState` (fundamental_metrics, market_sentiment, macro_news, technical_indicators)
3. Writes `EvidenceRecord` for each analyst with source attribution
4. Fetches financial statements from Yahoo Finance
5. Calls `derive_valuation()` → writes `DerivedValuation` to state
6. Builds `DataCoverageReport` (required vs missing inputs)
7. Builds `ProvenanceSummary` (list of data providers used)
8. Applies degradation policy (2+ failures abort)
9. Saves snapshot

### Debate stage

When enabled (default for equity):
1. **BullishResearcher**: deep-thinking LLM argues the bull case using analyst data
2. **BearishResearcher**: deep-thinking LLM argues the bear case
3. **DebateModerator**: synthesizes consensus from bull/bear arguments
4. Loop: repeats for `max_debate_rounds` (configurable, default from config)

### Trader stage

Deep-thinking LLM that:
1. Receives all analyst data + debate consensus + deterministic valuation
2. Produces a `TradeProposal`: action (Buy/Sell/Hold/Overweight/Underweight), target_price, stop_loss, confidence, rationale, valuation_assessment

### Risk stage

When enabled (default for equity):
1. **AggressiveRisk**: evaluates proposal from an aggressive risk perspective
2. **ConservativeRisk**: evaluates from a conservative perspective
3. **NeutralRisk**: evaluates from a neutral perspective
4. **RiskModerator**: synthesizes risk assessment
5. Loop: repeats for `max_risk_rounds`

Each risk agent produces a `RiskReport`: assessment, flags_violation (bool), recommended_adjustments.

**Auto-reject rule**: if both NeutralRisk and ConservativeRisk flag a violation, the proposal is auto-rejected.

### FundManager stage

Deep-thinking LLM that:
1. Receives trader proposal + risk review + all analyst data
2. Produces `ExecutionStatus`: decision (Approved/Rejected), final action, rationale, entry_guidance, suggested_position

### Auditor stage

When `auditor_enabled = true` (default for equity):
1. Reviews the entire pipeline output for internal consistency
2. Produces `AuditorReport`: findings (with severity: Critical/Warning/Info), summary
3. Fail-open: if auditor LLM fails, the run continues with deterministic findings only

---

## 7. State Model

### TradingState

The top-level state container (`crates/scorpio-core/src/state/trading_state.rs`) holds everything:

| Field                      | Type                           | Set by                          |
|----------------------------|--------------------------------|---------------------------------|
| `asset_symbol`             | `String`                       | CLI input                       |
| `target_date`              | `String`                       | CLI input                       |
| `execution_id`             | `Uuid`                         | Generated at start              |
| `fundamental_metrics`      | `Option<FundamentalData>`      | FundamentalAnalyst              |
| `market_sentiment`         | `Option<SentimentData>`        | SentimentAnalyst                |
| `macro_news`               | `Option<NewsData>`             | NewsAnalyst                     |
| `technical_indicators`     | `Option<TechnicalData>`        | TechnicalAnalyst                |
| `derived_valuation`        | `Option<DerivedValuation>`     | AnalystSyncTask                 |
| `analysis_runtime_policy`  | `Option<RuntimePolicy>`        | PreflightTask                   |
| `trader_proposal`          | `Option<TradeProposal>`        | TraderTask                      |
| `aggressive_risk_report`   | `Option<RiskReport>`           | AggressiveRiskTask              |
| `conservative_risk_report` | `Option<RiskReport>`           | ConservativeRiskTask            |
| `neutral_risk_report`      | `Option<RiskReport>`           | NeutralRiskTask                 |
| `final_execution_status`   | `Option<ExecutionStatus>`      | FundManagerTask                 |
| `audit_status`             | `AuditStatus`                  | AuditorTask                     |
| `audit_report`             | `Option<AuditorReport>`        | AuditorTask                     |
| `debate_history`           | `Vec<DebateMessage>`           | Bullish/Bearish/DebateModerator |
| `consensus_summary`        | `Option<String>`               | DebateModerator                 |
| `current_price`            | `Option<f64>`                  | PreflightTask                   |
| `market_volatility`        | `Option<MarketVolatilityData>` | PreflightTask                   |
| `data_coverage`            | `Option<DataCoverageReport>`   | AnalystSyncTask                 |
| `provenance_summary`       | `Option<ProvenanceSummary>`    | AnalystSyncTask                 |
| `token_usage`              | `TokenUsageTracker`            | All tasks                       |
| `enrichment_*`             | `EnrichmentState<T>`           | PreflightTask                   |
| `evidence_*`               | `Option<EvidenceRecord<T>>`    | AnalystSyncTask                 |

### EquityState

Ten equity-specific fields stored on `TradingState` (accessed via methods):
- `fundamental_metrics` → `Option<FundamentalData>`
- `technical_indicators` → `Option<TechnicalData>`
- `market_sentiment` → `Option<SentimentData>`
- `macro_news` → `Option<NewsData>`
- `evidence_fundamental` → `Option<EvidenceRecord<FundamentalData>>`
- `evidence_sentiment` → `Option<EvidenceRecord<SentimentData>>`
- `evidence_news` → `Option<EvidenceRecord<NewsData>>`
- `evidence_technical` → `Option<EvidenceRecord<TechnicalData>>`
- `market_volatility` → `Option<MarketVolatilityData>`
- `derived_valuation` → `Option<DerivedValuation>`

### EvidenceRecord

Each analyst output is wrapped in an `EvidenceRecord<T>` that tracks:
- `kind`: `EvidenceKind` (Fundamental, Sentiment, News, Technical)
- `payload`: the typed data
- `sources`: `Vec<EvidenceSource>` with provider name, datasets, fetched_at
- `quality_flags`: empty for now

---

## 8. Report Generation

The terminal report is rendered by `format_final_report()` in `crates/scorpio-reporters/src/terminal/final_report.rs`. It calls section writers in order:

### Section 1: Header

```
Final Report: AAPL
As of: 2026-03-14  |  Execution ID: <uuid>  |  Strategy: Balanced Institutional
Trader Proposal: Buy  |  Fund Manager Decision: Approved  |  Final Recommendation: Buy
Timestamp: 2026-03-14T12:00:00Z
```

- Strategy label comes from `RuntimePolicy.report_strategy_label`
- Action is color-coded: Buy=green, Sell=red, Hold=yellow
- Decision is color-coded: Approved=green, Rejected=red

### Section 2: Executive Summary

The `ExecutionStatus.rationale` text from the Fund Manager. Includes `entry_guidance` and `suggested_position` if present.

### Section 3: Trader Proposal

Table with:
- **Action**: Buy/Sell/Hold/Overweight/Underweight (color-coded)
- **Current Price**: from state
- **Confidence**: 0.0-1.0 (green > 0.7, yellow 0.4-0.7, red < 0.4)
- **Target Price**: from proposal
- **Stop Loss**: from proposal
- **Valuation**: model-authored assessment (only shown when deterministic valuation is a `CorporateEquity` or `Etf` scenario; omitted with explanation for `NotAssessed` or missing)

### Section 4: Analyst Evidence Snapshot

Table with columns: Analyst | Key Evidence | Status

| Analyst      | Key Evidence                                | Status           |
|--------------|---------------------------------------------|------------------|
| Fundamentals | First sentence of `FundamentalData.summary` | Complete/Missing |
| Sentiment    | First sentence of `SentimentData.summary`   | Complete/Missing |
| News         | First sentence of `NewsData.summary`        | Complete/Missing |
| Technical    | First sentence of `TechnicalData.summary`   | Complete/Missing |
| VIX          | `MarketVolatilityData.summary()`            | Complete/Missing |

Below the table, full summaries are printed for each present analyst.

### Section 5: Enrichment Data

Shows status of:
- **Event-news**: `EnrichmentStatus` (Available/NotConfigured/Disabled/FetchFailed/NotAvailable) + event count + first 5 events
- **Consensus estimates**: status + EPS estimate, revenue estimate, analyst count, as-of date

### Section 6: Scenario Valuation

From `write_scenario_valuation()` in `terminal/valuation.rs`:

For **CorporateEquity** shape:
```
Asset shape: Corporate equity
Valuation model: Corporate Equity
  DCF intrinsic value: 185.42 (FCF: 1200000000, discount rate: 10.0%)
  EV/EBITDA: 22.5 (implied: 192.00)
  Forward P/E: 26.2 (forward EPS: 7.25)
  PEG ratio: 1.80
```

Each metric is only shown when its computation succeeded. If all are `None`:
```
  No valuation metrics computed (insufficient inputs).
```

For **Fund/Unknown** shape:
```
Asset shape: Fund
Valuation: not assessed for this asset shape.
Reason: fund_style_asset
```

For **No valuation**:
```
Not computed for this run.
```

### Section 7: Data Quality and Coverage

Shows required inputs vs missing inputs from `DataCoverageReport`.

### Section 8: Evidence Provenance

Lists unique data providers from all `EvidenceRecord` sources (e.g., finnhub, yfinance, fred, alpha_vantage, sec_edgar, reddit).

### Section 9: Research Debate Summary

- Consensus summary from `DebateModerator`
- Strongest bullish evidence (first sentence of last bull message)
- Strongest bearish evidence (first sentence of last bear message)

### Section 10: Risk Review

Table with columns: Persona | Violation | Assessment | Adjustments

| Persona      | Violation | Assessment                   | Adjustments             |
|--------------|-----------|------------------------------|-------------------------|
| Aggressive   | No        | First sentence of assessment | Recommended adjustments |
| Neutral      | No        | ...                          | ...                     |
| Conservative | No        | ...                          | ...                     |

Violation is color-coded: Yes=red, No=green.

Full assessments printed below the table.

### Section 11: Deterministic Safety Check

```
  Neutral flags violation: false
  Conservative flags violation: false
  Auto-reject rule triggered: No
```

Auto-reject triggers when both Neutral AND Conservative flag violations.

### Section 12: Auditor Review

When auditor ran:
- **Passed**: "No findings. Proposal is internally consistent."
- **Findings**: table with Severity (CRITICAL=red, WARNING=yellow) | Location | Description
- **FailedOpen**: "Auditor failed — run not blocked (fail-open). Showing deterministic findings only."

### Section 13: Token Usage Summary

```
Quick-thinking model: gpt-4o-mini
Deep-thinking model: o3
```

Table with columns: Phase | Prompt | Completion | Total | Duration

Phases include: Analyst Fan-Out, Trader Synthesis, Risk Review, Fund Manager, Auditor Review.

### Section 14: Disclaimer

Standard disclaimer about AI-generated analysis, not financial advice.

---

## 9. Pack Classification at Runtime

### How the pack is selected

`classify_runtime_pack()` in `workflow/pack_classifier.rs`:
1. Fetches `Profile` from Yahoo Finance for the symbol
2. `Profile::Company` → `PackId::Baseline` (equity)
3. `Profile::Fund` + supported ETF kind → `PackId::EtfBaseline`
4. `Profile::Fund` + unsupported kind → fallback to `Baseline`
5. No profile → fallback to `Baseline`

The fallback reason is stored in `state.etf_routing_fallback_reason` and displayed in the report header when applicable.

### Two paths in `run_analysis_cycle()`

1. **`from_pack` path**: Pipeline was built with a pre-resolved manifest (tests, feature flags). `runtime_policy` is `Some` on the pipeline.
2. **Production path**: Pipeline was built via `try_new`. Pack is classified at runtime from the symbol's profile. `runtime_policy` is `None`, so it fetches profile, classifies, resolves.

---

## 10. Reddit Sentiment Sidecar

### Configuration

The equity baseline pack declares 5 subreddits in `EQUITY_BASELINE_REDDIT_SUBREDDITS` (in `constants.rs`):
- `stocks`, `investing`, `wallstreetbets`, `StockMarket`, `Daytrading`

### Ambiguous symbol denylist

`REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST` (constants.rs) contains equity tickers that collide with common words on Reddit (e.g., "AI", "ARE", "FOR"). Reddit ingestion is skipped for these symbols.

### Lane split

Reddit articles are routed to the sentiment lane only. The vetted lane (NewsAnalyst) never sees Reddit rows. This is enforced by `build_sentiment_news()` in `agents/analyst/mod.rs`.

### Source tagging

Reddit articles are tagged with `source: "Reddit r/<subreddit>"` so the SentimentAnalyst can identify them as crowd commentary.

---

## 11. Key Source Files

| File                                   | Purpose                                                       |
|----------------------------------------|---------------------------------------------------------------|
| `analysis_packs/equity/baseline.rs`    | Pack definition, prompt composition                           |
| `analysis_packs/manifest/schema.rs`    | `AnalysisPackManifest` struct                                 |
| `analysis_packs/registry.rs`           | Pack lookup                                                   |
| `analysis_packs/selection.rs`          | `RuntimePolicy` resolution                                    |
| `agents/analyst/mod.rs`                | News pre-fetch, `run_analyst_team()`                          |
| `agents/analyst/equity/fundamental.rs` | Fundamental Analyst agent                                     |
| `agents/analyst/equity/sentiment.rs`   | Sentiment Analyst agent                                       |
| `agents/analyst/equity/news.rs`        | News Analyst agent                                            |
| `agents/analyst/equity/technical.rs`   | Technical Analyst agent                                       |
| `agents/analyst/equity/common.rs`      | Shared inference plumbing                                     |
| `state/valuation_derive.rs`            | `derive_valuation()` — DCF, EV/EBITDA, Forward P/E, PEG       |
| `state/derived.rs`                     | `AssetShape`, `ScenarioValuation`, `CorporateEquityValuation` |
| `state/equity.rs`                      | `EquityState` fields                                          |
| `state/fundamental.rs`                 | `FundamentalData` struct                                      |
| `state/sentiment.rs`                   | `SentimentData` struct                                        |
| `state/news.rs`                        | `NewsData` struct                                             |
| `state/technical.rs`                   | `TechnicalData` struct                                        |
| `workflow/builder.rs`                  | `build_graph_from_pack()` — pipeline graph construction       |
| `workflow/tasks/analyst.rs`            | Analyst task implementations + AnalystSync                    |
| `workflow/tasks/preflight.rs`          | Preflight task                                                |
| `workflow/pack_classifier.rs`          | Runtime pack classification                                   |
| `workflow/pipeline/runtime.rs`         | `run_analysis_cycle()` — main execution loop                  |
| `valuation/equity/default.rs`          | `EquityDefaultValuator`                                       |
| `reporters/terminal/final_report.rs`   | Terminal report rendering                                     |
| `reporters/terminal/valuation.rs`      | Valuation section rendering                                   |
| `constants.rs`                         | Reddit subreddits, ambiguous symbol denylist                  |
