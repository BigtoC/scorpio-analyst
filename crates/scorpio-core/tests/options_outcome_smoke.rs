//! Fixture-driven outcome smoke for [`YFinanceOptionsProvider`].
// Only compiled when the `test-helpers` feature is active (needed for
// `StubbedFinancialResponses` and `YFinanceClient::with_stubbed_financials`).
#![cfg(feature = "test-helpers")]
//
//!
//! Each test builds a minimal [`StubbedFinancialResponses`] corresponding to
//! one of the five scenarios documented in
//! `tests/fixtures/options_outcomes/*.json` and asserts the returned
//! [`OptionsOutcome`] variant matches the expected result.
//!
//! Run with:
//! ```sh
//! cargo nextest run -p scorpio-core --test options_outcome_smoke --features test-helpers
//! ```
//!
//! No LLM, no rig pipeline, no live network calls.

use std::collections::BTreeMap;

use chrono::TimeZone as _;
use chrono_tz::US::Eastern;
use scorpio_core::data::traits::options::OptionsOutcome;
use scorpio_core::data::{StubbedFinancialResponses, YFinanceClient, YFinanceOptionsProvider};
use scorpio_core::domain::Symbol;
use yfinance_rs::ticker::{OptionChain, OptionContract};

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn today_eastern() -> String {
    let now = Eastern.from_utc_datetime(&chrono::Utc::now().naive_utc());
    now.date_naive().format("%Y-%m-%d").to_string()
}

fn past_date(days_ago: i64) -> String {
    let now = Eastern.from_utc_datetime(&chrono::Utc::now().naive_utc());
    (now.date_naive() - chrono::Duration::days(days_ago))
        .format("%Y-%m-%d")
        .to_string()
}

fn make_candle(close: f64) -> scorpio_core::data::Candle {
    scorpio_core::data::Candle {
        date: today_eastern(),
        open: close,
        high: close + 1.0,
        low: close - 1.0,
        close,
        volume: Some(1_000_000),
    }
}

fn make_contract(
    strike: f64,
    iv: Option<f64>,
    volume: Option<u64>,
    oi: Option<u64>,
    expiry: &str,
) -> OptionContract {
    use paft_money::{Currency, IsoCurrency, Money};
    use rust_decimal::Decimal;

    let d = Decimal::try_from(strike).unwrap();
    let money = Money::new(d, Currency::Iso(IsoCurrency::USD)).unwrap();
    let exp_date =
        chrono::NaiveDate::parse_from_str(expiry, "%Y-%m-%d").expect("valid expiry date");

    OptionContract {
        contract_symbol: paft_domain::Symbol::new("SMOKE240101C00000000").unwrap(),
        strike: money,
        price: None,
        bid: None,
        ask: None,
        volume,
        open_interest: oi,
        implied_volatility: iv,
        in_the_money: false,
        expiration_date: exp_date,
        expiration_at: None,
        last_trade_at: None,
        greeks: None,
    }
}

fn make_provider(stub: StubbedFinancialResponses) -> YFinanceOptionsProvider {
    let client = YFinanceClient::with_stubbed_financials(stub);
    YFinanceOptionsProvider::new(client)
}

fn aapl() -> Symbol {
    use scorpio_core::domain::Ticker;
    Symbol::Equity(Ticker::parse("AAPL").unwrap())
}

// Fixed future expiry timestamp for reproducible fixtures.
fn expiry_ts() -> i64 {
    chrono::NaiveDate::from_ymd_opt(2030, 1, 18)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

// ─── Scenario: aapl_snapshot ──────────────────────────────────────────────────
//
// Fixture: tests/fixtures/options_outcomes/aapl_snapshot.json
// spot=$150, seven strikes centred on ATM, single expiry.
// Expected: Snapshot with non-empty near_term_strikes and plausible atm_iv.

#[tokio::test]
async fn aapl_snapshot_returns_snapshot_with_valid_payload() {
    let expiry = "2030-01-18";
    let ts = expiry_ts();
    let spot = 150.0;

    let mut chains = BTreeMap::new();
    chains.insert(
        ts,
        OptionChain {
            calls: vec![
                make_contract(142.5, Some(0.33), Some(50), Some(200), expiry),
                make_contract(145.0, Some(0.32), Some(60), Some(300), expiry),
                make_contract(147.5, Some(0.31), Some(80), Some(400), expiry),
                make_contract(150.0, Some(0.30), Some(100), Some(500), expiry),
                make_contract(152.5, Some(0.29), Some(80), Some(400), expiry),
                make_contract(155.0, Some(0.28), Some(60), Some(300), expiry),
                make_contract(157.5, Some(0.27), Some(50), Some(200), expiry),
            ],
            puts: vec![
                make_contract(142.5, Some(0.33), Some(50), Some(200), expiry),
                make_contract(145.0, Some(0.32), Some(60), Some(300), expiry),
                make_contract(147.5, Some(0.31), Some(80), Some(400), expiry),
                make_contract(150.0, Some(0.28), Some(80), Some(400), expiry),
                make_contract(152.5, Some(0.27), Some(80), Some(400), expiry),
                make_contract(155.0, Some(0.26), Some(60), Some(300), expiry),
                make_contract(157.5, Some(0.25), Some(50), Some(200), expiry),
            ],
        },
    );

    let provider = make_provider(StubbedFinancialResponses {
        ohlcv: Some(vec![make_candle(spot)]),
        option_expirations: Some(vec![ts]),
        option_chains: chains,
        ..StubbedFinancialResponses::default()
    });

    let outcome = provider
        .fetch_snapshot_impl(&aapl(), &today_eastern())
        .await
        .expect("should succeed");

    let snap = match outcome {
        OptionsOutcome::Snapshot(s) => s,
        other => panic!("expected Snapshot, got {other:?}"),
    };

    // Schema-level assertions (not value-level).
    assert!(snap.spot_price > 0.0, "spot_price must be positive");
    assert!(
        snap.atm_iv > 0.0 && snap.atm_iv < 5.0,
        "atm_iv={} is not plausible",
        snap.atm_iv
    );
    assert!(
        !snap.iv_term_structure.is_empty(),
        "iv_term_structure must be non-empty"
    );
    assert!(
        !snap.near_term_strikes.is_empty(),
        "near_term_strikes must be non-empty"
    );
    assert!(
        snap.max_pain_strike > 0.0,
        "max_pain_strike must be positive"
    );

    // Serialization: every expected top-level key is present in the JSON output.
    let json_val = serde_json::to_value(&snap).expect("Snapshot must serialize");
    let obj = json_val.as_object().expect("Snapshot serializes to object");
    for key in &[
        "spot_price",
        "atm_iv",
        "iv_term_structure",
        "put_call_volume_ratio",
        "put_call_oi_ratio",
        "max_pain_strike",
        "near_term_expiration",
        "near_term_strikes",
    ] {
        assert!(
            obj.contains_key(*key),
            "missing key in Snapshot JSON: {key}"
        );
    }
}

// ─── Scenario: no_listed ─────────────────────────────────────────────────────
//
// Fixture: tests/fixtures/options_outcomes/no_listed.json
// Empty expirations list → NoListedInstrument.

#[tokio::test]
async fn no_listed_returns_no_listed_instrument() {
    let provider = make_provider(StubbedFinancialResponses {
        ohlcv: Some(vec![make_candle(150.0)]),
        option_expirations: Some(vec![]),
        ..StubbedFinancialResponses::default()
    });

    let outcome = provider
        .fetch_snapshot_impl(&aapl(), &today_eastern())
        .await
        .expect("should succeed");

    assert_eq!(outcome, OptionsOutcome::NoListedInstrument);
}

// ─── Scenario: sparse_chain ───────────────────────────────────────────────────
//
// Fixture: tests/fixtures/options_outcomes/sparse_chain.json
// Strikes at $0.50, $1.00, $5.00 with spot $1.50. The OTM call at $5.00 is
// +233% of spot, past the ±20% cap. After capped expansion the call side has
// no qualifying strikes → SparseChain.

#[tokio::test]
async fn sparse_chain_returns_sparse_chain() {
    let expiry = "2030-01-18";
    let ts = expiry_ts();
    let spot = 1.50;

    let mut chains = BTreeMap::new();
    chains.insert(
        ts,
        OptionChain {
            calls: vec![
                make_contract(0.50, Some(0.80), Some(10), Some(50), expiry),
                make_contract(1.00, Some(0.60), Some(20), Some(100), expiry),
                make_contract(5.00, Some(0.40), Some(5), Some(20), expiry),
            ],
            puts: vec![
                make_contract(0.50, Some(0.80), Some(10), Some(50), expiry),
                make_contract(1.00, Some(0.60), Some(20), Some(100), expiry),
                make_contract(5.00, Some(0.40), Some(5), Some(20), expiry),
            ],
        },
    );

    let provider = make_provider(StubbedFinancialResponses {
        ohlcv: Some(vec![make_candle(spot)]),
        option_expirations: Some(vec![ts]),
        option_chains: chains,
        ..StubbedFinancialResponses::default()
    });

    let outcome = provider
        .fetch_snapshot_impl(&aapl(), &today_eastern())
        .await
        .expect("should succeed");

    assert_eq!(outcome, OptionsOutcome::SparseChain);
}

// ─── Scenario: historical_run ─────────────────────────────────────────────────
//
// Fixture: tests/fixtures/options_outcomes/historical_run.json
// target_date 30 days in the past → HistoricalRun.

#[tokio::test]
async fn historical_run_returns_historical_run() {
    let provider = make_provider(StubbedFinancialResponses {
        ohlcv: Some(vec![make_candle(150.0)]),
        option_expirations: Some(vec![expiry_ts()]),
        ..StubbedFinancialResponses::default()
    });

    let past = past_date(30);
    let outcome = provider
        .fetch_snapshot_impl(&aapl(), &past)
        .await
        .expect("should succeed");

    assert_eq!(outcome, OptionsOutcome::HistoricalRun);
}

// ─── Scenario: missing_spot ───────────────────────────────────────────────────
//
// Fixture: tests/fixtures/options_outcomes/missing_spot.json
// Empty OHLCV stub → MissingSpot.

#[tokio::test]
async fn missing_spot_returns_missing_spot() {
    let provider = make_provider(StubbedFinancialResponses {
        ohlcv: Some(vec![]),
        option_expirations: Some(vec![expiry_ts()]),
        ..StubbedFinancialResponses::default()
    });

    let outcome = provider
        .fetch_snapshot_impl(&aapl(), &today_eastern())
        .await
        .expect("should succeed");

    assert_eq!(outcome, OptionsOutcome::MissingSpot);
}
