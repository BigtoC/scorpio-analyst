//! Live smoke for every ETF-related surface, run sequentially in four
//! sections. Replaces and consolidates the prior separate examples:
//!
//! - `etf_quote_live_test`       → §1 ETF surface
//! - `etf_data_gap_live_test`    → §2 NAV / bid / ask / benchmark gap fill
//! - `etf_pack_live_test`        → §3 Runtime pack routing
//! - `etf_options_gex_live_test` → §4 Stage 3 dealer positioning (GEX)
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute
//! `examples/`. Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example etf_live_test
//!
//! # §4 prefers FRED for the risk-free rate (yfinance ^IRX is the fallback):
//! SCORPIO_FRED_API_KEY=... cargo run -p scorpio-core --example etf_live_test
//! ```
//!
//! Each section runs independently; a section that fails is reported
//! inline but does not abort the others.

use chrono::Utc;
use scorpio_core::config::ApiConfig;
use scorpio_core::data::YFinanceClient;
use scorpio_core::data::etf_benchmarks;
use scorpio_core::data::traits::options::OptionsOutcome;
use scorpio_core::data::yfinance::YFinanceOptionsProvider;
use scorpio_core::data::yfinance::etf::is_supported_etf_kind;
use scorpio_core::data::{Candle, FredClient};
use scorpio_core::domain::Symbol;
use scorpio_core::rate_limit::SharedRateLimiter;
use scorpio_core::state::EtfRiskFreeRateSource;
use scorpio_core::valuation::etf::premium_discount::compute_gex_summary;
use scorpio_core::workflow::{RuntimePackSelection, classify_runtime_pack};
use secrecy::SecretString;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .init();

    let yf = YFinanceClient::default();

    section_header("§1  ETF surface (profile / quote / fund_info / yield)");
    surface_smoke(&yf).await;

    section_header("§2  Data-gap fill (NAV + bid/ask + benchmark)");
    data_gap_smoke(&yf).await;

    section_header("§3  Runtime pack routing");
    pack_routing_smoke(&yf).await;

    section_header("§4  Stage 3 dealer positioning (GEX)");
    if let Err(e) = dealer_positioning_smoke(yf).await {
        eprintln!("§4 failed: {e}");
    }

    println!();
    println!("─────────────────────────────────────────────────────────────────");
    println!("  done");
    println!("─────────────────────────────────────────────────────────────────");
}

// ── §1 ETF surface ──────────────────────────────────────────────────────────

async fn surface_smoke(client: &YFinanceClient) {
    // Mix: vanilla ETF, index ETF, 3× leveraged, plain equity (fund_info
    // should be None), and a bogus ticker (fail-soft path).
    for symbol in ["SPY", "QQQ", "TQQQ", "AAPL", "BOGUS_TICKER_DOES_NOT_EXIST"] {
        let profile = client.get_profile(symbol).await;
        let quote = client.get_quote(symbol).await;
        let info = client.get_fund_info(symbol).await;
        let yld = client.get_distribution_yield_ttm(symbol).await;
        let is_etf_kind = info
            .as_ref()
            .and_then(|i| i.fund_kind.as_deref())
            .map(is_supported_etf_kind)
            .unwrap_or(false);

        println!("\n=== {symbol} ===");
        println!("profile     : {profile:?}");
        println!("quote       : {quote:?}");
        println!("info        : {info:?}");
        println!("dist_yld_ttm: {yld:?}");
        println!("is_etf_kind : {is_etf_kind}");
    }
}

// ── §2 Data-gap fill ────────────────────────────────────────────────────────

async fn data_gap_smoke(client: &YFinanceClient) {
    // Mapped ETFs (NAV+bid+ask+benchmark all populate), one equity
    // (bid/ask only — no NAV, no benchmark), one bogus (all dashes,
    // fail-soft).
    const SYMBOLS: &[&str] = &[
        "SPY",
        "QQQ",
        "IWM",
        "VTI",
        "SMH",
        "SOXX",
        "AAPL",
        "XYZ123_BOGUS",
    ];

    println!();
    println!(
        "{:<14} {:>10} {:>10} {:>10}   {:<10}",
        "SYMBOL", "NAV", "BID", "ASK", "BENCHMARK"
    );
    println!("{}", "─".repeat(60));

    for symbol in SYMBOLS {
        let quote = client.get_quote(symbol).await;
        let benchmark = etf_benchmarks::resolve(symbol).unwrap_or("—");
        let (nav, bid, ask) = match &quote {
            Some(q) => (q.nav, q.bid, q.ask),
            None => (None, None, None),
        };
        println!(
            "{:<14} {:>10} {:>10} {:>10}   {:<10}",
            symbol,
            fmt_field(nav),
            fmt_field(bid),
            fmt_field(ask),
            benchmark,
        );
    }

    println!();
    println!("Expected:");
    println!("  • SPY/QQQ/IWM/VTI/SMH/SOXX → NAV + bid + ask + benchmark all populated.");
    println!("  • AAPL                     → bid/ask only.");
    println!("  • XYZ123_BOGUS             → every field `—`; fail-soft path verified.");
}

fn fmt_field(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.2}"),
        None => "—".to_string(),
    }
}

// ── §3 Runtime pack routing ─────────────────────────────────────────────────

async fn pack_routing_smoke(yf: &YFinanceClient) {
    // SPY → EtfBaseline, AAPL → BaselineMatched, BOGUS → BaselineFallback.
    // Unexpected routings are logged to stderr but do not abort the run.
    println!();
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

// ── §4 Stage 3 dealer positioning ───────────────────────────────────────────

const GEX_SYMBOL: &str = "SPY";

async fn dealer_positioning_smoke(yf: YFinanceClient) -> Result<(), Box<dyn std::error::Error>> {
    let today = Utc::now().date_naive();
    let today_iso = today.format("%Y-%m-%d").to_string();
    println!("\n  {GEX_SYMBOL} @ {today_iso}");

    // ── Risk-free rate ─────────────────────────────────────────────────────
    let (rate, source) = match resolve_risk_free_rate(&yf).await? {
        Some((rate, src)) => {
            let label = match src {
                EtfRiskFreeRateSource::FredDgs3Mo => "FRED DGS3MO",
                EtfRiskFreeRateSource::YFinanceIrx => "yfinance ^IRX",
            };
            println!("  Rate source : {label} ({:.2}%)", rate * 100.0);
            (rate, src)
        }
        None => return Err("no live risk-free rate available (FRED + ^IRX both failed)".into()),
    };

    // ── Options snapshot ───────────────────────────────────────────────────
    let symbol = Symbol::parse(GEX_SYMBOL)?;
    let provider = YFinanceOptionsProvider::new(yf);
    let snap = match provider.fetch_snapshot_impl(&symbol, &today_iso).await? {
        OptionsOutcome::Snapshot(s) => s,
        other => return Err(format!("expected Snapshot, got: {other:?}").into()),
    };
    println!(
        "  Options     : spot={:.2}, atm_iv={:.4}, near_term_strikes={}, all_expirations={}",
        snap.spot_price,
        snap.atm_iv,
        snap.near_term_strikes.len(),
        snap.all_expirations.len(),
    );
    require(snap.spot_price > 0.0, "spot must be positive")?;
    require(
        !snap.near_term_strikes.is_empty(),
        "near_term_strikes must be non-empty",
    )?;
    require(
        snap.all_expirations.len() >= 2,
        "broad input needs >= 2 non-front-month expirations",
    )?;

    // ── compute_gex_summary ────────────────────────────────────────────────
    // SPY pays distributions; assume ~1.3% dividend yield for the smoke
    // (production reads it from EtfComposition.distribution_yield_ttm_pct).
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

    require(
        summary.net_gex_usd_per_1pct_move.is_finite(),
        "net GEX must be finite",
    )?;
    require(
        !summary.strikes.is_empty(),
        "expected gamma walls; got empty",
    )?;
    require(
        summary.strikes.len() <= 3,
        "should truncate to top-3 gamma walls",
    )?;

    let broad = summary.broad.as_ref().ok_or("broad GEX must populate")?;
    println!(
        "  Broad GEX             : net={:+.3e}, gross={:.3e}, expirations_used={}/{}",
        broad.net_gex_usd_per_1pct_move,
        broad.gross_gex_usd_per_1pct_move,
        broad.expirations_used,
        broad.expirations_total_considered,
    );
    require(
        broad.expirations_used >= 2,
        "broad must aggregate >= 2 expirations",
    )?;

    let vex = summary
        .vex_summary
        .as_ref()
        .ok_or("vex_summary must populate")?;
    println!(
        "  VEX                   : net={:+.3e}/volpt, gross={:.3e}/volpt",
        vex.net_vex_usd_per_volpt, vex.gross_vex_usd_per_volpt
    );
    require(
        vex.gross_vex_usd_per_volpt >= vex.net_vex_usd_per_volpt.abs(),
        "gross VEX must dominate |net VEX|",
    )?;

    let cex = summary
        .cex_summary
        .as_ref()
        .ok_or("cex_summary must populate")?;
    println!(
        "  CEX                   : net={:+.3e}/day, gross={:.3e}/day",
        cex.net_cex_usd_per_day, cex.gross_cex_usd_per_day
    );
    require(
        cex.gross_cex_usd_per_day >= cex.net_cex_usd_per_day.abs(),
        "gross CEX must dominate |net CEX|",
    )?;

    println!();
    println!("  source = {source:?}");
    println!("  §4 OK");
    Ok(())
}

/// FRED `DGS3MO` first; yfinance `^IRX` close as fallback.
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

// ── helpers ─────────────────────────────────────────────────────────────────

fn section_header(title: &str) {
    println!();
    println!("─────────────────────────────────────────────────────────────────");
    println!("  {title}");
    println!("─────────────────────────────────────────────────────────────────");
}

fn require(cond: bool, msg: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    if cond { Ok(()) } else { Err(msg.into()) }
}
