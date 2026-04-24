//! Canonical instrument identity for the runtime analysis pipeline.
//!
//! [`resolve_symbol`] converts a raw ticker string into a [`ResolvedInstrument`]
//! by delegating format validation to [`super::symbol::validate_symbol`] and
//! normalising the accepted symbol to uppercase.  In Stage 1, all metadata
//! fields (`issuer_name`, `exchange`, `instrument_type`, `aliases`) are left
//! unpopulated; they will be filled in by a live-metadata enrichment step
//! introduced in a later milestone.

use serde::{Deserialize, Serialize};

use crate::{
    data::symbol::validate_symbol,
    domain::{Symbol, Ticker},
    error::TradingError,
};

fn default_symbol_placeholder() -> Symbol {
    // Pre-typed-symbol snapshots don't carry a `symbol` field; at deserialize
    // time we substitute an unknown-placeholder ticker and rely on callers to
    // inspect `canonical_symbol` for the canonical string form. This value
    // cannot be constructed from user input (it has a `$` that
    // `validate_symbol` rejects) so it is unambiguous as a default marker.
    Symbol::Equity(
        Ticker::parse("UNKNOWN").expect("UNKNOWN is a valid ticker placeholder; this cannot panic"),
    )
}

/// An authoritative, normalised description of the instrument being analysed.
///
/// Stage 1: only `canonical_symbol` is populated.  All other fields are `None`
/// / empty and will be enriched by the transcript/consensus/event providers in
/// later milestones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedInstrument {
    /// Uppercase-normalised ticker symbol (e.g. `"NVDA"`, `"BRK.B"`).
    pub canonical_symbol: String,
    /// Typed identity mirror of `canonical_symbol`. Added in the Phase 1
    /// asset-class refactor so downstream code can dispatch by asset class
    /// without re-parsing the string. Populated by [`resolve_symbol`]; old
    /// snapshots without this field default to a placeholder ticker until a
    /// later cleanup retires the string form.
    #[serde(default = "default_symbol_placeholder")]
    pub symbol: Symbol,
    /// Legal name of the issuing entity (Stage 1: `None`).
    pub issuer_name: Option<String>,
    /// Primary exchange listing (Stage 1: `None`).
    pub exchange: Option<String>,
    /// Asset class / instrument type (Stage 1: `None`).
    pub instrument_type: Option<String>,
    /// Alternative ticker identifiers across venues (Stage 1: empty).
    pub aliases: Vec<String>,
}

/// Resolve and canonicalise a raw ticker string into a [`ResolvedInstrument`].
///
/// # Behaviour
///
/// 1. Delegates format validation to [`validate_symbol`], which trims leading /
///    trailing whitespace and enforces the project-wide ticker grammar.
/// 2. Uppercases the trimmed symbol to produce the canonical form.
/// 3. Returns a [`ResolvedInstrument`] with all metadata fields at their Stage 1
///    defaults (`None` / empty).
///
/// # Errors
///
/// Returns [`TradingError::SchemaViolation`] when the symbol fails format
/// validation (empty, too long, contains disallowed characters).
pub fn resolve_symbol(symbol: &str) -> Result<ResolvedInstrument, TradingError> {
    let trimmed = validate_symbol(symbol)?;
    let canonical_symbol = trimmed.to_ascii_uppercase();
    let ticker = Ticker::parse(trimmed)
        .expect("validate_symbol already accepted this input, so Ticker::parse cannot fail here");
    Ok(ResolvedInstrument {
        canonical_symbol,
        symbol: Symbol::Equity(ticker),
        issuer_name: None,
        exchange: None,
        instrument_type: None,
        aliases: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TradingError;

    #[test]
    fn resolve_symbol_uppercases_lowercase_ticker() {
        let instrument = resolve_symbol("nvda").expect("lowercase ticker should resolve");
        assert_eq!(instrument.canonical_symbol, "NVDA");
    }

    #[test]
    fn resolve_symbol_preserves_already_uppercase() {
        let instrument = resolve_symbol("AAPL").expect("uppercase ticker should resolve");
        assert_eq!(instrument.canonical_symbol, "AAPL");
    }

    #[test]
    fn resolve_symbol_trims_whitespace_before_uppercasing() {
        let instrument = resolve_symbol(" nvda ").expect("padded ticker should resolve");
        assert_eq!(instrument.canonical_symbol, "NVDA");
    }

    #[test]
    fn resolve_symbol_handles_dot_suffix() {
        let instrument = resolve_symbol("BRK.B").expect("dot-suffix ticker should resolve");
        assert_eq!(instrument.canonical_symbol, "BRK.B");
    }

    #[test]
    fn resolve_symbol_handles_index_prefix() {
        let instrument = resolve_symbol("^GSPC").expect("index ticker should resolve");
        assert_eq!(instrument.canonical_symbol, "^GSPC");
    }

    #[test]
    fn resolve_symbol_rejects_semicolon() {
        let err = resolve_symbol("DROP;TABLE").expect_err("semicolon symbol should fail");
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn resolve_symbol_rejects_empty() {
        let err = resolve_symbol("").expect_err("empty symbol should fail");
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn resolve_symbol_rejects_too_long() {
        let err = resolve_symbol(&"A".repeat(25)).expect_err("too-long symbol should fail");
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn resolve_symbol_stage1_metadata_fields_are_none() {
        let instrument = resolve_symbol("TSLA").expect("valid ticker");
        assert!(instrument.issuer_name.is_none());
        assert!(instrument.exchange.is_none());
        assert!(instrument.instrument_type.is_none());
        assert!(instrument.aliases.is_empty());
        match &instrument.symbol {
            Symbol::Equity(t) => assert_eq!(t.as_str(), "TSLA"),
            Symbol::Crypto(_) => panic!("resolve_symbol must populate Equity variant for tickers"),
        }
    }

    #[test]
    fn resolved_instrument_deserializes_snapshot_without_symbol_field() {
        // Pre-Phase-1 snapshots omit `symbol`; default placeholder preserves
        // back-compat so old rows keep deserializing.
        let json = r#"{
            "canonical_symbol": "AAPL",
            "issuer_name": null,
            "exchange": null,
            "instrument_type": null,
            "aliases": []
        }"#;
        let instrument: ResolvedInstrument =
            serde_json::from_str(json).expect("old snapshot should deserialize");
        assert_eq!(instrument.canonical_symbol, "AAPL");
        assert!(matches!(instrument.symbol, Symbol::Equity(_)));
    }

    #[test]
    fn resolved_instrument_serializes_and_deserializes() {
        let original = resolve_symbol("MSFT").expect("valid ticker");
        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let recovered: ResolvedInstrument =
            serde_json::from_str(&json).expect("deserialization should succeed");
        assert_eq!(original, recovered);
    }
}
