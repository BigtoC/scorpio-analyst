//! Domain-split [`DataProvider`] traits.
//!
//! Each sub-module owns a narrow provider contract for one upstream data
//! category: fundamentals, price history, news, macroeconomic indicators,
//! plus crypto placeholders (tokenomics, on-chain, derivatives, social).
//! Concrete clients implement the trait(s) they satisfy — today Finnhub
//! covers fundamentals + news, Yahoo Finance covers price history, FRED
//! covers macro. The crypto traits exist so the graph builder can express
//! crypto-pack routing once the crypto pack implementation lands.
//!
//! A routing helper in [`super::routing`] maps a [`crate::domain::Symbol`]
//! to the right provider for each trait; pipeline code that needs "the
//! fundamentals provider for this asset" goes through the helper rather
//! than reaching into concrete client structs.
pub mod derivatives;
pub mod fundamentals;
pub mod macroeconomic;
pub mod news;
pub mod onchain;
pub mod options;
pub mod price_history;
pub mod social;
pub mod tokenomics;

pub use derivatives::DerivativesProvider;
pub use fundamentals::FundamentalsProvider;
pub use macroeconomic::MacroProvider;
pub use news::NewsProvider;
pub use onchain::OnChainProvider;
pub use options::{IvTermPoint, NearTermStrike, OptionsOutcome, OptionsProvider, OptionsSnapshot};
pub use price_history::{PriceBar, PriceHistoryProvider};
pub use social::SocialProvider;
pub use tokenomics::TokenomicsProvider;
