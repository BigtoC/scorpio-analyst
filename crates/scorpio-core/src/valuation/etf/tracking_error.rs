//! ETF vs benchmark tracking error.

use crate::data::yfinance::Candle;
use crate::state::TrackingError;

/// Annualised tracking error.
/// Returns `None` when fewer than 30 overlapping daily-return samples are
/// present.
pub(crate) fn compute_tracking_error(
    etf_ohlcv: &[Candle],
    benchmark_ohlcv: &[Candle],
    benchmark_symbol: &str,
) -> Option<TrackingError> {
    let etf_returns = daily_returns(etf_ohlcv);
    let bench_returns = daily_returns(benchmark_ohlcv);
    let aligned: Vec<(f64, f64)> = etf_returns
        .iter()
        .zip(bench_returns.iter())
        .map(|(&a, &b)| (a, b))
        .collect();
    if aligned.len() < 30 {
        return None;
    }
    let te_90 = stdev_of_diff(&aligned, 63);
    let te_1y = stdev_of_diff(&aligned, aligned.len().min(252));
    Some(TrackingError {
        benchmark_symbol: benchmark_symbol.to_owned(),
        te_pct_90d: annualise(te_90),
        te_pct_1y: annualise(te_1y),
        sample_days: aligned.len() as u32,
    })
}

fn daily_returns(candles: &[Candle]) -> Vec<f64> {
    candles
        .windows(2)
        .map(|w| (w[1].close - w[0].close) / w[0].close)
        .collect()
}

fn stdev_of_diff(pairs: &[(f64, f64)], window: usize) -> f64 {
    let window = window.min(pairs.len());
    if window < 2 {
        return 0.0;
    }
    let diffs: Vec<f64> = pairs
        .iter()
        .rev()
        .take(window)
        .map(|(a, b)| a - b)
        .collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let var = diffs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / (diffs.len() - 1) as f64;
    var.sqrt()
}

fn annualise(daily_stdev: f64) -> f64 {
    daily_stdev * (252_f64).sqrt() * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_candle(close: f64) -> Candle {
        Candle {
            date: "2024-01-15".to_owned(),
            open: close,
            high: close,
            low: close,
            close,
            volume: None,
        }
    }

    #[test]
    fn compute_tracking_error_returns_none_for_short_series() {
        let etf: Vec<Candle> = (0..10).map(|i| synth_candle(100.0 + i as f64)).collect();
        let bench: Vec<Candle> = (0..10).map(|i| synth_candle(100.0 + i as f64)).collect();
        assert!(compute_tracking_error(&etf, &bench, "^GSPC").is_none());
    }

    #[test]
    fn compute_tracking_error_returns_zero_when_series_identical() {
        let etf: Vec<Candle> = (0..100).map(|i| synth_candle(100.0 + i as f64)).collect();
        let bench: Vec<Candle> = (0..100).map(|i| synth_candle(100.0 + i as f64)).collect();
        let te = compute_tracking_error(&etf, &bench, "^GSPC").expect("expected Some");
        assert!(te.te_pct_90d.abs() < 1e-9);
    }
}
