# Future Enhancements

This document records intentionally deferred enhancements that appear in design specs but are out of scope for the
current implementation phase.

Use it to keep promising ideas visible without expanding MVP scope too early.

## When to update this doc

Update this file whenever a design spec explicitly calls out a future enhancement, deferred trade-off, or post-MVP
follow-up that should be revisited later.

## Deferred Enhancements

> Will be considered for implementation after the MVP is complete and stable enough to evaluate whether the added
> complexity is justified by the benefits.

### Per-agent provider overrides

- **Status**: Deferred until after the MVP is finished
- **Source**: `openspec/changes/add-llm-providers/design.md`
- **Current baseline**: The provider layer uses tier-level providers (`llm.quick_thinking_provider`,
  `llm.deep_thinking_provider`) and tier-level models (`QuickThinking` / `DeepThinking`), not per-agent overrides.
- **Why it was deferred**: Tier-level provider selection keeps config and key management simpler than fully per-agent
  routing while still allowing quick/deep tiers to use different backends.
- **Why revisit later**: Different agents may eventually benefit from different providers, cost/performance profiles, or
  provider-specific capabilities.
- **Intentionally deferred details**:
    - Exact config shape for per-agent overrides
    - Override precedence rules
    - Validation and fallback behavior
    - Any migration path from the MVP config model
- **Revisit trigger**: After the MVP provider, agent, and workflow layers are stable enough to evaluate whether
  mixed-provider routing is worth the added complexity

### Copilot heuristic token estimation

- **Status**: Deferred until after the MVP is finished
- **Source**: `openspec/changes/add-copilot-provider/design.md`
- **Current baseline**: GitHub Copilot via ACP does not expose authoritative provider token counts. MVP records
  authoritative latency, and token count fields are treated as unavailable/not reported metadata for Copilot-backed
  calls.
- **Why it was deferred**: A client-side estimate can only be derived from visible prompt/response text and would miss
  hidden system prompts, backend prompt rewrites, model/tokenizer differences, and other provider-side accounting.
- **Why revisit later**: Approximate token estimates may still be useful for rough budgeting or comparative UX if they
  are clearly labeled as heuristic-only.
- **Intentionally deferred details**:
    - Whether estimates should be shown in CLI/TUI/GPUI by default or only in verbose/debug views
    - Which tokenizer or model-family fallback to use when Copilot does not expose a stable backend model ID
    - How to separate approximate estimates from authoritative provider-reported counts in summaries and exports
    - Whether aggregate totals should exclude heuristic estimates by default to preserve auditability
- **Revisit trigger**: After the MVP token-usage reporting and Copilot provider behavior are stable enough to evaluate
  whether approximate estimates add enough value to justify the extra complexity and caveats

### Hyperliquid perps DEX research input

- **Status**: Deferred until after the MVP is finished
- **Source**: Research-team planning follow-up and prompt guidance updates
- **Current baseline**: The Researcher Team debates using analyst outputs from fundamentals, news, sentiment, and
  technical analysis only. No Hyperliquid market structure or perps DEX data is provided yet.
- **Why it was deferred**: Hyperliquid introduces a narrower, symbol-dependent data path that only applies to a
  manually maintained whitelist of stock-linked markets. Keeping it out of the MVP avoids expanding the data layer,
  researcher inputs, and prompt context before the core workflow is stable.
- **Why revisit later**: For whitelisted symbols listed on Hyperliquid, perps DEX positioning and market structure may
  give the Researcher Team an additional real-time signal for momentum, crowding, and directional conviction.
- **Initial scope when revisited**:
    - Only enable the source for manually whitelisted symbols listed on Hyperliquid
    - Start with examples such as `QQQ`, `SPY`, and `NVDA`
    - Treat the whitelist as operator-managed rather than auto-discovered in MVP+1 planning
- **Prompt impact when revisited**:
    - Bull Researcher, Bear Researcher, and Debate Moderator prompts should accept an additional Hyperliquid perps DEX
      research input for eligible symbols
    - Prompts should explicitly ignore the source when the target symbol is not on the whitelist
- **Intentionally deferred details**:
    - Exact API/client integration and normalization rules for Hyperliquid market data
    - Which perps fields should be exposed to researchers first (for example price basis, funding, OI, volume, long/
      short imbalance)
    - Freshness requirements and whether the data should be analyst-produced or injected directly into researcher
      context
    - How to prevent researchers from over-weighting DEX-specific signals relative to fundamentals and macro context
- **Revisit trigger**: After the sentiment and research-team modules are stable enough to evaluate whether whitelisted
  Hyperliquid signals improve debate quality without adding excessive complexity

### Social-platform sentiment ingestion

- **Status**: Deferred until after the MVP is finished
- **Source**: `PRD.md`, `openspec/project.md`, and `openspec/changes/add-financial-data/design.md`
- **Current baseline**: The MVP Sentiment Analyst uses company-specific news data from `Finnhub` and/or `yfinance-rs`
  for sentiment analysis, with Gemini CLI as a fallback for company/news fetching. Direct Reddit and X/Twitter API usage
  is out of current scope.
- **Why it was deferred**: Social-platform ingestion expands the auth surface, moderation/compliance concerns, source
  normalization, and maintenance burden well beyond the current MVP data layer.
- **Why revisit later**: Reddit and X/Twitter may add earlier crowd-positioning and retail-conviction signals that are
  not always visible in company-news coverage alone.
- **Potential implementation options when revisited**:
    - Direct Reddit API ingestion for selected finance-focused communities
    - Direct X/Twitter ingestion where compliant access is available
    - `twitter-cli` as an operator-managed option for X/Twitter collection: https://github.com/jackwener/twitter-cli
- **Intentionally deferred details**:
    - Exact source-priority rules relative to Finnhub/Yahoo Finance news
    - Credential, compliance, and rate-limit handling for each platform
    - Deduplication and weighting rules between news-driven and social-driven sentiment
    - Whether social inputs should feed only the Sentiment Analyst or also enrich researcher debate context
- **Revisit trigger**: After the MVP news-based sentiment workflow is stable enough to evaluate whether social signals
  add material value beyond company-specific news sentiment

### Crypto-native technical analysis

- **Status**: Deferred until after the MVP is finished
- **Source**: `PRD.md` and `openspec/changes/add-technical-analysis/design.md`
- **Current baseline**: The MVP technical-analysis layer is designed for traditional OHLCV-based long-term investing
  workflows. It computes price/volume indicators from candle data and derives long-horizon support/resistance, but it is
  not treated as a complete crypto-analysis solution.
- **Why it was deferred**: Full crypto analysis expands scope beyond standard OHLCV indicators into 24/7 market
  structure, logarithmic-scale interpretation, and on-chain metrics such as MVRV. Adding that now would widen both the
  data layer and the interpretation layer before the core MVP workflow is stable.
- **Why revisit later**: Crypto assets may require different long-horizon interpretation rules and additional valuation
  signals that are not captured by OHLCV-derived indicators alone.
- **Potential implementation options when revisited**:
    - Add crypto-specific market assumptions such as explicit UTC candle/session boundaries for 24/7 trading
    - Add log-scale-aware trend and support/resistance interpretation for long-horizon crypto charts
    - Add on-chain valuation and positioning metrics such as MVRV as a separate capability layered on top of the core
      technical-analysis module
- **Intentionally deferred details**:
    - Whether crypto support should extend the existing `technical-analysis` capability or be introduced as a separate
      crypto-specific capability
    - Which on-chain metrics should be first-class in MVP+1 (for example MVRV, NUPL, realized cap, exchange flows)
    - Which market data provider(s) should supply crypto OHLCV and whether volume should be exchange-specific or
      aggregated
    - How log-scale interpretation should affect prompts, reports, and any future UI charting surfaces
- **Revisit trigger**: After the MVP technical-analysis and financial-data layers are stable enough to evaluate whether
  crypto-specific workflows justify a dedicated follow-up capability
