//! VIX market volatility data fetcher.
//!
//! Fetches 60 days of `^VIX` daily candles from Yahoo Finance and computes a
//! small set of deterministic metrics — no LLM involvement.  The result is
//! stored on [`TradingState::market_volatility`] before the analyst fan-out so
//! every downstream agent sees the same volatility context.

use chrono::NaiveDate;
use tracing::warn;

use crate::state::{MarketVolatilityData, VixRegime, VixTrend};

use super::yfinance::YFinanceClient;

/// VIX ticker symbol on Yahoo Finance.
const VIX_SYMBOL: &str = "^VIX";

/// Number of calendar days to look back when fetching VIX candles.
/// 60 calendar days ≈ 42 trading days, well above the 20-day SMA minimum.
const LOOKBACK_DAYS: i64 = 60;

/// Minimum number of candles required to compute all metrics.
const MIN_CANDLES: usize = 20;

/// Fetch VIX market volatility data as of `as_of_date` (YYYY-MM-DD).
///
/// Returns `None` on any error (network failure, insufficient history) so that
/// callers can degrade gracefully without blocking the pipeline.
pub async fn fetch_vix_data(
    yfinance: &YFinanceClient,
    as_of_date: &str,
) -> Option<MarketVolatilityData> {
    let end_date = as_of_date.parse::<NaiveDate>().ok().or_else(|| {
        warn!(as_of_date, "VIX fetch: failed to parse as_of_date");
        None
    })?;

    let start_date = end_date - chrono::Duration::days(LOOKBACK_DAYS);
    let start_str = start_date.to_string();

    let candles = match yfinance.get_ohlcv(VIX_SYMBOL, &start_str, as_of_date).await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "VIX fetch: Yahoo Finance request failed");
            return None;
        }
    };

    if candles.len() < MIN_CANDLES {
        warn!(
            got = candles.len(),
            need = MIN_CANDLES,
            "VIX fetch: insufficient candle history"
        );
        return None;
    }

    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();

    let vix_level = *closes.last().expect("checked len >= MIN_CANDLES");

    // 20-day SMA: average of the last 20 closing prices.
    let sma_20_slice = &closes[closes.len() - 20..];
    let vix_sma_20 = sma_20_slice.iter().sum::<f64>() / 20.0;

    // 5-day SMA for short-term trend direction.
    let sma_5_slice = &closes[closes.len() - 5..];
    let vix_sma_5 = sma_5_slice.iter().sum::<f64>() / 5.0;

    // Trend: compare SMA-5 to SMA-20 with a 5% band to avoid noise.
    let vix_trend = if vix_sma_5 > vix_sma_20 * 1.05 {
        VixTrend::Rising
    } else if vix_sma_5 < vix_sma_20 * 0.95 {
        VixTrend::Falling
    } else {
        VixTrend::Stable
    };

    // Regime classification based on absolute VIX level.
    let vix_regime = if vix_level < 15.0 {
        VixRegime::Low
    } else if vix_level < 20.0 {
        VixRegime::Normal
    } else if vix_level < 30.0 {
        VixRegime::Elevated
    } else {
        VixRegime::High
    };

    let fetched_at = candles
        .last()
        .map(|c| c.date.clone())
        .unwrap_or_else(|| as_of_date.to_owned());

    Some(MarketVolatilityData {
        vix_level,
        vix_sma_20,
        vix_trend,
        vix_regime,
        fetched_at,
    })
}

#[cfg(test)]
mod tests {
    use crate::state::{VixRegime, VixTrend};

    /// Build synthetic closes: `base` repeated `count` times then `tail` repeated 5 times.
    fn build_closes(base: f64, count: usize, tail: f64) -> Vec<f64> {
        let mut v: Vec<f64> = vec![base; count];
        v.extend(vec![tail; 5]);
        v
    }

    fn compute_metrics(closes: &[f64]) -> (f64, f64, VixTrend, VixRegime) {
        let vix_level = *closes.last().unwrap();
        let sma_20: f64 = closes[closes.len() - 20..].iter().sum::<f64>() / 20.0;
        let sma_5: f64 = closes[closes.len() - 5..].iter().sum::<f64>() / 5.0;
        let trend = if sma_5 > sma_20 * 1.05 {
            VixTrend::Rising
        } else if sma_5 < sma_20 * 0.95 {
            VixTrend::Falling
        } else {
            VixTrend::Stable
        };
        let regime = if vix_level < 15.0 {
            VixRegime::Low
        } else if vix_level < 20.0 {
            VixRegime::Normal
        } else if vix_level < 30.0 {
            VixRegime::Elevated
        } else {
            VixRegime::High
        };
        (vix_level, sma_20, trend, regime)
    }

    #[test]
    fn regime_low() {
        let closes: Vec<f64> = vec![12.0; 25];
        let (level, _, _, regime) = compute_metrics(&closes);
        assert!(level < 15.0);
        assert_eq!(regime, VixRegime::Low);
    }

    #[test]
    fn regime_normal() {
        let closes: Vec<f64> = vec![18.0; 25];
        let (_, _, _, regime) = compute_metrics(&closes);
        assert_eq!(regime, VixRegime::Normal);
    }

    #[test]
    fn regime_elevated() {
        let closes: Vec<f64> = vec![25.0; 25];
        let (_, _, _, regime) = compute_metrics(&closes);
        assert_eq!(regime, VixRegime::Elevated);
    }

    #[test]
    fn regime_high() {
        let closes: Vec<f64> = vec![35.0; 25];
        let (_, _, _, regime) = compute_metrics(&closes);
        assert_eq!(regime, VixRegime::High);
    }

    #[test]
    fn trend_rising() {
        // base 15 for 20 days, then spike to 25 for 5 days → SMA5 > SMA20 * 1.05
        let closes = build_closes(15.0, 20, 25.0);
        let (_, _, trend, _) = compute_metrics(&closes);
        assert_eq!(trend, VixTrend::Rising);
    }

    #[test]
    fn trend_falling() {
        // base 25 for 20 days, then drop to 14 for 5 days → SMA5 < SMA20 * 0.95
        let closes = build_closes(25.0, 20, 14.0);
        let (_, _, trend, _) = compute_metrics(&closes);
        assert_eq!(trend, VixTrend::Falling);
    }

    #[test]
    fn trend_stable() {
        // flat closes → SMA5 ≈ SMA20
        let closes: Vec<f64> = vec![20.0; 25];
        let (_, _, trend, _) = compute_metrics(&closes);
        assert_eq!(trend, VixTrend::Stable);
    }

    #[test]
    fn regime_boundary_exactly_15() {
        let closes: Vec<f64> = vec![15.0; 25];
        let (_, _, _, regime) = compute_metrics(&closes);
        assert_eq!(regime, VixRegime::Normal);
    }

    #[test]
    fn regime_boundary_exactly_20() {
        let closes: Vec<f64> = vec![20.0; 25];
        let (_, _, _, regime) = compute_metrics(&closes);
        assert_eq!(regime, VixRegime::Elevated);
    }

    #[test]
    fn regime_boundary_exactly_30() {
        let closes: Vec<f64> = vec![30.0; 25];
        let (_, _, _, regime) = compute_metrics(&closes);
        assert_eq!(regime, VixRegime::High);
    }
}
