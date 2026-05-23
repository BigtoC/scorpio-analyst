//! Live Reddit anonymous-API smoke test.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute
//! `examples/`. Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example reddit_live_test
//! ```
//!
//! Requires only a live internet connection — Reddit's anonymous JSON
//! endpoints need no credentials. The spec also requires running this
//! once from a deployed/runtime-like egress before rollout (developer
//! machines are not a sufficient signal).

use std::time::{Duration, Instant};

use scorpio_core::{
    config::RateLimitConfig,
    data::{RedditClient, RedditNewsProvider, traits::NewsProvider},
    domain::{Symbol, Ticker},
    rate_limit::SharedRateLimiter,
};

const BASELINE_EQUITY_SUBS: &[&str] = &["stocks", "investing", "wallstreetbets", "StockMarket"];

struct Results {
    pass: usize,
    fail: usize,
}

impl Results {
    fn new() -> Self {
        Self { pass: 0, fail: 0 }
    }
    fn check(&mut self, label: &str, ok: bool) {
        if ok {
            println!("  PASS  {label}");
            self.pass += 1;
        } else {
            eprintln!("  FAIL  {label}");
            self.fail += 1;
        }
    }
}

fn section(n: usize, title: &str) {
    println!("[{n}] {title}");
}

fn info(msg: &str) {
    println!("        {msg}");
}

fn build_client() -> RedditClient {
    let cfg = RateLimitConfig::default();
    let limiter = SharedRateLimiter::reddit_from_config(&cfg)
        .expect("default reddit_rpm must produce a limiter");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("http client");
    let ua = format!(
        "scorpio-analyst/{} (https://github.com/BigtoC/scorpio-analyst)",
        env!("CARGO_PKG_VERSION"),
    );
    RedditClient::new(http, limiter, ua)
}

fn equity_subs() -> Vec<String> {
    BASELINE_EQUITY_SUBS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

#[tokio::main]
async fn main() {
    println!("─────────────────────────────────────────────────────────────────");
    println!("  Reddit live API smoke test");
    println!("─────────────────────────────────────────────────────────────────");
    println!();

    let mut r = Results::new();
    let client = build_client();

    // Section 1: raw search for AAPL
    section(1, "RedditClient::search_submissions(stocks+..., AAPL, 100)");
    match client.search_submissions(&equity_subs(), "AAPL", 100).await {
        Ok(posts) => {
            info(&format!("returned {} raw posts", posts.len()));
            r.check(
                "AAPL search returned at least 1 raw submission",
                !posts.is_empty(),
            );
        }
        Err(e) => {
            eprintln!("  FAIL  search_submissions(AAPL) returned error: {e}");
            r.fail += 1;
        }
    }
    println!();

    // Section 2: raw search for a second in-scope equity symbol
    section(2, "RedditClient::search_submissions(stocks+..., MSFT, 100)");
    match client.search_submissions(&equity_subs(), "MSFT", 100).await {
        Ok(posts) => {
            info(&format!("returned {} raw posts", posts.len()));
            r.check(
                "MSFT search returned at least 1 raw submission",
                !posts.is_empty(),
            );
        }
        Err(e) => {
            eprintln!("  FAIL  search_submissions(MSFT) returned error: {e}");
            r.fail += 1;
        }
    }
    println!();

    // Section 3: full NewsProvider fetch
    section(3, "RedditNewsProvider::fetch(Symbol::Equity(AAPL))");
    let provider = RedditNewsProvider::new(client.clone(), equity_subs());
    let sym = Symbol::Equity(Ticker::parse("AAPL").expect("AAPL"));
    match provider.fetch(&sym).await {
        Ok(news) => {
            info(&format!(
                "{} normalized articles ({})",
                news.articles.len(),
                news.summary
            ));
            r.check("summary is non-empty", !news.summary.trim().is_empty());
            r.check(
                "every article carries 'Reddit r/' source",
                news.articles
                    .iter()
                    .all(|a| a.source.starts_with("Reddit r/")),
            );
            r.check(
                "every published_at parses as RFC3339",
                news.articles
                    .iter()
                    .all(|a| chrono::DateTime::parse_from_rfc3339(&a.published_at).is_ok()),
            );
            // Score-sorted assertion: relevance scores must be non-increasing.
            let scores: Vec<f64> = news
                .articles
                .iter()
                .map(|a| a.relevance_score.unwrap_or(0.0))
                .collect();
            let sorted = scores.windows(2).all(|w| w[0] >= w[1]);
            r.check("retained posts are sorted by score descending", sorted);
        }
        Err(e) => {
            eprintln!("  FAIL  RedditNewsProvider::fetch returned error: {e}");
            r.fail += 1;
        }
    }
    println!();

    // Section 4: rate-limit wall-clock check (3 sequential calls @ 10 rpm → ≥ 12s elapsed)
    section(
        4,
        "Rate-limiter wall-clock check (3 sequential search_submissions calls)",
    );
    let start = Instant::now();
    for i in 0..3 {
        if let Err(e) = client.search_submissions(&equity_subs(), "AAPL", 25).await {
            eprintln!("  FAIL  call {i} returned error: {e}");
            r.fail += 1;
            break;
        }
    }
    let elapsed = start.elapsed();
    info(&format!("elapsed: {:?}", elapsed));
    r.check(
        "rate limiter enforced ≥ 12s for 3 calls at 10 rpm (6s spacing × 2 gaps)",
        elapsed >= Duration::from_secs(12),
    );
    println!();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  Results: {} passed, {} failed", r.pass, r.fail);
    println!("─────────────────────────────────────────────────────────────────");

    if r.fail > 0 {
        std::process::exit(1);
    }
}
