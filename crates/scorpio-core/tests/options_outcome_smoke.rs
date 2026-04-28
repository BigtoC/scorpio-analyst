//! Fixture-driven outcome smoke for the serialized `get_options_snapshot` tool path.
#![cfg(feature = "test-helpers")]

use std::{collections::BTreeMap, fs, path::PathBuf};

use chrono::TimeZone as _;
use chrono_tz::US::Eastern;
use scorpio_core::data::traits::options::OptionsOutcome;
use scorpio_core::data::yfinance::options::OptionsSnapshotArgs;
use scorpio_core::data::{
    GetOptionsSnapshot, StubbedFinancialResponses, YFinanceClient, YFinanceOptionsProvider,
};
use scorpio_core::domain::Symbol;
use serde::Deserialize;
use yfinance_rs::ticker::{OptionChain, OptionContract};

#[derive(Debug, Deserialize)]
struct OutcomeFixture {
    scenario: String,
    expected_outcome: String,
    description: String,
    setup: OutcomeFixtureSetup,
}

#[derive(Debug, Deserialize)]
struct OutcomeFixtureSetup {
    #[serde(default)]
    spot_price: Option<f64>,
    target_date_offset_days: i64,
    #[serde(default)]
    front_expiry_ts: Option<i64>,
    #[serde(default)]
    strikes: Vec<f64>,
    #[serde(default)]
    atm_call_iv: Option<f64>,
    #[serde(default)]
    atm_put_iv: Option<f64>,
    #[serde(default)]
    option_expirations: Option<Vec<i64>>,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("options_outcomes")
}

fn load_fixture(name: &str) -> OutcomeFixture {
    let path = fixtures_dir().join(name);
    let json = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("failed to parse fixture {}: {error}", path.display()))
}

fn today_eastern() -> chrono::NaiveDate {
    let now = Eastern.from_utc_datetime(&chrono::Utc::now().naive_utc());
    now.date_naive()
}

fn target_date_from_offset(days: i64) -> String {
    (today_eastern() + chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

fn make_candle(close: f64) -> scorpio_core::data::Candle {
    scorpio_core::data::Candle {
        date: target_date_from_offset(0),
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

fn default_expiry_ts() -> i64 {
    chrono::NaiveDate::from_ymd_opt(2030, 1, 18)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp()
}

fn expiry_date_from_ts(ts: i64) -> String {
    chrono::Utc
        .timestamp_opt(ts, 0)
        .single()
        .expect("valid expiry timestamp")
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn snapshot_stub(fixture: &OutcomeFixture) -> StubbedFinancialResponses {
    let spot = fixture
        .setup
        .spot_price
        .expect("snapshot fixture must define spot_price");
    let ts = fixture
        .setup
        .front_expiry_ts
        .unwrap_or_else(default_expiry_ts);
    let expiry = expiry_date_from_ts(ts);
    let strikes = &fixture.setup.strikes;
    let atm_index = strikes
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - spot)
                .abs()
                .partial_cmp(&(*b - spot).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(idx, _)| idx)
        .expect("snapshot fixture must include strikes");
    let atm_call_iv = fixture.setup.atm_call_iv.unwrap_or(0.30);
    let atm_put_iv = fixture.setup.atm_put_iv.unwrap_or(0.28);

    let mut chains = BTreeMap::new();
    chains.insert(
        ts,
        OptionChain {
            calls: strikes
                .iter()
                .enumerate()
                .map(|(idx, strike)| {
                    let step = idx.abs_diff(atm_index) as f64;
                    make_contract(
                        *strike,
                        Some(atm_call_iv + ((atm_index as isize - idx as isize) as f64 * 0.01)),
                        Some((100usize.saturating_sub((step as usize) * 10)) as u64),
                        Some((500usize.saturating_sub((step as usize) * 100)) as u64),
                        &expiry,
                    )
                })
                .collect(),
            puts: strikes
                .iter()
                .enumerate()
                .map(|(idx, strike)| {
                    let step = idx.abs_diff(atm_index) as f64;
                    make_contract(
                        *strike,
                        Some(atm_put_iv + ((atm_index as isize - idx as isize) as f64 * 0.01)),
                        Some((80usize.saturating_sub((step as usize) * 10)) as u64),
                        Some((400usize.saturating_sub((step as usize) * 50)) as u64),
                        &expiry,
                    )
                })
                .collect(),
        },
    );

    StubbedFinancialResponses {
        ohlcv: Some(vec![make_candle(spot)]),
        option_expirations: Some(vec![ts]),
        option_chains: chains,
        ..StubbedFinancialResponses::default()
    }
}

fn sparse_chain_stub(fixture: &OutcomeFixture) -> StubbedFinancialResponses {
    let spot = fixture
        .setup
        .spot_price
        .expect("sparse fixture must define spot_price");
    let ts = fixture
        .setup
        .front_expiry_ts
        .unwrap_or_else(default_expiry_ts);
    let expiry = expiry_date_from_ts(ts);

    let mut chains = BTreeMap::new();
    chains.insert(
        ts,
        OptionChain {
            calls: fixture
                .setup
                .strikes
                .iter()
                .map(|strike| make_contract(*strike, Some(0.60), Some(10), Some(50), &expiry))
                .collect(),
            puts: fixture
                .setup
                .strikes
                .iter()
                .map(|strike| make_contract(*strike, Some(0.60), Some(10), Some(50), &expiry))
                .collect(),
        },
    );

    StubbedFinancialResponses {
        ohlcv: Some(vec![make_candle(spot)]),
        option_expirations: Some(vec![ts]),
        option_chains: chains,
        ..StubbedFinancialResponses::default()
    }
}

fn stub_for_fixture(fixture: &OutcomeFixture) -> StubbedFinancialResponses {
    match fixture.scenario.as_str() {
        "aapl_snapshot" => snapshot_stub(fixture),
        "no_listed" => StubbedFinancialResponses {
            ohlcv: fixture.setup.spot_price.map(|spot| vec![make_candle(spot)]),
            option_expirations: Some(fixture.setup.option_expirations.clone().unwrap_or_default()),
            ..StubbedFinancialResponses::default()
        },
        "sparse_chain" => sparse_chain_stub(fixture),
        "historical_run" => StubbedFinancialResponses {
            ohlcv: fixture.setup.spot_price.map(|spot| vec![make_candle(spot)]),
            ..StubbedFinancialResponses::default()
        },
        "missing_spot" => StubbedFinancialResponses {
            ohlcv: Some(vec![]),
            option_expirations: Some(vec![
                fixture
                    .setup
                    .front_expiry_ts
                    .unwrap_or_else(default_expiry_ts),
            ]),
            ..StubbedFinancialResponses::default()
        },
        other => panic!("unsupported options smoke fixture scenario: {other}"),
    }
}

async fn call_tool_for_fixture(name: &str) -> (OutcomeFixture, serde_json::Value) {
    let fixture = load_fixture(name);
    let target_date = target_date_from_offset(fixture.setup.target_date_offset_days);
    let tool = GetOptionsSnapshot::scoped(
        make_provider(stub_for_fixture(&fixture)),
        "AAPL",
        target_date.clone(),
    );

    let output = rig::tool::Tool::call(
        &tool,
        OptionsSnapshotArgs {
            symbol: "AAPL".to_owned(),
            target_date,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("tool call failed for fixture {}: {error}", fixture.scenario));

    (fixture, output)
}

fn assert_snapshot_shape(output: &serde_json::Value, scenario: &str) {
    let obj = output
        .as_object()
        .unwrap_or_else(|| panic!("snapshot tool output must be an object for {scenario}"));
    for key in [
        "kind",
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
            obj.contains_key(key),
            "snapshot tool output missing key {key} for fixture {scenario}: {output}"
        );
    }
    assert_eq!(obj.get("kind").and_then(|v| v.as_str()), Some("snapshot"));
    assert!(
        obj.get("reason").is_none(),
        "snapshot output must not include a failure reason"
    );
}

fn assert_non_snapshot_kind(output: &serde_json::Value, expected_kind: &str, scenario: &str) {
    assert_eq!(
        output.get("kind").and_then(|v| v.as_str()),
        Some(expected_kind),
        "tool output kind mismatch for fixture {scenario}: {output}"
    );
    assert!(
        output
            .get("reason")
            .and_then(|value| value.as_str())
            .is_some_and(|reason| !reason.trim().is_empty()),
        "non-snapshot output must include a human-readable reason for fixture {scenario}: {output}"
    );
}

#[tokio::test]
async fn aapl_snapshot_fixture_serializes_snapshot_tool_output() {
    let (fixture, output) = call_tool_for_fixture("aapl_snapshot.json").await;

    assert_eq!(
        fixture.expected_outcome, "snapshot",
        "{}",
        fixture.description
    );
    assert_snapshot_shape(&output, &fixture.scenario);
}

#[tokio::test]
async fn no_listed_fixture_serializes_no_listed_tool_output() {
    let (fixture, output) = call_tool_for_fixture("no_listed.json").await;

    assert_eq!(
        fixture.expected_outcome, "no_listed_instrument",
        "{}",
        fixture.description
    );
    assert_non_snapshot_kind(&output, "no_listed_instrument", &fixture.scenario);
}

#[tokio::test]
async fn sparse_chain_fixture_serializes_sparse_chain_tool_output() {
    let (fixture, output) = call_tool_for_fixture("sparse_chain.json").await;

    assert_eq!(
        fixture.expected_outcome, "sparse_chain",
        "{}",
        fixture.description
    );
    assert_non_snapshot_kind(&output, "sparse_chain", &fixture.scenario);
}

#[tokio::test]
async fn historical_run_fixture_serializes_historical_run_tool_output() {
    let (fixture, output) = call_tool_for_fixture("historical_run.json").await;

    assert_eq!(
        fixture.expected_outcome, "historical_run",
        "{}",
        fixture.description
    );
    assert_non_snapshot_kind(&output, "historical_run", &fixture.scenario);
}

#[tokio::test]
async fn missing_spot_fixture_serializes_missing_spot_tool_output() {
    let (fixture, output) = call_tool_for_fixture("missing_spot.json").await;

    assert_eq!(
        fixture.expected_outcome, "missing_spot",
        "{}",
        fixture.description
    );
    assert_non_snapshot_kind(&output, "missing_spot", &fixture.scenario);
}

#[test]
fn outcome_fixture_paths_cover_all_expected_scenarios() {
    let expected = [
        "aapl_snapshot.json",
        "historical_run.json",
        "missing_spot.json",
        "no_listed.json",
        "sparse_chain.json",
    ];

    for name in expected {
        assert!(
            fixtures_dir().join(name).exists(),
            "missing fixture file {name}"
        );
    }

    let _ = OptionsOutcome::HistoricalRun;
    let _ = aapl();
}
