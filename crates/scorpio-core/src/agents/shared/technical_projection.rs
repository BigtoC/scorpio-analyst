//! Compact projection of [`TechnicalData`] for downstream prompt seams.
//!
//! The full `TechnicalData` struct may contain large arrays (`near_term_strikes`,
//! `iv_term_structure`) that are unsuitable for injection into downstream agent
//! prompts (researcher, risk, trader, fund_manager). This module provides a
//! single helper, [`compact_technical_report`], that:
//!
//! 1. Serializes the full `TechnicalData` to a `serde_json::Value` (preserving
//!    all indicator fields such as RSI, MACD, ATR, etc.).
//! 2. Replaces the `options_context` key with a compact projection that strips
//!    `near_term_strikes` and `iv_term_structure` but retains the summary fields
//!    the downstream agents actually need.
//! 3. Leaves `options_summary` (a plain `Option<String>`) unchanged — it is
//!    already compact.
//!
//! The result is serialized back to a JSON string and sanitized before use in
//! prompts.

use serde_json::{Value, json};

use crate::{
    agents::shared::sanitize_prompt_context,
    data::traits::options::{OptionsOutcome, OptionsSnapshot},
    state::{TechnicalData, TechnicalOptionsContext},
};

/// Serialize `data` for prompt injection with the `options_context` field
/// compacted to summary statistics only.
///
/// The returned string is already sanitized and safe for direct prompt use.
///
/// Arrays that can reach hundreds of entries — `near_term_strikes` and
/// `iv_term_structure` inside an `OptionsSnapshot` — are dropped.
/// All other fields are preserved verbatim so downstream agents retain the
/// full indicator picture.
pub(crate) fn compact_technical_report(data: &TechnicalData) -> String {
    let compact = compact_technical_value(data);
    let serialized = serde_json::to_string(&compact).unwrap_or_else(|_| "null".to_owned());
    sanitize_prompt_context(&serialized)
}

/// Produce the compact `serde_json::Value` from `data`.
///
/// Exposed as a separate function so test code can inspect the structured
/// value rather than parse the serialized string.
///
/// Uses `to_string` + `from_str` rather than `to_value` to preserve the
/// struct field declaration order in the serialized JSON (serde_json's
/// `to_value` sorts keys alphabetically via `BTreeMap`; `to_string` uses
/// the declared order).
pub(crate) fn compact_technical_value(data: &TechnicalData) -> Value {
    // Start from the full serialization preserving struct field order.
    let json_str = serde_json::to_string(data).unwrap_or_else(|_| "null".to_owned());
    let mut value: Value = serde_json::from_str(&json_str).unwrap_or(Value::Null);

    if let Value::Object(ref mut map) = value {
        // Replace options_context with the compact projection.
        match &data.options_context {
            None => {
                // Field was absent to begin with; serde's skip_serializing_if
                // may have already dropped it, but we ensure it stays absent.
                map.remove("options_context");
            }
            Some(TechnicalOptionsContext::FetchFailed { reason }) => {
                map.insert(
                    "options_context".to_owned(),
                    json!({ "status": "fetch_failed", "reason": reason }),
                );
            }
            Some(TechnicalOptionsContext::Available { outcome }) => {
                let compact_outcome = compact_outcome_value(outcome);
                map.insert(
                    "options_context".to_owned(),
                    json!({ "status": "available", "outcome": compact_outcome }),
                );
            }
        }
    }

    value
}

/// Build a compact `Value` for a single `OptionsOutcome`, dropping large arrays.
fn compact_outcome_value(outcome: &OptionsOutcome) -> Value {
    match outcome {
        OptionsOutcome::Snapshot(snap) => compact_snapshot_value(snap),
        OptionsOutcome::NoListedInstrument => json!({ "kind": "no_listed_instrument" }),
        OptionsOutcome::SparseChain => json!({ "kind": "sparse_chain" }),
        OptionsOutcome::HistoricalRun => json!({ "kind": "historical_run" }),
        OptionsOutcome::MissingSpot => json!({ "kind": "missing_spot" }),
    }
}

/// Build a compact `Value` for an `OptionsSnapshot`, omitting
/// `near_term_strikes` and `iv_term_structure` but retaining the scalar
/// summary fields and one derived metric (`highest_oi_strike`).
fn compact_snapshot_value(snap: &OptionsSnapshot) -> Value {
    let highest_oi_strike = snap
        .near_term_strikes
        .iter()
        .filter_map(|s| {
            let total_oi = s.call_oi.unwrap_or(0).saturating_add(s.put_oi.unwrap_or(0));
            if total_oi > 0 { Some((s.strike, total_oi)) } else { None }
        })
        .max_by_key(|&(_, oi)| oi)
        .map(|(strike, oi)| json!({ "strike": strike, "oi": oi }));

    let mut v = json!({
        "kind": "snapshot",
        "atm_iv": snap.atm_iv,
        "put_call_volume_ratio": snap.put_call_volume_ratio,
        "put_call_oi_ratio": snap.put_call_oi_ratio,
        "max_pain_strike": snap.max_pain_strike,
        "near_term_expiration": snap.near_term_expiration,
    });

    if let Some(h) = highest_oi_strike {
        v["highest_oi_strike"] = h;
    }

    v
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::traits::options::{IvTermPoint, NearTermStrike, OptionsOutcome, OptionsSnapshot},
        state::{TechnicalData, TechnicalOptionsContext},
    };

    /// Build a `TechnicalData` with a realistic `Available { Snapshot }` options context
    /// that contains at least two `near_term_strikes` and two `iv_term_structure` entries.
    fn sample_technical_with_options_context_for_projection_tests() -> TechnicalData {
        let snap = OptionsSnapshot {
            spot_price: 182.0,
            atm_iv: 0.28,
            iv_term_structure: vec![
                IvTermPoint {
                    expiration: "2026-01-17".to_owned(),
                    atm_iv: 0.28,
                },
                IvTermPoint {
                    expiration: "2026-02-21".to_owned(),
                    atm_iv: 0.31,
                },
            ],
            put_call_volume_ratio: 1.1,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 180.0,
            near_term_expiration: "2026-01-17".to_owned(),
            near_term_strikes: vec![
                NearTermStrike {
                    strike: 175.0,
                    call_iv: Some(0.25),
                    put_iv: Some(0.30),
                    call_volume: Some(1_000),
                    put_volume: Some(2_000),
                    call_oi: Some(5_000),
                    put_oi: Some(7_500),
                },
                NearTermStrike {
                    strike: 180.0,
                    call_iv: Some(0.27),
                    put_iv: Some(0.28),
                    call_volume: Some(3_000),
                    put_volume: Some(1_500),
                    call_oi: Some(8_000),
                    put_oi: Some(4_500),
                },
            ],
        };

        TechnicalData {
            rsi: Some(58.0),
            macd: None,
            atr: Some(3.1),
            sma_20: Some(182.0),
            sma_50: Some(176.0),
            ema_12: Some(183.0),
            ema_26: Some(178.0),
            bollinger_upper: Some(188.0),
            bollinger_lower: Some(172.0),
            support_level: Some(176.5),
            resistance_level: Some(187.5),
            volume_avg: Some(65_000_000.0),
            summary: "Momentum constructive.".to_owned(),
            options_summary: Some("Near-term IV elevated.".to_owned()),
            options_context: Some(TechnicalOptionsContext::Available {
                outcome: OptionsOutcome::Snapshot(snap),
            }),
        }
    }

    // ── Core projection tests ─────────────────────────────────────────────

    #[test]
    fn compact_technical_value_preserves_indicator_fields() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let v = compact_technical_value(&data);
        // Indicator fields must survive
        assert_eq!(v["rsi"], json!(58.0));
        assert_eq!(v["atr"], json!(3.1));
        assert_eq!(v["sma_20"], json!(182.0));
        assert_eq!(v["summary"], json!("Momentum constructive."));
        assert_eq!(v["options_summary"], json!("Near-term IV elevated."));
    }

    #[test]
    fn compact_technical_value_includes_options_context_summary_fields() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let v = compact_technical_value(&data);

        let oc = &v["options_context"];
        assert_eq!(oc["status"], json!("available"));

        let outcome = &oc["outcome"];
        assert_eq!(outcome["kind"], json!("snapshot"));
        assert_eq!(outcome["atm_iv"], json!(0.28));
        assert_eq!(outcome["put_call_volume_ratio"], json!(1.1));
        assert_eq!(outcome["put_call_oi_ratio"], json!(1.0));
        assert_eq!(outcome["max_pain_strike"], json!(180.0));
        assert_eq!(outcome["near_term_expiration"], json!("2026-01-17"));
    }

    #[test]
    fn compact_technical_value_drops_near_term_strikes_array() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let v = compact_technical_value(&data);
        // near_term_strikes must not appear anywhere in the compact value
        let serialized = serde_json::to_string(&v).unwrap();
        assert!(
            !serialized.contains("near_term_strikes"),
            "near_term_strikes array must be stripped: {serialized}"
        );
    }

    #[test]
    fn compact_technical_value_drops_iv_term_structure_array() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let v = compact_technical_value(&data);
        let serialized = serde_json::to_string(&v).unwrap();
        assert!(
            !serialized.contains("iv_term_structure"),
            "iv_term_structure array must be stripped: {serialized}"
        );
    }

    #[test]
    fn compact_technical_value_drops_spot_price() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let v = compact_technical_value(&data);
        let serialized = serde_json::to_string(&v).unwrap();
        assert!(
            !serialized.contains("spot_price"),
            "spot_price must be stripped from options context: {serialized}"
        );
    }

    #[test]
    fn compact_technical_value_includes_highest_oi_strike() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let v = compact_technical_value(&data);
        // Strike 175 has OI = 5_000 + 7_500 = 12_500
        // Strike 180 has OI = 8_000 + 4_500 = 12_500 — tie; max_by_key picks last in iteration
        // (180 has same OI as 175 so last wins = 180)
        // Actually, 175: call_oi=5000 + put_oi=7500 = 12500, 180: call_oi=8000 + put_oi=4500 = 12500
        // max_by_key keeps the last of equal keys, so 180 wins
        let highest = &v["options_context"]["outcome"]["highest_oi_strike"];
        assert_eq!(highest["oi"], json!(12_500u64));
    }

    #[test]
    fn compact_technical_value_no_options_context_drops_key() {
        let mut data = sample_technical_with_options_context_for_projection_tests();
        data.options_context = None;
        let v = compact_technical_value(&data);
        let serialized = serde_json::to_string(&v).unwrap();
        assert!(
            !serialized.contains("options_context"),
            "options_context key must be absent when None: {serialized}"
        );
    }

    #[test]
    fn compact_technical_value_fetch_failed_produces_compact_object() {
        let mut data = sample_technical_with_options_context_for_projection_tests();
        data.options_context = Some(TechnicalOptionsContext::FetchFailed {
            reason: "timeout".to_owned(),
        });
        let v = compact_technical_value(&data);
        assert_eq!(v["options_context"]["status"], json!("fetch_failed"));
        assert_eq!(v["options_context"]["reason"], json!("timeout"));
    }

    #[test]
    fn compact_technical_value_no_listed_instrument_produces_compact_object() {
        let mut data = sample_technical_with_options_context_for_projection_tests();
        data.options_context = Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::NoListedInstrument,
        });
        let v = compact_technical_value(&data);
        assert_eq!(v["options_context"]["status"], json!("available"));
        assert_eq!(
            v["options_context"]["outcome"]["kind"],
            json!("no_listed_instrument")
        );
    }

    #[test]
    fn compact_technical_value_no_highest_oi_when_strikes_empty() {
        let snap = OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-01-17".to_owned(),
            near_term_strikes: vec![],
        };
        let mut data = sample_technical_with_options_context_for_projection_tests();
        data.options_context = Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(snap),
        });
        let v = compact_technical_value(&data);
        // highest_oi_strike must be absent when strikes is empty
        assert!(
            v["options_context"]["outcome"]["highest_oi_strike"].is_null(),
            "highest_oi_strike should be absent for empty strikes: {v:?}"
        );
    }

    #[test]
    fn compact_technical_report_returns_sanitized_string() {
        let data = sample_technical_with_options_context_for_projection_tests();
        let report = compact_technical_report(&data);
        // Must contain key fields
        assert!(report.contains("atm_iv"), "report: {report}");
        assert!(report.contains("near_term_expiration"), "report: {report}");
        // Must not contain stripped arrays
        assert!(
            !report.contains("near_term_strikes"),
            "near_term_strikes must be absent: {report}"
        );
        assert!(
            !report.contains("iv_term_structure"),
            "iv_term_structure must be absent: {report}"
        );
    }

    // ── Legacy compatibility test ─────────────────────────────────────────

    #[test]
    fn downstream_serializer_handles_legacy_options_summary_blob() {
        // TechnicalData with options_context: None but options_summary present
        // (the pre-Task-1 layout) must serialize coherently without treating
        // the blob as structured state.
        let data = TechnicalData {
            rsi: Some(55.0),
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "Legacy run.".to_owned(),
            options_summary: Some("{ old raw json blob }".to_owned()),
            options_context: None,
        };

        let v = compact_technical_value(&data);
        let serialized = serde_json::to_string(&v).unwrap();

        // options_summary is a plain string — must be present as-is
        assert!(
            serialized.contains("old raw json blob"),
            "legacy options_summary must pass through: {serialized}"
        );
        // options_context key must be absent
        assert!(
            !serialized.contains("options_context"),
            "options_context key must be absent for legacy data: {serialized}"
        );
        // near_term_strikes must not appear at all
        assert!(
            !serialized.contains("near_term_strikes"),
            "near_term_strikes must not appear in legacy output: {serialized}"
        );
    }
}
