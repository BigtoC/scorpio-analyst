//! Manual live smoke: end-to-end runtime classification + pack routing.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute
//! `examples/`. Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example etf_pack_live_test
//! ```
//!
//! Confirms that the Task 11 classifier handles real Yahoo responses end-to-end:
//! - `SPY`   → `EtfBaseline`
//! - `AAPL`  → `BaselineMatched`
//! - `BOGUS` → `BaselineFallback { reason: "profile_lookup_unavailable" }`
//!
//! Unexpected routings are logged to stderr but do not abort the run; the
//! purpose of this example is to surface real-world classifier behavior, not
//! to gate CI.

use scorpio_core::data::YFinanceClient;
use scorpio_core::workflow::{RuntimePackSelection, classify_runtime_pack};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .init();

    let yf = YFinanceClient::default();

    for symbol in ["SPY", "AAPL", "BOGUS"] {
        let profile = yf.get_profile(symbol).await;
        let fund_info = yf.get_fund_info(symbol).await;
        let result = classify_runtime_pack(profile.as_ref(), fund_info.as_ref());

        println!(
            "{symbol:<8} -> pack={:?} fallback={:?}",
            result.pack_id(),
            result.fallback_reason()
        );

        match (symbol, &result) {
            ("SPY", RuntimePackSelection::EtfBaseline) => {}
            ("AAPL", RuntimePackSelection::BaselineMatched) => {}
            ("BOGUS", RuntimePackSelection::BaselineFallback { reason })
                if *reason == "profile_lookup_unavailable" => {}
            (s, r) => eprintln!("unexpected routing for {s}: {r:?}"),
        }
    }
}
