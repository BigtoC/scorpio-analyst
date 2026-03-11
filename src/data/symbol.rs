//! Shared stock-symbol validation helpers for market data providers.
//!
//! Centralizing this logic keeps provider adapters consistent and avoids subtle
//! drift in accepted ticker formats or error messages.

use crate::error::TradingError;

/// Validate and normalize a stock or index symbol.
///
/// The symbol is trimmed and then checked against the project-wide provider
/// contract:
/// - non-empty after trimming
/// - at most 24 characters
/// - contains only ASCII alphanumeric characters plus `.`, `-`, `_`, or `^`
///
/// # Errors
///
/// Returns [`TradingError::SchemaViolation`] when the symbol does not satisfy
/// the allowed format.
pub(crate) fn validate_symbol(symbol: &str) -> Result<&str, TradingError> {
    let symbol = symbol.trim();
    let is_allowed_symbol_char =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '^');

    if symbol.is_empty() || symbol.len() > 24 || !symbol.chars().all(is_allowed_symbol_char) {
        return Err(TradingError::SchemaViolation {
            message: format!("invalid symbol: {symbol:?}"),
        });
    }

    Ok(symbol)
}

#[cfg(test)]
mod tests {
    use super::validate_symbol;
    use crate::error::TradingError;

    #[test]
    fn validate_symbol_accepts_common_ticker_formats() {
        assert_eq!(validate_symbol("AAPL").unwrap(), "AAPL");
        assert_eq!(validate_symbol("BRK.B").unwrap(), "BRK.B");
        assert_eq!(validate_symbol("BF-B").unwrap(), "BF-B");
        assert_eq!(validate_symbol(" ^GSPC ").unwrap(), "^GSPC");
        assert_eq!(validate_symbol("msft").unwrap(), "msft");
    }

    #[test]
    fn validate_symbol_rejects_invalid_values() {
        assert!(matches!(
            validate_symbol(""),
            Err(TradingError::SchemaViolation { .. })
        ));
        assert!(matches!(
            validate_symbol("BAD SYMBOL"),
            Err(TradingError::SchemaViolation { .. })
        ));
        assert!(matches!(
            validate_symbol("DROP;TABLE"),
            Err(TradingError::SchemaViolation { .. })
        ));
        assert!(matches!(
            validate_symbol(&"A".repeat(25)),
            Err(TradingError::SchemaViolation { .. })
        ));
    }
}
