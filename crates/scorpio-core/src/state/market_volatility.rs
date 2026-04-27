use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// VIX trend direction derived from comparing the 5-day SMA to the 20-day SMA.
///
/// A 5% band around the 20-day SMA defines the "Stable" zone; outside that band
/// the trend is classified as Rising or Falling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VixTrend {
    Rising,
    Falling,
    Stable,
}

impl fmt::Display for VixTrend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VixTrend::Rising => write!(f, "Rising"),
            VixTrend::Falling => write!(f, "Falling"),
            VixTrend::Stable => write!(f, "Stable"),
        }
    }
}

/// Market volatility regime based on the absolute VIX level.
///
/// Thresholds follow widely-used practitioner conventions:
/// - Low: VIX < 15 — complacency, benign conditions
/// - Normal: 15 ≤ VIX < 20 — typical historical range
/// - Elevated: 20 ≤ VIX < 30 — uncertainty, tighten risk controls
/// - High: VIX ≥ 30 — fear/crisis conditions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VixRegime {
    Low,
    Normal,
    Elevated,
    High,
}

impl fmt::Display for VixRegime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VixRegime::Low => write!(f, "Low"),
            VixRegime::Normal => write!(f, "Normal"),
            VixRegime::Elevated => write!(f, "Elevated"),
            VixRegime::High => write!(f, "High"),
        }
    }
}

/// Market-wide implied volatility snapshot derived from the CBOE VIX index.
///
/// Fetched from Yahoo Finance (`^VIX`) before the analyst fan-out so that all
/// downstream agents — researchers, trader, risk, fund manager — see the same
/// volatility context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MarketVolatilityData {
    /// Most recent closing VIX value.
    pub vix_level: f64,
    /// 20-day simple moving average of VIX closing prices.
    pub vix_sma_20: f64,
    /// Short-term trend direction (5-day SMA vs 20-day SMA with 5% band).
    pub vix_trend: VixTrend,
    /// Volatility regime classification based on absolute VIX level.
    pub vix_regime: VixRegime,
    /// ISO-8601 date of the most recent candle used (audit trail).
    pub fetched_at: String,
}

impl MarketVolatilityData {
    /// Returns a compact one-line summary suitable for prompts and the final report.
    pub fn summary(&self) -> String {
        format!(
            "VIX {:.1} ({} regime, {} trend, SMA-20: {:.1})",
            self.vix_level, self.vix_regime, self.vix_trend, self.vix_sma_20
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MarketVolatilityData {
        MarketVolatilityData {
            vix_level: 18.5,
            vix_sma_20: 17.2,
            vix_trend: VixTrend::Rising,
            vix_regime: VixRegime::Normal,
            fetched_at: "2026-04-04".to_owned(),
        }
    }

    #[test]
    fn serde_round_trip() {
        let data = sample();
        let json = serde_json::to_string(&data).unwrap();
        let back: MarketVolatilityData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn unknown_fields_are_tolerated_for_forward_compat() {
        // Snapshotted state structs (everything reachable from `TradingState`
        // via serde) must NOT use `#[serde(deny_unknown_fields)]` because it
        // turns every additive field into a backward-incompatible change.
        // This test pins the contract: an extra unknown key must not block
        // deserialization.
        let json = r#"{"vix_level":18.5,"vix_sma_20":17.2,"vix_trend":"rising","vix_regime":"normal","fetched_at":"2026-04-04","extra_field":"oops"}"#;
        let parsed: MarketVolatilityData = serde_json::from_str(json)
            .expect("snapshotted state must tolerate unknown fields for forward-compat");
        assert_eq!(parsed.vix_level, 18.5);
    }

    #[test]
    fn vix_regime_boundaries() {
        // boundary values for each regime
        let cases: &[(f64, VixRegime)] = &[
            (14.99, VixRegime::Low),
            (15.0, VixRegime::Normal),
            (19.99, VixRegime::Normal),
            (20.0, VixRegime::Elevated),
            (29.99, VixRegime::Elevated),
            (30.0, VixRegime::High),
        ];
        for (level, expected) in cases {
            let regime = if *level < 15.0 {
                VixRegime::Low
            } else if *level < 20.0 {
                VixRegime::Normal
            } else if *level < 30.0 {
                VixRegime::Elevated
            } else {
                VixRegime::High
            };
            assert_eq!(regime, *expected, "level={level}");
        }
    }

    #[test]
    fn trend_enum_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&VixTrend::Rising).unwrap(),
            "\"rising\""
        );
        assert_eq!(
            serde_json::to_string(&VixTrend::Falling).unwrap(),
            "\"falling\""
        );
        assert_eq!(
            serde_json::to_string(&VixTrend::Stable).unwrap(),
            "\"stable\""
        );
    }

    #[test]
    fn summary_format() {
        let s = sample().summary();
        assert!(s.contains("VIX 18.5"), "got: {s}");
        assert!(s.contains("Normal regime"), "got: {s}");
        assert!(s.contains("Rising trend"), "got: {s}");
        assert!(s.contains("SMA-20: 17.2"), "got: {s}");
    }
}
