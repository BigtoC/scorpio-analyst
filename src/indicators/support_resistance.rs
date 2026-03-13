//! Support and resistance level derivation from weekly-aggregated OHLCV data.
//!
//! Uses 5-bar swing pivots on weekly candles, 52-week high/low anchors, and
//! a clustering algorithm to produce deterministic support/resistance levels.

use kand::ohlcv::atr;

use crate::data::yfinance::Candle;

use super::utils::nan_to_opt;

// ─── Internal weekly candle ────────────────────────────────────────────────────

/// Aggregated weekly OHLCV bar.
struct WeeklyCandle {
    high: f64,
    low: f64,
    close: f64,
    /// Retained for future volume-weighted scoring; not yet used in MVP pivot selection.
    _volume: f64,
}

/// Aggregate daily `candles` into weekly bars by grouping consecutive sets of
/// five trading days.
fn aggregate_weekly(candles: &[Candle]) -> Vec<WeeklyCandle> {
    candles
        .chunks(5)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| WeeklyCandle {
            high: chunk
                .iter()
                .map(|c| c.high)
                .fold(f64::NEG_INFINITY, f64::max),
            low: chunk.iter().map(|c| c.low).fold(f64::INFINITY, f64::min),
            // SAFETY: chunk is non-empty – guaranteed by the filter above.
            close: chunk.last().unwrap().close,
            _volume: chunk.iter().map(|c| c.volume.unwrap_or(0) as f64).sum(),
        })
        .collect()
}

/// Compute the most-recent 14-period weekly ATR from a weekly candle slice.
fn weekly_atr_14(weekly: &[WeeklyCandle]) -> Option<f64> {
    if weekly.len() < 15 {
        return None;
    }
    let h: Vec<f64> = weekly.iter().map(|w| w.high).collect();
    let l: Vec<f64> = weekly.iter().map(|w| w.low).collect();
    let c: Vec<f64> = weekly.iter().map(|w| w.close).collect();
    let n = weekly.len();
    let mut out_atr = vec![0.0_f64; n];
    atr::atr(&h, &l, &c, 14, &mut out_atr).ok()?;
    out_atr.iter().rev().find_map(|&v| nan_to_opt(v))
}

/// 5-bar swing pivot highs: index `i` where `high[i]` strictly exceeds the
/// highs of the five bars before and after it.
fn swing_pivot_highs(weekly: &[WeeklyCandle]) -> Vec<f64> {
    let n = weekly.len();
    let w = 5_usize;
    let mut pivots = Vec::new();
    for i in w..n.saturating_sub(w) {
        let h = weekly[i].high;
        let left_ok = (i.saturating_sub(w)..i).all(|j| weekly[j].high < h);
        let right_ok = ((i + 1)..=(i + w).min(n - 1)).all(|j| weekly[j].high < h);
        if left_ok && right_ok {
            pivots.push(h);
        }
    }
    pivots
}

/// 5-bar swing pivot lows: index `i` where `low[i]` is strictly below the
/// lows of the five bars before and after it.
fn swing_pivot_lows(weekly: &[WeeklyCandle]) -> Vec<f64> {
    let n = weekly.len();
    let w = 5_usize;
    let mut pivots = Vec::new();
    for i in w..n.saturating_sub(w) {
        let l = weekly[i].low;
        let left_ok = (i.saturating_sub(w)..i).all(|j| weekly[j].low > l);
        let right_ok = ((i + 1)..=(i + w).min(n - 1)).all(|j| weekly[j].low > l);
        if left_ok && right_ok {
            pivots.push(l);
        }
    }
    pivots
}

/// Cluster a sorted list of price levels that are within `tolerance` of each
/// other, returning the centroid of each cluster.
fn cluster_levels(mut levels: Vec<f64>, tolerance: f64) -> Vec<f64> {
    if levels.is_empty() {
        return Vec::new();
    }
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut clusters: Vec<Vec<f64>> = Vec::new();
    let mut current = vec![levels[0]];
    for &level in &levels[1..] {
        let centroid = current.iter().sum::<f64>() / current.len() as f64;
        if (level - centroid).abs() <= tolerance {
            current.push(level);
        } else {
            clusters.push(current.clone());
            current = vec![level];
        }
    }
    clusters.push(current);
    clusters
        .into_iter()
        .map(|c| c.iter().sum::<f64>() / c.len() as f64)
        .collect()
}

/// Derive deterministic support and resistance boundaries from OHLCV data.
///
/// Uses weekly-aggregated candles, 5-bar swing pivots, 52-week high/low
/// anchors, and a clustering tolerance of `max(2 % of current close, 1×
/// weekly ATR(14))`. Returns `(support, resistance)` where support is the
/// highest clustered level below the current close price and resistance is the
/// lowest clustered level above it.
///
/// Returns `(None, None)` when there are insufficient candles to form weekly
/// bars with valid pivots.
pub fn derive_support_resistance(candles: &[Candle]) -> (Option<f64>, Option<f64>) {
    if candles.is_empty() {
        return (None, None);
    }
    let current_close = candles.last().unwrap().close;
    // Use up to 104 weeks (≈520 trading days) of history.
    let trailing = candles.len().min(520);
    let recent = &candles[candles.len() - trailing..];
    let weekly = aggregate_weekly(recent);

    // Need at least 11 weekly bars to find any 5-bar pivot.
    if weekly.len() < 11 {
        return (None, None);
    }

    // Cluster tolerance: max(2% of current close, 1× weekly ATR(14)).
    let tolerance = {
        let pct = current_close * 0.02;
        match weekly_atr_14(&weekly) {
            Some(w_atr) => pct.max(w_atr),
            None => pct,
        }
    };

    // 52-week high / low anchor levels.
    let lookback_52 = weekly.len().min(52);
    let recent_w = &weekly[weekly.len() - lookback_52..];
    let high_52w = recent_w
        .iter()
        .map(|w| w.high)
        .fold(f64::NEG_INFINITY, f64::max);
    let low_52w = recent_w.iter().map(|w| w.low).fold(f64::INFINITY, f64::min);

    // Pivot levels.
    let mut resistance_cands = swing_pivot_highs(&weekly);
    resistance_cands.push(high_52w);

    let mut support_cands = swing_pivot_lows(&weekly);
    support_cands.push(low_52w);

    // Cluster and select.
    let r_clusters = cluster_levels(resistance_cands, tolerance);
    let s_clusters = cluster_levels(support_cands, tolerance);

    let support = s_clusters
        .iter()
        .filter(|&&l| l < current_close)
        .copied()
        .reduce(f64::max); // highest support below price

    let resistance = r_clusters
        .iter()
        .filter(|&&l| l > current_close)
        .copied()
        .reduce(f64::min); // lowest resistance above price

    (support, resistance)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::test_utils::*;

    #[test]
    fn support_resistance_empty_returns_none() {
        let (s, r) = derive_support_resistance(&[]);
        assert!(s.is_none() && r.is_none());
    }

    #[test]
    fn support_resistance_insufficient_weekly_bars_returns_none() {
        // 10 daily candles → 2 weekly bars → below the 11-bar threshold.
        let candles = rising_candles(10, 100.0, 1.0);
        let (s, r) = derive_support_resistance(&candles);
        assert!(s.is_none());
        assert!(r.is_none());
    }

    #[test]
    fn support_below_and_resistance_above_current_price() {
        let mut candles = Vec::new();
        for i in 0..60 {
            candles.push(candle(100.0 + i as f64 * 1.5));
        }
        for i in 0..30 {
            candles.push(candle(190.0 - i as f64 * 2.0));
        }
        for i in 0..60 {
            candles.push(candle(130.0 + i as f64 * 1.5));
        }
        let current = candles.last().unwrap().close;
        let (support, resistance) = derive_support_resistance(&candles);

        if let Some(s) = support {
            assert!(s < current, "Support {s} should be below current {current}");
        }
        if let Some(r) = resistance {
            assert!(
                r > current,
                "Resistance {r} should be above current {current}"
            );
        }
    }
}
