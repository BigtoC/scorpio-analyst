use super::*;
use crate::error::TradingError;

#[test]
fn parse_equity_ticker_succeeds() {
    let sym = Symbol::parse("AAPL").expect("AAPL should parse as equity");
    match sym {
        Symbol::Equity(t) => assert_eq!(t.as_str(), "AAPL"),
        Symbol::Crypto(_) => panic!("AAPL must resolve to Equity, got Crypto"),
    }
}

#[test]
fn parse_uppercases_lowercase_input() {
    let sym = Symbol::parse("msft").expect("lowercase ticker should parse");
    assert_eq!(sym.to_string(), "MSFT");
}

#[test]
fn parse_preserves_dot_suffix_ticker() {
    let sym = Symbol::parse("BRK.B").expect("dot-suffix ticker should parse");
    assert_eq!(sym.to_string(), "BRK.B");
}

#[test]
fn parse_preserves_index_prefix() {
    let sym = Symbol::parse("^GSPC").expect("index ticker should parse");
    assert_eq!(sym.to_string(), "^GSPC");
}

#[test]
fn parse_empty_rejects() {
    let err = Symbol::parse("").expect_err("empty input must fail");
    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}

#[test]
fn parse_invalid_chars_rejects() {
    let err = Symbol::parse("DROP;TABLE").expect_err("semicolon must fail");
    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}

#[test]
fn parse_overlong_input_rejects() {
    let err = Symbol::parse(&"A".repeat(25)).expect_err("25-char input must fail");
    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}

#[test]
fn symbol_display_round_trips() {
    let sym = Symbol::parse("tsla").expect("tsla should parse");
    assert_eq!(sym.to_string(), "TSLA");
}

#[test]
fn symbol_serde_round_trips_equity() {
    let sym = Symbol::parse("NVDA").expect("valid ticker");
    let json = serde_json::to_string(&sym).expect("serialize");
    let back: Symbol = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(sym, back);
}

#[test]
fn symbol_serde_uses_tagged_representation() {
    let sym = Symbol::parse("AAPL").expect("valid ticker");
    let json = serde_json::to_string(&sym).expect("serialize");
    assert!(
        json.contains("equity"),
        "expected tagged 'equity' key in JSON, got: {json}"
    );
}

#[test]
fn asset_class_of_equity_symbol_is_equity() {
    let sym = Symbol::parse("AAPL").expect("valid ticker");
    assert_eq!(AssetClass::of(&sym), AssetClass::Equity);
}

#[test]
fn caip_parse_is_unimplemented_in_this_slice() {
    let err = CaipAssetId::parse("eip155:1/slip44:60").expect_err("crypto should fail");
    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}

#[test]
fn ticker_as_str_returns_canonical_form() {
    let t = Ticker::parse(" nvda ").expect("padded ticker");
    assert_eq!(t.as_str(), "NVDA");
}
