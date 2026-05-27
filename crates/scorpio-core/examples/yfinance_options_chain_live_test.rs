//! Live smoke: yfinance options chain populates `OptionsSnapshot.all_expirations`.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute `examples/`.
//! Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example yfinance_options_chain_live_test
//! ```
//!
//! Asserts that the Stage 3 broad-GEX input — the transient
//! `OptionsSnapshot.all_expirations` field — populates with at least two
//! non-front-month expirations during a live SPY pull, and that none of
//! those expirations duplicates the front-month slice.

use chrono::Utc;
use scorpio_core::data::traits::options::OptionsOutcome;
use scorpio_core::data::yfinance::{YFinanceClient, YFinanceOptionsProvider};
use scorpio_core::domain::Symbol;

const SYMBOL: &str = "SPY";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  yfinance options-chain live smoke ({SYMBOL})");
    println!("─────────────────────────────────────────────────────────────────");
    println!("  Date: {today}");
    println!();

    let symbol = Symbol::parse(SYMBOL)?;
    let client = YFinanceClient::default();
    let provider = YFinanceOptionsProvider::new(client);

    let outcome = provider.fetch_snapshot_impl(&symbol, &today).await?;
    let snap = match outcome {
        OptionsOutcome::Snapshot(s) => s,
        other => {
            return Err(format!(
                "expected Snapshot(_) for {SYMBOL} options, got: {other:?}"
            )
            .into());
        }
    };

    println!("  near_term_expiration = {}", snap.near_term_expiration);
    println!("  near_term_strikes    = {}", snap.near_term_strikes.len());
    println!("  all_expirations      = {}", snap.all_expirations.len());

    assert!(
        snap.all_expirations.len() >= 2,
        "expected >= 2 additional expirations on {SYMBOL}, got {}",
        snap.all_expirations.len()
    );

    for extra in &snap.all_expirations {
        println!(
            "    {} → {} strikes",
            extra.expiration,
            extra.strikes.len()
        );
        assert_ne!(
            extra.expiration, snap.near_term_expiration,
            "all_expirations must not include the front-month slice"
        );
        assert!(
            !extra.strikes.is_empty(),
            "expiration {} has no strikes",
            extra.expiration
        );
    }

    // Negative sanity: a bogus ticker yields a non-Snapshot outcome (no panic).
    let bogus = Symbol::parse("ZZZZZZZ").expect("ZZZZZZZ parses as a Symbol");
    let bogus_outcome = provider.fetch_snapshot_impl(&bogus, &today).await?;
    assert!(
        !matches!(bogus_outcome, OptionsOutcome::Snapshot(_)),
        "bogus ticker must not produce a Snapshot, got: {bogus_outcome:?}"
    );

    println!();
    println!("─────────────────────────────────────────────────────────────────");
    println!("  OK");
    println!("─────────────────────────────────────────────────────────────────");
    Ok(())
}
