//! Live Finnhub API smoke test.
//!
//! **NOT run automatically in CI** - `cargo nextest` does not execute `examples/`.
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example finnhub_live_test
//! ```
//!
//! Requires:
//! - a live internet connection
//! - `SCORPIO_FINNHUB_API_KEY` to be set in the environment
//!
//! Covers every public Finnhub client method currently exposed from
//! `crates/scorpio-core/src/data/finnhub.rs`:
//! - `FinnhubClient::get_fundamentals`
//! - `FinnhubClient::get_earnings`
//! - `FinnhubClient::get_insider_transactions`
//! - `FinnhubClient::fetch_company_news`
//! - `FinnhubClient::get_structured_news`
//! - `FinnhubClient::get_market_news`
//! - `FinnhubClient::fetch_earnings_calendar`

use chrono::{Duration, Utc};
use scorpio_core::{config::ApiConfig, data::FinnhubClient, rate_limit::SharedRateLimiter};
use secrecy::SecretString;

/// Well-known liquid equity used as the primary test subject.
const EQUITY_SYMBOL: &str = "GLW";
/// Number of calendar days in the look-back window for company news.
const LOOKBACK_DAYS: i64 = 30;
/// Number of calendar days in the look-back window for the earnings calendar
/// (mirrors the transcript-quarter resolution path in
/// `workflow::pipeline::runtime::resolve_transcript_quarter`).
const EARNINGS_CALENDAR_LOOKBACK_DAYS: i64 = 120;

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

    fn check_result(&mut self, label: &str, result: Result<(), String>) {
        match result {
            Ok(()) => self.check(label, true),
            Err(msg) => {
                eprintln!("  FAIL  {label}: {msg}");
                self.fail += 1;
            }
        }
    }
}

fn section(n: usize, title: &str) {
    println!("[{n}] {title}");
}

fn info(msg: &str) {
    println!("        {msg}");
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set"))
}

#[tokio::main]
async fn main() {
    let api_key = match required_env("SCORPIO_FINNHUB_API_KEY") {
        Ok(key) => key,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

    let today = Utc::now().date_naive();
    let from = (today - Duration::days(LOOKBACK_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let to = today.format("%Y-%m-%d").to_string();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  Finnhub live API smoke test");
    println!("─────────────────────────────────────────────────────────────────");
    println!("  Equity : {EQUITY_SYMBOL}");
    println!("  Window : {from} → {to}");
    println!("─────────────────────────────────────────────────────────────────");
    println!();

    let client = FinnhubClient::new(
        &ApiConfig {
            finnhub_api_key: Some(SecretString::from(api_key)),
            fred_api_key: None,
            alpha_vantage_api_key: None,
        },
        SharedRateLimiter::new("finnhub", 10),
    )
    .expect("client should construct with API key");

    let mut r = Results::new();

    section(
        1,
        &format!("FinnhubClient::get_fundamentals ({EQUITY_SYMBOL})"),
    );
    match client.get_fundamentals(EQUITY_SYMBOL).await {
        Err(e) => {
            eprintln!("  FAIL  get_fundamentals returned error: {e}");
            r.fail += 1;
        }
        Ok(data) => {
            info(&format!("summary: {}", data.summary));
            r.check(
                "get_fundamentals returns non-empty summary",
                !data.summary.trim().is_empty(),
            );
        }
    }
    println!();

    section(2, &format!("FinnhubClient::get_earnings ({EQUITY_SYMBOL})"));
    match client.get_earnings(EQUITY_SYMBOL).await {
        Err(e) => {
            eprintln!("  FAIL  get_earnings returned error: {e}");
            r.fail += 1;
        }
        Ok(data) => {
            info(&format!("summary: {}", data.summary));
            r.check(
                "get_earnings returns non-empty summary",
                !data.summary.trim().is_empty(),
            );
        }
    }
    println!();

    section(
        3,
        &format!("FinnhubClient::get_insider_transactions ({EQUITY_SYMBOL})"),
    );
    match client.get_insider_transactions(EQUITY_SYMBOL).await {
        Err(e) => {
            eprintln!("  FAIL  get_insider_transactions returned error: {e}");
            r.fail += 1;
        }
        Ok(data) => {
            info(&format!(
                "{} insider transaction(s)",
                data.insider_transactions.len()
            ));
            r.check(
                "get_insider_transactions returns non-empty summary",
                !data.summary.trim().is_empty(),
            );
        }
    }
    println!();

    section(
        4,
        &format!("FinnhubClient::fetch_company_news ({EQUITY_SYMBOL})"),
    );
    match client.fetch_company_news(EQUITY_SYMBOL, &from, &to).await {
        Err(e) => {
            eprintln!("  FAIL  fetch_company_news returned error: {e}");
            r.fail += 1;
        }
        Ok(news) => {
            info(&format!("{} raw article(s)", news.len()));
            r.check("fetch_company_news completes successfully", true);
            r.check_result(
                "all raw company-news timestamps are non-negative",
                if news.iter().all(|item| item.datetime >= 0) {
                    Ok(())
                } else {
                    Err("one or more news items had negative timestamps".to_owned())
                },
            );
        }
    }
    println!();

    section(
        5,
        &format!("FinnhubClient::get_structured_news ({EQUITY_SYMBOL})"),
    );
    match client.get_structured_news(EQUITY_SYMBOL).await {
        Err(e) => {
            eprintln!("  FAIL  get_structured_news returned error: {e}");
            r.fail += 1;
        }
        Ok(news) => {
            info(&format!(
                "{} article(s), {} macro event(s)",
                news.articles.len(),
                news.macro_events.len()
            ));
            r.check(
                "get_structured_news returns non-empty summary",
                !news.summary.trim().is_empty(),
            );
        }
    }
    println!();

    section(6, "FinnhubClient::get_market_news");
    match client.get_market_news().await {
        Err(e) => {
            eprintln!("  FAIL  get_market_news returned error: {e}");
            r.fail += 1;
        }
        Ok(news) => {
            info(&format!(
                "{} market article(s), {} macro event(s)",
                news.articles.len(),
                news.macro_events.len()
            ));
            r.check(
                "get_market_news returns non-empty summary",
                !news.summary.trim().is_empty(),
            );
            r.check(
                "get_market_news returns at least one article",
                !news.articles.is_empty(),
            );
        }
    }
    println!();

    section(
        7,
        &format!("FinnhubClient::fetch_earnings_calendar ({EQUITY_SYMBOL})"),
    );
    let earnings_from = (today - Duration::days(EARNINGS_CALENDAR_LOOKBACK_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    info(&format!("window: {earnings_from} → {to}"));
    match client
        .fetch_earnings_calendar(&earnings_from, &to, Some(EQUITY_SYMBOL))
        .await
    {
        Err(e) => {
            eprintln!("  FAIL  fetch_earnings_calendar returned error: {e}");
            r.fail += 1;
        }
        Ok(releases) => {
            info(&format!("{} earnings release(s)", releases.len()));
            if let Some(latest) = releases
                .iter()
                .filter(|rel| rel.date.is_some())
                .max_by(|a, b| a.date.cmp(&b.date))
            {
                info(&format!(
                    "latest: symbol={:?} date={:?} year={:?} quarter={:?}",
                    latest.symbol, latest.date, latest.year, latest.quarter,
                ));
            }
            r.check("fetch_earnings_calendar completes successfully", true);
            r.check_result(
                "every release for AAPL matches the requested symbol",
                if releases
                    .iter()
                    .all(|rel| rel.symbol.as_deref() == Some(EQUITY_SYMBOL))
                {
                    Ok(())
                } else {
                    Err("one or more releases had a mismatched or missing symbol".to_owned())
                },
            );
            r.check_result(
                "every release with a quarter has it in 1..=4",
                if releases
                    .iter()
                    .all(|rel| rel.quarter.is_none_or(|q| (1..=4).contains(&q)))
                {
                    Ok(())
                } else {
                    Err("one or more releases had an out-of-range quarter".to_owned())
                },
            );
        }
    }
    println!();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  Results: {} passed, {} failed", r.pass, r.fail);
    println!("─────────────────────────────────────────────────────────────────");

    if r.fail > 0 {
        std::process::exit(1);
    }
}
