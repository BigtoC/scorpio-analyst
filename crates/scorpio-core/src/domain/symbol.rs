//! Typed symbol grammar for instruments routed through the analysis pipeline.
//!
//! The plain-string `asset_symbol` used across the codebase is tolerant to the
//! point of ambiguity — any `&str` round-trips through it, including values that
//! could never be parsed as a ticker. [`Symbol`] closes that hole by encoding
//! the two concrete asset-class grammars the system currently needs:
//!
//! - [`Symbol::Equity`] wraps a [`Ticker`] — the classic equity ticker format
//!   validated by [`crate::data::symbol::validate_symbol`].
//! - [`Symbol::Crypto`] wraps a [`CaipAssetId`] — a placeholder CAIP-10-style
//!   identifier. Full CAIP parsing lands with the crypto pack; this slice only
//!   needs a distinct newtype so downstream code can dispatch by variant.
//!
//! The `Display` impl returns the canonical string form, which is what the
//! transitional `TradingState::asset_symbol` mirror consumes.
//!
//! Serde uses a tagged representation (`{"equity": "AAPL"}`) so snapshots stay
//! unambiguous across class boundaries.
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::data::symbol::validate_symbol;
use crate::error::TradingError;

/// An equity ticker in canonical (uppercase, trimmed) form.
///
/// Construct via [`Ticker::parse`] — direct field access is disallowed so
/// invalid strings cannot be forged into the type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Ticker(String);

impl Ticker {
    /// Parse a raw ticker string through [`validate_symbol`] and uppercase it.
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::SchemaViolation`] when the input fails ticker
    /// grammar validation.
    pub fn parse(raw: &str) -> Result<Self, TradingError> {
        let trimmed = validate_symbol(raw)?;
        Ok(Self(trimmed.to_ascii_uppercase()))
    }

    /// Borrow the canonical string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Ticker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// CAIP asset identifier placeholder for crypto assets.
///
/// A future crypto pack will replace this stub with a full CAIP-10 parser.
/// For Phase 1 the newtype just has to exist so [`Symbol::Crypto`] has a
/// distinct payload variant — it is never constructed from user input yet and
/// every factory returns [`TradingError::SchemaViolation`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CaipAssetId(String);

impl CaipAssetId {
    /// Placeholder parser. Always returns an error in this slice.
    ///
    /// # Errors
    ///
    /// Always returns [`TradingError::SchemaViolation`] — crypto support lands
    /// with a follow-up change.
    pub fn parse(raw: &str) -> Result<Self, TradingError> {
        Err(TradingError::SchemaViolation {
            message: format!("crypto asset parsing is not implemented yet; received: {raw:?}"),
        })
    }

    /// Borrow the raw CAIP string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CaipAssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Typed symbol for a single instrument flowing through the pipeline.
///
/// Serialized form is a single-key tagged object, e.g.
/// `{"equity": "AAPL"}` or `{"crypto": "eip155:1/slip44:60"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Symbol {
    /// Equity ticker (e.g. `AAPL`, `BRK.B`, `^GSPC`).
    Equity(Ticker),
    /// Crypto asset addressed by a CAIP identifier (placeholder in this slice).
    Crypto(CaipAssetId),
}

impl Symbol {
    /// Parse a raw symbol string.
    ///
    /// Strategy: try [`Ticker::parse`] first. Crypto parsing is not implemented
    /// in this slice; callers that know they're dealing with a crypto asset
    /// must currently surface their own error. When crypto parsing lands it
    /// will plug in here as a fallback.
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::SchemaViolation`] when the input matches no
    /// supported asset-class grammar.
    pub fn parse(raw: &str) -> Result<Self, TradingError> {
        Ticker::parse(raw).map(Symbol::Equity)
    }

    /// Access the underlying equity ticker, if this is an equity symbol.
    #[must_use]
    pub fn as_equity(&self) -> Option<&Ticker> {
        match self {
            Symbol::Equity(t) => Some(t),
            Symbol::Crypto(_) => None,
        }
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Symbol::Equity(t) => t.fmt(f),
            Symbol::Crypto(c) => c.fmt(f),
        }
    }
}
