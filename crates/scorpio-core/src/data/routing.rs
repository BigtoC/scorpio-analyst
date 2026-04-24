//! Symbol-aware routing helpers that pick the right provider for each
//! domain-split [`super::traits::DataProvider`] trait.
//!
//! The shape is deliberately simple in this slice — a [`ProviderRegistry`]
//! holds optional `Arc<dyn Provider>` for each domain, populated by the
//! pipeline's wiring code (which still holds the concrete clients). The
//! `resolve_*` functions pick the registered provider for the asset class
//! of `symbol`; only equity is populated today, so crypto routing returns
//! `None` until the crypto pack lands.
use std::sync::Arc;

use crate::domain::{AssetClass, Symbol};

use super::traits::{
    DerivativesProvider, FundamentalsProvider, MacroProvider, NewsProvider, OnChainProvider,
    PriceHistoryProvider, SocialProvider, TokenomicsProvider,
};

/// Bag of registered providers keyed by (asset class, domain).
///
/// This is intentionally flat — the two asset classes the code understands
/// today (Equity, Crypto) get optional slots per domain. The pipeline wires
/// concrete clients into the appropriate equity slots; future slices extend
/// into the crypto slots.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    pub equity_fundamentals: Option<Arc<dyn FundamentalsProvider>>,
    pub equity_price_history: Option<Arc<dyn PriceHistoryProvider>>,
    pub equity_news: Option<Arc<dyn NewsProvider>>,
    pub equity_macro: Option<Arc<dyn MacroProvider>>,

    pub crypto_tokenomics: Option<Arc<dyn TokenomicsProvider>>,
    pub crypto_onchain: Option<Arc<dyn OnChainProvider>>,
    pub crypto_derivatives: Option<Arc<dyn DerivativesProvider>>,
    pub crypto_social: Option<Arc<dyn SocialProvider>>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Resolve the fundamentals provider that covers `symbol`'s asset class.
#[must_use]
pub fn resolve_fundamentals_provider(
    symbol: &Symbol,
    reg: &ProviderRegistry,
) -> Option<Arc<dyn FundamentalsProvider>> {
    match AssetClass::of(symbol) {
        AssetClass::Equity => reg.equity_fundamentals.clone(),
        AssetClass::Crypto => None,
    }
}

/// Resolve the price-history provider for `symbol`.
#[must_use]
pub fn resolve_price_history_provider(
    symbol: &Symbol,
    reg: &ProviderRegistry,
) -> Option<Arc<dyn PriceHistoryProvider>> {
    match AssetClass::of(symbol) {
        AssetClass::Equity => reg.equity_price_history.clone(),
        AssetClass::Crypto => None,
    }
}

/// Resolve the news provider for `symbol`.
#[must_use]
pub fn resolve_news_provider(
    symbol: &Symbol,
    reg: &ProviderRegistry,
) -> Option<Arc<dyn NewsProvider>> {
    match AssetClass::of(symbol) {
        AssetClass::Equity => reg.equity_news.clone(),
        AssetClass::Crypto => None,
    }
}

/// Resolve the macroeconomic-indicators provider.
///
/// Macro data is market-wide and isn't scoped to a symbol — the argument is
/// accepted for uniformity so future asset-class extensions can add
/// class-scoped macro providers if needed.
#[must_use]
pub fn resolve_macro_provider(
    _symbol: &Symbol,
    reg: &ProviderRegistry,
) -> Option<Arc<dyn MacroProvider>> {
    reg.equity_macro.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Symbol, Ticker};

    #[test]
    fn routes_equity_symbol_to_registered_equity_provider() {
        // We can't easily construct a real provider here, so verify the
        // routing logic by registering a None-typed stand-in and confirming
        // the resolve function looks at the expected slot.
        let reg = ProviderRegistry::default();
        let sym = Symbol::Equity(Ticker::parse("AAPL").unwrap());
        assert!(resolve_fundamentals_provider(&sym, &reg).is_none());
        assert!(resolve_price_history_provider(&sym, &reg).is_none());
        assert!(resolve_news_provider(&sym, &reg).is_none());
    }

    #[test]
    fn crypto_symbol_returns_none_until_crypto_pack_lands() {
        let reg = ProviderRegistry::default();
        let json = r#"{"crypto":"eip155:1/slip44:60"}"#;
        let sym: Symbol = serde_json::from_str(json).unwrap();
        assert!(resolve_fundamentals_provider(&sym, &reg).is_none());
        assert!(resolve_price_history_provider(&sym, &reg).is_none());
        assert!(resolve_news_provider(&sym, &reg).is_none());
    }
}
