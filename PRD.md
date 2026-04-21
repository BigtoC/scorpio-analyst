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

#### Selected Provider: `rig-core` (v0.31.0)

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

#### Selected Orchestrator: `graph-flow` (v0.4.0)

`graph-flow` is a high-performance, type-safe framework explicitly designed to bring LangGraph-inspired stateful
execution to the Rust ecosystem. It treats the primary workflow as a directed graph, where each execution node
implements an asynchronous `Task` trait. The framework features a centralized `Context` object that provides thread-safe
state sharing across the workflow, allowing data aggregated by the Analyst Team to persist through the debate and
execution phases. Enable the optional `"rig"` feature flag (`graph-flow = { version = "0.2", features = ["rig"] }`) for
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
   - Options Chains (for IV and Put/Call ratio), 
   - full Financial Statements (Cashflow, Balance Sheet, Income Statement, Shares) for DCF and EV/EBITDA valuation math, 
   - Analyst Estimates (Forward EPS/Revenue, Upgrades/Downgrades, Price Targets), 
   - Institutional/Insider ownership (including Net Insider Shares Bought/Sold), 
   - Corporate Calendar (Earnings dates). It DOES NOT provide ESG data (the `sustainability()` endpoint is broken).

3. **Macroeconomic Indicators**: The FRED (Federal Reserve Economic Data) API provides authoritative macroeconomic
   time-series data, replacing the paid Finnhub `economic().data()` endpoint for interest-rate and inflation indicators.
   The custom `FredClient` implementation fetches the latest observations from two key series: the Federal Funds
   Effective Rate (`FEDFUNDS`) and the Consumer Price Index growth rate (`CPALTT01USM657N`). Both series are fetched
   concurrently and classified into `MacroEvent` structs with impact direction and confidence scores. The client is
   rate-limited (2 requests per second, configurable via `rate_limits.fred_rps`) and implements linear-backoff retry
   logic (max 3 attempts, 45-second total budget). An API key is required via the `SCORPIO_FRED_API_KEY` environment
   variable.

4. **Entity Resolution**: Before any data ingestion begins, the `PreflightTask` canonicalizes the user-supplied ticker
   symbol via the entity resolution module (`src/data/entity.rs`). This module delegates ticker-format validation to the
   existing `src/data/symbol.rs` logic and produces a `ResolvedInstrument` containing the original input, the
   canonicalized uppercase symbol, and optional metadata fields (issuer name, exchange, instrument type, aliases). In
   the initial implementation, metadata fields default to `None`; future milestones may enrich them via API lookups.
   Invalid or empty symbols cause an immediate hard failure at preflight, preventing wasted LLM calls downstream.

5. **Enrichment Adapter Contracts**: The system defines provider-agnostic trait contracts for optional enrichment data
   sources in `src/data/adapters/`: `TranscriptProvider` (earnings call transcripts), `EstimatesProvider` (consensus
   revenue/EPS estimates), and `EventNewsProvider` (event-driven news feeds). Each trait returns a normalized evidence
   struct (`TranscriptEvidence`, `ConsensusEvidence`, `EventNewsEvidence`) that can be consumed uniformly by downstream
   agents regardless of the upstream provider. In the initial implementation, no concrete providers are wired — the
   `PreflightTask` seeds the workflow context with `null` placeholders for each enrichment cache key, and
   `ProviderCapabilities` flags (derived from `DataEnrichmentConfig`) indicate which enrichment categories are enabled.
   Under the current provider-constrained roadmap, event/news enrichment is the first concrete target, consensus
   estimates are only in-scope if free-tier provider verification succeeds, and transcript enrichment is intentionally
   deferred. This seam allows future milestones to plug in real providers behind the existing contracts without modifying
   agent or orchestration code.

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
│  • Derives ProviderCapabilities from config             │
│  • Seeds cache keys with null placeholders              │
│  • Hard-fails on invalid symbol                         │
└──────────────────────────┬──────────────────────────────┘
                           │
         ┌─────────────────┼─────────────────┐─────────────────┐
         ▼                 ▼                 ▼                 ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  Fundamental │  │   Sentiment  │  │     News     │  │  Technical   │
│   Analyst    │  │   Analyst    │  │   Analyst    │  │   Analyst    │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘
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
│  → Approve / Reject  (deterministic fallback if         │
│    Conservative + Neutral both flag violation)          │
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

    // Evidence and Provenance (Stage 1 — dual-write alongside legacy fields above)
    pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
    pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
    pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
    pub evidence_news: Option<EvidenceRecord<NewsData>>,
    pub data_coverage: Option<DataCoverageReport>,
    pub provenance_summary: Option<ProvenanceSummary>,

    // Token Usage Tracking
    pub token_usage: TokenUsageTracker,
}

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
   canonicalizes the input symbol via entity resolution (`src/data/entity.rs`), writes the canonical
   `ResolvedInstrument` to the workflow context, derives `ProviderCapabilities` from the `DataEnrichmentConfig`, writes
   baseline coverage expectations, and seeds enrichment cache keys with explicit `null` placeholders. If the symbol is
   invalid, the pipeline fails immediately rather than wasting LLM calls on a bad input. This step also establishes the
   data-quality contract that downstream agents can reference.
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
5. **Risk Fan-Out**: Similar to the initial data ingestion, the risk assessment phase utilizes a parallel Fan-Out
   pattern. The Aggressive, Neutral, and Conservative risk agents simultaneously evaluate the `TradeProposal` against
   the technical and fundamental data, appending their distinct `RiskReport` objects to the state.
6. **Managerial Arbitration**: The graph terminates at the Fund Manager node, which uses LLM judgment informed by a
   tri-state dual-risk escalation indicator derived from the three risk reports to approve or reject the trade.

## Agent Role Specifications and Implementation Directives

Each persona within the TradingAgents framework requires specific LLM backbone routing, precise system prompt
engineering, and distinct tool access. Following the original paper's architecture, all agents must operate using the *
*ReAct (Reasoning and Acting)** prompting framework, synergizing step-by-step reasoning with tool execution before
emitting their final structured schemas. The implementation will utilize a multi-provider factory pattern via `rig` to
ensure seamless task routing across a diverse suite of models, including OpenAI, Anthropic, Google Gemini, and a custom
GitHub Copilot integration, and other more LLM providers.

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

* **Tool Bindings**: Accesses company-specific news data from `finnhub` and/or `yfinance-rs` where available, as well as `yfinance-rs` Options Chains (IV, Put/Call ratio) and Analyst Upgrades/Downgrades. If direct
  API access is unavailable or insufficient for the target company/news query, the Gemini CLI can be used as a fallback
  for web-search-based news retrieval.
* **Execution Logic**: The agent analyzes recent company-specific news to identify tone shifts, recurring themes,
  management or product narratives, and event-driven sentiment that could affect trading decisions. The goal is to
  aggregate news-driven sentiment into a normalized view of market perception over the past week. Direct Reddit and
  X/Twitter ingestion is intentionally deferred to future improvements.
* **Prompt specification**: [Sentiment Analyst](docs/prompts.md#sentiment-analyst)

#### 3. News Analyst Task

The News Analyst contextualizes the asset within the broader global macroeconomic environment.

* **Tool Bindings**: Accesses `finnhub` market news endpoints (`GetNews`, `GetCachedNews`, `GetMarketNews`) for
  breaking news and company-specific coverage. Macroeconomic indicators are sourced from the FRED API via the
  `GetEconomicIndicators` tool, which returns the latest Federal Funds Rate and CPI inflation data classified into
  `MacroEvent` structs with impact direction and confidence scores. Yahoo Finance news data may also be used when it
  provides relevant company-specific coverage. If direct API access is unavailable for certain sources, the Gemini CLI
  can be used as an alternative for web-search-based news analysis.
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
  `calculate_atr`, `calculate_bollinger_bands`) on those candles. Rust does not pre-fetch or pre-compute anything
  before the agent is invoked.
* **Execution Logic**: The LLM calls the OHLCV and indicator tools during its reasoning pass, then interprets the
  `f64` statistical outputs — RSI overbought/oversold conditions (>70 / <30), MACD signal-line crossovers, ATR
  historical volatility, and Bollinger Band support/resistance boundaries — producing a definitive structured summary.
  The LLM does not perform the mathematical calculations; it invokes the tools and interprets the results. This MVP
  interpretation path is designed for traditional long-term investing workflows; crypto-native interpretation concerns
  such as logarithmic scaling, 24/7 market structure, and MVRV-style on-chain metrics are intentionally deferred
  beyond the MVP.
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
  to the broader market, demanding strict adherence to capital preservation.
  — *Prompt specification*: [Conservative Risk Analyst](docs/prompts.md#conservative-risk-analyst)

* **Neutral Risk Agent**: Functions as the moderating force, attempting to optimize the Sharpe Ratio by balancing the
  aggressive upside targets against the conservative downside protections.
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
  are absent, these sections render the fallback string "Unavailable" rather than omitting the section.
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
/// deferred.
#[derive(serde::Deserialize)]
pub struct DataEnrichmentConfig {
    pub enable_transcripts: bool,           // default: false
    pub enable_consensus_estimates: bool,   // default: false
    pub enable_event_news: bool,            // default: false
    pub max_evidence_age_hours: u64,        // default: 48
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
facilitating continuous framework optimization.

### Token Usage Tracking and Run Statistics

Every LLM invocation throughout the pipeline must record its token consumption. The `rig` completion response includes
prompt and completion token counts; each agent task wrapper extracts these values and appends them to the
`TokenUsageTracker` in the `TradingState` immediately after each call returns. This tracking is mandatory — no LLM
call may bypass the accounting layer.

#### Per-Step Tracking

Each phase of the execution graph records its own `PhaseTokenUsage` entry:

| Phase                      | Agents Tracked                                                       |
|:---------------------------|:---------------------------------------------------------------------|
| Phase 1: Analyst Team      | Fundamental, Sentiment, News, Technical (each individually)          |
| Phase 2: Researcher Debate | Bullish Researcher, Bearish Researcher (per round), Debate Moderator |
| Phase 3: Trader Synthesis  | Trader Agent                                                         |
| Phase 4: Risk Discussion   | Aggressive, Conservative, Neutral (per round), Risk Moderator        |
| Phase 5: Fund Manager      | Fund Manager                                                         |

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
──────────────────────────────────────────────────────────────────────────────────────
TOTAL                          38,220      15,610    53,830     24.1s
LLM calls:                     14
══════════════════════════════════════════════════════════════════════════════════════
```

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
