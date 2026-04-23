//! Live Yahoo Finance API smoke test.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute `examples/`.
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example yfinance_live_test
//! ```
//!
//! Requires a live internet connection to reach the Yahoo Finance API.
//!
//! Covers every public method currently in `crates/scorpio-core/src/data/yfinance/`:
//! - `YFinanceClient::get_ohlcv` (OHLCV bars)
//! - `get_latest_close` (derived price query)
//! - `fetch_vix_data` (VIX volatility snapshot)
//! - `YFinanceClient::get_quarterly_cashflow`
//! - `YFinanceClient::get_quarterly_balance_sheet`
//! - `YFinanceClient::get_quarterly_income_stmt`
//! - `YFinanceClient::get_quarterly_shares`
//! - `YFinanceClient::get_earnings_trend`
//! - `YFinanceClient::get_profile`
//!
//! Also exercises the ETF degradation path via `SPY` to confirm that
//! financial statement fetchers return `None`/empty gracefully and that
//! `get_profile` returns `Profile::Fund` (or degrades without panicking).

use chrono::{Duration, NaiveDate, Utc};
use scorpio_core::data::{YFinanceClient, fetch_vix_data, get_latest_close};
use yfinance_rs::profile::Profile;

/// Well-known liquid equity used as the primary test subject.
const EQUITY_SYMBOL: &str = "AAPL";
/// Well-known ETF used to validate the fund-style degradation path.
const ETF_SYMBOL: &str = "SPY";
/// Number of calendar days in the look-back window used for OHLCV checks.
const LOOKBACK_DAYS: i64 = 30;

// ─── Pass/fail tracker ────────────────────────────────────────────────────────

struct Results {
    pass: usize,
    fail: usize,
}

impl Results {
    fn new() -> Self {
        Self { pass: 0, fail: 0 }
    }

    /// Record a boolean check, printing a labelled PASS or FAIL line.
    fn check(&mut self, label: &str, ok: bool) {
        if ok {
            println!("  PASS  {label}");
            self.pass += 1;
        } else {
            eprintln!("  FAIL  {label}");
            self.fail += 1;
        }
    }

    /// Record a check whose result comes from a `Result`, printing the error
    /// string on failure so the root cause is immediately visible.
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

// ─── Section helpers ──────────────────────────────────────────────────────────

fn section(n: usize, title: &str) {
    println!("[{n}] {title}");
}

fn info(msg: &str) {
    println!("        {msg}");
}

fn warn(msg: &str) {
    println!("  WARN  {msg}");
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Surface warnings from the yfinance layer without requiring RUST_LOG=debug.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .init();

    let today = Utc::now().date_naive();
    let start = (today - Duration::days(LOOKBACK_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let end = today.format("%Y-%m-%d").to_string();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  Yahoo Finance live API smoke test");
    println!("─────────────────────────────────────────────────────────────────");
    println!("  Equity : {EQUITY_SYMBOL}");
    println!("  ETF    : {ETF_SYMBOL}");
    println!("  Window : {start} → {end}");
    println!("─────────────────────────────────────────────────────────────────");
    println!();

    let client = YFinanceClient::default();
    let mut r = Results::new();

    // ── 1. get_ohlcv ─────────────────────────────────────────────────────────

    section(1, &format!("YFinanceClient::get_ohlcv ({EQUITY_SYMBOL})"));

    match client.get_ohlcv(EQUITY_SYMBOL, &start, &end).await {
        Err(e) => {
            eprintln!("  FAIL  get_ohlcv returned error: {e}");
            r.fail += 1;
        }
        Ok(candles) => {
            info(&format!("{} candles returned", candles.len()));

            r.check("returns a non-empty Vec<Candle>", !candles.is_empty());

            // Validate that every date string parses as YYYY-MM-DD.
            let bad_dates: Vec<&str> = candles
                .iter()
                .filter(|c| NaiveDate::parse_from_str(&c.date, "%Y-%m-%d").is_err())
                .map(|c| c.date.as_str())
                .collect();
            r.check_result(
                "all candle dates are in YYYY-MM-DD format",
                if bad_dates.is_empty() {
                    Ok(())
                } else {
                    Err(format!("unparseable dates: {bad_dates:?}"))
                },
            );

            // All OHLC prices must be strictly positive; volume is optional.
            let non_positive: Vec<&str> = candles
                .iter()
                .filter(|c| c.open <= 0.0 || c.high <= 0.0 || c.low <= 0.0 || c.close <= 0.0)
                .map(|c| c.date.as_str())
                .collect();
            r.check_result(
                "all candle OHLC values are positive",
                if non_positive.is_empty() {
                    Ok(())
                } else {
                    Err(format!("non-positive OHLC on dates: {non_positive:?}"))
                },
            );
        }
    }
    println!();

    // ── 2. get_latest_close ──────────────────────────────────────────────────

    section(2, &format!("get_latest_close ({EQUITY_SYMBOL})"));

    match get_latest_close(&client, EQUITY_SYMBOL, &end).await {
        None => {
            eprintln!("  FAIL  get_latest_close returned None");
            r.fail += 1;
        }
        Some(price) => {
            info(&format!("latest close = {price:.2}"));
            r.check("returns Some(price) with price > 0.0", price > 0.0);
        }
    }
    println!();

    // ── 3. fetch_vix_data ────────────────────────────────────────────────────

    section(3, "fetch_vix_data (^VIX)");

    match fetch_vix_data(&client, &end).await {
        None => {
            eprintln!("  FAIL  fetch_vix_data returned None");
            r.fail += 1;
        }
        Some(vix) => {
            info(&format!(
                "vix_level={:.2}, regime={:?}, trend={:?}",
                vix.vix_level, vix.vix_regime, vix.vix_trend
            ));
            r.check_result(
                "vix_level is in plausible range (1.0–100.0)",
                if vix.vix_level > 1.0 && vix.vix_level < 100.0 {
                    Ok(())
                } else {
                    Err(format!("vix_level={}", vix.vix_level))
                },
            );
        }
    }
    println!();

    // ── 4. Financial statement fetchers (AAPL) ───────────────────────────────

    section(
        4,
        &format!("Financial statement fetchers ({EQUITY_SYMBOL})"),
    );

    match client.get_quarterly_cashflow(EQUITY_SYMBOL).await {
        None => {
            eprintln!("  FAIL  get_quarterly_cashflow returned None");
            r.fail += 1;
        }
        Some(rows) => {
            info(&format!("cashflow: {} rows", rows.len()));
            r.check(
                "get_quarterly_cashflow returns non-empty vec",
                !rows.is_empty(),
            );
        }
    }

    match client.get_quarterly_balance_sheet(EQUITY_SYMBOL).await {
        None => {
            eprintln!("  FAIL  get_quarterly_balance_sheet returned None");
            r.fail += 1;
        }
        Some(rows) => {
            info(&format!("balance_sheet: {} rows", rows.len()));
            r.check(
                "get_quarterly_balance_sheet returns non-empty vec",
                !rows.is_empty(),
            );
        }
    }

    match client.get_quarterly_income_stmt(EQUITY_SYMBOL).await {
        None => {
            eprintln!("  FAIL  get_quarterly_income_stmt returned None");
            r.fail += 1;
        }
        Some(rows) => {
            info(&format!("income_stmt: {} rows", rows.len()));
            r.check(
                "get_quarterly_income_stmt returns non-empty vec",
                !rows.is_empty(),
            );
        }
    }

    match client.get_quarterly_shares(EQUITY_SYMBOL).await {
        None => {
            eprintln!("  FAIL  get_quarterly_shares returned None");
            r.fail += 1;
        }
        Some(rows) => {
            info(&format!("shares: {} rows", rows.len()));
            r.check(
                "get_quarterly_shares returns non-empty vec",
                !rows.is_empty(),
            );
        }
    }

    match client.get_earnings_trend(EQUITY_SYMBOL).await {
        None => {
            eprintln!("  FAIL  get_earnings_trend returned None");
            r.fail += 1;
        }
        Some(rows) => {
            info(&format!("earnings_trend: {} rows", rows.len()));
            r.check("get_earnings_trend returns non-empty vec", !rows.is_empty());
        }
    }
    println!();

    // ── 5. get_profile (AAPL equity) ─────────────────────────────────────────

    section(5, &format!("YFinanceClient::get_profile ({EQUITY_SYMBOL})"));

    match client.get_profile(EQUITY_SYMBOL).await {
        None => {
            eprintln!("  FAIL  get_profile returned None for equity symbol");
            r.fail += 1;
        }
        Some(profile) => {
            let is_company = matches!(profile, Profile::Company(_));
            info(&format!(
                "profile type: {}",
                if is_company { "Company" } else { "Fund" }
            ));
            r.check("get_profile returns Some(_) for equity symbol", true);
            r.check("AAPL profile is Profile::Company", is_company);
        }
    }
    println!();

    // ── 6. ETF degradation path (SPY) ────────────────────────────────────────

    section(6, &format!("ETF degradation path ({ETF_SYMBOL})"));

    // Profile: expect Profile::Fund for SPY; None is an acceptable degradation
    // if Yahoo returns an error.
    match client.get_profile(ETF_SYMBOL).await {
        None => {
            warn("get_profile returned None for ETF symbol (acceptable degradation)");
        }
        Some(profile) => {
            let is_fund = matches!(profile, Profile::Fund(_));
            info(&format!(
                "profile type: {}",
                if is_fund { "Fund" } else { "Company" }
            ));
            r.check("SPY profile is Profile::Fund", is_fund);
        }
    }

    // Financial statement fetchers for SPY must complete without panicking.
    // None or empty results are domain-valid for fund instruments.
    let cashflow = client.get_quarterly_cashflow(ETF_SYMBOL).await;
    r.check(
        "get_quarterly_cashflow(SPY) completes without panic",
        true, // reaching this line proves no panic occurred
    );
    info(&format!(
        "SPY cashflow: {}",
        match &cashflow {
            Some(rows) => format!("{} rows", rows.len()),
            None => "None (expected for ETF)".to_owned(),
        }
    ));

    let balance = client.get_quarterly_balance_sheet(ETF_SYMBOL).await;
    r.check(
        "get_quarterly_balance_sheet(SPY) completes without panic",
        true,
    );
    info(&format!(
        "SPY balance_sheet: {}",
        match &balance {
            Some(rows) => format!("{} rows", rows.len()),
            None => "None (expected for ETF)".to_owned(),
        }
    ));

    let income = client.get_quarterly_income_stmt(ETF_SYMBOL).await;
    r.check(
        "get_quarterly_income_stmt(SPY) completes without panic",
        true,
    );
    info(&format!(
        "SPY income_stmt: {}",
        match &income {
            Some(rows) => format!("{} rows", rows.len()),
            None => "None (expected for ETF)".to_owned(),
        }
    ));

    let shares = client.get_quarterly_shares(ETF_SYMBOL).await;
    r.check("get_quarterly_shares(SPY) completes without panic", true);
    info(&format!(
        "SPY shares: {}",
        match &shares {
            Some(rows) => format!("{} rows", rows.len()),
            None => "None (expected for ETF)".to_owned(),
        }
    ));

    let trend = client.get_earnings_trend(ETF_SYMBOL).await;
    r.check("get_earnings_trend(SPY) completes without panic", true);
    info(&format!(
        "SPY earnings_trend: {}",
        match &trend {
            Some(rows) => format!("{} rows", rows.len()),
            None => "None (expected for ETF)".to_owned(),
        }
    ));
    println!();

    // ── Summary ───────────────────────────────────────────────────────────────

    println!("─────────────────────────────────────────────────────────────────");
    println!("  Results: {} passed, {} failed", r.pass, r.fail);
    println!("─────────────────────────────────────────────────────────────────");

    if r.fail > 0 {
        std::process::exit(1);
    }
}
