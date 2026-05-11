//! Live FRED API smoke test.
//!
//! **NOT run automatically in CI** - `cargo nextest` does not execute `examples/`.
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example fred_live_test
//! ```
//!
//! Requires:
//! - a live internet connection
//! - `SCORPIO_FRED_API_KEY` to be set in the environment
//!
//! Covers every public FRED client method currently exposed from
//! `crates/scorpio-core/src/data/fred.rs`:
//! - `FredClient::get_series_latest` (FEDFUNDS + CPI series)
//! - `FredClient::get_economic_indicators`
//! - `FredClient::release_dates` for every `release_id::*` constant

use chrono::{Duration, Utc};
use scorpio_core::{
    config::ApiConfig,
    data::{fred::release_id, FredClient},
    rate_limit::SharedRateLimiter,
};
use secrecy::SecretString;

/// Number of calendar days for the release-dates look-back window.
const LOOKBACK_DAYS: i64 = 60;

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
    let api_key = match required_env("SCORPIO_FRED_API_KEY") {
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
    println!("  FRED live API smoke test");
    println!("─────────────────────────────────────────────────────────────────");
    println!("  Window : {from} → {to}");
    println!("─────────────────────────────────────────────────────────────────");
    println!();

    let client = FredClient::new(
        &ApiConfig {
            fred_api_key: Some(SecretString::from(api_key)),
            finnhub_api_key: None,
        },
        SharedRateLimiter::new("fred", 10),
    )
    .expect("client should construct with API key");

    let mut r = Results::new();

    section(1, "FredClient::get_series_latest (FEDFUNDS)");
    match client.get_series_latest("FEDFUNDS").await {
        Err(e) => {
            eprintln!("  FAIL  get_series_latest(FEDFUNDS) returned error: {e}");
            r.fail += 1;
        }
        Ok(value) => {
            info(&format!("value: {value:?}"));
            r.check_result(
                "get_series_latest(FEDFUNDS) returns Some(f64)",
                match value {
                    Some(v) if v.is_finite() => Ok(()),
                    Some(v) => Err(format!("non-finite value: {v}")),
                    None => Err("got None — series missing or all-dot".to_owned()),
                },
            );
        }
    }
    println!();

    section(2, "FredClient::get_series_latest (CPI — CPALTT01USM657N)");
    match client.get_series_latest("CPALTT01USM657N").await {
        Err(e) => {
            eprintln!("  FAIL  get_series_latest(CPI) returned error: {e}");
            r.fail += 1;
        }
        Ok(value) => {
            info(&format!("value: {value:?}"));
            r.check_result(
                "get_series_latest(CPALTT01USM657N) returns Some(f64)",
                match value {
                    Some(v) if v.is_finite() => Ok(()),
                    Some(v) => Err(format!("non-finite value: {v}")),
                    None => Err("got None — series missing or all-dot".to_owned()),
                },
            );
        }
    }
    println!();

    section(3, "FredClient::get_economic_indicators");
    match client.get_economic_indicators().await {
        Err(e) => {
            eprintln!("  FAIL  get_economic_indicators returned error: {e}");
            r.fail += 1;
        }
        Ok(events) => {
            info(&format!("{} macro event(s)", events.len()));
            for event in &events {
                info(&format!(
                    "  {} → {:?} (confidence {:.1})",
                    event.event, event.impact_direction, event.confidence
                ));
            }
            r.check(
                "get_economic_indicators returns at least one macro event",
                !events.is_empty(),
            );
        }
    }
    println!();

    section(4, "FredClient::release_dates — all release_id::* constants");
    for (label, id) in [
        ("CPI", release_id::CPI),
        ("Nonfarm Payrolls", release_id::NONFARM_PAYROLLS),
        ("FOMC decision", release_id::FOMC_DECISION),
        ("GDP", release_id::GDP),
        ("ISM Manufacturing", release_id::ISM_MANUFACTURING),
        ("Retail Sales", release_id::RETAIL_SALES),
    ] {
        match client.release_dates(id, &from, &to).await {
            Err(e) => {
                eprintln!(
                    "  FAIL  release_dates({label}, id={id}) returned error: {e}"
                );
                r.fail += 1;
            }
            Ok(dates) => {
                info(&format!(
                    "release_dates({label}, id={id}): {} date(s) — {dates:?}",
                    dates.len()
                ));
                r.check_result(
                    &format!("release_dates({label}) returns >= 1 row in {LOOKBACK_DAYS}-day window"),
                    if !dates.is_empty() {
                        Ok(())
                    } else {
                        Err(format!(
                            "got 0 dates for release_id={id} ({label}) in {from}..{to} — verify the ID is correct"
                        ))
                    },
                );
            }
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
