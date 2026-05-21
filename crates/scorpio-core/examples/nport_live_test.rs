//! Manual live smoke: SEC EDGAR N-PORT-P fetch.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute
//! `examples/`. Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example nport_live_test
//! ```
//!
//! Exercises the fund-CIK + N-PORT-P methods added in Task 9:
//! - `SecEdgarClient::resolve_fund_cik` (ticker → zero-padded CIK)
//! - `SecEdgarClient::fetch_latest_nport_p` (latest holdings inside a window)
//!
//! Construction mirrors `build_default_sec_edgar_client` in
//! `workflow/pipeline/mod.rs` (10 rps under the `"sec-edgar"` label).

use std::sync::Arc;

use scorpio_core::data::SecEdgarClient;
use scorpio_core::rate_limit::SharedRateLimiter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .init();

    // Mirrors `build_default_sec_edgar_client` in `workflow/pipeline/mod.rs`.
    let edgar = Arc::new(
        SecEdgarClient::new(SharedRateLimiter::new("sec-edgar", 10))
            .expect("SecEdgarClient construction must succeed (reqwest builder)"),
    );

    for symbol in ["SPY", "QQQ", "BOGUS_FUND"] {
        let cik = edgar.resolve_fund_cik(symbol).await;
        println!("\n=== {symbol} ===");
        println!("CIK: {cik:?}");
        if let Some(cik) = cik {
            let holdings = edgar.fetch_latest_nport_p(&cik, 180).await;
            match holdings {
                Some(h) => {
                    println!(
                        "filing_date={} holdings_count={} sectors={}",
                        h.filing_date,
                        h.holdings.len(),
                        h.sector_breakdown.len()
                    );
                    if let Some(h0) = h.holdings.first() {
                        println!("first holding: {} ({:.2}%)", h0.name, h0.weight_pct);
                    }
                }
                None => println!("no N-PORT-P available within 180 days"),
            }
        }
    }
}
