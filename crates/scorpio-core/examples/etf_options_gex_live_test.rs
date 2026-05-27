//! Live smoke: full Stage 3 ETF Phase 2 dealer-positioning path.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute
//! `examples/`. Run manually with:
//!
//! ```sh
//! SCORPIO_FRED_API_KEY=... cargo run -p scorpio-core \
//!     --example etf_options_gex_live_test
//! ```
//!
//! Bypasses the LLM pipeline. Instead exercises the three pieces that
//! produce `EtfValuation.options_gex` from live data:
//!
//! 1. Risk-free rate fetch (FRED `DGS3MO` first, yfinance `^IRX` fallback)
//! 2. yfinance options-chain pull with `all_expirations` populated
//! 3. `compute_gex_summary` projection into the Stage 3 `GexSummary` shape
//!
//! Asserts the resulting summary carries near-term GEX, top-3 gamma walls,
//! a populated `broad`, `vex_summary`, and `cex_summary`.

use chrono::Utc;
use scorpio_core::config::ApiConfig;
use scorpio_core::data::traits::options::OptionsOutcome;
use scorpio_core::data::yfinance::{YFinanceClient, YFinanceOptionsProvider};
use scorpio_core::data::{Candle, FredClient};
use scorpio_core::domain::Symbol;
use scorpio_core::rate_limit::SharedRateLimiter;
use scorpio_core::state::EtfRiskFreeRateSource;
use scorpio_core::valuation::etf::premium_discount::compute_gex_summary;
use secrecy::SecretString;

const SYMBOL: &str = "SPY";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let today = Utc::now().date_naive();
    let today_iso = today.format("%Y-%m-%d").to_string();

    println!("─────────────────────────────────────────────────────────────────");
    println!("  ETF dealer-positioning live smoke ({SYMBOL} @ {today_iso})");
    println!("─────────────────────────────────────────────────────────────────");
    println!();

    // ── Risk-free rate ─────────────────────────────────────────────────────
    let yf_client = YFinanceClient::default();
    let (rate, source) = match resolve_risk_free_rate(&yf_client).await? {
        Some((rate, EtfRiskFreeRateSource::FredDgs3Mo)) => {
            println!("  Rate source : FRED DGS3MO ({:.2}%)", rate * 100.0);
            (rate, EtfRiskFreeRateSource::FredDgs3Mo)
        }
        Some((rate, EtfRiskFreeRateSource::YFinanceIrx)) => {
            println!("  Rate source : yfinance ^IRX ({:.2}%)", rate * 100.0);
            (rate, EtfRiskFreeRateSource::YFinanceIrx)
        }
        None => {
            return Err(
                "no live risk-free rate available — both FRED and yfinance ^IRX failed".into(),
            );
        }
    };

    // ── Options snapshot ───────────────────────────────────────────────────
    let symbol = Symbol::parse(SYMBOL)?;
    let provider = YFinanceOptionsProvider::new(yf_client);
    let snap = match provider.fetch_snapshot_impl(&symbol, &today_iso).await? {
        OptionsOutcome::Snapshot(s) => s,
        other => {
            return Err(
                format!("expected Snapshot(_) for {SYMBOL} options, got: {other:?}").into(),
            );
        }
    };
    println!(
        "  Options     : spot={:.2}, atm_iv={:.4}, near_term_strikes={}, all_expirations={}",
        snap.spot_price,
        snap.atm_iv,
        snap.near_term_strikes.len(),
        snap.all_expirations.len(),
    );
    assert!(snap.spot_price > 0.0, "spot must be positive");
    assert!(
        !snap.near_term_strikes.is_empty(),
        "near_term_strikes must be non-empty"
    );
    assert!(
        snap.all_expirations.len() >= 2,
        "broad input needs >= 2 non-front-month expirations, got {}",
        snap.all_expirations.len()
    );

    // ── compute_gex_summary ────────────────────────────────────────────────
    // SPY pays distributions; assume a ~1.3% dividend yield for the smoke
    // (the production path reads this from EtfComposition.distribution_yield_ttm_pct).
    let q = 0.013;
    let summary =
        compute_gex_summary(&snap, rate, q, today).ok_or("compute_gex_summary returned None")?;

    println!();
    println!(
        "  Near-term GEX (net)   : {:+.3e} USD per 1% move",
        summary.net_gex_usd_per_1pct_move
    );
    println!(
        "  Near-term GEX (gross) : {:.3e} USD per 1% move",
        summary.gross_gex_usd_per_1pct_move
    );
    println!("  Call/Put OI ratio     : {:.2}", summary.call_put_oi_ratio);
    println!("  Max-pain strike       : ${:.0}", summary.max_pain_strike);
    println!(
        "  Gamma walls           : {} entries",
        summary.strikes.len()
    );

    assert!(
        summary.net_gex_usd_per_1pct_move.is_finite(),
        "net GEX must be finite"
    );
    assert!(
        !summary.strikes.is_empty(),
        "expected gamma walls; got empty"
    );
    assert!(
        summary.strikes.len() <= 3,
        "should truncate to top-3, got {}",
        summary.strikes.len()
    );

    let broad = summary.broad.as_ref().ok_or("broad GEX must populate")?;
    println!(
        "  Broad GEX             : net={:+.3e}, gross={:.3e}, expirations_used={}/{}",
        broad.net_gex_usd_per_1pct_move,
        broad.gross_gex_usd_per_1pct_move,
        broad.expirations_used,
        broad.expirations_total_considered,
    );
    assert!(
        broad.expirations_used >= 2,
        "broad must aggregate >= 2 expirations"
    );

    let vex = summary
        .vex_summary
        .as_ref()
        .ok_or("vex_summary must populate")?;
    println!(
        "  VEX                   : net={:+.3e}/volpt, gross={:.3e}/volpt",
        vex.net_vex_usd_per_volpt, vex.gross_vex_usd_per_volpt
    );
    assert!(
        vex.gross_vex_usd_per_volpt >= vex.net_vex_usd_per_volpt.abs(),
        "gross VEX must dominate |net VEX|"
    );

    let cex = summary
        .cex_summary
        .as_ref()
        .ok_or("cex_summary must populate")?;
    println!(
        "  CEX                   : net={:+.3e}/day, gross={:.3e}/day",
        cex.net_cex_usd_per_day, cex.gross_cex_usd_per_day
    );
    assert!(
        cex.gross_cex_usd_per_day >= cex.net_cex_usd_per_day.abs(),
        "gross CEX must dominate |net CEX|"
    );

    println!();
    println!("  source = {source:?}");
    println!("─────────────────────────────────────────────────────────────────");
    println!("  OK");
    println!("─────────────────────────────────────────────────────────────────");
    Ok(())
}

/// FRED `DGS3MO` first; yfinance `^IRX` close as fallback. Returns `None`
/// when both sources are unavailable.
async fn resolve_risk_free_rate(
    yf: &YFinanceClient,
) -> Result<Option<(f64, EtfRiskFreeRateSource)>, Box<dyn std::error::Error>> {
    if let Ok(fred_key) = std::env::var("SCORPIO_FRED_API_KEY") {
        let fred = FredClient::new(
            &ApiConfig {
                fred_api_key: Some(SecretString::from(fred_key)),
                finnhub_api_key: None,
                alpha_vantage_api_key: None,
            },
            SharedRateLimiter::new("fred", 10),
        )?;
        if let Ok(Some(pct)) = fred.get_series_latest("DGS3MO").await {
            return Ok(Some((pct / 100.0, EtfRiskFreeRateSource::FredDgs3Mo)));
        }
    }

    let today = Utc::now().date_naive();
    let start = today - chrono::Duration::days(14);
    let candles: Vec<Candle> = yf
        .get_ohlcv("^IRX", &start.to_string(), &today.to_string())
        .await?;
    if let Some(close) = candles.last().map(|c| c.close) {
        return Ok(Some((close / 100.0, EtfRiskFreeRateSource::YFinanceIrx)));
    }

    Ok(None)
}
