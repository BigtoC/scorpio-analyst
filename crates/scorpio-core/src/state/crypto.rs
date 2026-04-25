//! Crypto-scoped state — placeholder shape for the crypto pack slice.
//!
//! `TradingState::crypto` is always `None` in this slice because no crypto
//! pack is wired through to the runtime. The struct exists so the
//! asset-class seam on [`super::TradingState`] is symmetric with the
//! equity branch, and the crypto-pack implementation can fill in fields
//! without touching the shared shape.
// TODO: implement in crypto-pack change.
use serde::{Deserialize, Serialize};

/// Placeholder container for crypto-pack analyst outputs, evidence
/// records, and derived artifacts. Empty today; the crypto-pack slice
/// will mirror the [`super::EquityState`] layout (tokenomics, on-chain,
/// social, derivatives, etc.).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CryptoState {}
