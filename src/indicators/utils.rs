//! Crate-private helper utilities shared across the calculator submodules.

use crate::data::yfinance::Candle;
use crate::error::TradingError;

// ─── Error mapping ────────────────────────────────────────────────────────────

pub(crate) fn kand_err(err: kand::KandError, context: &str) -> TradingError {
    TradingError::SchemaViolation {
        message: format!("{context}: {err}"),
    }
}

// ─── NaN helpers ──────────────────────────────────────────────────────────────

#[inline]
pub(crate) fn nan_to_opt(v: f64) -> Option<f64> {
    if v.is_nan() { None } else { Some(v) }
}

pub(crate) fn to_opt_vec(buf: &[f64]) -> Vec<Option<f64>> {
    buf.iter().copied().map(nan_to_opt).collect()
}

/// Returns the last non-`None` value in a series.
pub(crate) fn last_valid(series: &[Option<f64>]) -> Option<f64> {
    series.iter().rev().find_map(|v| *v)
}

// ─── Price-series extractors ──────────────────────────────────────────────────

pub(crate) fn closes(candles: &[Candle]) -> Vec<f64> {
    candles.iter().map(|c| c.close).collect()
}

pub(crate) fn highs(candles: &[Candle]) -> Vec<f64> {
    candles.iter().map(|c| c.high).collect()
}

pub(crate) fn lows(candles: &[Candle]) -> Vec<f64> {
    candles.iter().map(|c| c.low).collect()
}
