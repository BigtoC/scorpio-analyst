//! Manual live smoke: yfinance ETF surface methods.
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute
//! `examples/`. Run manually with:
//!
//! ```sh
//! cargo run -p scorpio-core --example etf_quote_live_test
//! ```
//!
//! Exercises every ETF-specific public method added in Task 8:
//! - `YFinanceClient::get_quote` (ETF price snapshot)
//! - `YFinanceClient::get_fund_info` (ETF metadata)
//! - `YFinanceClient::get_distribution_yield_ttm` (trailing yield)
//! - `YFinanceClient::get_profile` (used by the runtime classifier)
//! - `is_supported_etf_kind` helper
//!
//! The symbol list mixes a vanilla ETF (`SPY`), a sector/index ETF (`QQQ`),
//! a 3× leveraged ETF (`TQQQ`), an equity (`AAPL`) — where `get_fund_info`
//! is expected to return `None` — and a bogus ticker that exercises the
//! fail-soft path.

use scorpio_core::data::YFinanceClient;
use scorpio_core::data::yfinance::etf::is_supported_etf_kind;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .init();

    let client = YFinanceClient::default();

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
        println!("profile: {profile:?}");
        println!("quote: {quote:?}");
        println!("info: {info:?}");
        println!("dist_yld_ttm: {yld:?}");
        println!("is_etf_kind: {is_etf_kind}");
    }
}
