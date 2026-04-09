//! Derived price queries built on top of [`YFinanceClient`].
//!
//! While [`super::ohlcv`] is the raw data layer (fetching and caching OHLCV
//! bars), this module provides higher-level, domain-specific price functions:
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_latest_close`] | Most recent closing price for any symbol |
//! | [`fetch_vix_data`]   | CBOE VIX snapshot with SMA-based trend and regime |
//!
//! Both functions accept a `&YFinanceClient` reference so they benefit from the
//! client's session-level in-memory cache without carrying their own state.

use chrono::NaiveDate;
use tracing::warn;

use crate::state::{MarketVolatilityData, VixRegime, VixTrend};

use super::ohlcv::{YFinanceClient, parse_date};

// ─── Latest close ─────────────────────────────────────────────────────────────

/// Fetch the most recent closing price for `symbol` by looking back up to
/// 7 calendar days from `as_of_date` (YYYY-MM-DD).
///
/// Returns `None` if no candles are available in that window (e.g. on
/// weekends/holidays with no recent trading).
pub async fn get_latest_close(
    client: &YFinanceClient,
    symbol: &str,
    as_of_date: &str,
) -> Option<f64> {
    let end_date = parse_date(as_of_date).ok()?;
    let start_date = end_date - chrono::Duration::days(7);
    let candles = client
        .get_ohlcv(symbol, &start_date.to_string(), &end_date.to_string())
        .await
        .ok()?;
    candles.last().map(|c| c.close)
}

// ─── VIX market volatility fetcher ───────────────────────────────────────────

/// VIX ticker symbol on Yahoo Finance.
const VIX_SYMBOL: &str = "^VIX";

/// Number of calendar days to look back when fetching VIX candles.
/// 60 calendar days ≈ 42 trading days, well above the 20-day SMA minimum.
const VIX_LOOKBACK_DAYS: i64 = 60;

/// Minimum number of candles required to compute all VIX metrics.
const VIX_MIN_CANDLES: usize = 20;

/// Fetch VIX market volatility data as of `as_of_date` (YYYY-MM-DD).
///
/// Returns `None` on any error (network failure, insufficient history) so that
/// callers can degrade gracefully without blocking the pipeline.
pub async fn fetch_vix_data(
    client: &YFinanceClient,
    as_of_date: &str,
) -> Option<MarketVolatilityData> {
    let end_date = as_of_date.parse::<NaiveDate>().ok().or_else(|| {
        warn!(as_of_date, "VIX fetch: failed to parse as_of_date");
        None
    })?;

    let start_date = end_date - chrono::Duration::days(VIX_LOOKBACK_DAYS);
    let start_str = start_date.to_string();

    let candles = match client.get_ohlcv(VIX_SYMBOL, &start_str, as_of_date).await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "VIX fetch: Yahoo Finance request failed");
            return None;
        }
    };

    if candles.len() < VIX_MIN_CANDLES {
        warn!(
            got = candles.len(),
            need = VIX_MIN_CANDLES,
            "VIX fetch: insufficient candle history"
        );
        return None;
    }

    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();

    let vix_level = *closes.last().expect("checked len >= VIX_MIN_CANDLES");

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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{fetch_vix_data, get_latest_close};
    use crate::data::YFinanceClient;
    use crate::state::{VixRegime, VixTrend};

    fn candle(date: &str, close: f64) -> crate::data::Candle {
        crate::data::Candle {
            date: date.to_owned(),
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: Some(1_000),
        }
    }

    // ── VIX metrics ───────────────────────────────────────────────────────

    /// Build synthetic closes: `base` repeated `count` times then `tail` repeated 5 times.
    fn build_closes(base: f64, count: usize, tail: f64) -> Vec<f64> {
        let mut v: Vec<f64> = vec![base; count];
        v.extend(vec![tail; 5]);
        v
    }

    fn compute_vix_metrics(closes: &[f64]) -> (f64, f64, VixTrend, VixRegime) {
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
    fn vix_regime_low() {
        let closes: Vec<f64> = vec![12.0; 25];
        let (level, _, _, regime) = compute_vix_metrics(&closes);
        assert!(level < 15.0);
        assert_eq!(regime, VixRegime::Low);
    }

    #[test]
    fn vix_regime_normal() {
        let closes: Vec<f64> = vec![18.0; 25];
        let (_, _, _, regime) = compute_vix_metrics(&closes);
        assert_eq!(regime, VixRegime::Normal);
    }

    #[test]
    fn vix_regime_elevated() {
        let closes: Vec<f64> = vec![25.0; 25];
        let (_, _, _, regime) = compute_vix_metrics(&closes);
        assert_eq!(regime, VixRegime::Elevated);
    }

    #[test]
    fn vix_regime_high() {
        let closes: Vec<f64> = vec![35.0; 25];
        let (_, _, _, regime) = compute_vix_metrics(&closes);
        assert_eq!(regime, VixRegime::High);
    }

    #[test]
    fn vix_trend_rising() {
        // base 15 for 20 days, then spike to 25 for 5 days → SMA5 > SMA20 * 1.05
        let closes = build_closes(15.0, 20, 25.0);
        let (_, _, trend, _) = compute_vix_metrics(&closes);
        assert_eq!(trend, VixTrend::Rising);
    }

    #[test]
    fn vix_trend_falling() {
        // base 25 for 20 days, then drop to 14 for 5 days → SMA5 < SMA20 * 0.95
        let closes = build_closes(25.0, 20, 14.0);
        let (_, _, trend, _) = compute_vix_metrics(&closes);
        assert_eq!(trend, VixTrend::Falling);
    }

    #[test]
    fn vix_trend_stable() {
        // flat closes → SMA5 ≈ SMA20
        let closes: Vec<f64> = vec![20.0; 25];
        let (_, _, trend, _) = compute_vix_metrics(&closes);
        assert_eq!(trend, VixTrend::Stable);
    }

    #[test]
    fn vix_regime_boundary_exactly_15() {
        let closes: Vec<f64> = vec![15.0; 25];
        let (_, _, _, regime) = compute_vix_metrics(&closes);
        assert_eq!(regime, VixRegime::Normal);
    }

    #[test]
    fn vix_regime_boundary_exactly_20() {
        let closes: Vec<f64> = vec![20.0; 25];
        let (_, _, _, regime) = compute_vix_metrics(&closes);
        assert_eq!(regime, VixRegime::Elevated);
    }

    #[test]
    fn vix_regime_boundary_exactly_30() {
        let closes: Vec<f64> = vec![30.0; 25];
        let (_, _, _, regime) = compute_vix_metrics(&closes);
        assert_eq!(regime, VixRegime::High);
    }

    #[tokio::test]
    async fn get_latest_close_returns_last_cached_close() {
        let client = YFinanceClient::default();
        client
            .cache_seed(
                "AAPL",
                "2024-01-08",
                "2024-01-15",
                vec![candle("2024-01-12", 101.0), candle("2024-01-15", 103.5)],
            )
            .await;

        let close = get_latest_close(&client, "AAPL", "2024-01-15").await;
        assert_eq!(close, Some(103.5));
    }

    #[tokio::test]
    async fn get_latest_close_returns_none_for_invalid_date() {
        let client = YFinanceClient::default();
        let close = get_latest_close(&client, "AAPL", "not-a-date").await;
        assert_eq!(close, None);
    }

    #[tokio::test]
    async fn get_latest_close_returns_none_when_no_cached_history_exists() {
        let client = YFinanceClient::default();
        client
            .cache_seed("AAPL", "2024-01-08", "2024-01-15", vec![])
            .await;
        let close = get_latest_close(&client, "AAPL", "2024-01-15").await;
        assert_eq!(close, None);
    }

    #[tokio::test]
    async fn get_latest_close_uses_prior_trading_day_within_lookback_window() {
        let client = YFinanceClient::default();
        client
            .cache_seed(
                "AAPL",
                "2024-01-09",
                "2024-01-16",
                vec![candle("2024-01-12", 101.0)],
            )
            .await;

        let close = get_latest_close(&client, "AAPL", "2024-01-16").await;
        assert_eq!(close, Some(101.0));
    }

    #[tokio::test]
    async fn fetch_vix_data_returns_none_for_invalid_date() {
        let client = YFinanceClient::default();
        let data = fetch_vix_data(&client, "not-a-date").await;
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn fetch_vix_data_returns_none_for_insufficient_history() {
        let client = YFinanceClient::default();
        client
            .cache_seed(
                "^VIX",
                "2024-01-16",
                "2024-03-16",
                vec![candle("2024-03-15", 18.0); 10],
            )
            .await;

        let data = fetch_vix_data(&client, "2024-03-16").await;
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn fetch_vix_data_returns_expected_market_volatility_snapshot() {
        let client = YFinanceClient::default();
        let closes: Vec<f64> = vec![15.0; 20].into_iter().chain(vec![25.0; 5]).collect();
        let candles: Vec<crate::data::Candle> = closes
            .into_iter()
            .enumerate()
            .map(|(index, close)| candle(&format!("2024-03-{:02}", index + 1), close))
            .collect();
        client
            .cache_seed("^VIX", "2024-01-16", "2024-03-16", candles)
            .await;

        let data = fetch_vix_data(&client, "2024-03-16")
            .await
            .expect("expected a VIX snapshot");

        assert_eq!(data.vix_level, 25.0);
        assert!((data.vix_sma_20 - 17.5).abs() < f64::EPSILON);
        assert_eq!(data.vix_trend, VixTrend::Rising);
        assert_eq!(data.vix_regime, VixRegime::Elevated);
        assert_eq!(data.fetched_at, "2024-03-25");
    }
}
