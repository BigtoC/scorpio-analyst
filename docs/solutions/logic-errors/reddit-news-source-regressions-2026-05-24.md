---
title: Fix Reddit news-source kill-switch, coverage, and provenance regressions
date: 2026-05-24
category: logic-errors
module: workflow-reddit-news-pipeline
problem_type: logic_error
component: assistant
symptoms:
  - "runtime Reddit subreddit routing remained active when reddit_rpm was set to 0"
  - "an empty or disabled Reddit sidecar could shrink sentiment vetted coverage from 30 articles to 10"
  - "provenance_summary over-claimed finnhub during Reddit-only sentiment fallback runs"
  - "Reddit routing, lane-split, and provenance regressions were not covered by regression tests"
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - workflow-runtime
  - analyst-sync
  - sentiment-lane
  - provenance-summary
  - testing-framework
tags:
  - reddit
  - reddit-rpm
  - sentiment-lane
  - provenance
  - prefetch-analyst-news
  - analyst-sync
  - regression-tests
---

# Fix Reddit news-source kill-switch, coverage, and provenance regressions

## Problem
The Reddit sentiment-sidecar rollout introduced a cluster of linked logic regressions across the analyst news pipeline. The runtime kill switch did not fully disable Reddit, the sentiment merge path could shrink vetted coverage even when Reddit returned nothing, and sentiment provenance could claim `finnhub` even when a degraded run only consumed Reddit rows.

Because these bugs all lived on the boundary between runtime routing, cached news lane construction, and `AnalystSyncTask` evidence aggregation, partial fixes could make one regression pass while leaving another one behind.

## Symptoms
- Setting `reddit_rpm = 0` did not fully disable cycle-level Reddit routing.
- The sentiment lane could drop from 30 vetted items to 10 when Reddit returned an empty payload.
- `evidence_sentiment` and `provenance_summary.providers_used` could report `finnhub` for Reddit-only fallback runs.
- Under vetted-news saturation, the Reddit sidecar could be trimmed out of the capped sentiment lane.

## What Didn't Work
- The first sidecar-retention fix reserved Reddit budget before checking whether Reddit actually contributed rows. That preserved Reddit under saturation, but it also let an empty Reddit fetch shrink the vetted lane.
- The first provenance fix appended Reddit when present, but still defaulted the sentiment evidence to Finnhub-backed semantics instead of deriving providers from the cached sentiment lane itself.
- Initial targeted regressions covered the original review findings, but a follow-up reviewer pass surfaced two still-missing cases: empty Reddit shrinking the lane and Reddit-only provenance still claiming Finnhub.

## Solution
Apply the fix at the three code boundaries where the contract drifted, then lock the behavior with regression coverage.

1. Disable subreddit routing when `reddit_rpm == 0`.

File: `crates/scorpio-core/src/workflow/pipeline/runtime.rs`

```rust
pub(crate) fn reddit_subreddits_for_cycle(
    config: &Config,
    runtime_policy: &crate::analysis_packs::RuntimePolicy,
) -> Vec<String> {
    if config.rate_limits.reddit_rpm == 0 {
        vec![]
    } else {
        runtime_policy.reddit_subreddits.clone()
    }
}
```

2. Treat an empty Reddit fetch as no sidecar contribution and preserve the full vetted sentiment feed.

File: `crates/scorpio-core/src/agents/analyst/mod.rs`

```rust
fn build_sentiment_news(vetted: &NewsData, reddit: NewsData) -> Option<NewsData> {
    if reddit.articles.is_empty() {
        return if vetted.articles.is_empty() {
            None
        } else {
            Some(vetted.clone())
        };
    }
}
```

The rest of the function still caps vetted articles before merging, but only after Reddit has proven it actually contributed rows.

3. Derive sentiment provenance from the cached sentiment lane contents instead of assuming Finnhub participated.

File: `crates/scorpio-core/src/workflow/tasks/analyst.rs`

```rust
let has_reddit = news
    .articles
    .iter()
    .any(|article| article.source.starts_with("Reddit r/"));
let has_non_reddit = news
    .articles
    .iter()
    .any(|article| !article.source.starts_with("Reddit r/"));

if has_non_reddit || news.articles.is_empty() {
    sources.push(stage1_source(
        "finnhub",
        vec!["company_news_sentiment_inputs".to_owned()],
    ));
}
if has_reddit {
    sources.push(stage1_source(
        "reddit",
        vec!["crowd_commentary_sentiment_inputs".to_owned()],
    ));
}
```

4. Add regression coverage for both the original and reviewer-found gaps.

Files:
- `crates/scorpio-core/tests/reddit_prefetch_lane_split.rs`
- `crates/scorpio-core/src/workflow/tasks/tests.rs`
- `crates/scorpio-core/src/workflow/pipeline/tests.rs`

Added regressions cover:
- Reddit sidecar retained under vetted saturation
- Empty Reddit sidecar preserves full vetted coverage
- `reddit_rpm = 0` disables runtime Reddit routing
- Cached sentiment news containing Reddit records Reddit provenance
- Reddit-only sentiment fallback does not claim `finnhub`

## Why This Works
The root problem was not a single bad branch. It was contract drift across three phases that all treated Reddit participation differently.

- Runtime routing now decides whether Reddit is active before subreddit selection starts.
- Sentiment merge logic now distinguishes an actual Reddit sidecar from an empty Reddit success result.
- Evidence/provenance now follows the materialized sentiment lane instead of inferred upstream assumptions.

That keeps the two desired behaviors compatible:

- real Reddit rows survive the capped sentiment lane when they exist
- empty or disabled Reddit does not penalize vetted sentiment coverage

## Prevention
- Tie provenance to the cached lane contents that downstream analysts actually consumed, not to broad success flags or intended provider participation.
- Treat empty sidecar responses as absence, not contribution.
- Keep separate regressions for saturation retention and empty-sidecar fallback; they guard opposite failure modes.
- When runtime config is intended to be a kill switch, add a focused regression at the routing boundary instead of relying on downstream behavior to imply disablement.
- Re-run the repo verification sequence for cross-phase pipeline fixes:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast`

## Related Issues
- Related learning: `docs/solutions/logic-errors/stale-trading-state-evidence-and-unavailable-data-quality-fallbacks-2026-04-07.md`
- Related learning: `docs/solutions/logic-errors/shared-options-evidence-regression-2026-04-29.md`
- Related guidance: `docs/solutions/best-practices/concrete-enrichment-provider-pattern-2026-04-10.md`
