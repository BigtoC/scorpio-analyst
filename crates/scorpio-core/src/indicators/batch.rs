//! Batch indicator computation and the named-indicator selection API.
//!
//! - [`calculate_indicator_by_name`] – compute a single indicator by its
//!   prompt-compatible name string.
//! - [`calculate_all_indicators`] – compute all indicators and assemble a
//!   [`TechnicalData`] snapshot.

use crate::constants::MAX_INDICATOR_NAME_LEN;
use crate::data::yfinance::Candle;
use crate::error::TradingError;
use crate::state::{MacdValues, TechnicalData};

use super::core_math::{
    calculate_atr, calculate_bollinger_bands, calculate_ema, calculate_macd, calculate_rsi,
    calculate_sma, calculate_vwma,
};
use super::support_resistance::derive_support_resistance;
use super::types::NamedIndicatorOutput;
use super::utils::last_valid;

// ─── Named indicator API ──────────────────────────────────────────────────────

/// Compute a single technical indicator identified by its prompt-compatible
/// name and return a [`NamedIndicatorOutput`].
///
/// Supported names:
/// `close_50_sma`, `close_200_sma`, `close_10_ema`, `macd`, `macds`,
/// `macdh`, `rsi`, `boll`, `boll_ub`, `boll_lb`, `atr`, `vwma`.
///
/// MACD-family names (`macd`, `macds`, `macdh`) all internally compute the
/// full MACD(12/26/9) and return only the requested series.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` for an empty candle array or an
/// unrecognised indicator name.
pub fn calculate_indicator_by_name(
    name: &str,
    candles: &[Candle],
) -> Result<NamedIndicatorOutput, TradingError> {
    // Reject excessively long names before they appear in any error message.
    if name.len() > MAX_INDICATOR_NAME_LEN {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "indicator name exceeds {MAX_INDICATOR_NAME_LEN} characters; \
                 supported names: close_50_sma, close_200_sma, close_10_ema, \
                 macd, macds, macdh, rsi, boll, boll_ub, boll_lb, atr, vwma"
            ),
        });
    }
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute indicator: empty candle array".to_owned(),
        });
    }
    let values: Vec<Option<f64>> = match name {
        "close_50_sma" => calculate_sma(candles, 50)?,
        "close_200_sma" => calculate_sma(candles, 200)?,
        "close_10_ema" => calculate_ema(candles, 10)?,
        "rsi" => calculate_rsi(candles, 14)?,
        "atr" => calculate_atr(candles, 14)?,
        "vwma" => calculate_vwma(candles, 20)?,
        // Compute MACD once for the whole family; pick the requested sub-series.
        "macd" | "macds" | "macdh" => {
            let result = calculate_macd(candles, 12, 26, 9)?;
            match name {
                "macds" => result.signal_line,
                "macdh" => result.histogram,
                _ => result.macd_line,
            }
        }
        // Compute Bollinger Bands once for the whole family; pick the requested band.
        "boll" | "boll_ub" | "boll_lb" => {
            let result = calculate_bollinger_bands(candles, 20, 2.0)?;
            match name {
                "boll_ub" => result.upper,
                "boll_lb" => result.lower,
                _ => result.middle,
            }
        }
        other => {
            // Truncate the reflected name so that large values cannot inflate
            // the error message. The full name has already been length-checked
            // above, but this provides an extra explicit bound.
            let display: String = other.chars().take(32).collect();
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "unknown indicator name: {display:?}. Supported: \
                     close_50_sma, close_200_sma, close_10_ema, \
                     macd, macds, macdh, rsi, boll, boll_ub, boll_lb, atr, vwma"
                ),
            });
        }
    };
    Ok(NamedIndicatorOutput {
        indicator: name.to_owned(),
        values,
    })
}

// ─── Batch calculation ────────────────────────────────────────────────────────

/// Compute all technical indicators with default periods and assemble a
/// [`TechnicalData`] snapshot.
///
/// Default periods: RSI 14, MACD 12/26/9, ATR 14, Bollinger 20/2.0,
/// SMA 50 and SMA 20 (Bollinger middle), EMA 12 and EMA 26 (MACD EMAs),
/// VWMA 20.
///
/// If an indicator cannot be computed due to insufficient candles, the
/// corresponding field is `None`. Only an empty input produces an error.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` if `candles` is empty.
pub fn calculate_all_indicators(candles: &[Candle]) -> Result<TechnicalData, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute indicators: empty candle array".to_owned(),
        });
    }

    // RSI 14
    let rsi_series = calculate_rsi(candles, 14)?;
    let rsi_val = last_valid(&rsi_series);

    // MACD 12/26/9
    let macd_res = calculate_macd(candles, 12, 26, 9)?;
    let macd_val = match (
        last_valid(&macd_res.macd_line),
        last_valid(&macd_res.signal_line),
        last_valid(&macd_res.histogram),
    ) {
        (Some(line), Some(sig), Some(hist)) => Some(MacdValues {
            macd_line: line,
            signal_line: sig,
            histogram: hist,
        }),
        _ => None,
    };

    // ATR 14
    let atr_series = calculate_atr(candles, 14)?;
    let atr_val = last_valid(&atr_series);

    // Bollinger Bands 20/2.0
    let boll = calculate_bollinger_bands(candles, 20, 2.0)?;
    let boll_upper = last_valid(&boll.upper);
    let boll_lower = last_valid(&boll.lower);
    // sma_20 → Bollinger middle band (SMA 20)
    let sma_20_val = last_valid(&boll.middle);

    // SMA 50
    let sma50 = calculate_sma(candles, 50)?;
    let sma_50_val = last_valid(&sma50);

    // EMA 12 (fast MACD component) and EMA 26 (slow MACD component)
    let ema12 = calculate_ema(candles, 12)?;
    let ema_12_val = last_valid(&ema12);
    let ema26 = calculate_ema(candles, 26)?;
    let ema_26_val = last_valid(&ema26);

    // VWMA 20 → volume_avg field
    let vwma = calculate_vwma(candles, 20)?;
    let vwma_val = last_valid(&vwma);

    // Support / Resistance
    let (support, resistance) = derive_support_resistance(candles);

    // Summary
    let summary = build_summary(rsi_val, macd_val.as_ref(), atr_val, sma_50_val);

    Ok(TechnicalData {
        rsi: rsi_val,
        macd: macd_val,
        atr: atr_val,
        sma_20: sma_20_val,
        sma_50: sma_50_val,
        ema_12: ema_12_val,
        ema_26: ema_26_val,
        bollinger_upper: boll_upper,
        bollinger_lower: boll_lower,
        support_level: support,
        resistance_level: resistance,
        volume_avg: vwma_val,
        summary,
    })
}

pub(crate) fn build_summary(
    rsi: Option<f64>,
    macd: Option<&MacdValues>,
    atr: Option<f64>,
    sma_50: Option<f64>,
) -> String {
    let mut parts = Vec::new();
    if let Some(v) = rsi {
        parts.push(format!("RSI={v:.1}"));
    }
    if let Some(m) = macd {
        parts.push(format!(
            "MACD={:.4}|Signal={:.4}|Hist={:.4}",
            m.macd_line, m.signal_line, m.histogram
        ));
    }
    if let Some(v) = atr {
        parts.push(format!("ATR={v:.4}"));
    }
    if let Some(v) = sma_50 {
        parts.push(format!("SMA50={v:.2}"));
    }
    if parts.is_empty() {
        "Insufficient data for indicator summary".to_owned()
    } else {
        parts.join("; ")
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::yfinance::Candle;
    use crate::error::TradingError;
    use crate::indicators::test_utils::*;

    // ── Named indicator API ────────────────────────────────────────────────

    #[test]
    fn named_indicator_empty_returns_schema_violation() {
        let err = calculate_indicator_by_name("rsi", &[]).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn named_indicator_unknown_name_returns_schema_violation() {
        let candles = rising_candles(50, 100.0, 1.0);
        let err = calculate_indicator_by_name("unknown_xyz", &candles).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn named_indicator_all_prompt_names_resolve() {
        let names = [
            "close_50_sma",
            "close_200_sma",
            "close_10_ema",
            "macd",
            "macds",
            "macdh",
            "rsi",
            "boll",
            "boll_ub",
            "boll_lb",
            "atr",
            "vwma",
        ];
        let candles = rising_candles(250, 100.0, 0.5);
        for name in &names {
            let result = calculate_indicator_by_name(name, &candles);
            assert!(
                result.is_ok(),
                "Named indicator {name} failed: {:?}",
                result.err()
            );
            let out = result.unwrap();
            assert_eq!(&out.indicator, name);
            assert_eq!(out.values.len(), 250);
        }
    }

    #[test]
    fn close_200_sma_with_100_candles_all_none() {
        let candles = rising_candles(100, 50.0, 1.0);
        let out = calculate_indicator_by_name("close_200_sma", &candles).unwrap();
        assert!(
            out.values.iter().all(|v| v.is_none()),
            "Expected all None for SMA-200 with 100 candles"
        );
    }

    // ── calculate_all_indicators ──────────────────────────────────────────

    #[test]
    fn calculate_all_empty_returns_schema_violation() {
        let err = calculate_all_indicators(&[]).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn calculate_all_200_candles_populates_common_fields() {
        let candles = rising_candles(200, 50.0, 1.0);
        let td = calculate_all_indicators(&candles).unwrap();
        assert!(td.rsi.is_some(), "rsi should be Some");
        assert!(td.macd.is_some(), "macd should be Some");
        assert!(td.atr.is_some(), "atr should be Some");
        assert!(td.sma_50.is_some(), "sma_50 should be Some");
        assert!(
            td.bollinger_upper.is_some(),
            "bollinger_upper should be Some"
        );
        assert!(
            td.bollinger_lower.is_some(),
            "bollinger_lower should be Some"
        );
        assert!(td.ema_12.is_some(), "ema_12 should be Some");
        assert!(td.ema_26.is_some(), "ema_26 should be Some");
        assert!(td.volume_avg.is_some(), "volume_avg (VWMA) should be Some");
        assert!(!td.summary.is_empty(), "summary should not be empty");
    }

    #[test]
    fn calculate_all_100_candles_partial_results() {
        let candles = rising_candles(100, 50.0, 1.0);
        let td = calculate_all_indicators(&candles).unwrap();
        assert!(
            td.rsi.is_some(),
            "RSI should be computable with 100 candles"
        );
        assert!(
            td.atr.is_some(),
            "ATR should be computable with 100 candles"
        );
        assert!(
            td.sma_50.is_some(),
            "SMA 50 should be computable with 100 candles"
        );
    }

    #[test]
    fn no_nan_in_technical_data_struct() {
        let candles = rising_candles(200, 100.0, 1.0);
        let td = calculate_all_indicators(&candles).unwrap();
        if let Some(v) = td.rsi {
            assert!(!v.is_nan(), "rsi is NaN");
        }
        if let Some(v) = td.atr {
            assert!(!v.is_nan(), "atr is NaN");
        }
        if let Some(v) = td.sma_20 {
            assert!(!v.is_nan(), "sma_20 is NaN");
        }
        if let Some(v) = td.sma_50 {
            assert!(!v.is_nan(), "sma_50 is NaN");
        }
        if let Some(v) = td.ema_12 {
            assert!(!v.is_nan(), "ema_12 is NaN");
        }
        if let Some(v) = td.ema_26 {
            assert!(!v.is_nan(), "ema_26 is NaN");
        }
        if let Some(v) = td.bollinger_upper {
            assert!(!v.is_nan(), "bollinger_upper is NaN");
        }
        if let Some(v) = td.bollinger_lower {
            assert!(!v.is_nan(), "bollinger_lower is NaN");
        }
        if let Some(v) = td.volume_avg {
            assert!(!v.is_nan(), "volume_avg is NaN");
        }
        if let Some(ref m) = td.macd {
            assert!(!m.macd_line.is_nan(), "macd_line is NaN");
            assert!(!m.signal_line.is_nan(), "signal_line is NaN");
            assert!(!m.histogram.is_nan(), "histogram is NaN");
        }
    }

    // ── Integration ────────────────────────────────────────────────────────

    #[test]
    fn integration_200_candles_full_population() {
        let candles = rising_candles(200, 50.0, 0.5);
        let td = calculate_all_indicators(&candles).unwrap();
        assert!(td.rsi.is_some());
        assert!(td.macd.is_some());
        assert!(td.atr.is_some());
        assert!(td.sma_20.is_some());
        assert!(td.sma_50.is_some());
        assert!(td.ema_12.is_some());
        assert!(td.ema_26.is_some());
        assert!(td.bollinger_upper.is_some());
        assert!(td.bollinger_lower.is_some());
        assert!(td.volume_avg.is_some());
        let m = td.macd.unwrap();
        assert!(!m.macd_line.is_nan());
        assert!(!m.signal_line.is_nan());
        assert!(!m.histogram.is_nan());
        assert!(!td.summary.is_empty());
    }

    #[test]
    fn integration_pipeline_candles_to_technical_data() {
        let mock_candles: Vec<Candle> = (0..250)
            .map(|i| {
                let base = 100.0 + (i as f64 * 0.5).sin() * 10.0 + i as f64 * 0.1;
                Candle {
                    date: format!("2023-{:02}-{:02}", (i / 28) + 1, (i % 28) + 1),
                    open: base,
                    high: base * 1.01,
                    low: base * 0.99,
                    close: base,
                    volume: Some(500_000 + (i as u64) * 1_000),
                }
            })
            .collect();

        let td = calculate_all_indicators(&mock_candles).unwrap();
        assert!(td.rsi.is_some());
        assert!(td.macd.is_some());
        assert!(td.atr.is_some());
        assert!(td.sma_50.is_some());
        assert!(td.volume_avg.is_some());

        let rsi_out = calculate_indicator_by_name("rsi", &mock_candles).unwrap();
        assert_eq!(rsi_out.values.len(), mock_candles.len());
        assert_eq!(rsi_out.indicator, "rsi");
    }
}
