//! Crypto-pack manifest definitions.
//!
//! Phase 7 introduces the `CryptoDigitalAsset` pack as a registered-but-
//! non-selectable stub. The manifest validates so the pack registry can
//! return a fully-formed [`super::AnalysisPackManifest`], but
//! [`super::PackId::from_str`] deliberately omits this variant so CLI /
//! config selection cannot pick it up until the crypto implementation
//! lands end-to-end.
pub mod digital_asset;

pub use digital_asset::digital_asset_pack;
