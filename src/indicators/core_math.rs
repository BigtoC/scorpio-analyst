//! Individual technical indicator functions backed by the [`kand`] crate.
//!
//! Each function accepts a `&[Candle]` slice and returns either a per-bar
//! `Vec<Option<f64>>` or a typed result struct. `None` at a position means
//! the indicator did not have enough history at that bar.

use kand::ohlcv::{atr, bbands, ema, macd, rsi, sma};

use crate::data::yfinance::Candle;
use crate::error::TradingError;

use super::types::{BollingerResult, MacdResult};
use super::utils::{closes, highs, kand_err, lows, to_opt_vec};

// ─── 1. Individual indicator functions ───────────────────────────────────────

/// Compute RSI (Relative Strength Index) using Wilder's smoothing.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty.
/// If `candles.len()` is below the lookback period, returns
/// `Ok(vec![None; n])` rather than an error.
pub fn calculate_rsi(candles: &[Candle], period: usize) -> Result<Vec<Option<f64>>, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute RSI: empty candle array".to_owned(),
        });
    }
    let n = candles.len();
    let prices = closes(candles);
    let mut out_rsi = vec![0.0_f64; n];
    let mut out_avg_gain = vec![0.0_f64; n];
    let mut out_avg_loss = vec![0.0_f64; n];

    match rsi::rsi(
        &prices,
        period,
        &mut out_rsi,
        &mut out_avg_gain,
        &mut out_avg_loss,
    ) {
        Ok(()) => Ok(to_opt_vec(&out_rsi)),
        Err(kand::KandError::InsufficientData) => Ok(vec![None; n]),
        Err(e) => Err(kand_err(e, "RSI")),
    }
}

/// Compute MACD (Moving Average Convergence Divergence).
///
/// Returns a [`MacdResult`] containing the MACD line, signal line, and
/// histogram, each as a `Vec<Option<f64>>`.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty.
/// Returns all-`None` series when there are insufficient candles for the
/// requested periods.
pub fn calculate_macd(
    candles: &[Candle],
    fast: usize,
    slow: usize,
    signal: usize,
) -> Result<MacdResult, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute MACD: empty candle array".to_owned(),
        });
    }
    let n = candles.len();
    let prices = closes(candles);
    let mut out_macd = vec![0.0_f64; n];
    let mut out_signal = vec![0.0_f64; n];
    let mut out_hist = vec![0.0_f64; n];
    let mut out_fast_ema = vec![0.0_f64; n];
    let mut out_slow_ema = vec![0.0_f64; n];

    match macd::macd(
        &prices,
        fast,
        slow,
        signal,
        &mut out_macd,
        &mut out_signal,
        &mut out_hist,
        &mut out_fast_ema,
        &mut out_slow_ema,
    ) {
        Ok(()) => Ok(MacdResult {
            macd_line: to_opt_vec(&out_macd),
            signal_line: to_opt_vec(&out_signal),
            histogram: to_opt_vec(&out_hist),
        }),
        Err(kand::KandError::InsufficientData) => Ok(MacdResult {
            macd_line: vec![None; n],
            signal_line: vec![None; n],
            histogram: vec![None; n],
        }),
        Err(e) => Err(kand_err(e, "MACD")),
    }
}

/// Compute ATR (Average True Range) using Wilder's RMA smoothing.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty.
/// Returns `Ok(vec![None; n])` when there are insufficient candles for
/// `period`.
pub fn calculate_atr(candles: &[Candle], period: usize) -> Result<Vec<Option<f64>>, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute ATR: empty candle array".to_owned(),
        });
    }
    let n = candles.len();
    let h = highs(candles);
    let l = lows(candles);
    let c = closes(candles);
    let mut out_atr = vec![0.0_f64; n];

    match atr::atr(&h, &l, &c, period, &mut out_atr) {
        Ok(()) => Ok(to_opt_vec(&out_atr)),
        Err(kand::KandError::InsufficientData) => Ok(vec![None; n]),
        Err(e) => Err(kand_err(e, "ATR")),
    }
}

/// Compute Bollinger Bands (upper, middle SMA, lower).
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty.
/// Returns all-`None` series when there are insufficient candles for `period`.
pub fn calculate_bollinger_bands(
    candles: &[Candle],
    period: usize,
    std_dev: f64,
) -> Result<BollingerResult, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute Bollinger Bands: empty candle array".to_owned(),
        });
    }
    let n = candles.len();
    let prices = closes(candles);
    let mut out_upper = vec![0.0_f64; n];
    let mut out_middle = vec![0.0_f64; n];
    let mut out_lower = vec![0.0_f64; n];
    let mut tmp_sma = vec![0.0_f64; n];
    let mut tmp_var = vec![0.0_f64; n];
    let mut tmp_sum = vec![0.0_f64; n];
    let mut tmp_sum_sq = vec![0.0_f64; n];

    match bbands::bbands(
        &prices,
        period,
        std_dev,
        std_dev,
        &mut out_upper,
        &mut out_middle,
        &mut out_lower,
        &mut tmp_sma,
        &mut tmp_var,
        &mut tmp_sum,
        &mut tmp_sum_sq,
    ) {
        Ok(()) => Ok(BollingerResult {
            upper: to_opt_vec(&out_upper),
            middle: to_opt_vec(&out_middle),
            lower: to_opt_vec(&out_lower),
        }),
        Err(kand::KandError::InsufficientData) => Ok(BollingerResult {
            upper: vec![None; n],
            middle: vec![None; n],
            lower: vec![None; n],
        }),
        Err(e) => Err(kand_err(e, "Bollinger Bands")),
    }
}

/// Compute SMA (Simple Moving Average).
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty.
/// Returns `Ok(vec![None; n])` when `candles.len() < period`.
pub fn calculate_sma(candles: &[Candle], period: usize) -> Result<Vec<Option<f64>>, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute SMA: empty candle array".to_owned(),
        });
    }
    let n = candles.len();
    let prices = closes(candles);
    let mut out_sma = vec![0.0_f64; n];

    match sma::sma(&prices, period, &mut out_sma) {
        Ok(()) => Ok(to_opt_vec(&out_sma)),
        Err(kand::KandError::InsufficientData) => Ok(vec![None; n]),
        Err(e) => Err(kand_err(e, "SMA")),
    }
}

/// Compute EMA (Exponential Moving Average) using the default `2/(period+1)`
/// smoothing factor.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty.
/// Returns `Ok(vec![None; n])` when there are insufficient candles for
/// `period`.
pub fn calculate_ema(candles: &[Candle], period: usize) -> Result<Vec<Option<f64>>, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute EMA: empty candle array".to_owned(),
        });
    }
    let n = candles.len();
    let prices = closes(candles);
    let mut out_ema = vec![0.0_f64; n];

    match ema::ema(&prices, period, None, &mut out_ema) {
        Ok(()) => Ok(to_opt_vec(&out_ema)),
        Err(kand::KandError::InsufficientData) => Ok(vec![None; n]),
        Err(e) => Err(kand_err(e, "EMA")),
    }
}

/// Compute VWMA (Volume-Weighted Moving Average).
///
/// Candles with `None` volume contribute zero weight. If all candles in a
/// window have zero or missing volume, the window result is `None`.
///
/// # Errors
///
/// Returns `TradingError::SchemaViolation` when `candles` is empty or
/// `period < 2`.
pub fn calculate_vwma(candles: &[Candle], period: usize) -> Result<Vec<Option<f64>>, TradingError> {
    if candles.is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "cannot compute VWMA: empty candle array".to_owned(),
        });
    }
    if period < 2 {
        return Err(TradingError::SchemaViolation {
            message: format!("VWMA period must be >= 2, got {period}"),
        });
    }
    let n = candles.len();
    if n < period {
        return Ok(vec![None; n]);
    }
    let mut result = vec![None; n];
    for i in (period - 1)..n {
        let window = &candles[(i + 1 - period)..=i];
        let sum_pv: f64 = window
            .iter()
            .map(|c| c.close * c.volume.unwrap_or(0) as f64)
            .sum();
        let sum_v: f64 = window.iter().map(|c| c.volume.unwrap_or(0) as f64).sum();
        if sum_v > 0.0 {
            result[i] = Some(sum_pv / sum_v);
        }
    }
    Ok(result)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::yfinance::Candle;
    use crate::error::TradingError;
    use crate::indicators::test_utils::*;
    // ── RSI ────────────────────────────────────────────────────────────────

    #[test]
    fn rsi_empty_returns_schema_violation() {
        let err = calculate_rsi(&[], 14).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn rsi_insufficient_candles_returns_all_none() {
        let candles = rising_candles(5, 100.0, 1.0);
        let result = calculate_rsi(&candles, 14).unwrap();
        assert_eq!(result.len(), 5);
        assert!(result.iter().all(|v| v.is_none()));
    }

    #[test]
    fn rsi_valid_data_in_range_0_to_100() {
        let candles = rising_candles(200, 100.0, 1.0);
        let result = calculate_rsi(&candles, 14).unwrap();
        assert_eq!(result.len(), 200);
        for v in result.iter().flatten() {
            assert!((0.0..=100.0).contains(v), "RSI out of range: {v}");
        }
        let last = result.iter().rev().find_map(|v| *v).unwrap();
        assert!(last > 50.0, "Expected high RSI for uptrend, got {last}");
    }

    #[test]
    fn rsi_downtrend_is_low() {
        let candles = falling_candles(100, 200.0, 1.0);
        let result = calculate_rsi(&candles, 14).unwrap();
        let last = result.iter().rev().find_map(|v| *v).unwrap();
        assert!(last < 50.0, "Expected low RSI for downtrend, got {last}");
    }

    #[test]
    fn rsi_uptrend_approaches_overbought() {
        let candles = rising_candles(60, 100.0, 2.0);
        let result = calculate_rsi(&candles, 14).unwrap();
        let last = result.iter().rev().find_map(|v| *v).unwrap();
        assert!(
            last > 70.0,
            "Expected RSI > 70 for persistent uptrend, got {last}"
        );
    }

    #[test]
    fn rsi_single_candle_returns_all_none() {
        let result = calculate_rsi(&[candle(100.0)], 14).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_none());
    }

    // ── MACD ───────────────────────────────────────────────────────────────

    #[test]
    fn macd_empty_returns_schema_violation() {
        let err = calculate_macd(&[], 12, 26, 9).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn macd_insufficient_candles_returns_all_none() {
        let candles = rising_candles(10, 100.0, 1.0);
        let result = calculate_macd(&candles, 12, 26, 9).unwrap();
        assert!(result.macd_line.iter().all(|v| v.is_none()));
        assert!(result.signal_line.iter().all(|v| v.is_none()));
        assert!(result.histogram.iter().all(|v| v.is_none()));
    }

    #[test]
    fn macd_valid_data_returns_typed_result() {
        let candles = rising_candles(80, 100.0, 1.0);
        let result = calculate_macd(&candles, 12, 26, 9).unwrap();
        assert_eq!(result.macd_line.len(), 80);
        assert_eq!(result.signal_line.len(), 80);
        assert_eq!(result.histogram.len(), 80);
        assert!(result.macd_line.last().unwrap().is_some());
        assert!(result.signal_line.last().unwrap().is_some());
        assert!(result.histogram.last().unwrap().is_some());
    }

    #[test]
    fn macd_uptrend_has_positive_macd_line() {
        let candles = rising_candles(80, 100.0, 1.0);
        let result = calculate_macd(&candles, 12, 26, 9).unwrap();
        let last_line = result.macd_line.iter().rev().find_map(|v| *v).unwrap();
        assert!(
            last_line > 0.0,
            "Expected positive MACD for uptrend, got {last_line}"
        );
    }

    #[test]
    fn macd_histogram_equals_line_minus_signal() {
        let candles = rising_candles(80, 100.0, 1.0);
        let result = calculate_macd(&candles, 12, 26, 9).unwrap();
        for i in 0..result.macd_line.len() {
            match (
                result.macd_line[i],
                result.signal_line[i],
                result.histogram[i],
            ) {
                (Some(l), Some(s), Some(h)) => {
                    let expected = l - s;
                    assert!(
                        (h - expected).abs() < 1e-9,
                        "Histogram mismatch at {i}: {h} vs {expected}"
                    );
                }
                _ => {}
            }
        }
    }

    // ── ATR ────────────────────────────────────────────────────────────────

    #[test]
    fn atr_empty_returns_schema_violation() {
        let err = calculate_atr(&[], 14).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn atr_insufficient_candles_returns_all_none() {
        let candles = rising_candles(5, 100.0, 1.0);
        let result = calculate_atr(&candles, 14).unwrap();
        assert!(result.iter().all(|v| v.is_none()));
    }

    #[test]
    fn atr_valid_data_returns_positive_values() {
        let candles = rising_candles(50, 100.0, 1.0);
        let result = calculate_atr(&candles, 14).unwrap();
        assert_eq!(result.len(), 50);
        let valid: Vec<f64> = result.iter().flatten().copied().collect();
        assert!(!valid.is_empty(), "Expected some valid ATR values");
        for &v in &valid {
            assert!(v > 0.0, "ATR should be positive, got {v}");
        }
    }

    #[test]
    fn atr_measures_volatility() {
        let low_vol = rising_candles(50, 100.0, 0.1);
        let high_vol = rising_candles(50, 100.0, 5.0);
        let atr_low = calculate_atr(&low_vol, 14)
            .unwrap()
            .iter()
            .rev()
            .find_map(|v| *v)
            .unwrap();
        let atr_high = calculate_atr(&high_vol, 14)
            .unwrap()
            .iter()
            .rev()
            .find_map(|v| *v)
            .unwrap();
        assert!(
            atr_high > atr_low,
            "Higher swing ATR({atr_high}) should exceed low swing ATR({atr_low})"
        );
    }

    // ── Bollinger Bands ────────────────────────────────────────────────────

    #[test]
    fn bollinger_empty_returns_schema_violation() {
        let err = calculate_bollinger_bands(&[], 20, 2.0).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn bollinger_insufficient_candles_returns_all_none() {
        let candles = rising_candles(5, 100.0, 1.0);
        let result = calculate_bollinger_bands(&candles, 20, 2.0).unwrap();
        assert!(result.upper.iter().all(|v| v.is_none()));
        assert!(result.middle.iter().all(|v| v.is_none()));
        assert!(result.lower.iter().all(|v| v.is_none()));
    }

    #[test]
    fn bollinger_upper_above_middle_above_lower() {
        let candles = rising_candles(100, 50.0, 1.0);
        let result = calculate_bollinger_bands(&candles, 20, 2.0).unwrap();
        for i in 0..result.upper.len() {
            match (result.upper[i], result.middle[i], result.lower[i]) {
                (Some(u), Some(m), Some(l)) => {
                    assert!(u > m, "Upper {u} should be > middle {m} at index {i}");
                    assert!(m > l, "Middle {m} should be > lower {l} at index {i}");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn bollinger_bands_symmetric_around_middle() {
        let candles = alternating_candles(60);
        let result = calculate_bollinger_bands(&candles, 20, 2.0).unwrap();
        for i in 0..result.upper.len() {
            if let (Some(u), Some(m), Some(l)) =
                (result.upper[i], result.middle[i], result.lower[i])
            {
                let upper_dist = u - m;
                let lower_dist = m - l;
                assert!(
                    (upper_dist - lower_dist).abs() < 1e-9,
                    "Bands not symmetric at {i}: upper_dist={upper_dist:.6} lower_dist={lower_dist:.6}"
                );
            }
        }
    }

    // ── SMA ────────────────────────────────────────────────────────────────

    #[test]
    fn sma_empty_returns_schema_violation() {
        let err = calculate_sma(&[], 50).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn sma_insufficient_candles_returns_all_none() {
        let candles = rising_candles(3, 100.0, 1.0);
        let result = calculate_sma(&candles, 50).unwrap();
        assert!(result.iter().all(|v| v.is_none()));
    }

    #[test]
    fn sma_known_values_correct() {
        // Closes: 2, 4, 6, 8, 10 – SMA(3) = [None, None, 4, 6, 8]
        let candles: Vec<Candle> = [2.0, 4.0, 6.0, 8.0, 10.0]
            .iter()
            .copied()
            .map(candle)
            .collect();
        let result = calculate_sma(&candles, 3).unwrap();
        assert!(result[0].is_none());
        assert!(result[1].is_none());
        assert!((result[2].unwrap() - 4.0).abs() < 1e-9);
        assert!((result[3].unwrap() - 6.0).abs() < 1e-9);
        assert!((result[4].unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn sma_exactly_at_period_boundary() {
        let candles: Vec<Candle> = [10.0, 20.0, 30.0].iter().copied().map(candle).collect();
        let result = calculate_sma(&candles, 3).unwrap();
        assert!(result[0].is_none());
        assert!(result[1].is_none());
        assert!((result[2].unwrap() - 20.0).abs() < 1e-9);
    }

    // ── EMA ────────────────────────────────────────────────────────────────

    #[test]
    fn ema_empty_returns_schema_violation() {
        let err = calculate_ema(&[], 10).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn ema_insufficient_candles_returns_all_none() {
        let candles = rising_candles(3, 100.0, 1.0);
        let result = calculate_ema(&candles, 10).unwrap();
        assert!(result.iter().all(|v| v.is_none()));
    }

    #[test]
    fn ema_weights_recent_bars_more_than_sma() {
        let descending: Vec<Candle> = (0..10).map(|i| candle(100.0 - i as f64)).collect();
        let ascending: Vec<Candle> = (0..10).map(|i| candle(91.0 + i as f64)).collect();
        let mut candles = descending;
        candles.extend(ascending);

        let sma = calculate_sma(&candles, 10).unwrap();
        let ema = calculate_ema(&candles, 10).unwrap();

        let sma_last = sma.last().unwrap().unwrap();
        let ema_last = ema.last().unwrap().unwrap();
        assert!(
            ema_last > sma_last,
            "EMA({ema_last:.4}) should be above SMA({sma_last:.4}) in a V-recovery"
        );
    }

    #[test]
    fn ema_over_1000_candles_no_nan() {
        let candles = rising_candles(1000, 100.0, 0.1);
        let result = calculate_ema(&candles, 10).unwrap();
        for (i, v) in result.iter().enumerate() {
            if let Some(val) = v {
                assert!(!val.is_nan(), "EMA NaN at index {i}");
                assert!(val.is_finite(), "EMA non-finite at index {i}: {val}");
            }
        }
    }

    // ── VWMA ───────────────────────────────────────────────────────────────

    #[test]
    fn vwma_empty_returns_schema_violation() {
        let err = calculate_vwma(&[], 20).unwrap_err();
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn vwma_insufficient_candles_returns_all_none() {
        let candles = rising_candles(5, 100.0, 1.0);
        let result = calculate_vwma(&candles, 20).unwrap();
        assert!(result.iter().all(|v| v.is_none()));
    }

    #[test]
    fn vwma_known_values_correct() {
        // 3 candles: close=[10,20,30], volume=[1,2,3], period=3
        // VWMA = (10*1 + 20*2 + 30*3) / (1+2+3) = 140/6 ≈ 23.333
        let candles = vec![
            Candle {
                date: "d1".to_owned(),
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: Some(1),
            },
            Candle {
                date: "d2".to_owned(),
                open: 20.0,
                high: 21.0,
                low: 19.0,
                close: 20.0,
                volume: Some(2),
            },
            Candle {
                date: "d3".to_owned(),
                open: 30.0,
                high: 31.0,
                low: 29.0,
                close: 30.0,
                volume: Some(3),
            },
        ];
        let result = calculate_vwma(&candles, 3).unwrap();
        assert!(result[0].is_none());
        assert!(result[1].is_none());
        let expected = 140.0_f64 / 6.0;
        assert!(
            (result[2].unwrap() - expected).abs() < 1e-9,
            "VWMA mismatch: got {:?}, expected {expected}",
            result[2]
        );
    }

    #[test]
    fn vwma_higher_volume_bars_dominate() {
        let candles = vec![
            Candle {
                date: "d1".to_owned(),
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: Some(1),
            },
            Candle {
                date: "d2".to_owned(),
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: Some(1),
            },
            Candle {
                date: "d3".to_owned(),
                open: 200.0,
                high: 201.0,
                low: 199.0,
                close: 200.0,
                volume: Some(1_000),
            },
        ];
        let result = calculate_vwma(&candles, 3).unwrap();
        let v = result[2].unwrap();
        assert!(
            v > 190.0,
            "VWMA should skew toward high-volume bar, got {v}"
        );
    }
}
