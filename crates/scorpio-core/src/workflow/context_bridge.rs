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

/// Validate a prefixed fan-out leaf key before building a composite context key.
///
/// Prefixes may themselves be namespace-style strings containing `.` separators,
/// such as `"usage.analyst"`. The leaf `key` must be a single path segment so
/// the composite `"<prefix>.<key>"` contract remains unambiguous.
///
/// # Errors
///
/// Returns [`TradingError::SchemaViolation`] when `key` is empty or contains `.`.
fn validate_prefixed_leaf_key(prefix: &str, key: &str) -> Result<(), TradingError> {
    if key.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "invalid prefixed result leaf key for prefix '{prefix}': leaf key must not be empty"
            ),
        });
    }

    if key.contains('.') {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "invalid prefixed result leaf key '{key}' for prefix '{prefix}': leaf key must not contain '.'"
            ),
        });
    }

    Ok(())
}

/// Compute the composite context key for a prefixed fan-out entry.
///
/// Prefixes may contain `.` namespace separators, but `key` must be a single
/// leaf segment that is non-empty and does not contain `.`.
///
/// # Errors
///
/// Returns [`TradingError::SchemaViolation`] when the leaf key is invalid.
fn prefixed_key(prefix: &str, key: &str) -> Result<String, TradingError> {
    validate_prefixed_leaf_key(prefix, key)?;
    Ok(format!("{prefix}.{key}"))
}

/// Serialize `value` as JSON and store it under `"<prefix>.<key>"` in `context`.
///
/// Used by individual analyst child tasks to write their results without
/// interfering with each other or with the main `TradingState` blob.
///
/// `prefix` may be a namespace-style path that already contains `.` separators,
/// such as `"usage.analyst"`. By contrast, `key` is always the terminal leaf
/// segment and must be non-empty and must not contain `.`.
///
/// # Errors
///
/// Returns [`TradingError::SchemaViolation`] if the leaf key is invalid or if
/// serialization fails.
pub async fn write_prefixed_result<T: Serialize>(
    context: &Context,
    prefix: &str,
    key: &str,
    value: &T,
) -> Result<(), TradingError> {
    let full_key = prefixed_key(prefix, key)?;
    let json = serde_json::to_string(value).map_err(|e| TradingError::SchemaViolation {
        message: format!("failed to serialize prefixed result '{full_key}': {e}"),
    })?;
    context.set(full_key, json).await;
    Ok(())
}

/// Read and deserialize the JSON value stored under `"<prefix>.<key>"` in `context`.
///
/// `prefix` may be a namespace-style path that already contains `.` separators,
/// but `key` must be a single leaf segment so the composite key remains
/// unambiguous.
///
/// # Errors
///
/// Returns [`TradingError::SchemaViolation`] if the leaf key is invalid, if the
/// composite key is missing, or if the JSON cannot be deserialized into `T`.
pub async fn read_prefixed_result<T: DeserializeOwned>(
    context: &Context,
    prefix: &str,
    key: &str,
) -> Result<T, TradingError> {
    let full_key = prefixed_key(prefix, key)?;
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
    use chrono::Utc;

    use super::*;
    use crate::state::{
        DataCoverageReport, EvidenceKind, EvidenceRecord, EvidenceSource, FundamentalData,
        ProvenanceSummary, TradingState,
    };

    fn sample_state() -> TradingState {
        TradingState::new("AAPL", "2026-01-15")
    }

    fn sample_evidence_fundamental() -> EvidenceRecord<FundamentalData> {
        EvidenceRecord {
            kind: EvidenceKind::Fundamental,
            payload: FundamentalData {
                revenue_growth_pct: None,
                pe_ratio: Some(25.0),
                eps: None,
                current_ratio: None,
                debt_to_equity: None,
                gross_margin: None,
                net_income: None,
                insider_transactions: vec![],
                summary: "test".to_owned(),
            },
            sources: vec![EvidenceSource {
                provider: "finnhub".to_owned(),
                datasets: vec!["fundamentals".to_owned()],
                fetched_at: Utc::now(),
                effective_at: None,
                url: None,
                citation: None,
            }],
            quality_flags: vec![],
        }
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
        assert_eq!(
            original.fundamental_metrics(),
            recovered.fundamental_metrics()
        );
        assert_eq!(original.debate_history, recovered.debate_history);
    }

    #[tokio::test]
    async fn evidence_fields_survive_context_round_trip() {
        let ctx = Context::new();
        let mut original = sample_state();

        original.set_evidence_fundamental(sample_evidence_fundamental());
        original.data_coverage = Some(DataCoverageReport {
            required_inputs: vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ],
            missing_inputs: vec!["technical".to_owned()],
        });
        original.provenance_summary = Some(ProvenanceSummary {
            providers_used: vec!["finnhub".to_owned()],
        });

        serialize_state_to_context(&original, &ctx)
            .await
            .expect("serialization should succeed");

        let recovered = deserialize_state_from_context(&ctx)
            .await
            .expect("deserialization should succeed");

        assert!(
            recovered.evidence_fundamental().is_some(),
            "evidence_fundamental must survive context round-trip"
        );
        assert_eq!(
            recovered.evidence_fundamental().unwrap().payload.pe_ratio,
            Some(25.0)
        );
        assert_eq!(
            recovered.data_coverage.as_ref().unwrap().missing_inputs,
            vec!["technical"]
        );
        assert_eq!(
            recovered
                .provenance_summary
                .as_ref()
                .unwrap()
                .providers_used,
            vec!["finnhub"]
        );
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
    async fn prefixed_namespace_style_prefix_is_allowed() {
        let ctx = Context::new();

        write_prefixed_result(&ctx, "usage.analyst", "fundamental", &42_u64)
            .await
            .expect("write should succeed for namespace-style prefixes");

        let recovered: u64 = read_prefixed_result(&ctx, "usage.analyst", "fundamental")
            .await
            .expect("read should succeed for namespace-style prefixes");

        assert_eq!(recovered, 42);
    }

    #[tokio::test]
    async fn prefixed_write_rejects_empty_leaf_key() {
        let ctx = Context::new();

        let err = write_prefixed_result(&ctx, "analyst", "", &"fund data".to_string())
            .await
            .expect_err("write should fail for an empty leaf key");

        assert!(matches!(
            err,
            TradingError::SchemaViolation { ref message }
                if message.contains("leaf key") && message.contains("must not be empty")
        ));
    }

    #[tokio::test]
    async fn prefixed_read_rejects_empty_leaf_key() {
        let ctx = Context::new();

        let err = read_prefixed_result::<String>(&ctx, "analyst", "")
            .await
            .expect_err("read should fail for an empty leaf key");

        assert!(matches!(
            err,
            TradingError::SchemaViolation { ref message }
                if message.contains("leaf key") && message.contains("must not be empty")
        ));
    }

    #[tokio::test]
    async fn prefixed_write_rejects_leaf_key_with_separator() {
        let ctx = Context::new();

        let err = write_prefixed_result(&ctx, "analyst", "risk.score", &"fund data".to_string())
            .await
            .expect_err("write should fail for a leaf key containing the separator");

        assert!(matches!(
            err,
            TradingError::SchemaViolation { ref message }
                if message.contains("leaf key")
                    && message.contains("risk.score")
                    && message.contains("must not contain '.'")
        ));
    }

    #[tokio::test]
    async fn prefixed_read_rejects_leaf_key_with_separator() {
        let ctx = Context::new();

        let err = read_prefixed_result::<String>(&ctx, "analyst", "risk.score")
            .await
            .expect_err("read should fail for a leaf key containing the separator");

        assert!(matches!(
            err,
            TradingError::SchemaViolation { ref message }
                if message.contains("leaf key")
                    && message.contains("risk.score")
                    && message.contains("must not contain '.'")
        ));
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
