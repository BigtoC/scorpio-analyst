# Reddit News Provider — Design

**Date:** 2026-05-22
**Status:** Approved (pending implementation plan)
**Scope:** Add Reddit as a third `NewsProvider` alongside Finnhub and Yahoo Finance, so submission posts from a curated, per-pack set of subreddits flow into the same merged `NewsData` that both the Sentiment and News analysts already consume.

## Goals

1. Expand the analyst news feed with crowdsourced commentary from Reddit, surfaced as ordinary `NewsArticle` rows the LLM can read alongside Finnhub and Yahoo articles.
2. Reuse the existing `prefetch_analyst_news` → `Arc<NewsData>` → `GetCachedNews` path. No new `TradingState`, `NewsArticle`, or `SentimentData` fields, no new analyst, no new LLM tool.
3. Keep the LLM honest about provenance: Reddit rows are crowd commentary, not vetted news. The pack prompts guide the model to weight Reddit lower for factual claims and accept it as sentiment input.
4. Honor Reddit's anonymous-API rate limits (10 req/min) without new credentials or wizard steps.

## Non-goals (v1)

- OAuth (script app, 100 req/min, refresh tokens). The client is structured so a future OAuth swap is contained, but no abstraction is built up-front.
- User-configurable subreddit list via TOML or env. Per-pack constants only.
- Persistent cross-cycle cache. Per-process behavior matches Finnhub.
- Comment-tree traversal. Submissions only.
- Live `GetReddit` LLM tool. Analysts see Reddit only via the prefetch path.
- Multi-query enrichment (e.g., search `BTC OR Bitcoin`). Single-ticker query in v1.

## Architecture

Reddit is the third provider in the existing two-provider prefetch path:

```
PreflightTask
   │  (writes RuntimePolicy → state, including pack.reddit_subreddits)
   ▼
run_analyst_team
   ├── prefetch_analyst_news(finnhub, yfinance, reddit)        ← extended to 3-way
   │      │ tokio::join! all three
   │      │ partial-success: any 1+ success → Some(Arc<NewsData>)
   │      │ all-fail (0/3) → None (existing live-tool fallback path)
   │      ▼
   │   merged Arc<NewsData>  (deduped, sorted newest-first, capped)
   │
   ├── SentimentAnalyst  ─┐
   │                       │ both bind GetCachedNews(shared Arc<NewsData>)
   └── NewsAnalyst       ─┘ both see Reddit posts as source = "Reddit r/<sub>"
```

**Invariants preserved:**

- Pack-owned prompts. Subreddit list lives in `RuntimePolicy.reddit_subreddits`, populated by the active pack's manifest. Baseline (equity) and crypto packs ship different lists.
- Rate limiting is centralized. A new `SharedRateLimiter` labelled `"reddit"` is built from `RateLimitConfig.reddit_rpm` (default 10) using `Quota::with_period(Duration::from_secs(60) / rpm)` — exact 6-second spacing under 10 rpm.
- Reddit failure is non-fatal. As long as at least one of the three providers succeeds, prefetch returns `Some(...)`. Reddit-only outage degrades to today's Finnhub+Yahoo behavior.
- No new credentials. Anonymous JSON endpoint; a single descriptive `User-Agent: scorpio-analyst/<CARGO_PKG_VERSION> (https://github.com/BigtoC/scorpio-analyst)` header on every request.
- Existing `NewsArticle` shape carries Reddit rows without schema change.
- `SentimentData.source_breakdown` can grow an LLM-emitted `SentimentSource { source_name: "Reddit r/<sub>", ... }` entry without struct changes.

## Module layout

```
crates/scorpio-core/src/data/reddit/
├── mod.rs           # pub use re-exports of RedditClient, RedditNewsProvider
├── client.rs        # RedditClient: HTTP wrapper + rate-limited search()
├── news_provider.rs # RedditNewsProvider: impl NewsProvider for the news pipeline
└── types.rs         # RawListing, RawChild, RawSubmission (serde mirrors of Reddit JSON)
```

### `client.rs` — `RedditClient`

- `pub fn new(http: reqwest::Client, limiter: SharedRateLimiter, user_agent: String) -> Self`
- `pub fn for_test() -> Self` — disabled rate limiter, base URL is overridable for HTTP stubbing
- `pub async fn search_submissions(&self, subreddits: &[&str], query: &str, limit: u32) -> Result<Vec<RawSubmission>, TradingError>`
  - Builds `https://www.reddit.com/r/<sub1+sub2+...>/search.json?q=<query>&restrict_sr=on&sort=new&over_18=false&stickied=false&limit=<limit>`
  - `limiter.acquire().await` before the call
  - Sets the `User-Agent` header from `self.user_agent`
  - Maps 429 / 5xx / transport errors → `TradingError::NetworkTimeout`
  - Maps JSON parse failures → `TradingError::SchemaViolation`

`over_18=false` and `stickied=false` are server-side hints. They are not documented for the `search.json` endpoint and may be silently ignored — defense-in-depth client filters below preserve correctness either way.

### `news_provider.rs` — `RedditNewsProvider`

```rust
pub struct RedditNewsProvider {
    client: RedditClient,
    subreddits: &'static [&'static str],
}

impl NewsProvider for RedditNewsProvider {
    fn provider_name(&self) -> &'static str { "reddit" }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> { /* … */ }
}
```

`fetch` flow:

1. Extract the canonical ticker string from `Symbol` (equity ticker as-is; crypto symbol e.g. `BTC` as-is — defer "BTC OR Bitcoin" multi-query to a follow-up spec).
2. `client.search_submissions(self.subreddits, &ticker, REDDIT_PER_SUB_FETCH_LIMIT)`.
3. Apply curated client-side filters (cheap, defense-in-depth):
   - skip `over_18 == true`
   - skip `stickied == true`
   - skip `score < REDDIT_MIN_SCORE` (constant = **50**)
   - skip posts older than `NEWS_ANALYSIS_DAYS`
4. Normalize each surviving `RawSubmission` to `NewsArticle`:
   - `title` ← submission title (truncated to `NEWS_TITLE_MAX_CHARS`)
   - `source` ← `format!("Reddit r/{}", submission.subreddit)`
   - `published_at` ← `created_utc` → RFC3339
   - `snippet` ← `selftext` truncated to `NEWS_SNIPPET_MAX_CHARS`; empty when `selftext` is empty (link/image posts)
   - `url` ← `Some(format!("https://www.reddit.com{}", submission.permalink))`
   - `relevance_score` ← `Some(((score as f64 + 1.0).log10() / 1000_f64.log10()).clamp(0.0, 1.0))` — saturates at 1.0 for score ≥ 1000
5. Sort by `published_at` descending; cap to `REDDIT_PER_PROVIDER_MAX_ARTICLES = NEWS_PREFETCH_MAX_ARTICLES / 3` (leaves headroom for Finnhub and Yahoo in the post-merge cap).
6. Return `NewsData { articles, macro_events: vec![], summary: format!("Reddit: {n} posts from {} subreddits", subreddits.len()) }`.

Reddit does not surface macro events; `macro_events` is always empty for Reddit-only data. The post-merge step preserves `macro_events` from the primary (Finnhub) source.

## Wiring touches

| File                                                                                                         | Change                                                                                                                                                                                                                                                                                                                                                                                                                                        |
|--------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/data/mod.rs`                                                                        | `pub mod reddit;` + re-export `RedditClient`, `RedditNewsProvider`                                                                                                                                                                                                                                                                                                                                                                            |
| `crates/scorpio-core/src/constants.rs`                                                                       | `REDDIT_MIN_SCORE: u32 = 50`, `REDDIT_PER_SUB_FETCH_LIMIT: u32 = 100`, `REDDIT_PER_PROVIDER_MAX_ARTICLES = NEWS_PREFETCH_MAX_ARTICLES / 3`, `REDDIT_USER_AGENT_PREFIX: &str = "scorpio-analyst"` (suffixed with `CARGO_PKG_VERSION` and the repo URL at construction time)                                                                                                                                                                    |
| `crates/scorpio-core/src/config.rs`                                                                          | `RateLimitConfig::reddit_rpm: u32` with default `10`                                                                                                                                                                                                                                                                                                                                                                                          |
| `crates/scorpio-core/src/rate_limit.rs`                                                                      | `SharedRateLimiter::reddit_from_config(cfg) -> Option<Self>` using `Quota::with_period(Duration::from_secs(60) / cfg.reddit_rpm)`                                                                                                                                                                                                                                                                                                             |
| `crates/scorpio-core/src/analysis_packs/manifest.rs` (or wherever `RuntimePolicy` lives today)               | Add `pub reddit_subreddits: &'static [&'static str]`                                                                                                                                                                                                                                                                                                                                                                                          |
| `crates/scorpio-core/src/analysis_packs/builtin.rs`                                                          | Baseline pack subs: `["stocks","wallstreetbets","investing","stockmarket","Daytrading","SecurityAnalysis","ValueInvesting","options","pennystocks","dividends"]`. Crypto pack subs: `["CryptoCurrency","CryptoMarkets","Bitcoin","ethfinance"]`                                                                                                                                                                                               |
| Baseline pack `PromptBundle.sentiment_analyst` and `PromptBundle.news_analyst`                               | Replace "Do not assume Reddit / X/Twitter / StockTwits data is available" with: "Articles whose `source` begins with `Reddit r/` are crowd commentary, not vetted news — weight them lower for factual claims (use only as corroboration of facts present in non-Reddit sources), and treat aggregate Reddit signal (volume, upvote-weighted tone) as a legitimate sentiment input. Do not assume X/Twitter or StockTwits data is available." |
| `crates/scorpio-core/src/agents/analyst/equity/sentiment.rs` (test `system_prompt_forbids_social_platforms`) | Rename to `system_prompt_treats_reddit_as_crowd_commentary`. Retain assertions for `X/Twitter`, `StockTwits`, `Do not assume`. Replace the negative-form `Reddit` assertion with positive-form assertions on `crowd commentary` and `Reddit r/`                                                                                                                                                                                               |
| `crates/scorpio-core/src/agents/analyst/mod.rs` (`prefetch_analyst_news`)                                    | Accept a third provider parameter. Extend `tokio::join!` to 3-way; partial-success: `Some(merged)` iff any of the 3 succeeded, `None` only when all three fail. Merge order: Finnhub primary → Yahoo → Reddit, dedup via existing `is_duplicate` at each step. `sort_and_cap_news` unchanged                                                                                                                                                  |
| `crates/scorpio-core/src/workflow/pipeline/runtime.rs`                                                       | Instantiate `RedditClient` (sharing a `reqwest::Client` where convenient) and `RedditNewsProvider` with `policy.reddit_subreddits`; thread through to `prefetch_analyst_news`                                                                                                                                                                                                                                                                 |

No new top-level Cargo dependencies — `reqwest`, `serde`, `serde_json`, `chrono`, `tokio` are already in `[workspace.dependencies]`. Dev-deps: confirm whether HTTP stubbing uses `wiremock`, `mockito`, or a hand-rolled hyper test server before the plan stage (the codebase currently leans on `mockall` and `StubbedFinancialResponses`-style structural stubs rather than wire-level mocks).

## Data model

No persisted data-model changes to `TradingState`, `NewsArticle`, or `SentimentData`.

- `NewsArticle` carries Reddit rows as-is (`source = "Reddit r/<sub>"`, `url = permalink`, `snippet = truncated selftext`).
- `SentimentData.source_breakdown` accepts an LLM-emitted `SentimentSource { source_name: "Reddit r/<sub>", score, sample_size }` entry without modification.
- `TradingState` is untouched, so phase snapshots remain backward-compatible.
- `RuntimePolicy` gains one additive `&'static [&'static str]` field; it is not serialized into snapshots (lives on the in-memory policy struct only).

## Error handling

| Failure                                   | Mapping                                                         | Pipeline effect                                                                   |
|-------------------------------------------|-----------------------------------------------------------------|-----------------------------------------------------------------------------------|
| Reddit returns 429                        | `TradingError::NetworkTimeout { message: "rate-limited: ..." }` | Reddit slot is `Err`; other 2 providers cover; analyst run continues              |
| Reddit returns 5xx                        | `TradingError::NetworkTimeout`                                  | Same as above                                                                     |
| Transport / DNS failure                   | `TradingError::NetworkTimeout`                                  | Same as above                                                                     |
| Reddit returns non-JSON or malformed JSON | `TradingError::SchemaViolation`                                 | Same as above                                                                     |
| Reddit returns 0 posts                    | `Ok(NewsData { articles: vec![], … })`                          | Treated as a successful empty fetch; merged count from other providers unaffected |
| All 3 providers fail                      | `prefetch_analyst_news` returns `None`                          | Analysts fall back to live `GetNews` tool (existing behavior)                     |

The existing `warn!` log lines in `prefetch_analyst_news` are extended to include a `reddit_error` field for the all-fail case and a single-line warn in the partial-fail case.

## Configuration loading

`reddit_rpm` follows the existing precedence (env > user file > compiled defaults):

- Compiled default: `RateLimitConfig::reddit_rpm = 10`
- User file override: `[rate_limits] reddit_rpm = 6` in `~/.scorpio-analyst/config.toml`
- Env override: `SCORPIO__RATE_LIMITS__REDDIT_RPM=6`

No `PartialConfig` change (no secret to persist), no `cli/setup/steps.rs` change (no wizard step).

## Testing strategy

### Unit tests (`#[cfg(test)] mod tests`)

`data/reddit/client.rs`:

- URL construction: assert query string contains `q=<ticker>`, `restrict_sr=on`, `sort=new`, `over_18=false`, `stickied=false`, `limit=<limit>` (order-independent — verify each token is present)
- User-Agent header is set from the configured string
- 429 → `NetworkTimeout`
- 5xx → `NetworkTimeout`
- Malformed JSON body → `SchemaViolation`
- Empty `data.children` array → `Ok(vec![])`

`data/reddit/news_provider.rs` (feed `Vec<RawSubmission>` directly, no HTTP):

- NSFW skip when `over_18 == true` (defense-in-depth check fires even though the URL requested filtering)
- Stickied skip when `stickied == true`
- Score floor: `score = 49` is skipped, `score = 50` is kept
- Age window: post older than `NEWS_ANALYSIS_DAYS` is skipped, post inside the window is kept
- `selftext.len() > NEWS_SNIPPET_MAX_CHARS` is truncated
- Link post (`selftext == ""`) produces `snippet = ""` without panic
- `relevance_score` correctness at boundaries: `score = 0 → 0.0`, `score = 1000 → 1.0`, `score = 100_000 → 1.0` (clamped)
- RFC3339 conversion correctness for `created_utc`
- `source` formatting equals `"Reddit r/<subreddit>"`
- `url` equals `https://www.reddit.com<permalink>`
- Per-provider article cap: feed `REDDIT_PER_PROVIDER_MAX_ARTICLES + 5` valid posts, assert output length equals the cap and is sorted newest-first

### Integration tests (`crates/scorpio-core/tests/`)

`prefetch_analyst_news` 3-way:

- All 8 OK/Err combinations across `(finnhub, yfinance, reddit)` — assert `Some(...)` iff at least one OK, `None` only on 0/3.
- Dedup: when one submission URL appears in two providers' outputs, the merged list contains it exactly once.
- Pack-routing: load baseline `RuntimePolicy` → assert `reddit_subreddits == BASELINE_EQUITY_SUBS`; load crypto pack → assert `reddit_subreddits == CRYPTO_SUBS`. (Pure struct comparison, no network.)

Prompt drift-detection (`crates/scorpio-core/src/agents/analyst/equity/sentiment.rs`):

- Renamed test asserts: positive `crowd commentary` and `Reddit r/` substrings; retained `X/Twitter`, `StockTwits`, `Do not assume` substrings.

### Smoke test — `crates/scorpio-core/examples/reddit_live_test.rs`

Follows the existing `<provider>_live_test.rs` convention exactly. Runnable via:

```sh
cargo run -p scorpio-core --example reddit_live_test
```

No environment variables required. Uses the same `Results { pass, fail }` framework and exit-code-1-on-failure pattern. Sections:

1. `RedditClient::search_submissions(BASELINE_EQUITY_SUBS, "AAPL", 100)` — assert at least 1 raw submission returned.
2. `RedditClient::search_submissions(CRYPTO_SUBS, "BTC", 100)` — assert at least 1 raw submission returned.
3. `RedditNewsProvider::fetch(Symbol::Equity("AAPL"))` — assert non-empty `summary`, every article has `source` starting with `"Reddit r/"`, every `published_at` parses as RFC3339, no surviving article has `over_18 = true` in the underlying submission (assert via the raw client path).
4. Rate-limiter wall-clock check: issue 3 sequential `search_submissions` calls, assert elapsed ≥ 12 s (6 s spacing × 2 gaps at 10 rpm).

CI does not run `examples/`, so the smoke test is a manual validation tool, not a regression gate.

## Risk register

| Risk                                                | Likelihood | Mitigation                                                                                                                                         |
|-----------------------------------------------------|------------|----------------------------------------------------------------------------------------------------------------------------------------------------|
| Reddit returns 429 on shared CI IPs                 | Medium     | Reddit is non-fatal in the 3-way prefetch; Finnhub+Yahoo cover the gap. CI does not run `examples/`.                                               |
| Reddit JSON schema drift                            | Low–medium | `RawSubmission` uses `#[serde(default)]` on optional fields; deserialization failures surface as `SchemaViolation` and degrade gracefully.         |
| LLM treats Reddit upvotes as authoritative facts    | Medium     | Pack-prompt language frames Reddit as crowd commentary; drift-detection test enforces the wording.                                                 |
| Token / cost growth from larger merged news payload | Low        | Reddit cap = `NEWS_PREFETCH_MAX_ARTICLES / 3`; the final merge cap (`NEWS_PREFETCH_MAX_ARTICLES`) is unchanged.                                    |
| NSFW or abusive content slipping through            | Low        | `over_18=false` URL hint + client-side `over_18 == true` skip + `score >= 50` floor (most abusive posts are heavily downvoted).                    |
| `selftext` carries prompt-injection attempts        | Low–medium | Snippet truncated to `NEWS_SNIPPET_MAX_CHARS`; the existing `UNTRUSTED_CONTEXT_NOTICE` framing already marks news/transcript content as untrusted. |
| Low-volume tickers produce 0 Reddit posts           | Expected   | Score-50 floor explicitly trades coverage for signal quality. Empty result is a valid `Ok(NewsData)`; merge continues.                             |

## Open questions for the implementation plan

1. HTTP stubbing primitive — adopt `wiremock` as a new dev-dep, or hand-roll a hyper test server, or follow the `StubbedFinancialResponses` structural-stub pattern from `yfinance/ohlcv.rs`? Decision deferred to the plan stage.
2. Where exactly does `RuntimePolicy` live today, and is `&'static [&'static str]` compatible with how packs are registered? Plan stage will read `analysis_packs/manifest.rs` and the baseline pack registration site, then confirm or adjust.
3. Does the `reqwest::Client` for Reddit get shared with the existing `yfinance` session client, or constructed fresh? Plan stage will compare lifetime and TLS-config overlap.

## Out-of-scope follow-ups (separate specs)

- OAuth provider variant.
- User-configurable subreddit list and per-cycle override.
- Persistent cross-cycle news cache.
- Reddit comment-tree fetching for high-engagement posts.
- Live `GetReddit` LLM tool callable mid-inference.
- Multi-query enrichment (`BTC OR Bitcoin`, etc.).
