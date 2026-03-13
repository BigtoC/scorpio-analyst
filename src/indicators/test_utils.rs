#[cfg(test)]
use crate::data::yfinance::Candle;

/// Build a minimal `Candle` with close = `close`, high/low ± 0.5, volume = 1 000 000.
pub fn candle(close: f64) -> Candle {
    Candle {
        date: "2024-01-01".to_owned(),
        open: close,
        high: close + 0.5,
        low: (close - 0.5).max(0.01),
        close,
        volume: Some(1_000_000),
    }
}

/// Build `n` candles with linearly increasing close prices starting at
/// `start` and incrementing by `step` per bar.
pub fn rising_candles(n: usize, start: f64, step: f64) -> Vec<Candle> {
    (0..n)
        .map(|i| {
            let c = start + i as f64 * step;
            Candle {
                date: format!("2024-01-{:02}", (i % 28) + 1),
                open: c,
                high: c + step * 0.5,
                low: (c - step * 0.5).max(0.01),
                close: c,
                volume: Some(1_000_000 + i as u64 * 10_000),
            }
        })
        .collect()
}

/// Build `n` candles with linearly decreasing close prices.
pub fn falling_candles(n: usize, start: f64, step: f64) -> Vec<Candle> {
    (0..n)
        .map(|i| {
            let c = (start - i as f64 * step).max(0.01);
            Candle {
                date: format!("2024-01-{:02}", (i % 28) + 1),
                open: c,
                high: c + step * 0.3,
                low: (c - step * 0.3).max(0.01),
                close: c,
                volume: Some(1_000_000),
            }
        })
        .collect()
}

/// Build `n` candles with alternating prices to produce mid-range RSI.
pub fn alternating_candles(n: usize) -> Vec<Candle> {
    (0..n)
        .map(|i| {
            let c = if i % 2 == 0 { 100.0 } else { 98.0 };
            candle(c)
        })
        .collect()
}
