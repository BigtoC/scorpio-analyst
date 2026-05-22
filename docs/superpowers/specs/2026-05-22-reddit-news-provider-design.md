# Reddit News Provider — Design

**Date:** 2026-05-22
**Status:** Approved (pending implementation plan)
**Scope:** Add Reddit as a third news ingest source for baseline equity/ETF runs, but keep it in a separate sentiment-only lane so vetted Finnhub and Yahoo Finance articles remain the `NewsAnalyst` factual feed while Reddit augments `SentimentAnalyst` context.

**Motivation:** Yahoo Finance and Finnhub news endpoints are often not timely enough for fast-moving symbols. Reddit can provide near-real-time crowd commentary to backfill that lag for sentiment analysis without displacing vetted news coverage.

**V1 success:** `SentimentAnalyst` gains same-day crowd context when Finnhub/Yahoo lag, while `NewsAnalyst` keeps the full vetted feed.

## Goals

1. Expand `SentimentAnalyst` context with crowdsourced Reddit commentary while keeping `NewsAnalyst` on vetted Finnhub and Yahoo articles.
2. Reuse the existing prefetch step and `GetCachedNews` binding, but split cached feeds by role: vetted feed for `NewsAnalyst`, sentiment feed for `SentimentAnalyst`. No new `TradingState`, `NewsArticle`, or `SentimentData` fields, no new analyst, no new LLM tool.
3. Keep the LLM honest about provenance: the baseline sentiment prompt treats Reddit rows as crowd commentary, while the baseline news prompt continues to assume Reddit is unavailable because `NewsAnalyst` stays on the vetted lane. Both prompt contracts get drift tests.
4. Honor Reddit's anonymous-API rate limits (10 req/min) without new credentials or wizard steps.

## Non-goals (v1)

- OAuth (script app, 100 req/min, refresh tokens). The client is structured so a future OAuth swap is contained, but no abstraction is built up-front.
- Crypto-specific subreddit routing. v1 keeps `prefetch_analyst_news` equity and ETF only until the broader news-provider contract supports `Symbol::Crypto` end to end.
- User-configurable subreddit list via TOML or env. Per-pack constants only.
- Persistent cross-cycle cache. Per-process behavior matches Finnhub.
- Comment-tree traversal. Submissions only.
- Reddit in the `NewsAnalyst` factual lane. `NewsAnalyst` continues consuming vetted Finnhub/Yahoo articles only.
- Live `GetReddit` LLM tool. Analysts see Reddit only via the prefetch path.
- Multi-query enrichment (e.g., richer symbol-plus-name disambiguation). Single-ticker query plus an ambiguity denylist in v1.

## Architecture

Reddit is fetched alongside the existing two-provider prefetch path, but in v1 it only feeds the sentiment lane for equity/ETF runs:

```
PreflightTask
   │  (writes RuntimePolicy → state, including equity/ETF reddit_subreddits)
   ▼
run_analyst_team
   ├── prefetch_analyst_news(finnhub, yfinance, reddit)
   │      │ tokio::join! all three
   │      │ vetted_news: Finnhub + Yahoo only
   │      │ sentiment_news: vetted_news + Reddit sidecar
   │      │ NewsAnalyst fallback: None iff Finnhub and Yahoo both fail
   │      │ SentimentAnalyst fallback: None iff all 3 fail
   │      ▼
   │   vetted Arc<NewsData>          sentiment Arc<NewsData>
   │
   ├── NewsAnalyst       ─────────── binds GetCachedNews(vetted_news)
   └── SentimentAnalyst  ─────────── binds GetCachedNews(sentiment_news)
```

**Invariants preserved:**

- Pack-owned prompts. In v1 the baseline equity and ETF packs own `RuntimePolicy.reddit_subreddits`; crypto-specific subreddit routing stays out of scope until the broader news-provider contract supports `Symbol::Crypto` end to end.
- Prompt contract is lane-specific. Every in-scope sentiment prompt treats `source` values beginning with `Reddit r/` as crowd commentary, while in-scope news prompts keep the explicit `Do not assume Reddit` wording because `NewsAnalyst` stays on the vetted lane. Both prompt contracts get drift tests.
- Rate limiting is centralized. A new `SharedRateLimiter` labelled `"reddit"` is built from validated `RateLimitConfig.reddit_rpm` (default 10, must be `>= 1`) using `Quota::with_period(Duration::from_secs(60) / rpm)` — exact 6-second spacing under 10 rpm.
- Reddit never displaces vetted articles from `NewsAnalyst`. `NewsAnalyst` stays on Finnhub+Yahoo only; `SentimentAnalyst` gets a separate Reddit sidecar budget.
- Partial success is evaluated per lane. `NewsAnalyst` cached news exists iff Finnhub or Yahoo succeeds. `SentimentAnalyst` cached news exists iff Finnhub, Yahoo, or Reddit succeeds. If only Reddit succeeds, `SentimentAnalyst` still gets cached Reddit context while `NewsAnalyst` falls back to live `GetNews`.
- Ambiguous ticker symbols do not guess. A v1 denylist returns empty Reddit data for known ambiguous tickers rather than risking unrelated posts.
- No new credentials. Anonymous JSON endpoint; a single descriptive `User-Agent: scorpio-analyst/<CARGO_PKG_VERSION> (https://github.com/BigtoC/scorpio-analyst)` header on every request.
- Existing `NewsArticle` shape carries Reddit rows without schema change.
- `SentimentData.source_breakdown` can grow an LLM-emitted `SentimentSource { source_name: "Reddit r/<sub>", ... }` entry without struct changes.

## Module layout

```
crates/scorpio-core/src/data/reddit/
├── mod.rs           # pub use re-exports of RedditClient, RedditNewsProvider
├── client.rs        # RedditClient: HTTP wrapper + rate-limited search() + timeout/body-size guards
├── news_provider.rs # RedditNewsProvider: impl NewsProvider for the sentiment-sidecar pipeline
└── types.rs         # RawListing, RawChild, RawSubmission (serde mirrors of Reddit JSON)
```

### `client.rs` — `RedditClient`

- `pub fn new(http: reqwest::Client, limiter: SharedRateLimiter, user_agent: String) -> Self`
- `pub fn for_test() -> Self` — disabled rate limiter, base URL is overridable for HTTP stubbing
- `pub async fn search_submissions(&self, subreddits: &[String], query: &str, limit: u32) -> Result<Vec<RawSubmission>, TradingError>`
  - Builds `https://www.reddit.com/r/<sub1+sub2+...>/search.json?q=<query>&restrict_sr=on&sort=new&over_18=false&stickied=false&limit=<limit>`
  - `limiter.acquire().await` before the call
  - Sets the `User-Agent` header from `self.user_agent`
  - Enforces `REDDIT_REQUEST_TIMEOUT_SECS` and rejects bodies larger than `REDDIT_MAX_RESPONSE_BYTES`
  - Maps 429 / 5xx / timeout / transport errors → `TradingError::NetworkTimeout`
  - Maps oversized or malformed JSON responses → `TradingError::SchemaViolation`

`over_18=false` and `stickied=false` are server-side hints. They are not documented for the `search.json` endpoint and may be silently ignored — defense-in-depth client filters below preserve correctness either way.

### `news_provider.rs` — `RedditNewsProvider`

```rust
pub struct RedditNewsProvider {
    client: RedditClient,
    subreddits: Vec<String>,
}

impl NewsProvider for RedditNewsProvider {
    fn provider_name(&self) -> &'static str { "reddit" }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> { /* … */ }
}
```

`fetch` flow:

1. Extract the canonical equity/ETF ticker string from `Symbol`. If the ticker is in `REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST`, return empty `NewsData` so vetted sources carry the run. `Symbol::Crypto` stays out of scope in v1.
2. `client.search_submissions(&self.subreddits, &ticker, REDDIT_PER_SUB_FETCH_LIMIT)`.
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
5. After the score filter, sort retained posts by score descending (tie-break `published_at` descending); cap to `REDDIT_SENTIMENT_MAX_ARTICLES`.
6. Return `NewsData { articles, macro_events: vec![], summary: format!("Reddit: {n} posts from {} subreddits", subreddits.len()) }`.

This provider output is bound only into `SentimentAnalyst` cached news in v1.

Reddit does not surface macro events; Reddit-sidecar data always carries empty `macro_events`. The vetted `NewsAnalyst` lane preserves Finnhub `macro_events` when Finnhub succeeds; otherwise `macro_events` remain empty.

## Wiring touches

| File                                                                                                                   | Change                                                                                                                                                                                                                                                                                                                                  |
|------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/data/mod.rs`                                                                                  | `pub mod reddit;` + re-export `RedditClient`, `RedditNewsProvider`                                                                                                                                                                                                                                                                      |
| `crates/scorpio-core/src/constants.rs`                                                                                 | `REDDIT_MIN_SCORE: u32 = 50`, `REDDIT_PER_SUB_FETCH_LIMIT: u32 = 100`, `REDDIT_SENTIMENT_MAX_ARTICLES`, `REDDIT_REQUEST_TIMEOUT_SECS`, `REDDIT_MAX_RESPONSE_BYTES`, `REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST`, `REDDIT_USER_AGENT_PREFIX: &str = "scorpio-analyst"` (suffixed with `CARGO_PKG_VERSION` and the repo URL at construction time) |
| `crates/scorpio-core/src/config.rs`                                                                                    | `RateLimitConfig::reddit_rpm: u32` with default `10`; validate `reddit_rpm >= 1` at config load                                                                                                                                                                                                                                         |
| `crates/scorpio-core/src/rate_limit.rs`                                                                                | `SharedRateLimiter::reddit_from_config(cfg) -> Option<Self>` using validated `cfg.reddit_rpm`                                                                                                                                                                                                                                           |
| `crates/scorpio-core/src/analysis_packs/selection.rs` (where `RuntimePolicy` lives today)                              | Add `pub reddit_subreddits: Vec<String>` with `#[serde(default)]` so existing serialized runtime-policy values remain loadable                                                                                                                                                                                                          |
| `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` + `crates/scorpio-core/src/analysis_packs/etf/baseline.rs` | Baseline equity and ETF packs populate Reddit subreddit lists; crypto-specific subreddit routing stays out of scope until `Symbol::Crypto` is supported end to end.                                                                                                                                                                     |
| In-scope `PromptBundle.sentiment_analyst` and `PromptBundle.news_analyst` slots                                        | In-scope sentiment prompts add the `Reddit r/` crowd-commentary guidance. In-scope news prompts keep the explicit "Do not assume Reddit / X/Twitter / StockTwits data is available" wording because `NewsAnalyst` stays on the vetted lane.                                                                                             |
| `crates/scorpio-core/src/agents/analyst/...` prompt drift tests                                                        | Keep/add drift tests for every in-scope Reddit prompt contract: sentiment prompts assert positive Reddit crowd-commentary wording; news prompts assert Reddit remains unavailable to `NewsAnalyst`                                                                                                                                      |
| `crates/scorpio-core/src/agents/analyst/mod.rs` (`prefetch_analyst_news`)                                              | Accept a third provider parameter. Extend `tokio::join!` to 3-way; build two feeds: `vetted_news` (Finnhub primary → Yahoo) and `sentiment_news` (`vetted_news` plus Reddit sidecar). `NewsAnalyst` binds `vetted_news`; `SentimentAnalyst` binds `sentiment_news`.                                                                     |
| `crates/scorpio-core/src/workflow/pipeline/runtime.rs`                                                                 | Instantiate `RedditClient` (sharing a `reqwest::Client` where convenient) and `RedditNewsProvider` with `policy.reddit_subreddits.clone()`; thread the dual-feed outputs through the analyst bindings                                                                                                                                   |

No new top-level Cargo dependencies — `reqwest`, `serde`, `serde_json`, `chrono`, `tokio` are already in `[workspace.dependencies]`. Dev-deps: confirm whether HTTP stubbing uses `wiremock`, `mockito`, or a hand-rolled hyper test server before the plan stage (the codebase currently leans on `mockall` and `StubbedFinancialResponses`-style structural stubs rather than wire-level mocks).

## Data model

No persisted data-model changes to `TradingState`, `NewsArticle`, or `SentimentData`.

- `NewsArticle` carries Reddit rows as-is (`source = "Reddit r/<sub>"`, `url = permalink`, `snippet = truncated selftext`).
- `SentimentData.source_breakdown` accepts an LLM-emitted `SentimentSource { source_name: "Reddit r/<sub>", score, sample_size }` entry without modification.
- `TradingState` is untouched, so phase snapshots remain backward-compatible.
- `RuntimePolicy` gains `pub reddit_subreddits: Vec<String>` with `#[serde(default)]`. Because `RuntimePolicy` is serialized for context propagation and mirrored on `TradingState.analysis_runtime_policy`, the field stays owned and serde-friendly so older persisted values still deserialize.

## Error handling

| Failure                                              | Mapping                                                         | Pipeline effect                                                                                 |
|------------------------------------------------------|-----------------------------------------------------------------|-------------------------------------------------------------------------------------------------|
| Reddit returns 429                                   | `TradingError::NetworkTimeout { message: "rate-limited: ..." }` | Reddit sidecar is unavailable; vetted feeds continue                                            |
| Reddit returns 5xx                                   | `TradingError::NetworkTimeout`                                  | Same as above                                                                                   |
| Transport / DNS / request-timeout failure            | `TradingError::NetworkTimeout`                                  | Same as above                                                                                   |
| Reddit response exceeds `REDDIT_MAX_RESPONSE_BYTES`  | `TradingError::SchemaViolation`                                 | Reddit sidecar is dropped; vetted feeds continue                                                |
| Reddit returns non-JSON or malformed JSON            | `TradingError::SchemaViolation`                                 | Same as above                                                                                   |
| Symbol hits `REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST`      | `Ok(NewsData { articles: vec![], … })`                          | Reddit is skipped for that symbol; vetted feeds continue                                        |
| Reddit returns 0 posts                               | `Ok(NewsData { articles: vec![], … })`                          | Treated as a successful empty fetch; vetted feeds unaffected                                    |
| Finnhub + Yahoo fail, Reddit succeeds                | `vetted_news = None`, `sentiment_news = Some(Reddit-only)`      | `NewsAnalyst` falls back to live `GetNews`; `SentimentAnalyst` keeps cached Reddit context      |
| All 3 providers fail                                 | `vetted_news = None`, `sentiment_news = None`                   | Both analysts fall back to live `GetNews` tool (existing behavior)                              |

`macro_events` remain empty unless Finnhub succeeds in the vetted lane.

The existing `warn!` log lines in `prefetch_analyst_news` are extended to include `reddit_error` where relevant and to note when the vetted lane falls back while the sentiment lane can still proceed on Reddit-only context.

## Configuration loading

`reddit_rpm` follows the existing precedence (env > user file > compiled defaults):

- Compiled default: `RateLimitConfig::reddit_rpm = 10`
- User file override: `[rate_limits] reddit_rpm = 6` in `~/.scorpio-analyst/config.toml`
- Env override: `SCORPIO__RATE_LIMITS__REDDIT_RPM=6`

`reddit_rpm` must be `>= 1`; `0` is rejected during config loading rather than treated as an implicit disable flag in v1.

No `PartialConfig` change (no secret to persist), no `cli/setup/steps.rs` change (no wizard step).

## Testing strategy

### Unit tests (`#[cfg(test)] mod tests`)

`data/reddit/client.rs`:

- URL construction: assert query string contains `q=<ticker>`, `restrict_sr=on`, `sort=new`, `over_18=false`, `stickied=false`, `limit=<limit>` (order-independent — verify each token is present)
- User-Agent header is set from the configured string
- Request timeout → `NetworkTimeout`
- 429 → `NetworkTimeout`
- 5xx → `NetworkTimeout`
- Oversized response body → `SchemaViolation`
- Malformed JSON body → `SchemaViolation`
- Empty `data.children` array → `Ok(vec![])`

`data/reddit/news_provider.rs` (feed `Vec<RawSubmission>` directly, no HTTP):

- Ambiguous symbol denylist returns empty `NewsData` before any Reddit rows are normalized
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
- Retained posts are sorted by score descending after the `<50` filter, tie-break `published_at` descending
- Sentiment-sidecar cap: feed `REDDIT_SENTIMENT_MAX_ARTICLES + 5` valid posts, assert output length equals the cap

### Integration tests (`crates/scorpio-core/tests/`)

`prefetch_analyst_news` dual-feed 3-way:

- All 8 OK/Err combinations across `(finnhub, yfinance, reddit)` — assert `vetted_news` exists iff Finnhub or Yahoo succeeds, and `sentiment_news` exists iff any of the 3 succeeds.
- `NewsAnalyst` feed never includes `source` beginning with `"Reddit r/"`; if only Reddit succeeds, `SentimentAnalyst` gets Reddit-only cached news while `NewsAnalyst` falls back to live `GetNews`.
- Dedup: when one submission URL appears in vetted and Reddit outputs, the `sentiment_news` lane contains it exactly once.
- Runtime-policy serde compatibility: older serialized runtime-policy values without `reddit_subreddits` still deserialize via `#[serde(default)]`.
- Pack-routing: load baseline equity and ETF `RuntimePolicy` values → assert `reddit_subreddits` match the in-scope pack constants. (Pure struct comparison, no network.)

Prompt drift-detection (in-scope sentiment + news prompts):

- In-scope sentiment prompt tests assert positive `crowd commentary` and `Reddit r/` substrings.
- In-scope news prompt tests retain `Do not assume Reddit` because `NewsAnalyst` remains on the vetted lane.

### Smoke test — `crates/scorpio-core/examples/reddit_live_test.rs`

Follows the existing `<provider>_live_test.rs` convention exactly. Runnable via:

```sh
cargo run -p scorpio-core --example reddit_live_test
```

No environment variables required. Uses the same `Results { pass, fail }` framework and exit-code-1-on-failure pattern. Sections:

1. `RedditClient::search_submissions(BASELINE_EQUITY_SUBS, "AAPL", 100)` — assert at least 1 raw submission returned.
2. `RedditClient::search_submissions(BASELINE_EQUITY_SUBS, "SPY", 100)` — assert at least 1 raw submission returned for an ETF-style baseline symbol.
3. `RedditNewsProvider::fetch(Symbol::Equity("AAPL"))` — assert non-empty `summary`, every article has `source` starting with `"Reddit r/"`, every `published_at` parses as RFC3339, no surviving article has `over_18 = true` in the underlying submission (assert via the raw client path), and retained posts are score-sorted.
4. Rate-limiter wall-clock check: issue 3 sequential `search_submissions` calls, assert elapsed ≥ 12 s (6 s spacing × 2 gaps at 10 rpm).
5. Environment-representative validation: run the example once from a deployed/runtime-like egress environment before rollout; anonymous Reddit access is not considered validated from a developer machine alone.

CI does not run `examples/`, so the smoke test is a manual validation tool, not a regression gate.

## Risk register

| Risk                                                       | Likelihood | Mitigation                                                                                                                                         |
|------------------------------------------------------------|------------|----------------------------------------------------------------------------------------------------------------------------------------------------|
| Reddit returns 429 on shared CI IPs                        | Medium     | Reddit sidecar is non-fatal; `NewsAnalyst` stays on vetted feeds, and CI does not run `examples/`.                                                 |
| Reddit JSON schema drift                                   | Low–medium | `RawSubmission` uses `#[serde(default)]` on optional fields; deserialization failures surface as `SchemaViolation` and degrade gracefully.         |
| Ambiguous tickers match unrelated Reddit posts             | Medium     | `REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST` skips known collisions in v1; multi-token enrichment is deferred.                                              |
| LLM treats Reddit upvotes as authoritative facts           | Medium     | In-scope sentiment prompts frame Reddit as crowd commentary; `NewsAnalyst` never sees Reddit; drift tests enforce both prompt contracts.           |
| Token / cost growth from the sentiment sidecar             | Low        | `NewsAnalyst` feed size is unchanged; Reddit is bounded by `REDDIT_SENTIMENT_MAX_ARTICLES`.                                                        |
| Anonymous Reddit access is unavailable from runtime egress | Medium     | Representative runtime validation is required before rollout; launch can pause if anonymous access is blocked.                                     |
| `selftext` carries prompt-injection attempts               | Low–medium | Snippet truncated to `NEWS_SNIPPET_MAX_CHARS`; the existing `UNTRUSTED_CONTEXT_NOTICE` framing already marks news/transcript content as untrusted. |
| Low-volume tickers produce 0 Reddit posts                  | Expected   | Score-50 floor explicitly trades coverage for signal quality. Empty result is a valid `Ok(NewsData)`; vetted feeds continue.                       |

## Open questions for the implementation plan

1. HTTP stubbing primitive — adopt `wiremock` as a new dev-dep, or hand-roll a hyper test server, or follow the `StubbedFinancialResponses` structural-stub pattern from `yfinance/ohlcv.rs`? Decision deferred to the plan stage.
2. Should `RedditClient` reuse the existing `reqwest::Client` while layering explicit request-timeout / body-size guards, or construct a small dedicated client with the same limits? Plan stage will compare lifetime and TLS-config overlap.
3. What symbols belong on the v1 ambiguity denylist, and should the source of truth be a static constant or existing instrument metadata? Plan stage will inspect the current symbol/instrument helpers before locking the rule.

## Out-of-scope follow-ups (separate specs)

- OAuth provider variant.
- User-configurable subreddit list and per-cycle override.
- Persistent cross-cycle news cache.
- Reddit comment-tree fetching for high-engagement posts.
- Live `GetReddit` LLM tool callable mid-inference.
- Crypto-specific subreddit routing once the broader news-provider contract supports `Symbol::Crypto` end to end.
- Multi-query enrichment (`META OR Meta`, `BTC OR Bitcoin`, etc.).
