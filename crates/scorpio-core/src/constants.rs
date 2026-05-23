//! Shared global constants.
//!
//! Includes both size-cap constants and per-agent tool-turn ceilings.

use chrono::TimeDelta;

pub const MAX_RATIONALE_CHARS: usize = usize::MAX;
pub const MAX_PROMPT_CONTEXT_CHARS: usize = usize::MAX;
pub const MAX_SUMMARY_CHARS: usize = usize::MAX;
pub const MAX_USER_PROMPT_CHARS: usize = usize::MAX;
pub const MAX_RAW_RESPONSE_CHARS: usize = usize::MAX;
pub const MAX_DEBATE_CHARS: usize = usize::MAX;
pub const MAX_RISK_CHARS: usize = usize::MAX;
pub const MAX_RAW_MODEL_OUTPUT_CHARS: usize = usize::MAX;
pub const MAX_RISK_HISTORY_CHARS: usize = usize::MAX;
pub const NEWS_TITLE_MAX_CHARS: usize = usize::MAX;
pub const NEWS_SNIPPET_MAX_CHARS: usize = usize::MAX;
pub const MACRO_KEYWORD_SCAN_CHARS: usize = usize::MAX;
pub const MAX_ERROR_SUMMARY_CHARS: usize = usize::MAX;
pub const MAX_INDICATOR_NAME_LEN: usize = usize::MAX;

/// Maximum depth for multi-turn conversations (0 means no multi-turn).
/// A "smarter" model requires fewer turns
pub const FUNDAMENTAL_ANALYST_MAX_TURNS: usize = 100;
pub const NEWS_ANALYST_MAX_TURNS: usize = 100;
pub const SENTIMENT_ANALYST_MAX_TURNS: usize = 100;
pub const TECHNICAL_ANALYST_MAX_TURNS: usize = 100;
pub const TRADER_MAX_TURNS: usize = 100;

/// News analysis
pub const NEWS_ANALYSIS_DAYS: TimeDelta = chrono::Duration::days(30);

/// Health check timeout in seconds. Kept short so a failure surfaces quickly
/// at the end of the wizard rather than blocking for the full pipeline timeout.
pub const HEALTH_CHECK_TIMEOUT_SECS: u64 = 30;

// ─── Reddit ────────────────────────────────────────────────────────────

/// Minimum upvote score for a Reddit submission to be retained.
///
/// Tunes signal/noise; chosen empirically to filter low-engagement posts.
pub const REDDIT_MIN_SCORE: u32 = 50;

/// Per-search `limit` parameter for Reddit `search.json`.
///
/// Reddit caps per-page results at 100; we ask for the cap and apply
/// our own score/age filters client-side.
pub const REDDIT_PER_SUB_FETCH_LIMIT: u32 = 100;

/// Maximum Reddit articles included in the sentiment sidecar feed after
/// score+age filtering and ranking.
pub const REDDIT_SENTIMENT_MAX_ARTICLES: usize = 20;

/// Per-request timeout for Reddit HTTP calls.
pub const REDDIT_REQUEST_TIMEOUT_SECS: u64 = 15;

/// User-Agent prefix; the full header is built at construction time as
/// `"<prefix>/<CARGO_PKG_VERSION> (https://github.com/BigtoC/scorpio-analyst)"`.
pub const REDDIT_USER_AGENT_PREFIX: &str = "scorpio-analyst";

/// Static v1 denylist of equity tickers that collide with high-traffic
/// non-financial words on Reddit. Lookups are case-insensitive.
///
/// Reddit search results for these tickers return mostly unrelated posts;
/// `RedditNewsProvider::fetch` returns an empty `NewsData` when a request's
/// canonical ticker matches an entry here so vetted sources carry the run.
pub const REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST: &[&str] = &[
    "A", "ALL", "ARE", "BIG", "CAN", "FOR", "GO", "HAS", "IT", "ON", "OR", "REAL", "SO", "TRUE",
    "WELL", "WHO",
];

/// Subreddits the equity baseline pack consults for crowd-commentary
/// sentiment context. Names are case-sensitive and shipped without the `r/`
/// prefix (the URL builder adds it).
///
/// Owned by `constants.rs` so the pack manifest, manual smoke-test example,
/// and any future pack derivatives reference the same canonical list.
pub const EQUITY_BASELINE_REDDIT_SUBREDDITS: &[&str] =
    &["stocks", "investing", "wallstreetbets", "StockMarket", "Daytrading"];

/// Reddit subreddits for the crypto digital-asset pack.
///
/// Empty in v1 — the crypto pack opts out of the Reddit sentiment sidecar.
/// `RedditNewsProvider::fetch` short-circuits with an empty `NewsData` when
/// the resolved subreddit list is empty, so binding this constant onto the
/// pack manifest is the explicit opt-out signal.
pub const CRYPTO_DIGITAL_ASSET_REDDIT_SUBREDDITS: &[&str] = &[];
