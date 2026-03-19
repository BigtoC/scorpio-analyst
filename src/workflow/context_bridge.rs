//! Bridge utilities for serializing/deserializing [`TradingState`] through a
//! graph-flow [`Context`].
//!
//! The full `TradingState` is stored as a single JSON blob under the key
//! `"trading_state"`.  Analyst fan-out child results are stored under
//! `"<prefix>.<key>"` composite keys so each child task can write independently
//! without clobbering the main state blob.

use graph_flow::Context;
use serde::{Serialize, de::DeserializeOwned};

use crate::{error::TradingError, state::TradingState};

/// Key used to store/retrieve the full `TradingState` JSON blob in the graph-flow context.
pub const TRADING_STATE_KEY: &str = "trading_state";

// ────────────────────────────────────────────────────────────────────────────
// Full state serialization
// ────────────────────────────────────────────────────────────────────────────

/// Serialize `state` as JSON and store it in `context` under [`TRADING_STATE_KEY`].
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` if serialization fails (should not
/// happen for a well-formed `TradingState`, but guards against edge cases).
pub async fn serialize_state_to_context(
    state: &TradingState,
    context: &Context,
) -> Result<(), TradingError> {
    let json = serde_json::to_string(state).map_err(|e| TradingError::SchemaViolation {
        message: format!("failed to serialize TradingState to context: {e}"),
    })?;
    context.set(TRADING_STATE_KEY, json).await;
    Ok(())
}

/// Deserialize a `TradingState` from the JSON blob stored in `context`.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` if the key is missing or the JSON
/// cannot be parsed into `TradingState`.
pub async fn deserialize_state_from_context(
    context: &Context,
) -> Result<TradingState, TradingError> {
    let json: Option<String> = context.get(TRADING_STATE_KEY).await;
    let json = json.ok_or_else(|| TradingError::SchemaViolation {
        message: format!("context missing required key '{TRADING_STATE_KEY}'"),
    })?;
    serde_json::from_str(&json).map_err(|e| TradingError::SchemaViolation {
        message: format!("failed to deserialize TradingState from context: {e}"),
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Prefixed fan-out helpers
// ────────────────────────────────────────────────────────────────────────────

/// Compute the composite context key for a prefixed fan-out entry.
fn prefixed_key(prefix: &str, key: &str) -> String {
    format!("{prefix}.{key}")
}

/// Serialize `value` as JSON and store it under `"<prefix>.<key>"` in `context`.
///
/// Used by individual analyst child tasks to write their results without
/// interfering with each other or with the main `TradingState` blob.
pub async fn write_prefixed_result<T: Serialize>(
    context: &Context,
    prefix: &str,
    key: &str,
    value: &T,
) -> Result<(), TradingError> {
    let json = serde_json::to_string(value).map_err(|e| TradingError::SchemaViolation {
        message: format!("failed to serialize prefixed result '{prefix}.{key}': {e}"),
    })?;
    context.set(prefixed_key(prefix, key), json).await;
    Ok(())
}

/// Read and deserialize the JSON value stored under `"<prefix>.<key>"` in `context`.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` if the key is missing or the JSON
/// cannot be deserialized into `T`.
pub async fn read_prefixed_result<T: DeserializeOwned>(
    context: &Context,
    prefix: &str,
    key: &str,
) -> Result<T, TradingError> {
    let full_key = prefixed_key(prefix, key);
    let json: Option<String> = context.get(&full_key).await;
    let json = json.ok_or_else(|| TradingError::SchemaViolation {
        message: format!("context missing prefixed key '{full_key}'"),
    })?;
    serde_json::from_str(&json).map_err(|e| TradingError::SchemaViolation {
        message: format!("failed to deserialize prefixed result '{full_key}': {e}"),
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TradingState;

    fn sample_state() -> TradingState {
        TradingState::new("AAPL", "2026-01-15")
    }

    #[tokio::test]
    async fn round_trip_trading_state() {
        let ctx = Context::new();
        let original = sample_state();

        serialize_state_to_context(&original, &ctx)
            .await
            .expect("serialization should succeed");

        let recovered = deserialize_state_from_context(&ctx)
            .await
            .expect("deserialization should succeed");

        assert_eq!(original.asset_symbol, recovered.asset_symbol);
        assert_eq!(original.target_date, recovered.target_date);
        assert_eq!(original.fundamental_metrics, recovered.fundamental_metrics);
        assert_eq!(original.debate_history, recovered.debate_history);
    }

    #[tokio::test]
    async fn missing_trading_state_key_returns_error() {
        let ctx = Context::new();
        let err = deserialize_state_from_context(&ctx)
            .await
            .expect_err("should fail on missing key");

        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[tokio::test]
    async fn prefixed_write_and_read_round_trip() {
        let ctx = Context::new();
        let values = vec!["alpha".to_string(), "beta".to_string()];

        write_prefixed_result(&ctx, "analyst", "fundamental", &values)
            .await
            .expect("write should succeed");

        let recovered: Vec<String> = read_prefixed_result(&ctx, "analyst", "fundamental")
            .await
            .expect("read should succeed");

        assert_eq!(values, recovered);
    }

    #[tokio::test]
    async fn prefixed_multiple_analysts_independent() {
        let ctx = Context::new();

        write_prefixed_result(&ctx, "analyst", "fundamental", &"fund data".to_string())
            .await
            .unwrap();
        write_prefixed_result(&ctx, "analyst", "sentiment", &"sent data".to_string())
            .await
            .unwrap();
        write_prefixed_result(&ctx, "analyst", "news", &"news data".to_string())
            .await
            .unwrap();
        write_prefixed_result(&ctx, "analyst", "technical", &"tech data".to_string())
            .await
            .unwrap();

        let fund: String = read_prefixed_result(&ctx, "analyst", "fundamental")
            .await
            .unwrap();
        let sent: String = read_prefixed_result(&ctx, "analyst", "sentiment")
            .await
            .unwrap();
        let news: String = read_prefixed_result(&ctx, "analyst", "news").await.unwrap();
        let tech: String = read_prefixed_result(&ctx, "analyst", "technical")
            .await
            .unwrap();

        assert_eq!(fund, "fund data");
        assert_eq!(sent, "sent data");
        assert_eq!(news, "news data");
        assert_eq!(tech, "tech data");
    }

    #[tokio::test]
    async fn missing_prefixed_key_returns_error() {
        let ctx = Context::new();
        let err = read_prefixed_result::<String>(&ctx, "analyst", "missing")
            .await
            .expect_err("should fail on missing key");

        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }
}
