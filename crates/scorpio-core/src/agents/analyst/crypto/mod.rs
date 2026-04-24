//! Crypto-pack analyst placeholders.
//!
//! These modules reserve the names the crypto pack slice will fill in. Each
//! sub-module exposes an empty struct with a `// TODO: implement in crypto-pack
//! change` marker so the crate structure matches the target layout before the
//! real crypto implementation lands.
mod derivatives;
mod onchain;
mod social;
mod tokenomics;

pub use derivatives::DerivativesAnalyst;
pub use onchain::OnChainAnalyst;
pub use social::SocialAnalyst;
pub use tokenomics::TokenomicsAnalyst;
