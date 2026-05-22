//! ETF vs benchmark tracking error.

use std::collections::BTreeMap;

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
    let etf_returns = daily_returns_by_date(etf_ohlcv);
    let bench_returns = daily_returns_by_date(benchmark_ohlcv);
    let aligned: Vec<(f64, f64)> = etf_returns
        .iter()
        .filter_map(|(date, etf_return)| {
            bench_returns
                .get(date)
                .map(|benchmark_return| (*etf_return, *benchmark_return))
        })
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

fn daily_returns_by_date(candles: &[Candle]) -> BTreeMap<String, f64> {
    candles
        .windows(2)
        .map(|w| (w[1].date.clone(), (w[1].close - w[0].close) / w[0].close))
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

    fn synth_candle(date: &str, close: f64) -> Candle {
        Candle {
            date: date.to_owned(),
            open: close,
            high: close,
            low: close,
            close,
            volume: None,
        }
    }

    #[test]
    fn compute_tracking_error_returns_none_for_short_series() {
        let etf: Vec<Candle> = (0..10)
            .map(|i| synth_candle(&format!("2024-01-{:02}", i + 1), 100.0 + i as f64))
            .collect();
        let bench: Vec<Candle> = (0..10)
            .map(|i| synth_candle(&format!("2024-01-{:02}", i + 1), 100.0 + i as f64))
            .collect();
        assert!(compute_tracking_error(&etf, &bench, "^GSPC").is_none());
    }

    #[test]
    fn compute_tracking_error_returns_zero_when_series_identical() {
        let etf: Vec<Candle> = (0..100)
            .map(|i| {
                synth_candle(
                    &format!("2024-01-{:02}-{:02}", (i / 31) + 1, (i % 31) + 1),
                    100.0 + i as f64,
                )
            })
            .collect();
        let bench: Vec<Candle> = (0..100)
            .map(|i| {
                synth_candle(
                    &format!("2024-01-{:02}-{:02}", (i / 31) + 1, (i % 31) + 1),
                    100.0 + i as f64,
                )
            })
            .collect();
        let te = compute_tracking_error(&etf, &bench, "^GSPC").expect("expected Some");
        assert!(te.te_pct_90d.abs() < 1e-9);
    }

    #[test]
    fn compute_tracking_error_aligns_returns_by_date_instead_of_position() {
        let etf: Vec<Candle> = (0..35)
            .map(|i| synth_candle(&format!("2024-02-{:02}", i + 1), 100.0 + i as f64))
            .collect();
        let bench: Vec<Candle> = (0..36)
            .map(|i| synth_candle(&format!("2024-02-{:02}", i + 1), 200.0 + i as f64))
            .collect();

        let te = compute_tracking_error(&etf, &bench[1..], "^GSPC")
            .expect("date-aligned overlap should still produce tracking error");

        assert_eq!(te.sample_days, 33);
    }
}
