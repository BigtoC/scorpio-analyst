//! Trait implementations that adapt the concrete equity clients
//! ([`FinnhubClient`], [`FredClient`], [`YFinanceClient`]) to the
//! domain-split [`DataProvider`] traits in [`super::traits`].
//!
//! Keeping the impls in this dedicated module means the client files stay
//! focused on their upstream HTTP contract and tool-macro wiring, and the
//! trait → client bridge is reviewable as a single unit.
use async_trait::async_trait;

use super::{
    FinnhubClient, FredClient, YFinanceClient,
    traits::{FundamentalsProvider, MacroProvider, NewsProvider, PriceBar, PriceHistoryProvider},
    yfinance::news::YFinanceNewsProvider,
};
use crate::{
    domain::Symbol,
    error::TradingError,
    state::{FundamentalData, MacroEvent, NewsData},
};

// Extract the canonical ticker string from a Symbol, or fail cleanly if the
// caller handed us a non-equity symbol.
fn require_equity_ticker(symbol: &Symbol) -> Result<String, TradingError> {
    match symbol {
        Symbol::Equity(t) => Ok(t.as_str().to_owned()),
        Symbol::Crypto(_) => Err(TradingError::SchemaViolation {
            message: format!("equity-only provider received non-equity symbol {symbol}"),
        }),
    }
}

#[async_trait]
impl FundamentalsProvider for FinnhubClient {
    fn provider_name(&self) -> &'static str {
        "finnhub"
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<FundamentalData, TradingError> {
        let ticker = require_equity_ticker(symbol)?;
        self.get_fundamentals(&ticker).await
    }
}

#[async_trait]
impl NewsProvider for FinnhubClient {
    fn provider_name(&self) -> &'static str {
        "finnhub"
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> {
        let ticker = require_equity_ticker(symbol)?;
        self.get_structured_news(&ticker).await
    }
}

#[async_trait]
impl NewsProvider for YFinanceNewsProvider {
    fn provider_name(&self) -> &'static str {
        "yfinance"
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> {
        let ticker = require_equity_ticker(symbol)?;
        self.get_company_news(&ticker).await
    }
}

#[async_trait]
impl MacroProvider for FredClient {
    fn provider_name(&self) -> &'static str {
        "fred"
    }

    async fn fetch_indicators(&self) -> Result<Vec<MacroEvent>, TradingError> {
        self.get_economic_indicators().await
    }
}

#[async_trait]
impl PriceHistoryProvider for YFinanceClient {
    fn provider_name(&self) -> &'static str {
        "yfinance"
    }

    async fn fetch(
        &self,
        symbol: &Symbol,
        start: &str,
        end: &str,
    ) -> Result<Vec<PriceBar>, TradingError> {
        let ticker = require_equity_ticker(symbol)?;
        let candles = self.get_ohlcv(&ticker, start, end).await?;
        // Map the yfinance-scoped Candle into the provider-agnostic PriceBar.
        // The conversion is lossless for the fields we expose.
        Ok(candles
            .into_iter()
            .map(|c| PriceBar {
                timestamp: c.date,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                // Candle holds `Option<u64>` to tolerate incomplete provider
                // rows; 0.0 is the documented fallback when volume is absent
                // so downstream f64-only consumers don't need to branch.
                volume: c.volume.map(|v| v as f64).unwrap_or(0.0),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CaipAssetId, Ticker};

    #[test]
    fn require_equity_ticker_extracts_string_from_equity() {
        let sym = Symbol::Equity(Ticker::parse("AAPL").unwrap());
        assert_eq!(require_equity_ticker(&sym).unwrap(), "AAPL");
    }

    #[test]
    fn require_equity_ticker_rejects_crypto_with_schema_violation() {
        // CaipAssetId::parse is the unimplemented-path placeholder; synthesize
        // a crypto-variant symbol by hand-constructing via serde round-trip
        // from JSON so the test doesn't depend on the crypto parser.
        let json = r#"{"crypto":"eip155:1/slip44:60"}"#;
        let sym: Symbol = serde_json::from_str(json).unwrap();
        let err = require_equity_ticker(&sym).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
        // Type-guard so unused imports survive.
        let _unused: Option<CaipAssetId> = None;
    }
}
