# Product Requirements Document: Rust-Native Multi-Agent Financial Trading System

---

## Executive Summary and Strategic Rationale

The integration of Large Language Models into the financial technology sector has catalyzed a transition from
traditional quantitative algorithmic trading to autonomous, agentic decision-making systems.
Traditional deep learning and quantitative models, while mathematically rigorous, frequently struggle to incorporate
qualitative variables such as macroeconomic sentiment, geopolitical news, and company-specific narrative signals into
their predictive algorithms.
Furthermore, deep learning architectures often function as impenetrable black boxes, lacking the necessary
explainability required by institutional compliance and risk management protocols.
Multi-agent frameworks powered by Large Language Models resolve these deficiencies by mimicking the collaborative,
dialectical, and structured workflows of real-world trading firms.

The original TradingAgents framework, developed by researchers at UCLA (Yijia Xiao, Edward Sun, Di Luo, Wei Wang) and
published under the Tauric Research GitHub organization, empirically demonstrated that a highly specialized society of
autonomous agents—including fundamental analysts, technical analysts, bearish and bullish researchers, and
dedicated risk managers—can significantly outperform traditional rule-based trading strategies such as Simple Moving
Average crossover models, Zero Mean Reversion, and MACD momentum strategies. The original reference implementation was
engineered using Python and LangGraph, utilizing both OpenAI and open-source models as the cognitive engines. While
Python serves as the lingua franca for rapid artificial intelligence prototyping, deploying a Python-based multi-agent
orchestration layer in a high-frequency, production-grade enterprise environment presents profound architectural
bottlenecks. The Global Interpreter Lock restricts true parallel execution, and the reliance on virtual environments,
heavy dependency trees, and untyped data structures introduces significant latency and memory overhead. When simulating
dozens of concurrent analysts aggregating disparate financial application programming interfaces, Python's concurrency
model becomes a critical limiting factor.

This document mandates the comprehensive engineering architecture for a Rust-native reimplementation of the
TradingAgents framework. The architecture is also informed by reusable financial analysis and reporting patterns from
Anthropic's [financial-services-plugins](https://github.com/anthropics/financial-services-plugins) repository,
particularly around evidence handling, provenance tracking, source attribution discipline, and modular financial
workflows. Transitioning this complex multi-agent system to Rust addresses the fundamental limitations of
the Python ecosystem by introducing fearless concurrency, sub-millisecond technical indicator calculations,
deterministic memory management without garbage collection pauses, and absolute compile-time type safety. By leveraging
Rust's `tokio`asynchronous runtime, the system will execute data ingestion and agent inferences in true parallel
threads, vastly reducing the time required to evaluate market conditions (targeting a complete trade cycle in under 20
seconds end-to-end, compared to minutes for sequential Python implementations). This specification outlines the
migration strategy, the selection of the optimal Rust Large Language Model orchestration frameworks, the integration of
high-performance technical indicator libraries, and the stateful directed workflow topology required to replicate and
enhance the original TradingAgents paradigm.

## Conceptual Foundation: The TradingAgents Paradigm

To successfully architect the Rust reimplementation, the engineering team must fully assimilate the theoretical and
empirical foundations of the original TradingAgents framework. The framework was explicitly designed to resolve two
major limitations prevalent in early multi-agent artificial intelligence systems: the lack of realistic organizational
modeling and the degradation of data through inefficient communication interfaces.

### Organizational Modeling and Agent Taxonomy

Previous iterations of financial artificial intelligence typically relied on monolithic agents tasked with
simultaneously retrieving data, analyzing sentiment, and executing trades. This monolithic approach leads to severe
cognitive overload, prompt context degradation, and hallucination. TradingAgents resolves this by decomposing the
trading lifecycle into highly specialized roles constrained by specific systemic prompts and distinct toolsets.

The organizational structure is strictly partitioned into functional teams. The Analyst Team operates asynchronously at
the beginning of the cycle, retrieving raw data from the market. This team consists of the Fundamental Analyst,
Sentiment Analyst, News Analyst, and Technical Analyst. The output of this team forms the foundational state of the
market. Following data aggregation, the Researcher Team—comprising a Bullish Researcher and a Bearish Researcher—engages
in a multi-round dialectical debate to synthesize the raw data into actionable arguments. This debate provides a
balanced perspective, preventing the system from falling into positive feedback loops of irrational exuberance or
unwarranted panic. The Trader Agent subsequently processes these arguments to formulate a transactional proposal, which
is finally subjected to intense scrutiny by the Risk Management Team (Aggressive, Neutral, and Conservative agents) and
authorized by a Fund Manager. Replicating this exact taxonomy in Rust is a primary directive of this reimplementation.

### Resolution of the Telephone Effect

A critical vulnerability in framework designs like AutoGPT or early LangChain implementations is the reliance on
unstructured natural language as the primary state mechanism. As agents converse, critical numerical data points are
often summarized, altered, or entirely forgotten—a phenomenon the authors term the "telephone effect".
To combat this, the TradingAgents architecture enforces a structured communication protocol. Agents do not merely chat
in a shared buffer; they populate specific, structured document templates and reports. In the Rust reimplementation,
this concept will be drastically enhanced. Instead of relying on language models to format text reports reliably, the
system will utilize Rust's strictly typed struct definitions, serialized and deserialized via the serde_json crate.
Large Language Models will be forced to return data in rigid JSON schemas, entirely eliminating data drift as market
variables pass through the execution graph.

### Empirical Performance Benchmarks

The architectural complexity of the TradingAgents framework is justified by its empirical superiority over traditional
algorithmic approaches. Backtesting simulations conducted across major technology equities—including Apple (AAPL),
Google (GOOGL), Amazon (AMZN), Nvidia (NVDA), Microsoft (MSFT), and Meta (META)—between June and November 2024
demonstrated significant outperformance. The system evaluates performance using four quantitative metrics: Cumulative
Return, Annualized Return, Sharpe Ratio, and Maximum Drawdown.

The following table summarizes the comparative performance of the TradingAgents framework against standard baselines on
AAPL stock, underscoring the target performance benchmarks the Rust implementation must match or exceed:

| Strategy / Model      | Cumulative Return (%) | Annualized Return (%) | Sharpe Ratio | Maximum Drawdown (%) |
|:----------------------|:----------------------|:----------------------|:-------------|:---------------------|
| Market Buy & Hold     | -5.23                 | -5.09                 | -1.29        | 11.90                |
| MACD                  | -1.49                 | -1.48                 | -0.81        | 4.53                 |
| KDJ & RSI             | 2.05                  | 2.07                  | 1.64         | 1.09                 |
| Zero Mean Reversion   | 0.57                  | 0.57                  | 0.17         | 0.86                 |
| Simple Moving Average | -3.20                 | -2.97                 | -1.72        | 3.67                 |
| TradingAgents (Ours)  | 26.62                 | 30.50                 | 8.21         | 0.91                 |

The data indicates that while rule-based systems like KDJ & RSI excel at minimizing Maximum Drawdown (1.09%), they fail
to capture meaningful upside. Conversely, the TradingAgents framework achieved a 26.62% Cumulative Return while
simultaneously restricting Maximum Drawdown to an unprecedented 0.91%, resulting in a highly favorable Sharpe Ratio. The
Rust implementation must support a backtesting engine capable of ingesting historical OHLCV data to continuously
validate that the translated architecture maintains this risk-adjusted performance profile.

## Technology Stack Evaluation and Selection

Migrating a complex artificial intelligence orchestration framework from Python to Rust necessitates the careful
evaluation of the emerging Rust machine learning and agentic ecosystem. The following sections detail the selection of
the core crates required to build the LLM connector layer, the stateful workflow orchestrator, and the financial
mathematics engines.

### Large Language Model Orchestration Frameworks

The core requirement for the LLM connector is the ability to seamlessly abstract multiple provider application
programming interfaces (e.g., OpenAI, Anthropic, local instances via Ollama), manage conversation history, and
enforce strict tool-calling schemas.

#### Selected Provider: `rig-core` (v0.35.0)

As directed by project requirements, the framework will utilize `rig-core` (the primary crate name on crates.io) as the
foundational LLM provider connector. `rig` represents a modular, composable, and unopinionated approach to building
LLM-powered applications in Rust. It functions primarily as a robust abstraction layer, providing a unified application
programming interface across over twenty model providers.

`rig` excels in its developer ergonomics, specifically through its `#[tool]` macro, which effortlessly transforms
standard Rust functions into JSON schema-compliant tools accessible by the LLM. This is critical for connecting the
Analyst agents to the financial data APIs. Furthermore, `rig` integrates highly advanced capabilities for
Retrieval-Augmented Generation, including native interfaces for vector stores like `MongoDB`, `LanceDB`, and `Qdrant`,
alongside a sophisticated `EmbeddingsBuilder`. While the original TradingAgents framework relies mostly on live API
calls rather than historical vector retrieval, the ability to seamlessly inject long-term market history via `rig`'s
dynamic context windows provides a clear pathway for future architectural enhancements.

Most importantly, `rig` does not force the developer into a proprietary orchestration loop. Agents instantiated via
`rig::AgentBuilder` implement clear `prompt` and `chat` traits, allowing them to be embedded as discrete execution
nodes within a custom external state machine.

### Stateful Graph Orchestration

The original repository utilizes LangGraph to define the nodes and edges of the trading firm's workflow. LangGraph's
primary advantage is its ability to manage cyclic execution (such as the debate loop between researchers) and maintain a
shared, immutable state object across all nodes. To replicate this in Rust, the framework requires a stateful execution
engine.

#### Selected Orchestrator: `graph-flow` (v0.5.1)

`graph-flow` is a high-performance, type-safe framework explicitly designed to bring LangGraph-inspired stateful
execution to the Rust ecosystem. It treats the primary workflow as a directed graph, where each execution node
implements an asynchronous `Task` trait. The framework features a centralized `Context` object that provides thread-safe
state sharing across the workflow, allowing data aggregated by the Analyst Team to persist through the debate and
execution phases. Enable the optional `"rig"` feature flag (`graph-flow = { version = "0.5.1", features = ["rig"] }`) for
seamless integration with `rig-core` agents.

Crucially, `graph-flow` supports conditional routing and cyclical control flow through its `NextAction` enum, enabling
the framework to dictate whether a node should `Continue` to the next step, `GoBack` to a previous node, or trigger a
`GoTo` command based on runtime evaluations.

`graph-flow` was designed specifically to integrate seamlessly with the `rig` crate, making the combination of these
two libraries the optimal equivalent to the Python LangChain/LangGraph stack. Note that PostgreSQL JSONB persistence is
a planned Phase 2 feature; for the MVP, the complete `TradingState` will be snapshotted to disk via
`serde_json` after each phase, providing a recoverable audit trail pending the storage backend implementation.

#### Architectural Decision

`graph-flow` will orchestrate the execution topology. The `rig` agents will be encapsulated within `graph_flow::Task`
implementations, communicating exclusively through the `graph_flow::Context` state.

### Financial Data Ingestion Ecosystem

The Analyst Team relies entirely on the accuracy, speed, and breadth of the underlying financial data application
programming interfaces. The Rust implementation must leverage highly optimized HTTP clients to manage this ingestion.

1. **Fundamental and News Data**: The `finnhub` (v0.2.1) crate will serve as the primary conduit for corporate
   fundamentals, earnings reports, and global news. It provides 96% coverage of the `Finnhub` API, delivering strongly
   typed Rust models for income statements, insider transactions, and market news. Crucially, it features automatic rate
   limiting (managing 30 requests per second with burst capacity) and customizable retry logic, which is essential when
   executing four Analyst agents concurrently

2. **Market Pricing and Alternative Data**: The `yfinance-rs` (v0.7.2) crate will be utilized for:
   - historical OHLCV (Open, High, Low, Close, Volume) data,
   - full Financial Statements (Cashflow, Balance Sheet, Income Statement, Shares) for DCF and EV/EBITDA valuation math,
   - Analyst Estimates (Forward EPS/Revenue, Price Targets, Recommendations Summary) — fetched as part of the `ConsensusEvidence` enrichment at pipeline startup, rendered into every downstream agent's prompt,
   - Options & Derivatives — a summary snapshot (ATM implied volatility, IV term structure, put/call volume and OI ratios, max pain strike, 25-delta skew) plus a near-the-money strike slice (nearest expiration, strikes within ±5% of spot) exposed as the `get_options_snapshot` tool for the Technical Analyst,
   - Company News — yfinance news feed fetched alongside Finnhub news during Phase 1 prefetch; the two feeds are deduped (by normalized URL, with headline fallback) and merged into a single `NewsData` shared via `Arc` across the News and Sentiment Analysts,
   - Institutional/Insider ownership (including Net Insider Shares Bought/Sold),
   - Corporate Calendar (Earnings dates). It DOES NOT provide ESG data (the `sustainability()` endpoint is broken). Historical upgrade/downgrade event streams are deferred; the Recommendations Summary above is point-in-time only.

3. **Macroeconomic Indicators**: The FRED (Federal Reserve Economic Data) API provides authoritative macroeconomic
   time-series data, replacing the paid Finnhub `economic().data()` endpoint for interest-rate and inflation indicators.
   The custom `FredClient` implementation fetches the latest observations from two key series: the Federal Funds
   Effective Rate (`FEDFUNDS`) and the Consumer Price Index growth rate (`CPALTT01USM657N`). Both series are fetched
   concurrently and classified into `MacroEvent` structs with impact direction and confidence scores. The client is
   rate-limited (2 requests per second, configurable via `rate_limits.fred_rps`) and implements linear-backoff retry
   logic (max 3 attempts, 45-second total budget). An API key is required via the `SCORPIO_FRED_API_KEY` environment
   variable. FRED also exposes the `/fred/release/dates` endpoint used by the **forward-looking catalyst calendar** (see
   §"Forward-Looking Catalyst Calendar"): scheduled-release dates for six high-impact macro releases — CPI (release_id
   10), Nonfarm Payrolls (50), FOMC Decision (101), GDP (53), ISM Manufacturing (21), and Retail Sales (14) — feed
   directly into `CatalystEvent` instances tagged `category: MacroEvents` with the sentinel `symbol: "_MACRO"`.

4. **SEC EDGAR Filings (Tier 2 Catalyst Source)**: A lightweight `SecEdgarClient` wraps the public SEC EDGAR JSON
   submissions endpoint (`https://data.sec.gov/submissions/CIK<10-digit-padded-cik>.json`) and the canonical
   ticker→CIK map (`https://www.sec.gov/files/company_tickers.json`). The client mandates a fair-use
   `User-Agent: Scorpio Analyst scorpio@ledgerlylab.com` header per SEC policy, is rate-limited to 10 requests per
   second via `SharedRateLimiter`, and carries a per-instance circuit breaker that short-circuits subsequent calls to
   `Ok(empty)` after five consecutive runtime failures within a single pipeline run. No API key is required. The client
   surfaces recent 8-K item codes (1.01, 2.01, 2.02, 5.07, 7.01, 8.01) plus 13D/G activist/passive filings as
   `CatalystEvent` instances without parsing filing bodies — categorisation comes directly from the filing's own Item
   header. Tier 3 body parsing (S-1 lockup language, DEF M14A expected-close, FDA AdComm scraping) is explicitly out
   of scope. SEC EDGAR construction failure falls back to the Tier 1 catalyst provider; pipeline construction never
   aborts on a SEC EDGAR build failure.

5. **Entity Resolution**: Before any data ingestion begins, the `PreflightTask` canonicalizes the user-supplied ticker
   symbol via the entity resolution module (`crates/scorpio-core/src/data/entity.rs`). This module delegates ticker-format validation to the
   existing `crates/scorpio-core/src/data/symbol.rs` logic and produces a `ResolvedInstrument` containing the original input, the
   canonicalized uppercase symbol, and optional metadata fields (issuer name, exchange, instrument type, aliases). In
   the initial implementation, metadata fields default to `None`; future milestones may enrich them via API lookups.
   Invalid or empty symbols cause an immediate hard failure at preflight, preventing wasted LLM calls downstream.

6. **Enrichment Adapter Contracts**: The system defines provider-agnostic trait contracts for optional enrichment data
   sources in `crates/scorpio-core/src/data/adapters/`: `TranscriptProvider` (earnings call transcripts), `EstimatesProvider` (consensus
   revenue/EPS estimates, price targets, recommendations summary), `EventNewsProvider` (event-driven news feeds), and
   `CatalystCalendarProvider` (forward-looking catalyst events — earnings, IPO debut, scheduled macro releases,
   ex-dividend, SEC 8-K item codes, 13D/G activist filings).
   Each trait returns a normalized evidence struct (`TranscriptEvidence`, `ConsensusEvidence`, `EventNewsEvidence`) that
   can be consumed uniformly by downstream agents regardless of the upstream provider. The `ConsensusEvidence` contract
   is extended with optional `PriceTargetSummary` (mean/median/high/low + analyst count) and `RecommendationsSummary`
   (bucket counts: strong_buy/buy/hold/sell/strong_sell) sub-payloads, both populated by `YFinanceEstimatesProvider`
   with field-granular fail-open: if only the earnings trend endpoint succeeds, the evidence still publishes with the
   extras as `None`. A separate `OptionsProvider` trait under `data/traits/options.rs` contracts for structured equity
   options snapshots; it is distinct from the crypto-oriented `DerivativesProvider` stub (which carries an opaque
   `raw: String` payload). Under the current provider-constrained roadmap, event/news enrichment, consensus estimates
   (with the extended fields), yfinance options, and the **Tier 1 + Tier 2 catalyst calendar** are concrete targets;
   transcript enrichment is intentionally deferred. This seam allows future milestones to plug in alternate providers
   (Polygon, Tradier, additional news vendors) behind the existing contracts without modifying agent or orchestration
   code.

### Technical Analysis and Quantitative Mathematics

Deep learning models and LLMs lack the inherent architectural capacity to perform precise mathematical calculations on
large time-series arrays. To emulate the Technical Analyst agent, the system exposes indicator calculation as callable
tools — the LLM invokes these tools at inference time and interprets the results, rather than performing the math
itself or receiving pre-computed arrays injected into context.

The Python ecosystem relies on libraries like `pandas-ta`, which operate on dataframes. The Rust ecosystem offers
several alternatives, including `ta`, `rust_ti`, and `kand`. The `kand` crate (v0.2) is selected as the quantitative
engine.
Inspired by the C-based `TA-Lib`, `kand` is written entirely in pure Rust, providing a comprehensive suite of momentum,
volatility, and trend indicators. It is chosen specifically for its configurable precision modes; it can execute
calculations in `f64`extended precision, which prevents the subtle floating-point errors and `NaN` (Not a Number)
propagation issues frequently encountered when calculating iterative variables like the Relative Strength Index or
Exponential Moving Averages over long horizons. The speed of native Rust array processing allows the Technical Analyst
to calculate 60 distinct technical indicators across thousands of historical ticks in a fraction of a millisecond.

For the MVP, this technical-analysis layer is scoped to traditional OHLCV-based long-term investing workflows. While
some of the same indicators may later be reused for digital assets, the MVP MUST NOT be treated as a fully compatible
crypto-analysis solution. Full crypto-native analysis is deferred to future enhancements because it requires explicit
24/7 market-structure handling, logarithmic-scale-aware interpretation, and on-chain valuation metrics such as MVRV.

## Core System Architecture and Topographical Flow

The architecture of the Rust-native TradingAgents system enforces a strict separation between the cognitive reasoning
layer (the rig agents) and the data transport layer (the `graph-flow` state engine). This topographical rigidity ensures
deterministic execution pathways, preventing the system from deviating into endless autonomous reasoning loops.

### High-Level Execution Graph

The following Mermaid diagram outlines the stateful workflow graph topology detailing how information moves concurrently
and sequentially throughout the system. Note: while the primary data flow is acyclic, the debate loop introduces a
controlled cycle via the Moderator node's `NextAction::GoBack`; termination is guaranteed by the `max_debate_rounds`
parameter.

```
       Input: asset_symbol (e.g. "NVDA" or "nvda")
                          │
                          ▼
┌─────────────────────────────────────────────────────────┐
│  PreflightTask                                          │
│  • Validates & canonicalizes symbol ("nvda" → "NVDA")   │
│  • Writes ResolvedInstrument to context                 │
│  • Loads prior thesis memory for the same symbol        │
│  • Derives ProviderCapabilities from config             │
│  • Seeds cache keys with null placeholders              │
│  • Hard-fails on invalid symbol                         │
└──────────────────────────┬──────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────┐
│  EnrichmentPrefetch   [fail-open per stream]            │
│                                                         │
│  • FinnhubEventNewsProvider → enrichment_event_news     │
│    (pre-classified events for downstream prompts)       │
│  • YFinanceEstimatesProvider → enrichment_consensus     │
│    EXTENDED: EPS/revenue estimates + price target       │
│      (mean/median/high/low) + recommendations summary   │
│      (strong_buy/buy/hold/sell/strong_sell counts)      │
│    Field-granular fail-open across three endpoints      │
│  • Merged news prefetch → cached Arc<NewsData>          │
│    Finnhub + yfinance fetched concurrently, deduped     │
│    by normalized URL (headline fallback), shared        │
│    across News and Sentiment Analysts                   │
│  • CatalystCalendarProvider → enrichment_catalysts      │
│    Tier 1: Finnhub earnings + IPO, FRED release dates,  │
│      yfinance ex-dividend → CatalystEvent stream        │
│    Tier 2: SEC EDGAR 8-K item codes (1.01/2.01/2.02/    │
│      5.07/7.01/8.01) + 13D/G activist filings           │
│    tokio::join! fan-out: one source failing zeros out   │
│      only that source's contribution                    │
│    Tier 3 (FDA AdComm, S-1 lockup, DEF M14A close)      │
│      deferred to a follow-up plan                       │
└──────────────────────────┬──────────────────────────────┘
                           │
         ┌─────────────────┼─────────────────┐─────────────────┐
         ▼                 ▼                 ▼                 ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐
│  Fundamental │  │   Sentiment  │  │     News     │  │    Technical     │
│   Analyst    │  │   Analyst    │  │   Analyst    │  │     Analyst      │
│              │  │  reads       │  │  reads       │  │  OHLCV +         │
│  Finnhub +   │  │  cached      │  │  cached      │  │  kand indicators │
│  yfinance    │  │  Arc<News-   │  │  Arc<News-   │  │  + NEW tool      │
│  financials  │  │  Data> +     │  │  Data> +     │  │  get_options_    │
│              │  │  options/    │  │  FRED macro  │  │  snapshot →      │
│              │  │  consensus   │  │              │  │  OptionsProvider │
│              │  │  via shared  │  │              │  │  (summary +      │
│              │  │  enrichment  │  │              │  │  near-the-money  │
│              │  │  context     │  │              │  │  slice)          │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬───────────┘
       └─────────────────┼─────────────────┘─────────────────┘
                         ▼
┌─────────────────────────────────────────────────────────┐
│  AnalystSyncTask                                        │
│                                                         │
│  Dual-write (legacy + new typed fields):                │
│  • fundamental_metrics  +  evidence_fundamental         │
│  • market_sentiment     +  evidence_sentiment           │
│  • macro_news           +  evidence_news                │
│  • technical_indicators +  evidence_technical           │
│                                                         │
│  Computes:                                              │
│  • DataCoverageReport  → data_coverage                  │
│    (required/missing inputs from evidence_* presence)   │
│  • ProvenanceSummary   → provenance_summary             │
│    (providers: finnhub, fred, yfinance; timestamp)      │
└──────────────────────────┬──────────────────────────────┘
                           │
         ┌─────────────────┼──────────────┐
         ▼                                ▼
┌──────────────┐                 ┌───────────────┐
│  Bullish     │ ◄──── debate ── │  Bearish      │
│  Researcher  │ ──────────────► │  Researcher   │
└────────┬─────┘                 └────────┬──────┘
         └────────────────┬───────────────┘
                          ▼
                ┌──────────────────┐
                │ Debate Moderator │
                └────────┬─────────┘
                         │
                         ▼
          ┌─────────────────────────────────┐
          │      Trader → TradeProposal     │
          └────────────────┬────────────────┘
                           │
      ┌────────────────────┼────────────────────┐
      ▼                    ▼                    ▼
┌──────────┐        ┌──────────┐        ┌────────────┐
│Aggressive│◄─debate│ Neutral  │debate─►│Conservative│
│   Risk   │────────│   Risk   │────────│   Risk     │
└────┬─────┘        └─────┬────┘        └────┬───────┘
     └────────────────────┼──────────────────┘
                          ▼
                ┌──────────────────┐
                │  Risk Moderator  │
                └─────────┬────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────┐
│  Fund Manager  [Chunk 3: evidence context injected]     │
│  → Approve / Reject  (LLM judgment informed by all      │
│    three risk reports and dual-risk escalation status)  │
│  Snapshot Phase 5 persists here; Fund Manager remains   │
│    the business-final decision-maker.                   │
└──────────────────────────┬──────────────────────────────┘
                           │
                           ▼ (only when RuntimePolicy.auditor_enabled == true)
┌─────────────────────────────────────────────────────────┐
│  AuditorTask  [ADVISORY — fails open, never vetoes]     │
│                                                         │
│  • Curated AuditorInputView derived from final          │
│    TradingState (trader_proposal, execution_status,     │
│    analyst summaries, debate history, risk reports)     │
│  • Deterministic checks (BUY w/ target<current,         │
│    stop_loss>target_price, etc.) run locally first      │
│  • Quick-thinker LLM pass for semantic consistency,     │
│    unsourced numeric claims, valuation sanity bands     │
│  • Writes audit_status (Disabled/Pending/Passed/        │
│    Findings/FailedOpen) + audit_report on TradingState  │
│  • Any failure → AuditStatus::FailedOpen; the Fund      │
│    Manager decision still stands                        │
│  • No new snapshot phase: Phase 5 contract preserved    │
└──────────────────────────┬──────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────┐
│  Final Report  [EXTENDED — Chunk 4]                     │
│                                                         │
│  Existing sections:                                     │
│  • Analyst Evidence Snapshot                            │
│                                                         │
│  New sections (inserted after analyst snapshot):        │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Data Quality and Coverage        [NEW]           │   │
│  │  required_inputs: [fundamentals, sentiment, ...] │   │
│  │  missing_inputs:  [...]  or  Unavailable         │   │
│  └──────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Evidence Provenance              [NEW]           │   │
│  │  providers_used: [finnhub, fred, yfinance]       │   │
│  │  generated_at: 2026-04-05T...                    │   │
│  │  caveats: []  or  Unavailable                    │   │
│  └──────────────────────────────────────────────────┘   │
│                                                         │
│  Existing sections:                                     │
│  • Research Debate Summary                              │
│  • Risk Assessment                                      │
│  • Trade Decision                                       │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Auditor (advisory review)        [NEW]           │   │
│  │  Status: clean / attention needed / unavailable  │   │
│  │  Note: advisory only; Fund Manager is final.     │   │
│  │  Findings (top 5, Critical → Warning, sorted by  │   │
│  │    severity then location). Info findings hidden │   │
│  │    in terminal output for v1.                    │   │
│  │  Section is omitted entirely when                │   │
│  │    AuditStatus::Disabled.                        │   │
│  └──────────────────────────────────────────────────┘   │
│                                                         │
│  ETF-only sections                                      │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Dealer Positioning                               │   │
│  │  Near-term GEX (net/gross per 1% move)           │   │
│  │  Summary line (dampens/amplifies regime)         │   │
│  │  Gamma walls (top-3 strikes)                     │   │
│  │  Call/Put OI, Max-pain strike                    │   │
│  │  Broad GEX (Stage 3, all-expirations)            │   │
│  │  Secondary sensitivities VEX/CEX (Stage 3)       │   │
│  │  Risk-free rate source / degradation banner      │   │
│  │  Block hidden when options_gex is absent         │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

### Strongly Typed State Management

To circumvent the telephone effect, the `graph_flow::Context` will strictly regulate data exchange through a
meticulously defined, serializable Rust structure. When the system initiates an analysis cycle, a `TradingState` struct
is instantiated and injected into the context.

```rust
// Core State Definition
pub struct TradingState {
    pub execution_id: uuid::Uuid,
    pub asset_symbol: String,
    pub target_date: String,

    // Phase 1: Aggregated Analyst Data
    pub fundamental_metrics: Option<FundamentalData>,
    pub technical_indicators: Option<TechnicalData>,
    pub market_sentiment: Option<SentimentData>,
    pub macro_news: Option<NewsData>,

    // Phase 2: Dialectical Debate
    pub debate_history: Vec<rig::message::Message>,
    pub consensus_summary: Option<String>,

    // Phase 3 & 4: Synthesis and Risk
    pub trader_proposal: Option<TradeProposal>,
    pub risk_discussion_history: Vec<rig::message::Message>,
    pub aggressive_risk_report: Option<RiskReport>,
    pub neutral_risk_report: Option<RiskReport>,
    pub conservative_risk_report: Option<RiskReport>,

    // Phase 5: Final Execution
    pub final_execution_status: Option<ExecutionStatus>,

    // Phase 5+: Advisory post-decision audit (additive; #[serde(default)])
    pub audit_status: AuditStatus,
    pub audit_report: Option<AuditorReport>,

    // Thesis memory continuity across runs
    pub prior_thesis: Option<ThesisMemory>,
    pub current_thesis: Option<ThesisMemory>,

    // Evidence and Provenance (Stage 1 — dual-write alongside legacy fields above)
    pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
    pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
    pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
    pub evidence_news: Option<EvidenceRecord<NewsData>>,
    pub data_coverage: Option<DataCoverageReport>,
    pub provenance_summary: Option<ProvenanceSummary>,

    // Forward-looking catalyst calendar (Tier 1 + Tier 2; #[serde(default)])
    pub enrichment_catalysts: EnrichmentState<Vec<CatalystEvent>>,

    // ETF Phase 2 — risk-free rate for dealer-positioning math (#[serde(default)])
    pub etf_risk_free_rate: Option<f64>,
    pub etf_risk_free_rate_source: Option<EtfRiskFreeRateSource>,

    // Token Usage Tracking
    pub token_usage: TokenUsageTracker,
}

/// Persisted origin of the ETF risk-free-rate input.
pub enum EtfRiskFreeRateSource {
    FredDgs3Mo,   // FRED DGS3MO (3-month Treasury bill)
    YFinanceIrx,  // yfinance ^IRX (13-week Treasury bill) latest close
}

/// Advisory audit status produced by the post-decision `AuditorTask`.
/// Disabled is the default for runs where `RuntimePolicy.auditor_enabled == false`.
pub enum AuditStatus {
    Disabled,    // auditor stage not enabled for this run
    Pending,     // enabled but not yet executed (transient)
    Passed,      // auditor ran and produced zero findings
    Findings,    // auditor ran and attached one or more findings
    FailedOpen,  // auditor failed; Fund Manager decision still stands
}

pub enum Severity { Critical, Warning, Info }

pub struct Finding {
    pub severity: Severity,
    pub location: String,       // e.g. "trader_proposal.rationale"
    pub description: String,    // one-sentence issue description
    pub excerpt: Option<String>,// optional verbatim excerpt (≤512 chars)
}

pub struct AuditorReport {
    pub findings: Vec<Finding>,     // bounded to 20
    pub summary: String,            // ≤1024 chars
    pub audited_at: DateTime<Utc>,  // runtime-stamped, never trusted from model
    pub auditor_model_id: String,
}

/// Forward-looking catalyst event derived from the catalyst calendar providers.
/// Distinct from `EventNewsEvidence` (backward-looking news that already happened).
pub struct CatalystEvent {
    pub symbol: String,             // canonical ticker or "_MACRO" for FRED releases
    pub event_date: String,         // ISO-8601 YYYY-MM-DD
    pub category: CatalystCategory, // EarningsAndFinancial / CorporateEvents / IndustryEvents / MacroEvents
    pub impact: ImpactLevel,        // H / M / L
    pub headline: String,           // sanitized short label
    pub source_url: Option<String>, // canonical source (SEC primary doc, FRED release page)
    pub source: &'static str,       // "finnhub" | "fred" | "sec_edgar" | "yfinance"
}

pub enum CatalystCategory {
    EarningsAndFinancial,
    CorporateEvents,
    IndustryEvents,
    MacroEvents,
}

pub enum ImpactLevel { H, M, L }

/// Classifies the type of evidence attached to an `EvidenceRecord`.
pub enum EvidenceKind {
    Fundamental,
    Technical,
    Sentiment,
    News,
    Macro,
    Transcript,
    Estimates,
    Peers,
    Volatility,
}

/// Wraps analyst output `T` with provenance metadata and quality flags.
pub struct EvidenceRecord<T> {
    pub kind: EvidenceKind,
    pub payload: T,
    pub sources: Vec<EvidenceSource>,
    pub quality_flags: Vec<DataQualityFlag>,
}

/// Identifies a single data source used to produce evidence.
pub struct EvidenceSource {
    pub provider: String,        // e.g. "finnhub", "yfinance", "fred"
    pub dataset: String,         // e.g. "fundamentals", "ohlcv", "macro_indicators"
    pub fetched_at: String,      // RFC 3339 UTC
    pub effective_at: Option<String>,
    pub symbol: Option<String>,
    pub url: Option<String>,
    pub citation: Option<String>,
    pub freshness_hours: Option<u64>,
}

/// Quality flags attached to evidence records or coverage reports.
pub enum DataQualityFlag {
    Missing,
    Stale,
    Partial,
    Estimated,
    Conflicted,
    LowConfidence,
}

/// Summarizes which required analyst inputs were received vs. missing.
pub struct DataCoverageReport {
    pub required_inputs: Vec<String>,
    pub missing_inputs: Vec<String>,
    pub stale_inputs: Vec<String>,
    pub partial_inputs: Vec<String>,
}

/// Summarizes the provenance of all evidence used in the current run.
pub struct ProvenanceSummary {
    pub providers_used: Vec<String>,
    pub generated_at: String,     // RFC 3339 UTC
    pub caveats: Vec<String>,
}

/// Compact cross-run memory captured from the most recent completed thesis.
pub struct ThesisMemory {
    pub symbol: String,
    pub action: String,
    pub decision: String,
    pub rationale: String,
    pub summary: Option<String>,
    pub execution_id: String,
    pub target_date: String,
    pub captured_at: String,
}

/// Tracks token consumption per agent, per phase, and for the entire run.
pub struct TokenUsageTracker {
    pub phase_usage: Vec<PhaseTokenUsage>,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
}

pub struct PhaseTokenUsage {
    pub phase_name: String,           // e.g. "Analyst Team", "Researcher Debate Round 2"
    pub agent_usage: Vec<AgentTokenUsage>,
    pub phase_prompt_tokens: u64,
    pub phase_completion_tokens: u64,
    pub phase_total_tokens: u64,
    pub phase_duration_ms: u64,
}

pub struct AgentTokenUsage {
    pub agent_name: String,           // e.g. "Fundamental Analyst", "Bullish Researcher"
    pub model_id: String,             // e.g. "gpt-4o-mini", "o3"
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub latency_ms: u64,
}
```

By enforcing this structural schema, the Trader Agent does not need to parse a massive chat log to find the Gross
Margin; it directly accesses `context.fundamental_metrics.gross_margin`, radically reducing token consumption and
hallucination probabilities.

#### Thesis Memory Continuity

In addition to same-run state passing, the system maintains a lightweight thesis-memory channel across runs for the same
canonical symbol. `TradingState.prior_thesis` is populated during `PreflightTask` by loading the most recent compatible
phase-5 snapshot for that symbol within a bounded staleness window. `TradingState.current_thesis` is captured at the
end of the current run so it can be reused by future analyses. The payload is intentionally compact — action,
decision, rationale, summary, and capture metadata — so downstream prompts receive a concise historical thesis rather
than an entire prior chat transcript.

This memory is treated as **reference context only**, not as authoritative instruction. Prompt builders must frame it
as untrusted historical context to prevent positive-feedback loops where the system blindly repeats its own previous
conclusions. If no prior snapshot exists, if the snapshot is too old, or if schema evolution renders it incompatible,
the pipeline degrades gracefully and continues with `prior_thesis = None`.

#### Evidence Provenance and Dual-Write Transition

The `evidence_*` fields wrap the same analyst payload types (`FundamentalData`, `TechnicalData`, etc.) inside
`EvidenceRecord<T>`, adding provenance metadata (which provider, which dataset, when fetched) and quality flags. During
the initial rollout, both the legacy fields (e.g., `fundamental_metrics`) and the new evidence fields (e.g.,
`evidence_fundamental`) are populated — a "dual-write" strategy. Legacy fields remain for backward compatibility; newly
added readers (report sections, prompt context builders, downstream agents) consume the typed evidence fields as the
authoritative source. If legacy and new fields disagree, the typed evidence is authoritative and the discrepancy is
treated as a bug. The `DataCoverageReport` and `ProvenanceSummary` are derived deterministically by the
`AnalystSyncTask` from the presence or absence of evidence fields, not from legacy mirrors.

### Execution Workflow Topology Detailed

The execution topology dictates the chronological flow of the artificial intelligence firm. The `GraphBuilder` initiates
execution at the entry point and routes the `TradingState` through the necessary nodes.

1. **Preflight Validation (The PreflightTask)**: Before any analyst work begins, a `PreflightTask` validates and
   canonicalizes the input symbol via entity resolution (`crates/scorpio-core/src/data/entity.rs`), writes the canonical
   `ResolvedInstrument` to the workflow context, loads the most recent compatible thesis memory for that canonical
   symbol from a prior phase-5 snapshot, derives `ProviderCapabilities` from the `DataEnrichmentConfig`, writes
   baseline coverage expectations, and seeds enrichment cache keys with explicit `null` placeholders. If the symbol is
   invalid, the pipeline fails immediately rather than wasting LLM calls on a bad input. Missing, stale, or
   schema-incompatible thesis memory is a fail-open condition: the run continues with no prior thesis attached. This
   step also establishes the data-quality contract that downstream agents can reference.
2. **Parallel Data Ingestion (The Fan-Out Pattern)**: The workflow proceeds by utilizing a `FanOutTask`, a composite task
   provided by `graph-flow `that executes multiple child tasks simultaneously. The Fundamental, Sentiment, News, and
   Technical tasks are executed concurrently using `tokio::spawn`. Each task invokes the respective external application
   programming interface, performs its isolated reasoning using a quick-thinking LLM, and writes its specific data
   structure back to the `TradingState`.
3. **Dialectical Evaluation (The Cyclic Pattern)**: Following the synchronization of the Fan-Out task, the graph
   transitions to the Researcher Team. Here, `graph-flow`'s conditional edges are utilized to construct a loop. The
   graph alternates execution between the `BullishResearcher` and `BearishResearcher` tasks. A discrete
   `DebateModerator` task evaluates the number of completed iterations against a `max_debate_rounds` parameter (
   typically set to 2 or 3). Crucially, the Moderator acts as a "Reflective Agent" for the team: once the threshold is
   met, it explicitly reviews the debate history, selects the prevailing perspective, and records it as a structured
   `consensus_summary` before updating the `NextAction` to exit the loop, moving the state to the Trader Agent.
4. **Synthesis and Proposal**: The Trader Agent task operates sequentially, utilizing the complete `TradingState` to
   generate a formalized TradeProposal.
5. **Risk Review**: The risk assessment phase evaluates the `TradeProposal` through the Aggressive, Conservative, and
   Neutral risk agents, then synthesizes their outputs via the Risk Moderator before handing the discussion to the Fund
   Manager.
6. **Managerial Arbitration**: The Fund Manager node uses LLM judgment informed by a tri-state dual-risk escalation
   indicator derived from the Conservative and Neutral risk reports, while still reading all three risk reports to
   approve or reject the trade. Snapshot Phase 5 persists here, and the Fund Manager remains the business-final
   decision-maker. The graph terminates here unless `RuntimePolicy.auditor_enabled == true`.
7. **Advisory Post-Decision Audit (gated)**: When `auditor_enabled` is true on the resolved runtime policy, the graph
   continues into an advisory `AuditorTask` that re-checks the completed `TradingState` via a curated
   `AuditorInputView`. Deterministic checks run locally first; a quick-thinking LLM handles semantic consistency and
   unsourced numeric claims. Any failure transitions `audit_status` to `FailedOpen` while preserving deterministic
   findings; the auditor never alters `final_execution_status` and never adds a new snapshot phase.

## Agent Role Specifications and Implementation Directives

Each persona within the TradingAgents framework requires specific LLM backbone routing, precise system prompt
engineering, and distinct tool access. Following the original paper's architecture, all agents must operate using the *
*ReAct (Reasoning and Acting)** prompting framework, synergizing step-by-step reasoning with tool execution before
emitting their final structured schemas. The implementation will utilize a multi-provider factory pattern via `rig` to
ensure seamless task routing across a diverse suite of models, including OpenAI, Anthropic, Google Gemini,
OpenRouter, DeepSeek, and a custom GitHub Copilot integration.

### Dual-Tier Cognitive Routing

The framework implements a tiered approach to LLM inference to optimize both latency and operational expenditure. The
system will support a dynamic model picker allowing seamless execution across providers.

* **Quick-Thinking Models**: Tasks that involve simple data extraction, summarization, or formatting (e.g., converting
  JSON data into a readable technical summary) will utilize highly optimized, low-latency models such as `gpt-4o-mini`
  (the model used in the original paper), `claude-haiku`, or `gemini-flash`. The entire Analyst Team operates on this
  tier.
* **Deep-Thinking Models**: Tasks requiring multistep logical deduction, complex spatial reasoning, or strategic
  synthesis will utilize frontier reasoning models such as `o3` / `o4-mini`, `claude-opus`, Gemini advanced reasoning
  models, or GitHub Copilot. The original paper used `o1-preview` for this tier. The Researcher Team, Trader, and Risk
  Management Team operate exclusively on this tier to ensure maximum decision fidelity.

### Custom GitHub Copilot Integration via ACP and Rig

Because GitHub Copilot does not offer a public REST API for direct third-party orchestration, `rig` does not support it
natively out of the box. To fulfill the requirement of utilizing Copilot as a cognitive engine within the multi-agent
firm, the engineering team will implement a custom model provider within the `rig` ecosystem leveraging the official
Agent Client Protocol (ACP).

* **Rig Trait Implementation**: The team will create a custom struct representing the Copilot client that implements
  `rig`'s `ProviderClient`, `CompletionClient`, and `CompletionModel` traits. This strict trait boundary ensures the
  custom Copilot integration can seamlessly plug into the existing `rig::AgentBuilder` pipeline alongside native OpenAI
  or Gemini clients.

* **Transport Layer Execution via ACP**: To route requests to Copilot, the custom `CompletionModel` implementation will
  act as an ACP Client. It will spawn the GitHub Copilot CLI in ACP mode utilizing standard input/output streams via the
  command `copilot --acp --stdio`.
* `Protocol Lifecycle`: The Rust client will communicate using JSON-RPC 2.0 formatted over NDJSON streams. The execution
  flow within the custom `CompletionModel::completion` method will follow the ACP standard: establishing a
  `ClientSideConnection`, sending an `initialize` request to negotiate capabilities, creating a new session via
  `session/new`, dispatching the translated agent prompt via `session/prompt`, and handling the agent's response chunks
  before terminating the session gracefully. This mechanism provides an officially supported, secure, and local bridge
  to GitHub Copilot's reasoning engine directly within the Rust application.

### The Analyst Team Execution Specifications

The Analyst Team represents the sensory input layer of the framework. Each agent will be implemented as a `rig` Agent
equipped with specific tools generated via the `#[tool_macro]`.

#### 1. Fundamental Analyst Task

The Fundamental Analyst is responsible for evaluating issuer fundamentals when that analysis shape is applicable to the
target asset.

* **Tool Bindings**: This agent is granted access to tools bridging the `finnhub` crate endpoints (e.g. `company_profile`), 
  and uses `yfinance-rs` for Institutional/Insider net flows and full financial statements.
* **Execution Logic**: The agent fetches quarterly revenue growth, Price-to-Earnings (P/E) ratios, current liquidity
  ratios, and recent executive stock sales. The `rig` agent is prompted to evaluate these metrics against sector
  averages, identifying severe vulnerabilities such as high leverage in a rising interest rate environment or massive
  insider dumping. For ETF/fund-like instruments, many corporate metrics may be structurally absent; this is treated as
  domain-valid absence rather than data corruption, and later valuation layers may honestly emit `NotAssessed` instead
  of forcing corporate-equity valuation. The output is serialized directly into the `FundamentalData` structure.
* **Prompt specification**: [Fundamentals Analyst](docs/prompts.md#fundamentals-analyst)

#### 2. Sentiment Analyst Task

This agent quantifies company-specific sentiment and narrative shifts using recent news coverage rather than direct
social-platform ingestion in the MVP.

* **Tool Bindings**: Consumes the merged `NewsData` (Finnhub + yfinance, deduped) via the shared `GetCachedNews` tool
  path — the same cache the News Analyst reads, populated once in `prefetch_analyst_news` and handed to both analysts
  via `Arc<NewsData>` to avoid duplicate API calls. Analyst price targets and recommendations summary are available
  indirectly via the `ConsensusEvidence` enrichment rendered into the shared agent context, not fetched by Sentiment
  directly. Options data is scoped to the Technical Analyst (see §4 below) and does not flow into Sentiment in this
  iteration. If direct API access is unavailable or insufficient for the target company/news query, the Gemini CLI can
  be used as a fallback for web-search-based news retrieval.
* **Execution Logic**: The agent analyzes recent company-specific news to identify tone shifts, recurring themes,
  management or product narratives, and event-driven sentiment that could affect trading decisions. The goal is to
  aggregate news-driven sentiment into a normalized view of market perception over the past week. Direct Reddit and
  X/Twitter ingestion is intentionally deferred to future improvements.
* **Prompt specification**: [Sentiment Analyst](docs/prompts.md#sentiment-analyst)

#### 3. News Analyst Task

The News Analyst contextualizes the asset within the broader global macroeconomic environment.

* **Tool Bindings**: Accesses the merged company-specific news feed via `GetCachedNews` (populated by
  `prefetch_analyst_news` from both `FinnhubNewsProvider` and `YFinanceNewsProvider`, deduped by normalized URL with
  headline fallback, capped at `NEWS_MAX_ARTICLES`) and `finnhub` market news endpoints (`GetNews`, `GetMarketNews`)
  for broader breaking-news coverage. Macroeconomic indicators are sourced from the FRED API via the
  `GetEconomicIndicators` tool, which returns the latest Federal Funds Rate and CPI inflation data classified into
  `MacroEvent` structs with impact direction and confidence scores. The merged feed is fail-open at both the
  per-provider and combined level: if one provider fails the other's feed still reaches the analyst; if both fail the
  analyst sees the existing "news unavailable" marker and the pipeline continues. If direct API access is unavailable
  for all sources, the Gemini CLI can be used as an alternative for web-search-based news analysis.
* **Execution Logic**: The agent processes breaking news articles to extract causal relationships and interprets
  macroeconomic indicators from FRED to contextualize broader monetary policy impacts. For example, if analyzing a
  semiconductor equity, the agent is prompted to identify specific geopolitical tensions, tariff implementations, or
  federal reserve interest rate changes (sourced from the FEDFUNDS series) that directly impact the supply chain or
  discount rates.
* **Prompt specification**: [News Analyst](docs/prompts.md#news-analyst)

#### 4. Technical Analyst Task

The Technical Analyst identifies actionable entry and exit signals based entirely on historical price action and volume.

* **Tool Bindings**: Exposes `yfinance-rs` OHLCV retrieval and `kand` indicator calculation as callable tools bound
  to the `rig` agent. The LLM calls `get_ohlcv` at inference time to fetch historical candles, then calls
  `calculate_all_indicators` (or individual indicator tools such as `calculate_rsi`, `calculate_macd`,
  `calculate_atr`, `calculate_bollinger_bands`) on those candles. Before the LLM turn, `TechnicalAnalyst::run()`
  performs one scoped options prefetch via `YFinanceOptionsProvider`, storing the result in a write-once
  `OptionsToolContext`. A `get_options_snapshot` tool — wrapping `YFinanceOptionsProvider` behind the `OptionsProvider`
  trait and returning a compact `OptionsSnapshot` with ATM IV, IV term structure, put/call volume and OI ratios, max
  pain strike, 25-delta skew, and a near-the-money strike slice (nearest expiration, strikes within ±5% of spot) — is
  bound only when the prefetch succeeded; the tool reads from the pre-fetched context rather than making a second live
  call. On prefetch failure the tool is omitted and the prompt is rendered with a variant that omits all
  options-tool guidance and explicitly states the live options provider was unavailable for this run.
* **Execution Logic**: The LLM calls the OHLCV, indicator, and options tools during its reasoning pass, then
   interprets the `f64` statistical outputs — RSI overbought/oversold conditions (>70 / <30), MACD signal-line
   crossovers, ATR historical volatility, Bollinger Band support/resistance boundaries, and implied-volatility regime
   plus positioning skew from the options snapshot — producing a `TechnicalAnalystResponse` (the LLM output contract).
   After inference, `TechnicalAnalyst::run()` applies a merge step to construct the persisted `TechnicalData`:
   `options_context: Option<TechnicalOptionsContext>` is set from the pre-fetched result, carrying either the live
   `OptionsOutcome` or an explicit `FetchFailed { reason }` record; `options_summary` (the model-authored interpretation)
   is cleared if `options_context` does not carry a live snapshot, preventing hallucinated options analysis from reaching
   downstream agents. The `options_context` field flows to researchers, trader, risk managers, and fund manager via the
   serialized `technical_report`, providing Rust-owned structured options evidence alongside the model-authored
   `options_summary`. For **ETF runs**, the `AnalystSyncTask` extracts the live `OptionsSnapshot` from
   `TechnicalOptionsContext::Available { outcome: Snapshot(_) }` and threads it into `ValuationInputs.etf_options`
   so the ETF valuator can compute the `GexSummary` dealer-positioning overlay. The LLM does not perform the
   mathematical calculations; it invokes the tools and interprets the results. This MVP interpretation path is
   designed for traditional long-term investing workflows; crypto-native interpretation concerns such as logarithmic
   scaling, 24/7 market structure, and MVRV-style on-chain metrics are intentionally deferred beyond the MVP.
* **Prompt specification**: [Market / Technical Analyst](docs/prompts.md#market--technical-analyst)

### The Researcher Team: Dialectical Synthesis

The Researcher Team operates within the `graph-flow` cyclic loop, embodying a rigorous adversarial debate. This
dialectical process forces the "deep-thinking" models to thoroughly cross-examine the initial data, drastically reducing
the probability of confirmation bias.

* **Bullish Researcher**: Configured via a `rig` preamble to adopt a structurally optimistic persona. Its objective is
  to synthesize the data provided by the Analysts to formulate a compelling thesis for capital appreciation. It
  highlights robust cash flows, technical breakouts, and favorable market sentiment.
  — *Prompt specification*: [Bull Researcher](docs/prompts.md#bull-researcher)
* **Bearish Researcher**: Configured with a highly skeptical preamble. Its objective is to actively dismantle the
  Bullish Researcher's arguments. It searches the `TradingState` for counter-indicators, emphasizing insider selling,
  overextended P/E ratios, macroeconomic headwinds, and impending technical resistance levels.
  — *Prompt specification*: [Bear Researcher](docs/prompts.md#bear-researcher)
* **Debate Moderator**: Evaluates completed debate rounds, selects the prevailing perspective, and records a structured
  `consensus_summary` before routing the state to the Trader Agent.
  — *Prompt specification*: [Debate Moderator (Research Manager)](docs/prompts.md#debate-moderator-research-manager)

During each cycle, the `rig` chat history is updated, allowing each agent to directly address the specific claims made
by its counterpart in the previous iteration. This produces a highly nuanced, multi-dimensional evaluation of the asset
that a single unified prompt could never achieve.

### The Trader Agent

The Trader Agent acts as the central executive intelligence.

* **Execution Logic**: The Trader Task retrieves the full `TradingState`, including the multi-round debate history.
  Utilizing a deep-thinking model, it weighs the validity of the bullish catalysts against the bearish risks. It must
  output a strict `TradeProposal` JSON schema indicating the proposed action (Buy/Sell/Hold), a specific target price, a
  justified stop-loss threshold, and a confidence metric. This structured output ensures that downstream components
  receive a mathematically actionable directive rather than a vague natural language suggestion.
* **Prompt specification**: [Trader](docs/prompts.md#trader)

### The Risk Management Team

Capital preservation is prioritized over alpha generation. Per the original paper, the Risk Management Team mirrors
the structure of the Researcher Team: the three risk agents engage in multi-round natural language discussion guided
by a `RiskModerator`, rather than simply producing independent reports. The implementation will replicate this cyclic
debate pattern within the risk phase. They use `yfinance-rs` Corporate Calendar to flag earnings risk and Options Implied Volatility.

* **Risk-Seeking Agent** (mapped to "Aggressive" in this implementation): Evaluates whether the proposed stop-loss is
  too tight to survive normal market volatility, specifically referencing the Average True Range calculated by the
  Technical Analyst. It advocates for wider stops to capture massive momentum breakouts.
  — *Prompt specification*: [Aggressive Risk Analyst](docs/prompts.md#aggressive-risk-analyst)

* **Risk-Conservative Agent**: Evaluates the proposal entirely from the perspective of Maximum Drawdown. It actively
   vetoes trades if the asset exhibits overbought RSI conditions, severe macroeconomic uncertainty, or high beta relative
   to the broader market, demanding strict adherence to capital preservation. For leveraged/inverse ETFs
   (`leverage_factor` ≠ 1.0), the system prompt carries a leverage warning suffix with `{leverage_factor}` substitution
   (e.g., `3x`, `-2x`) sourced from `etf_leverage_warning.md`, separated by an explicit `---` divider.
   — *Prompt specification*: [Conservative Risk Analyst](docs/prompts.md#conservative-risk-analyst)

* **Neutral Risk Agent**: Functions as the moderating force, attempting to optimize the Sharpe Ratio by balancing the
   aggressive upside targets against the conservative downside protections. Like the Conservative agent, leveraged/inverse
   ETF runs receive a leverage warning suffix in the system prompt.
   — *Prompt specification*: [Neutral Risk Analyst](docs/prompts.md#neutral-risk-analyst)

A `RiskModerator` node coordinates the discussion loop, identical in structure to the `DebateModerator` in the
Researcher Team, and exits once consensus is reached or `max_risk_rounds` is exhausted. Acting as a reflective
summarizer, it ensures the aggregated discussion is clearly distilled and written to `risk_discussion_history` in the
`TradingState` for auditability.
— *Prompt specification*: [Risk Manager (Judge)](docs/prompts.md#risk-manager-judge)

### The Fund Manager

The Fund Manager is an LLM-powered agent (using the deep-thinking tier) that reviews the full risk discussion history
and the three `RiskReport` objects from the context, then determines the appropriate risk adjustments and renders a
final decision. This matches the paper's description where the Fund Manager "reviews the discussion" and "determines
appropriate risk adjustments." When both Conservative and Neutral risk reports flag a material violation, a tri-state
dual-risk escalation indicator (`present`) is surfaced to the LLM; the LLM must acknowledge it explicitly in the first
rationale line using the required prefix contract, enabling transparent override or deferral rather than a silent
automatic rejection. If
the Fund Manager approves the trade, it serializes the final order for dispatch to a brokerage API such as Alpaca; if
it rejects, it appends a structured rationale to `ExecutionStatus` for the audit trail.

### The Post-Decision Auditor (Advisory)

The `AuditorTask` is a gated, advisory stage that runs **after** the Fund Manager has rendered its decision and only
when `RuntimePolicy.auditor_enabled == true` (sourced from the active `AnalysisPackManifest`; default `false` on the
baseline equity pack). The Fund Manager remains the business-final decision-maker — the auditor never vetoes a
completed analysis and never alters `final_execution_status`.

* **Architectural contract**: The auditor conceptually re-checks the full final `TradingState`, but the implementation
  passes a curated `AuditorInputView` to the LLM so only semantically relevant, trust-labeled fields cross the LLM
  boundary. Untrusted free-text (debate transcripts, analyst summaries, rationale excerpts) is wrapped in structured
  labels so the system prompt's trust-boundary rule matches the real payload. Runtime metadata, config, secrets, token
  usage, and snapshot internals are explicitly omitted.

* **Two-layer review**: Deterministic checks run locally first (e.g., BUY with `target_price < current_price`,
   `stop_loss > target_price`, or other obvious ordering contradictions). The quick-thinker LLM tier then handles
   semantic consistency, sourcing of numeric claims, cross-phase contradictions, and bounded numeric heuristics such as
   valuation sanity-band warnings (terminal value share, WACC bands). For leveraged/inverse ETFs, the auditor prompt
   carries the same leverage warning suffix injected into the Conservative and Neutral risk prompts.

* **Output schema**: The auditor produces an `AuditorReport` containing up to 20 `Finding` entries
  (`severity` ∈ {Critical, Warning, Info}, `location`, `description`, optional `excerpt`) plus a one-paragraph
  `summary`. Runtime code — not the model — owns the `audited_at` timestamp and `auditor_model_id`. `AuditStatus`
  transitions across the run: `Disabled` (default), `Pending` (set as Fund Manager hands off when enabled), `Passed`
  (zero findings), `Findings` (one or more), `FailedOpen` (LLM/parse failure; deterministic findings are preserved and
  a stamped report explains that semantic review was unavailable).

* **Fail-open contract**: The auditor MUST NOT return `TaskExecutionFailed` for model failure, parse failure, or
  malformed output once Phase 5 has completed successfully. Deterministic findings survive the fail-open path. The
  persisted Phase-5 snapshot is not overwritten, and no new snapshot phase is introduced. Snapshot-backed
  `scorpio report show` output remains Phase-5-only in v1: pre-auditor Phase-5 snapshots scrub live-only auditor state
  from the public historical report surface so historical reports never present a misleading auditor result.

* **Rollout policy**: Ships behind a manifest/runtime-policy flag (`auditor_enabled`, default `false`). Dogfooded via
  fixture replay and temporary manifest flips before promotion. Success criteria: auditor failures never fail a
  completed run; replay/dogfood corpus shows <5% false-positive Critical findings; at least one real contradiction or
  unsourced claim caught during dogfooding; terminal copy is unambiguously advisory.

* **Out of scope for v1**: audit-driven veto (`--strict` mode that converts Critical findings into a process veto is a
  follow-up plan), cross-run auditor scoring, and auditing of intermediate phases (analyst phase or debate).

## Forward-Looking Catalyst Calendar

To replace the news-discovered-events-only degraded mode in the News Analyst's Theme G coverage, the system fetches a
real forward-looking catalyst calendar during preflight enrichment. The calendar runs **unconditionally for the equity
baseline pack** (no new enrichment flag) because the cost is bounded — one Finnhub earnings range call, one Finnhub IPO
range call, one FRED release-dates call per macro release ID shared across all symbols in a run, plus one yfinance
per-ticker calendar call and an optional SEC EDGAR submissions call. Three tiers ship independently:

* **Tier 1 — Structured APIs only**: `Tier1CatalystProvider` composes Finnhub `calendar().earnings(from,to,sym)` and
  `calendar().ipo(from,to)` (free-tier confirmed available — the paid `calendar/economic` and `misc().fda_calendar()`
  endpoints return 403 and MUST NOT be called), FRED `/fred/release/dates` for six high-impact release IDs (CPI, NFP,
  FOMC, GDP, ISM Manufacturing, Retail Sales), and yfinance `Ticker.calendar()` for ex-dividend dates. The composer
  uses `tokio::join!` (not `try_join!`) so one source failing zeros out only that source's contribution. Maps each row
  into `CatalystEvent`: Finnhub earnings → `EarningsAndFinancial`/H; Finnhub IPO → `CorporateEvents`/M (H for the
  analysed ticker); FRED releases → `_MACRO`/`MacroEvents` (CPI/NFP/FOMC = H, GDP/ISM/Retail = M); yfinance
  ex-dividend → `EarningsAndFinancial`/L.

* **Tier 2 — SEC EDGAR 8-K monitor**: `SecEdgar8kProvider` pulls recent 8-K item codes and 13D/G filings from the
  canonical `https://data.sec.gov/submissions/CIK<padded>.json` JSON endpoint (HTML index NOT scraped). Maps form/item
  pairs to categories and impact tiers — 8-K 1.01/2.01 (Material agreement / Acquisition disposition) → H, 8-K 2.02
  (Earnings results) → H, 8-K 5.07/7.01/8.01 (Shareholder vote / Reg FD / Other material) → M, SC 13D (Activist) → H,
  SC 13G (Passive) → M. Headlines stay generic ("Acquisition / disposition: <accession>"); the news analyst's existing
  news-fetch path can pull the body when it wants specifics. `Tier2CatalystProvider` composes Tier 1 + SEC EDGAR via
  `tokio::join!` with the same fail-soft semantics.

* **Tier 3 — Optional filing-body parsing** (deferred): S-1 lockup language → lockup expiry date, DEF M14A → expected
  close date, FDA AdComm scraping. Explicitly out of scope. The user-facing prompt continues to say `data not wired`
  for these specific subcategories until Tier 3 lands.

* **Failure-mode discipline**: The catalyst calendar is non-blocking enrichment — the pipeline always proceeds, even
  when every source fails. Per-source invariants: construct-time fallibility is permitted, but runtime
  `fetch_catalysts(...)` NEVER returns `Err` — every HTTP/JSON/timeout/rate-limit failure is converted to
  `Ok(Vec::new())` plus a `tracing::warn!` with `kind = "catalyst_fetch_failed"`. EnrichmentState semantics:
  `payload: None` = preflight skipped the fetch; `payload: Some(Vec::new())` = fetch ran but returned zero events
  (genuine quiet window or all-sources-failed, indistinguishable to the prompt by design); `payload: Some(events)` =
  at least one source returned events. SEC EDGAR construction failure falls back to Tier 1 (logged once at startup).
  A per-instance circuit breaker on `SecEdgarClient` short-circuits to `Ok(empty)` after five consecutive runtime
  failures within a single pipeline run.

* **Prompt rendering**: `build_catalyst_calendar_block(state)` renders the active catalyst window into the News
  Analyst's `{catalyst_calendar}` slot, sorted by relevance bucket (analysed ticker → macro `_MACRO` → unrelated IPO
  → other), then by `event_date` ascending within each bucket, capped at 25 lines. Each line is tagged `[H]`/`[M]`/
  `[L]` so the prompt's H/M/L tier rule has structured input. Empty payload renders as
  `(no upcoming catalysts in the next 30 days)` (genuine quiet window) or `(no upcoming catalysts: data unavailable)`
  (preflight skipped). The renderer never branches on per-source success — debug status lives in `tracing` only.

## Analytical Themes Port (Equity Baseline Pack)

The equity baseline pack adapts eight portable analytical frameworks from
[anthropics/financial-services](https://github.com/anthropics/financial-services) (Apache 2.0) into its prompt set.
These are quality-floor improvements (not a new user-selectable strategy mode) that tighten evidentiary discipline and
report structure for roles the baseline pack already runs:

| Theme | Frame                                              | Inserted into                                                                                            | Status                                                                         |
|-------|----------------------------------------------------|----------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
| A     | Valuation sanity bands (WACC, multiples, terminal) | `fundamental_analyst.md`, `conservative_risk.md`                                                         | ✅ Ship                                                                         |
| B     | Industry KPI matrix (sector-specific metrics)      | `fundamental_analyst.md`                                                                                 | ✅ Ship                                                                         |
| C     | Management-commentary red-flag taxonomy            | `news_analyst.md`, `sentiment_analyst.md`, `conservative_risk.md`                                        | ⚠️ Degraded mode (full power needs `TranscriptEvidence` provider)              |
| D     | Beat/miss decision tree (actual vs consensus)      | `news_analyst.md`, `trader.md`                                                                           | ⚠️ Gated on a prerequisite audit: same-period actual + consensus must be wired |
| E     | Falsifiable theses (pillars + thesis breakers)     | `bullish_researcher.md`, `bearish_researcher.md`, `debate_moderator.md`, `neutral_risk.md`               | ✅ Ship                                                                         |
| F     | "Contrarian needs a catalyst" rule                 | `bullish_researcher.md`, `aggressive_risk.md`                                                            | ✅ Ship                                                                         |
| G     | Catalyst taxonomy + H/M/L impact tier              | `news_analyst.md`                                                                                        | ✅ Wired via the Tier 1 catalyst calendar; Tier 3 gaps remain deferred          |
| H     | Sourcing hierarchy + injection defense             | `fundamental_analyst.md`, `news_analyst.md`, `sentiment_analyst.md`, `technical_analyst.md`, `trader.md` | ✅ Ship                                                                         |

Acceptance is verified by deterministic checks (prompt-bundle regression fixtures + targeted
`render_baseline_prompt_for_role(...)` string assertions) plus a single live smoke run per shipped batch. Themes C and
G ship in degraded mode with explicit user-visible caveats (`degraded mode: headline/summary only` and
`degraded mode: news-discovered events only`), each carrying a `<!-- TODO -->` seam comment for the future upgrade.
Theme E ships as prompt-steering only; structural runtime enforcement of pillar/breaker shape is explicitly out of
scope. `[UNSOURCED]` and degraded-mode disclosures are prompt-first requirements in this port — renderer/runtime
enforcement is a separate hardening follow-up.

Theme E's required output shape is consumed downstream: the Debate Moderator marks turns invalid when a pillar lacks
an evidence anchor or a thesis breaker lacks a measurable signal, names the surviving Bull and Bear pillars at the end
of debate, and ends the consensus summary with an explicit `Buy`, `Sell`, or `Hold` stance. The Neutral Risk agent
runs a falsifiability check against the surviving pillars.

## ETF Baseline Pack — Dealer-Positioning Overlay (Phase 2)

The ETF baseline pack extends the equity analysis pipeline with a secondary dealer-positioning overlay derived from
listed equity options chains. This overlay is designed as a **secondary signal** that sits beneath the primary ETF
anchors (premium/discount, composition, tracking) and adds a quantitative lens on how dealer hedging activity may
dampen or amplify near-term price moves.

### Architecture: BSM Greeks and Chain Aggregation

Dealer-positioning math lives in `crates/scorpio-core/src/indicators/gex.rs` — a pure, I/O-free module that computes
Black-Scholes-Merton gamma, vanna, and charm, then aggregates per-strike contributions across the options chain using
the SqueezeMetrics dealer convention (short calls, long puts). Degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`.
The aggregator emits:

- **Near-term GEX** — front-month expiration, per-strike + aggregate signed net and non-negative gross exposures
  denominated in USD per 1% spot move.
- **Broad GEX** (Stage 3) — all-expirations single-rate approximation combining the front-month slice with
  NTM slices for additional listed expirations.
- **VEX/CEX** (Stage 3) — secondary sensitivities to absolute IV moves (vanna-derived) and one calendar day of
  time decay (charm-derived).

The `compute_gex_summary` helper in the ETF valuator (`valuation/etf/premium_discount.rs`) maps the aggregator's
`AggregateResult` directly into the additive `GexSummary` state shape. Gamma walls (top-N strikes by `|net_gex|`)
are sorted and truncated to the top 3 for the durable state. The call/put OI ratio is inverted from the yfinance
put/call convention to the canonical call/put form.

### Risk-Free-Rate Sourcing

Dealer-positioning math requires a risk-free rate (`r`). The system enforces a strict no-hardcoded-fallback policy:

1. For **live/today ETF runs**, the `PreflightTask` fetches FRED `DGS3MO` (3-month Treasury bill rate). If FRED
   is unavailable, it falls back to the most recent yfinance `^IRX` (13-week Treasury bill) close.
2. If **both sources fail**, `etf_risk_free_rate` remains `None` and the downstream valuator degrades
   `options_gex` to `None` — the report honestly states dealer-positioning is unavailable.
3. **Historical runs** skip live-rate fetches entirely to preserve reproducibility; dealer-positioning degrades
   to unavailable.

The rate source (`FredDgs3Mo` or `YFinanceIrx`) is persisted on `TradingState` so `scorpio report` can render
the same source/degradation banner from reloaded snapshots.

### Leverage-Warning Injection

Leveraged and inverse ETFs (e.g., TQQQ, SQQQ, SPXU) carry structural decay risks that standard analysis may
understate. When `EtfValuation.leverage_factor` diverges from `1.0` beyond a `1e-6` tolerance, the system injects
a leverage warning into:

- **Conservative Risk** and **Neutral Risk** system prompts (renderer-side, after placeholder substitution)
- **Auditor** system prompt (same injection point)

The warning is sourced from `etf_leverage_warning.md` and carries a `{leverage_factor}` placeholder that is
substituted at runtime (e.g., `3x`, `-2x`, `1.5x`). An explicit `---` divider separates the base prompt from
the warning. The Aggressive Risk, Trader, and Fund Manager prompts are intentionally excluded — the warning is
a conservative/neutral concern, not a universal signal.

### State Schema Additions

The `GexSummary` struct on `EtfValuation` carries the following fields (all additive with `#[serde(default)]`,
no `THESIS_MEMORY_SCHEMA_VERSION` bump):

| Field                         | Type                 | Stage | Description                                          |
|:------------------------------|:---------------------|:------|:-----------------------------------------------------|
| `net_gex_usd_per_1pct_move`   | `f64`                | 1     | Signed net near-term GEX per 1% spot move            |
| `gross_gex_usd_per_1pct_move` | `f64`                | 1     | Non-negative gross near-term GEX                     |
| `call_put_oi_ratio`           | `f64`                | 1     | Call OI / put OI (inverted from yfinance convention) |
| `max_pain_strike`             | `f64`                | 1     | Max-pain strike from the options chain               |
| `near_term_expiration`        | `NaiveDate`          | 1     | Front-month expiration date                          |
| `strikes`                     | `Vec<StrikeGex>`     | 1     | Top-3 gamma walls by `                               |net_gex|` |
| `broad`                       | `Option<BroadGex>`   | 3     | All-expirations single-rate approximation            |
| `vex_summary`                 | `Option<VexSummary>` | 3     | Vanna-derived sensitivity to IV moves                |
| `cex_summary`                 | `Option<CexSummary>` | 3     | Charm-derived sensitivity to time decay              |

`TradingState` gains two additional additive fields:

| Field                       | Type                            | Description                                  |
|:----------------------------|:--------------------------------|:---------------------------------------------|
| `etf_risk_free_rate`        | `Option<f64>`                   | Decimal risk-free rate from FRED or yfinance |
| `etf_risk_free_rate_source` | `Option<EtfRiskFreeRateSource>` | Persisted origin (FRED/yfinance)             |

### Terminal Reporter: DEALER POSITIONING Block

When `options_gex` is populated, the terminal report renders a `DEALER POSITIONING` block after the tracking
section containing:

- **Summary line** — plain-English regime description (dampens/amplifies moves) plus gamma-wall strike range
- **Net/Gross GEX** per 1% spot move (signed USD, scaled to B/M/K)
- **Call/Put OI ratio** and **Max-pain strike**
- **Gamma walls** — top-3 strikes with signed net GEX
- **Partial-data note** — when walls or broad GEX are unavailable
- **Secondary sensitivities** (Stage 3) — VEX/CEX net/gross exposures
- **All expirations** (Stage 3) — broad GEX with expiration coverage label

When `options_gex` is absent, the block is hidden and a one-line warning appears in the data-availability
section. A risk-free-rate source/degradation banner renders under the Analysis Pack header.

### Prompt Integration

The ETF technical analyst prompt (`etf_tracking_options_focus.md`) discusses dealer-positioning as a secondary
overlay on top of premium/discount, composition, and tracking evidence. It uses a single generic absence branch:
if no usable derived dealer-positioning overlay is available, it says so and anchors the rest of the analysis on
the primary ETF signals. Split no-snapshot vs unusable-snapshot copy is deferred until an explicit derivation
status field exists.

### Stage Gating

Phase 2 is delivered in three stages:

- **Stage 1** — Near-term GEX core math + state plumbing (BSM helpers, per-strike aggregation, `GexSummary`
  schema, `compute_gex_summary` mapping, `AnalystSyncTask` hydration, roundtrip tests).
- **Stage 2** — Surfaced validation slice (leverage-warning injection, technical-prompt rewrite, terminal
  `DEALER POSITIONING` block, live risk-free-rate sourcing with FRED `DGS3MO` + yfinance `^IRX` fallback,
  prompt-bundle regression-gate refresh). **Stage 2 is the go/no-go gate for Stage 3.**
- **Stage 3** — Contingent context expansion (broad GEX, VEX/CEX surfacing, `OptionsSnapshot.all_expirations`
  transient field, reporter Stage 3 expansion, live smoke tests). Requires explicit user approval after Stage 2
  validation.

## User Interaction Interface

The original TradingAgents research framework operates as a headless batch process, lacking any user-facing interaction
layer. For scorpio-analyst to function as a practical portfolio management tool, the system must provide intuitive
interfaces through which users can configure analyses, trigger trade cycles, monitor agent deliberations in real time,
and review historical decision rationale. The interaction layer is delivered in three sequential phases to balance rapid
utility with progressive user experience refinement.

### Phase 1: Command-Line Interface (MVP)

The initial release exposes all system functionality through a structured command-line interface built with the `clap`
crate. The CLI serves as the primary user touchpoint during early development, providing full access to the trading
pipeline without requiring graphical dependencies. This approach enables rapid iteration, scriptable automation, and
seamless integration with CI/CD and cron-based scheduling workflows.

#### Core Commands

The CLI supports both structured subcommands and natural language queries. Structured subcommands follow modern Rust CLI
conventions for deterministic, scriptable usage:

```bash
# Trigger a full analysis cycle for a specific asset
scorpio-analyst analyze --symbol AAPL --date 2024-11-15

# Run analysis with custom model configuration
scorpio-analyst analyze --symbol NVDA --analyst-model gpt-4o-mini --researcher-model o3

# Run backtesting over a historical window
scorpio-analyst backtest --symbol AAPL --start 2024-06-01 --end 2024-11-30

# Display the current configuration (redacting API keys)
scorpio-analyst config show

# Validate API connectivity and model availability
scorpio-analyst config check

# View the most recent trade decision and its full audit trail
scorpio-analyst history --last 1 --verbose
```

#### Natural Language Queries

In addition to structured subcommands, the CLI accepts natural language queries via the `ask` subcommand, enabling
users to interact with the trading agent team conversationally from the very first release:

```bash
# Natural language analysis requests
scorpio-analyst ask "Analyze AAPL for today"
scorpio-analyst ask "What's the risk profile for NVDA?"
scorpio-analyst ask "Run a backtest on MSFT for the last 6 months"
scorpio-analyst ask "Show me the last 3 trade decisions"
```

The `ask` command routes the user's natural language input through a lightweight LLM intent parser that maps the query
to the appropriate pipeline action (analyze, backtest, history retrieval, etc.), extracts parameters (symbol, date
ranges, model overrides), and dispatches execution through the same code paths as the structured subcommands. This
design lowers the barrier to entry — users do not need to memorize flags or subcommand syntax — while preserving the
deterministic structured subcommands for scripting and automation. The intent parser uses the quick-thinking LLM tier
to minimize latency overhead.

#### Output Formatting

The CLI supports multiple output formats to accommodate both human operators and downstream tooling:

* **Human-readable** (default): Richly formatted terminal output using the `colored` or `comfy-table` crate, displaying
   agent phase transitions, debate summaries, final trade proposals with color-coded risk indicators, and a post-run
   statistics summary showing per-phase and per-agent token usage and latency. The final report includes two additional
   sections after the analyst evidence snapshot: **Data Quality and Coverage** (listing required and missing analyst
   inputs) and **Evidence Provenance** (listing the data providers used and any caveats). If the backing state fields
   are absent, these sections render the fallback string "Unavailable" rather than omitting the section. For ETF runs
   with a populated `options_gex`, a **Dealer Positioning** block renders after the tracking section with near-term GEX
   summary, gamma walls, and (Stage 3) secondary sensitivities and broad GEX. A risk-free-rate source/degradation banner
   appears under the Analysis Pack header.
* **JSON** (`--output json`): Machine-readable structured output mirroring the serialized `TradingState` (including the
  full `TokenUsageTracker`, `DataCoverageReport`, and `ProvenanceSummary`), enabling piping into `jq`, logging
  infrastructure, or external dashboards.
* **Quiet mode** (`--quiet`): Suppresses intermediate agent output, emitting only the final `TradeProposal`,
  `ExecutionStatus`, and the run statistics summary — designed for cron jobs and scripted pipelines.

#### Real-Time Streaming

During an active analysis cycle, the CLI streams agent progress to the terminal in real time. Each phase transition,
tool invocation, and debate round is emitted as a structured log line via the `tracing` subscriber, allowing the user
to observe the multi-agent deliberation as it unfolds. An optional `--no-stream` flag disables real-time output for
batch execution contexts.

### Phase 2: Interactive Terminal UI

The second phase introduces a rich interactive terminal user interface, inspired by conversational developer tools like
Claude Code. Rather than a fire-and-forget CLI invocation, Phase 2 transforms scorpio-analyst into a persistent,
conversational terminal application where users interact with the trading agent team through a full-screen terminal
interface built with the `ratatui` and `crossterm` crates.

#### Interaction Model

The interactive TUI operates as a long-running session within the terminal. Upon launch via
`scorpio-analyst interactive`
(or simply `scorpio-analyst` when no subcommand is provided), the user enters a conversational loop where they can:

* **Conversational natural language interaction**: Building on the `ask` command introduced in Phase 1, the TUI
  elevates natural language queries into a persistent, multi-turn conversational experience. Users can issue follow-up
  questions, refine analysis parameters, and chain queries without restarting — e.g., "Analyze AAPL" followed by
  "Now compare it with NVDA" or "Tighten the stop-loss to 2%".
* **Monitor live agent activity**: A dedicated panel renders the real-time execution of each agent phase. Users observe
  the Analyst Team's data retrieval, the Bullish/Bearish debate rounds, the Trader's proposal formulation, and the Risk
  Team's deliberation — all streaming within styled terminal panels with progress indicators and spinners.
* **Review and approve decisions**: When the pipeline produces a `TradeProposal`, the TUI presents it inline with
  syntax-highlighted details (action, target price, stop-loss, confidence). The user can approve, reject, or request
  additional analysis rounds interactively — without restarting the process.
* **Browse history**: Navigate past trade cycles using keyboard shortcuts, with scrollable panels displaying the full
  `TradingState` audit trail for each historical decision.

#### Architecture

The TUI layer is a thin presentation shell over the same core library (`lib.rs`) used by the Phase 1 CLI. It subscribes
to the `tracing` event stream and `graph_flow::Context` state updates, rendering them into `ratatui` widgets. The
application uses `crossterm` as the terminal backend for cross-platform compatibility (macOS, Linux, Windows). An
event loop driven by `tokio::select!` concurrently processes user keyboard input and async agent pipeline events,
ensuring the interface remains responsive during long-running analysis cycles.

#### Phase 2 Dependencies

| Crate       | Purpose                                                               |
|:------------|:----------------------------------------------------------------------|
| `ratatui`   | Terminal UI framework for rendering widgets, layouts, and styled text |
| `crossterm` | Cross-platform terminal manipulation (raw mode, input events, colors) |

### Phase 3: Native Desktop Application (GPUI)

The third phase introduces a high-performance native desktop application built with
[GPUI](https://www.gpui.rs/) — the GPU-accelerated UI framework created by the [Zed](https://zed.dev) team, written
entirely in Rust. GPUI is selected specifically for its zero-compromise alignment with the project's Rust-native
philosophy: it compiles directly into the application binary, eliminating Electron-style runtime overhead, and
delivers 120fps rendering through direct GPU composition. Because GPUI is a Rust crate, it integrates seamlessly with
the existing `tokio` async runtime, `rig` agent infrastructure, and `graph-flow` state machine without requiring
foreign function interfaces or inter-process communication bridges.

#### Architectural Integration

The GPUI application layer will be structured as an optional Cargo feature (`--features gui`), keeping the CLI and TUI
as the default build targets. The GUI shares the identical core library (`lib.rs`), ensuring complete behavioral parity.
The GPUI layer consumes the same `graph_flow::Context` state and `tracing` event streams, translating them into reactive
UI updates via GPUI's retained-mode component model.

#### Planned Interface Capabilities

The desktop application will provide the following capabilities upon Phase 3 completion:

1. **Live Workflow Dashboard**: A real-time visualization of the 5-phase execution graph. Each agent node displays its
   current status (idle, executing, completed, failed), with animated transitions as the `graph-flow` state machine
   progresses. The debate rounds between Bullish and Bearish researchers are rendered as a side-by-side conversational
   panel, enabling the user to observe dialectical synthesis as it occurs.

2. **Asset Configuration Panel**: A form-driven interface for selecting target assets, configuring model tiers (quick-
   thinking vs. deep-thinking), adjusting debate round limits, and setting risk tolerance parameters — replacing the
   CLI flags and `config.toml` with an interactive settings surface.

3. **Trade Proposal Review**: Upon cycle completion, the `TradeProposal` and aggregated `RiskReport` objects are
   rendered in a structured card layout, presenting the proposed action (Buy/Sell/Hold), target price, stop-loss
   threshold, confidence metric, and the dissenting risk arguments. The user can approve, reject, or request additional
   analysis rounds before execution.

4. **Historical Audit Trail**: A searchable, filterable timeline of all past trade cycles, displaying the complete
   `TradingState` snapshot for each decision. Users can drill into any historical cycle to review the exact analyst
   data, debate arguments, and risk assessments that informed the final decision, supporting regulatory compliance
   and strategy refinement.

5. **Performance Analytics**: Interactive charts rendering backtesting results — Cumulative Return, Annualized Return,
   Sharpe Ratio, and Maximum Drawdown — plotted against baseline strategies (Buy & Hold, SMA, MACD). These
   visualizations leverage GPUI's GPU-accelerated rendering for smooth pan, zoom, and hover interactions across large
   historical datasets.

#### Phase 3 Dependencies

The GPUI integration introduces the following additional crate dependencies:

| Crate  | Purpose                                        |
|:-------|:-----------------------------------------------|
| `gpui` | GPU-accelerated native UI framework (from Zed) |

The `gpui` crate is added behind the `gui` feature flag to prevent the desktop application's GPU and windowing
dependencies from affecting the headless CLI and TUI builds.

## Non-Functional Requirements and Enterprise Operations

Reimplementing TradingAgents in Rust introduces several critical operational mandates that ensure the framework meets
enterprise reliability standards.

### Concurrency and Thread Safety

The application relies heavily on the `tokio` asynchronous runtime. Because network input/output operations (such as
waiting for an LLM API response or fetching `Finnhub` data) account for the majority of execution time, blocking threads
is unacceptable. All `rig` API calls and `graph-flow` tasks must utilize asynchronous await syntax.

Rust's `Send + Sync` requirements enforced by `graph-flow`'s `Context` prevent data races, but additional care is
required to avoid logical concurrency issues:

* **Per-field locking**: The `TradingState` fields written by concurrent Fan-Out tasks (e.g., `fundamental_metrics`,
  `technical_indicators`) must use `Arc<RwLock<Option<T>>>` per field rather than a single lock on the entire struct,
  to prevent the Fan-Out from serializing into a bottleneck.
* **No Mutex across `.await` points**: `std::sync::Mutex` must never be held across an `.await` boundary. Use
  `tokio::sync::RwLock` exclusively for any lock that spans async operations.
* **`Send + Sync` is necessary but not sufficient**: It prevents memory-level data races but does not prevent logical
  races (e.g., two tasks reading the same value and making conflicting decisions). Sequential phase transitions
  enforced by the `graph-flow` topology naturally mitigate this for inter-phase data.

### Error Handling and Resilience

Financial data streams and third-party LLM providers are inherently volatile. Rate limits, timeouts, and malformed JSON
responses must be handled gracefully. The framework will utilize the `anyhow` crate for flexible context propagation and
`thiserror` for explicitly typed domain errors:

```rust
#[derive(thiserror::Error, Debug)]
pub enum TradingError {
    #[error("Analyst execution failed: {agent}")]
    AnalystError { agent: String, source: anyhow::Error },
    #[error("API rate limit exceeded on {provider}")]
    RateLimitExceeded { provider: String },
    #[error("Network timeout after {retries} retries")]
    NetworkTimeout { retries: usize },
    #[error("LLM returned invalid schema: {0}")]
    SchemaViolation(String),
    #[error(transparent)]
    Rig(#[from] rig::error::RigError),
}
```

If a specific LLM invocation fails or returns a schema violation, the `rig` agent will implement a localized retry
mechanism with exponential backoff (max 3 retries, base delay 500ms). Fan-Out failures follow a graceful degradation
policy:

* If one analyst fails, the cycle continues with the available data; the researcher prompt notes the missing input.
* If two or more analysts fail, the entire cycle aborts with a structured `TradingError` rather than a panic.
* A per-analyst timeout of 30 seconds (configurable) is enforced via `tokio::time::timeout`.

If the failure is unrecoverable, the `graph-flow` task returns an error variant, triggering a deterministic rollback of
the state machine.

### Configuration Management

The framework requires a layered configuration system to manage API keys, model selection, and operational parameters
without hardcoding sensitive values.

```rust
#[derive(serde::Deserialize)]
pub struct Config {
    pub llm: LLMConfig,
    pub trading: TradingConfig,
    pub apis: ApiConfig,
    #[serde(default)]
    pub enrichment: DataEnrichmentConfig,
}

#[derive(serde::Deserialize)]
pub struct LLMConfig {
    pub analyst_model: String,       // e.g. "gpt-4o-mini"
    pub researcher_model: String,    // e.g. "o3"
    pub max_debate_rounds: u8,       // default: 3
    pub max_risk_rounds: u8,         // default: 2
    pub analyst_timeout_secs: u64,   // default: 30
}

/// Controls optional enrichment data sources. All flags default to `false`,
/// so the system operates with only the four baseline analyst inputs until
/// concrete enrichment providers are implemented. In the active roadmap,
/// event news is the first concrete target, consensus estimates are
/// conditional on free-tier provider verification, and transcripts remain
/// deferred. The forward-looking catalyst calendar is intentionally NOT
/// gated by a config flag — it runs unconditionally for the equity baseline
/// pack because per-run cost is bounded (shared range calls across the
/// run).
#[derive(serde::Deserialize)]
pub struct DataEnrichmentConfig {
    pub enable_transcripts: bool,           // default: false
    pub enable_consensus_estimates: bool,   // default: false
    pub enable_event_news: bool,            // default: false
    pub max_evidence_age_hours: u64,        // default: 48
}

/// Per-pack manifest flag gating the advisory post-decision auditor stage.
/// Default `false` on the baseline equity pack; promoted only after dogfooding
/// shows acceptable false-positive Critical rate and clearly advisory copy.
/// When `true`, preflight requires a populated `PromptBundle.auditor` slot,
/// builds run topology with `auditor_enabled = true`, and seeds
/// `RoutingFlags.skip_auditor = false` so the graph continues into the
/// `AuditorTask` after `FundManagerTask`.
pub struct AnalysisPackManifest {
    // ... existing fields ...
    pub auditor_enabled: bool,              // default: false
}
```

Loading strategy (highest priority last):

1. `config.toml` — non-sensitive defaults checked into the repository
2. `.env` file via `dotenvy` — local secrets, git-ignored
3. Environment variables — CI/CD and production overrides

API keys must be wrapped in `secrecy::SecretString` to ensure they are zeroed from memory on drop and never appear in
`Debug` output or log traces.

Enrichment configuration follows the same layered strategy. The `config.toml` provides safe defaults (all enrichment
disabled), and environment variables use the `SCORPIO__ENRICHMENT__` prefix (e.g.,
`SCORPIO__ENRICHMENT__ENABLE_TRANSCRIPTS=true`). The `PreflightTask` derives `ProviderCapabilities` from these config
fields at runtime.

### Rate Limiting

Multiple concurrent agents hitting the same APIs require coordinated rate limiting. A global rate limiter using the
`governor` crate will be instantiated at startup and passed via `Arc` into all agent tasks:

```rust
let finnhub_limiter: Arc<DefaultDirectRateLimiter> = Arc::new(
RateLimiter::direct(Quota::per_second(nonzero!(30u32)))
);
// In each task:
finnhub_limiter.until_ready().await;
let result = finnhub_client.fetch(...).await?;
```

This ensures that the four concurrent Analyst agents cannot collectively exceed the Finnhub free-tier limit of 30
requests per second, preventing 429 errors that would trigger costly retries. A separate FRED rate limiter (default 2
requests per second, configurable via `rate_limits.fred_rps`) gates the News Analyst's macroeconomic indicator fetches.

### Observability and Explainable AI

The primary advantage of an agentic trading system is the preservation of the analytical rationale behind every capital
allocation. The Rust implementation will integrate the `tracing` and `tracing-subscriber` crates to emit structured logs
for every state transition, tool call, and LLM prompt hook. By persisting the complete `TradingState` across sessions
via`graph-flow`'s storage backend (e.g., PostgreSQL JSONB), quantitative researchers can historically audit the exact
sequence of debate arguments and risk assessments that led to a specific trade, ensuring total regulatory compliance and
facilitating continuous framework optimization. The same snapshot trail also enables thesis memory: future runs can load
the last compatible same-symbol thesis as bounded historical context, while schema-version checks and fail-open
deserialization rules ensure stale or incompatible memories never block execution.

### Token Usage Tracking and Run Statistics

Every LLM invocation throughout the pipeline must record its token consumption. The `rig` completion response includes
prompt and completion token counts; each agent task wrapper extracts these values and appends them to the
`TokenUsageTracker` in the `TradingState` immediately after each call returns. This tracking is mandatory — no LLM
call may bypass the accounting layer.

#### Per-Step Tracking

Each phase of the execution graph records its own `PhaseTokenUsage` entry:

| Phase                                                    | Agents Tracked                                                                                         |
|:---------------------------------------------------------|:-------------------------------------------------------------------------------------------------------|
| Phase 1: Analyst Team                                    | Fundamental, Sentiment, News, Technical (each individually)                                            |
| Phase 2: Researcher Debate                               | Bullish Researcher, Bearish Researcher (per round), Debate Moderator                                   |
| Phase 3: Trader Synthesis                                | Trader Agent                                                                                           |
| Phase 4: Risk Discussion                                 | Aggressive, Conservative, Neutral (per round), Risk Moderator                                          |
| Phase 5: Fund Manager                                    | Fund Manager                                                                                           |
| Phase 5+: Auditor (advisory, gated on `auditor_enabled`) | Auditor (quick-thinking tier; attributed to quick-thinking model usage in the token-summary breakdown) |

Within each phase, individual `AgentTokenUsage` entries capture the model used, prompt/completion token counts, and
wall-clock latency for that specific invocation. For cyclic phases (debate and risk rounds), each round produces a
separate `PhaseTokenUsage` entry (e.g., "Researcher Debate Round 1", "Researcher Debate Round 2"), enabling granular
analysis of token cost per debate iteration.

#### Post-Run Statistics Display

After every completed analysis cycle, the system **must** emit a comprehensive run statistics summary. This summary is
displayed regardless of output format (human-readable, JSON, or quiet mode — in quiet mode it is appended after the
final `TradeProposal`).

The statistics report includes:

1. **Phase-by-phase token breakdown**: A table showing prompt tokens, completion tokens, total tokens, and wall-clock
   duration for each phase.
2. **Agent-level detail**: Within each phase, a nested breakdown per agent showing model used, token counts, and
   latency.
3. **Run totals**: Aggregate prompt tokens, completion tokens, total tokens, total LLM calls, and total wall-clock
   time for the entire pipeline.

Example human-readable output:

```
══════════════════════════════════════════════════════════════════════════════════════
                    Run Statistics — AAPL 2024-11-15
══════════════════════════════════════════════════════════════════════════════════════

Phase                          Prompt    Completion    Total     Duration
──────────────────────────────────────────────────────────────────────────────────────
Phase 1: Analyst Team           8,420       3,210    11,630      4.2s
  ├─ Fundamental Analyst        2,100         830     2,930      1.1s  (gpt-4o-mini)
  ├─ Sentiment Analyst          2,340         790     3,130      1.3s  (gpt-4o-mini)
  ├─ News Analyst               1,980         810     2,790      1.0s  (gpt-4o-mini)
  └─ Technical Analyst          2,000         780     2,780      0.8s  (gpt-4o-mini)
Phase 2: Researcher Debate     12,600       5,800    18,400      8.1s
  ├─ Round 1                    6,200       2,900     9,100      4.0s
  │   ├─ Bullish Researcher     3,100       1,500     4,600      2.1s  (o3)
  │   └─ Bearish Researcher     3,100       1,400     4,500      1.9s  (o3)
  ├─ Round 2                    6,400       2,900     9,300      4.1s
  │   ├─ Bullish Researcher     3,200       1,500     4,700      2.1s  (o3)
  │   └─ Bearish Researcher     3,200       1,400     4,600      2.0s  (o3)
  └─ Debate Moderator           1,800         600     2,400      0.9s  (o3)
Phase 3: Trader Synthesis       4,200       1,800     6,000      3.2s  (o3)
Phase 4: Risk Discussion        9,800       3,600    13,400      6.5s
  ├─ Round 1                    4,900       1,800     6,700      3.2s
  │   ├─ Aggressive Risk        1,600         600     2,200      1.0s  (o3)
  │   ├─ Conservative Risk      1,700         600     2,300      1.1s  (o3)
  │   └─ Neutral Risk           1,600         600     2,200      1.1s  (o3)
  └─ Risk Moderator             1,500         500     2,000      0.8s  (o3)
Phase 5: Fund Manager           3,200       1,200     4,400      2.1s  (o3)
Phase 5+: Auditor (advisory)    1,800         400     2,200      1.3s  (gpt-4o-mini)
──────────────────────────────────────────────────────────────────────────────────────
TOTAL                          40,020      16,010    56,030     25.4s
LLM calls:                     15
══════════════════════════════════════════════════════════════════════════════════════
```

The `Phase 5+: Auditor (advisory)` row is omitted entirely when `auditor_enabled = false` (the default on the baseline
equity pack).

In JSON output mode (`--output json`), the `token_usage` field is included as a top-level key in the serialized
`TradingState`, containing the full `TokenUsageTracker` structure.

In the Phase 2 interactive TUI, the statistics panel is rendered as a persistent sidebar widget that updates in real
time as each agent completes, showing a live running total. In the Phase 3 GPUI desktop application, the statistics
are displayed in a dedicated "Run Metrics" card within the workflow dashboard.

### Testing Strategy

The framework requires three distinct test layers to validate correctness, integration, and financial performance.

1. **Unit Tests**: Each agent task is tested in isolation with mocked API responses using the `mockall` crate.
   Assertions verify that the correct `TradingState` fields are populated with properly deserialized structs.
   ```rust
   #[tokio::test]
   async fn test_fundamental_analyst_populates_state() { /* ... */ }
   ```

2. **Integration Tests**: The full `graph-flow` workflow is executed end-to-end with all external APIs replaced by
   deterministic stubs. This validates phase transitions, the debate cycle termination, and the risk moderation loop
   without incurring API costs.

3. **Backtesting Framework**: The system ingests historical OHLCV data from `yfinance-rs` for the June–November 2024
   window and replays analyst decisions day-by-day, ensuring no look-ahead bias (agents only access data up to the
   target date). The backtesting harness computes Cumulative Return, Annualized Return, Sharpe Ratio, and Maximum
   Drawdown to validate parity with the paper's benchmark results. LLM calls during backtests use a cached response
   layer to ensure determinism and cost control.

4. **Property-Based Tests**: The `proptest` crate validates that the `TradingState` serialization round-trips
   correctly under arbitrary inputs and that the `TradingError` hierarchy handles all edge cases.

## Conclusion

The Rust-native reimplementation of the TradingAgents framework represents a critical evolution from a highly effective
academic prototype to an enterprise-grade financial operating system. By faithfully preserving the specialized
organizational taxonomy of analysts, dialectical researchers, and rigorous risk managers, the system retains the complex
cognitive capabilities required to navigate unstructured financial markets. Concurrently, by migrating the orchestration
layer to the `graph-flow` state machine and the `rig` LLM abstraction library, the architecture permanently eliminates
the performance bottlenecks, concurrency limitations, and state degradation issues inherent to Python-based artificial
intelligence stacks. The resulting framework achieves deterministic execution, mathematical stability via pure Rust
technical indicator libraries, and absolute transparent explainability, positioning it at the forefront of autonomous
quantitative trading infrastructure.

## Reference

- TradingAgents: Multi-Agents LLM Financial Trading Framework (https://arxiv.org/pdf/2412.20138)
- TauricResearch/TradingAgents (https://github.com/TauricResearch/TradingAgents/)
- Anthropic Financial Services Plugins (https://github.com/anthropics/financial-services-plugins) — evidence discipline, provenance reporting, and modular financial workflow patterns
