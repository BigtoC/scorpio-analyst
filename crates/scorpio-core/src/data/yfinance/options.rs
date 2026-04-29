//! Yahoo Finance options snapshot provider.
//!
//! Provides a [`YFinanceOptionsProvider`] that fetches a live options-chain
//! snapshot for equity symbols and normalizes it into an [`OptionsOutcome`].
//!
//! The provider is intentionally **today-only**: if `target_date` does not
//! match the current US/Eastern calendar date the method returns
//! `OptionsOutcome::HistoricalRun` without making any network calls, since
//! Yahoo Finance only publishes current live options data.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::TimeZone as _;
use chrono_tz::US::Eastern;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use yfinance_rs::CacheMode;
use yfinance_rs::YfError;
use yfinance_rs::core::conversions::money_to_f64;
use yfinance_rs::ticker::{OptionChain, Ticker};

use super::ohlcv::YFinanceClient;
use crate::data::provider_impls::require_equity_ticker;
use crate::data::traits::options::{
    IvTermPoint, NearTermStrike, OptionsOutcome, OptionsProvider, OptionsSnapshot,
};
use crate::domain::Symbol;
use crate::error::TradingError;

// ─── Constants ───────────────────────────────────────────────────────────────

const OPTIONS_NTM_STRIKE_BAND_PCT: f64 = 0.05;
const OPTIONS_NTM_MIN_STRIKES_PER_SIDE: usize = 2;
const OPTIONS_NTM_MAX_BAND_EXPANSION_PCT: f64 = 0.20;
const OPTIONS_FETCH_TIMEOUT_SECS: u64 = 30;

// ─── Provider ────────────────────────────────────────────────────────────────

/// Fetches a live options-chain snapshot from Yahoo Finance for the current
/// US/Eastern trading date.
#[derive(Clone)]
pub struct YFinanceOptionsProvider {
    client: YFinanceClient,
}

impl std::fmt::Debug for YFinanceOptionsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YFinanceOptionsProvider")
            .field("client", &self.client)
            .finish()
    }
}

impl YFinanceOptionsProvider {
    /// Create a new provider backed by an existing [`YFinanceClient`].
    pub fn new(client: YFinanceClient) -> Self {
        Self { client }
    }

    /// Core implementation — separated so tests can call it directly.
    pub async fn fetch_snapshot_impl(
        &self,
        symbol: &Symbol,
        target_date: &str,
    ) -> Result<OptionsOutcome, TradingError> {
        let ticker = require_equity_ticker(symbol)?;

        // ── Stub guard (test + test-helpers) ────────────────────────────
        #[cfg(any(test, feature = "test-helpers"))]
        if let Some(ref stub) = self.client.stubbed_financials {
            return fetch_from_stub(stub, &ticker, target_date).await;
        }

        // ── Check market-local date ──────────────────────────────────────
        let market_today = market_local_today_eastern();
        if target_date != market_today {
            return Ok(OptionsOutcome::HistoricalRun);
        }

        // ── Spot price ───────────────────────────────────────────────────
        let spot = match super::price::get_latest_close(&self.client, &ticker, target_date).await {
            Some(p) => p,
            None => return Ok(OptionsOutcome::MissingSpot),
        };

        // ── Expiration dates ─────────────────────────────────────────────
        let yf_ticker =
            Ticker::new(self.client.session.client(), &ticker).cache_mode(CacheMode::Use);

        let mut expirations = tokio::time::timeout(
            std::time::Duration::from_secs(OPTIONS_FETCH_TIMEOUT_SECS),
            yf_ticker.options(),
        )
        .await
        .map_err(|_| TradingError::NetworkTimeout {
            elapsed: std::time::Duration::from_secs(OPTIONS_FETCH_TIMEOUT_SECS),
            message: "options expiration fetch timed out".to_owned(),
        })?
        .map_err(map_yf_options_err)?;

        if expirations.is_empty() {
            return Ok(OptionsOutcome::NoListedInstrument);
        }

        expirations.sort_unstable();
        let front_month_ts = expirations[0];

        // ── Front-month chain ────────────────────────────────────────────
        let front_chain = tokio::time::timeout(
            std::time::Duration::from_secs(OPTIONS_FETCH_TIMEOUT_SECS),
            yf_ticker.option_chain(Some(front_month_ts)),
        )
        .await
        .map_err(|_| TradingError::NetworkTimeout {
            elapsed: std::time::Duration::from_secs(OPTIONS_FETCH_TIMEOUT_SECS),
            message: "options chain fetch timed out".to_owned(),
        })?
        .map_err(map_yf_options_err)?;

        // ── NTM slice ────────────────────────────────────────────────────
        let Some(near_term_strikes) = build_ntm_slice(&front_chain, spot) else {
            return Ok(OptionsOutcome::SparseChain);
        };

        // ── ATM IV from front-month ──────────────────────────────────────
        let atm_iv = compute_atm_iv(&front_chain, spot);

        // ── Front-month expiration date string ───────────────────────────
        let near_term_expiration = front_chain
            .calls
            .first()
            .or_else(|| front_chain.puts.first())
            .map(|c| c.expiration_date.to_string())
            .unwrap_or_else(|| timestamp_to_date_str(front_month_ts));

        // ── Max pain from front-month ────────────────────────────────────
        let max_pain_strike = compute_max_pain(&front_chain, spot);

        // ── All-expiration chains for ratios + term structure ────────────
        let mut all_chains: Vec<(i64, OptionChain)> = Vec::with_capacity(expirations.len());
        // Reuse front-month chain
        all_chains.push((front_month_ts, front_chain));

        for &exp_ts in &expirations[1..] {
            match tokio::time::timeout(
                std::time::Duration::from_secs(OPTIONS_FETCH_TIMEOUT_SECS),
                yf_ticker.option_chain(Some(exp_ts)),
            )
            .await
            {
                Ok(Ok(chain)) => all_chains.push((exp_ts, chain)),
                Ok(Err(_)) | Err(_) => {
                    // Skip individual expiration fetch failures — don't propagate
                }
            }
        }

        // ── Put/call ratios over all chains ──────────────────────────────
        let (put_call_volume_ratio, put_call_oi_ratio) = compute_pc_ratios(&all_chains);

        // ── IV term structure ────────────────────────────────────────────
        let iv_term_structure = build_term_structure(&all_chains, spot);

        Ok(OptionsOutcome::Snapshot(OptionsSnapshot {
            spot_price: spot,
            atm_iv,
            iv_term_structure,
            put_call_volume_ratio,
            put_call_oi_ratio,
            max_pain_strike,
            near_term_expiration,
            near_term_strikes,
        }))
    }
}

#[async_trait]
impl OptionsProvider for YFinanceOptionsProvider {
    fn provider_name(&self) -> &'static str {
        "yfinance"
    }

    async fn fetch_snapshot(
        &self,
        symbol: &Symbol,
        target_date: &str,
    ) -> Result<OptionsOutcome, TradingError> {
        self.fetch_snapshot_impl(symbol, target_date).await
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Return current US/Eastern calendar date as `"YYYY-MM-DD"`.
fn market_local_today_eastern() -> String {
    let eastern_now = Eastern.from_utc_datetime(&chrono::Utc::now().naive_utc());
    eastern_now.date_naive().to_string()
}

/// Map a `YfError` to the nearest `TradingError` variant.
fn map_yf_options_err(e: YfError) -> TradingError {
    TradingError::NetworkTimeout {
        elapsed: std::time::Duration::ZERO,
        message: e.to_string(),
    }
}

/// Convert a Unix-seconds timestamp to an ISO-8601 date string.
fn timestamp_to_date_str(ts: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Utc
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.date_naive().to_string())
        .unwrap_or_default()
}

/// Compute the ATM implied volatility from an option chain given the spot price.
///
/// Finds the call and put whose strike is closest to `spot` and averages their
/// `implied_volatility`. Returns `0.0` if no IV data is available.
fn compute_atm_iv(chain: &OptionChain, spot: f64) -> f64 {
    let closest_call = chain
        .calls
        .iter()
        .filter(|c| c.implied_volatility.is_some())
        .min_by(|a, b| {
            let da = (money_to_f64(&a.strike) - spot).abs();
            let db = (money_to_f64(&b.strike) - spot).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

    let closest_put = chain
        .puts
        .iter()
        .filter(|c| c.implied_volatility.is_some())
        .min_by(|a, b| {
            let da = (money_to_f64(&a.strike) - spot).abs();
            let db = (money_to_f64(&b.strike) - spot).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

    match (
        closest_call.and_then(|c| c.implied_volatility),
        closest_put.and_then(|c| c.implied_volatility),
    ) {
        (Some(c_iv), Some(p_iv)) => (c_iv + p_iv) / 2.0,
        (Some(iv), None) | (None, Some(iv)) => iv,
        (None, None) => 0.0,
    }
}

/// Compute max-pain strike from a single expiration's option chain.
///
/// For each candidate strike `S`, compute the total open-interest pain:
/// - Calls lose when `S < strike`: pain = `(strike - S) * call_OI`
/// - Puts lose when `S > strike`: pain = `(S - strike) * put_OI`
///
/// Returns the strike that minimizes total pain. Falls back to the ATM strike
/// if no OI data is available.
fn compute_max_pain(chain: &OptionChain, spot: f64) -> f64 {
    // Collect all unique strikes with their OI.
    use std::collections::BTreeMap;
    let mut call_oi: BTreeMap<u64, u64> = BTreeMap::new(); // strike_bits -> OI
    let mut put_oi: BTreeMap<u64, u64> = BTreeMap::new();

    for c in &chain.calls {
        let k = money_to_f64(&c.strike);
        let oi = c.open_interest.unwrap_or(0);
        *call_oi.entry(k.to_bits()).or_insert(0) += oi;
    }
    for p in &chain.puts {
        let k = money_to_f64(&p.strike);
        let oi = p.open_interest.unwrap_or(0);
        *put_oi.entry(k.to_bits()).or_insert(0) += oi;
    }

    // Collect all unique strike values.
    let mut all_strikes_bits: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    all_strikes_bits.extend(call_oi.keys());
    all_strikes_bits.extend(put_oi.keys());

    if all_strikes_bits.is_empty() {
        return spot;
    }

    let all_strikes: Vec<f64> = all_strikes_bits
        .iter()
        .map(|&bits| f64::from_bits(bits))
        .collect();

    let total_oi: u64 = call_oi.values().sum::<u64>() + put_oi.values().sum::<u64>();
    if total_oi == 0 {
        // No OI data — fall back to ATM strike.
        return all_strikes
            .iter()
            .copied()
            .min_by(|a, b| {
                (a - spot)
                    .abs()
                    .partial_cmp(&(b - spot).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(spot);
    }

    let mut best_strike = spot;
    let mut best_pain = f64::MAX;

    for &candidate_bits in &all_strikes_bits {
        let candidate = f64::from_bits(candidate_bits);
        let mut pain = 0.0_f64;

        // Calls: holder loses when candidate is BELOW the strike.
        for (&strike_bits, &oi) in &call_oi {
            let strike = f64::from_bits(strike_bits);
            pain += (strike - candidate).max(0.0) * oi as f64;
        }
        // Puts: holder loses when candidate is ABOVE the strike.
        for (&strike_bits, &oi) in &put_oi {
            let strike = f64::from_bits(strike_bits);
            pain += (candidate - strike).max(0.0) * oi as f64;
        }

        if pain < best_pain {
            best_pain = pain;
            best_strike = candidate;
        }
    }

    best_strike
}

/// strike_bits → (implied_volatility, volume, open_interest)
type StrikeData = std::collections::BTreeMap<u64, (Option<f64>, Option<u64>, Option<u64>)>;

/// Build NTM (near-the-money) strike slice with band expansion logic.
///
/// Returns `None` if the chain is too sparse after the capped expansion.
fn build_ntm_slice(chain: &OptionChain, spot: f64) -> Option<Vec<NearTermStrike>> {
    // Collect all unique strikes across calls and puts.
    // call_data[strike_bits] = (iv, volume, oi)
    let mut call_data: StrikeData = StrikeData::new();
    for c in &chain.calls {
        let k = money_to_f64(&c.strike);
        call_data
            .entry(k.to_bits())
            .or_insert((c.implied_volatility, c.volume, c.open_interest));
    }
    let mut put_data: StrikeData = StrikeData::new();
    for p in &chain.puts {
        let k = money_to_f64(&p.strike);
        put_data
            .entry(k.to_bits())
            .or_insert((p.implied_volatility, p.volume, p.open_interest));
    }

    let all_strikes_bits: std::collections::BTreeSet<u64> =
        call_data.keys().chain(put_data.keys()).copied().collect();
    let all_strikes: Vec<f64> = all_strikes_bits
        .iter()
        .map(|&b| f64::from_bits(b))
        .collect();

    if all_strikes.is_empty() {
        return None;
    }

    // Expand band from initial 5% to up to 20%, ensuring at least
    // OPTIONS_NTM_MIN_STRIKES_PER_SIDE on each side.
    let initial_lo = spot * (1.0 - OPTIONS_NTM_STRIKE_BAND_PCT);
    let initial_hi = spot * (1.0 + OPTIONS_NTM_STRIKE_BAND_PCT);
    let cap_lo = spot * (1.0 - OPTIONS_NTM_MAX_BAND_EXPANSION_PCT);
    let cap_hi = spot * (1.0 + OPTIONS_NTM_MAX_BAND_EXPANSION_PCT);

    // Strikes below spot (ITM calls / OTM puts) sorted descending.
    let mut below: Vec<f64> = all_strikes.iter().copied().filter(|&s| s <= spot).collect();
    below.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    // Strikes above spot (OTM calls / ITM puts) sorted ascending.
    let mut above: Vec<f64> = all_strikes.iter().copied().filter(|&s| s > spot).collect();
    above.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Determine effective low and high bounds.
    // Start with initial band, then expand if needed (capped at ±20%).
    let mut lo = initial_lo;
    let mut hi = initial_hi;

    // Check initial band counts.
    let count_below = |lo_bound: f64| below.iter().filter(|&&s| s >= lo_bound).count();
    let count_above = |hi_bound: f64| above.iter().filter(|&&s| s <= hi_bound).count();

    // Expand low (downward) if needed.
    if count_below(lo) < OPTIONS_NTM_MIN_STRIKES_PER_SIDE {
        // Expand to include more strikes below, up to cap_lo.
        let needed = OPTIONS_NTM_MIN_STRIKES_PER_SIDE;
        if let Some(&nth) = below.iter().filter(|&&s| s >= cap_lo).nth(needed - 1) {
            lo = nth; // Tighten lo to just include the Nth strike.
        } else {
            // Even with the cap, we can't get enough strikes on the low side.
            // Use whatever we can get within cap.
            lo = cap_lo;
        }
    }

    // Expand high (upward) if needed.
    if count_above(hi) < OPTIONS_NTM_MIN_STRIKES_PER_SIDE {
        let needed = OPTIONS_NTM_MIN_STRIKES_PER_SIDE;
        if let Some(&nth) = above.iter().filter(|&&s| s <= cap_hi).nth(needed - 1) {
            hi = nth;
        } else {
            hi = cap_hi;
        }
    }

    // Final count check after potential expansion.
    let final_below = count_below(lo);
    let final_above = count_above(hi);

    if final_below < OPTIONS_NTM_MIN_STRIKES_PER_SIDE
        || final_above < OPTIONS_NTM_MIN_STRIKES_PER_SIDE
    {
        return None;
    }

    // Build the final NTM slice.
    let selected: Vec<f64> = all_strikes
        .iter()
        .copied()
        .filter(|&s| s >= lo && s <= hi)
        .collect();

    if selected.is_empty() {
        return None;
    }

    let result = selected
        .into_iter()
        .map(|strike| {
            let (call_iv, call_vol, call_oi) = call_data
                .get(&strike.to_bits())
                .copied()
                .unwrap_or((None, None, None));
            let (put_iv, put_vol, put_oi) = put_data
                .get(&strike.to_bits())
                .copied()
                .unwrap_or((None, None, None));
            NearTermStrike {
                strike,
                call_iv,
                put_iv,
                call_volume: call_vol,
                put_volume: put_vol,
                call_oi,
                put_oi,
            }
        })
        .collect();

    Some(result)
}

/// Compute put/call volume and OI ratios across all expiration chains.
///
/// Returns `(put_call_volume_ratio, put_call_oi_ratio)`.
fn compute_pc_ratios(chains: &[(i64, OptionChain)]) -> (f64, f64) {
    let mut total_call_vol = 0u64;
    let mut total_put_vol = 0u64;
    let mut total_call_oi = 0u64;
    let mut total_put_oi = 0u64;

    for (_, chain) in chains {
        for c in &chain.calls {
            total_call_vol += c.volume.unwrap_or(0);
            total_call_oi += c.open_interest.unwrap_or(0);
        }
        for p in &chain.puts {
            total_put_vol += p.volume.unwrap_or(0);
            total_put_oi += p.open_interest.unwrap_or(0);
        }
    }

    let vol_ratio = if total_call_vol == 0 {
        0.0
    } else {
        total_put_vol as f64 / total_call_vol as f64
    };

    let oi_ratio = if total_call_oi == 0 {
        0.0
    } else {
        total_put_oi as f64 / total_call_oi as f64
    };

    (vol_ratio, oi_ratio)
}

/// Build the IV term structure from all chains.
fn build_term_structure(chains: &[(i64, OptionChain)], spot: f64) -> Vec<IvTermPoint> {
    chains
        .iter()
        .filter_map(|(ts, chain)| {
            let iv = compute_atm_iv(chain, spot);
            // Only include if we got actual IV data.
            if iv == 0.0
                && chain.calls.iter().all(|c| c.implied_volatility.is_none())
                && chain.puts.iter().all(|p| p.implied_volatility.is_none())
            {
                return None;
            }
            let expiration = chain
                .calls
                .first()
                .or_else(|| chain.puts.first())
                .map(|c| c.expiration_date.to_string())
                .unwrap_or_else(|| timestamp_to_date_str(*ts));
            Some(IvTermPoint {
                expiration,
                atm_iv: iv,
            })
        })
        .collect()
}

// ─── Stub helper (test + test-helpers) ───────────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
async fn fetch_from_stub(
    stub: &super::ohlcv::StubbedFinancialResponses,
    _ticker: &str,
    target_date: &str,
) -> Result<OptionsOutcome, TradingError> {
    // Check market-local date.
    let market_today = market_local_today_eastern();
    if target_date != market_today {
        return Ok(OptionsOutcome::HistoricalRun);
    }

    // Spot price from stub ohlcv.
    let spot = if let Some(ref ohlcv) = stub.ohlcv {
        if let Some(last) = ohlcv.last() {
            last.close
        } else {
            return Ok(OptionsOutcome::MissingSpot);
        }
    } else {
        // No ohlcv stub — try the cache via a real lookup. But in tests, if
        // there's no ohlcv stub and no cached data this will return None.
        // Return MissingSpot to keep tests deterministic.
        return Ok(OptionsOutcome::MissingSpot);
    };

    // Expiration dates.
    if let Some(ref err_msg) = stub.option_expirations_error {
        return Err(TradingError::NetworkTimeout {
            elapsed: std::time::Duration::ZERO,
            message: err_msg.clone(),
        });
    }

    let mut expirations = match &stub.option_expirations {
        Some(v) => v.clone(),
        None => return Ok(OptionsOutcome::NoListedInstrument),
    };

    if expirations.is_empty() {
        return Ok(OptionsOutcome::NoListedInstrument);
    }

    expirations.sort_unstable();
    let front_month_ts = expirations[0];

    // Front-month chain.
    if let Some(err_msg) = stub.option_chain_errors.get(&front_month_ts) {
        return Err(TradingError::NetworkTimeout {
            elapsed: std::time::Duration::ZERO,
            message: err_msg.clone(),
        });
    }

    let front_chain = match stub.option_chains.get(&front_month_ts) {
        Some(c) => c.clone(),
        None => {
            // Synthesize empty chain if no stub provided.
            OptionChain {
                calls: vec![],
                puts: vec![],
            }
        }
    };

    // NTM slice.
    let Some(near_term_strikes) = build_ntm_slice(&front_chain, spot) else {
        return Ok(OptionsOutcome::SparseChain);
    };

    let atm_iv = compute_atm_iv(&front_chain, spot);

    let near_term_expiration = front_chain
        .calls
        .first()
        .or_else(|| front_chain.puts.first())
        .map(|c| c.expiration_date.to_string())
        .unwrap_or_else(|| timestamp_to_date_str(front_month_ts));

    let max_pain_strike = compute_max_pain(&front_chain, spot);

    // Fetch all chains from stub.
    let mut all_chains: Vec<(i64, OptionChain)> = Vec::with_capacity(expirations.len());
    all_chains.push((front_month_ts, front_chain));

    for &exp_ts in &expirations[1..] {
        if stub.option_chain_errors.contains_key(&exp_ts) {
            // Skip errored chains (don't propagate for non-front-month expirations).
            continue;
        }
        let chain = match stub.option_chains.get(&exp_ts) {
            Some(c) => c.clone(),
            None => OptionChain {
                calls: vec![],
                puts: vec![],
            },
        };
        all_chains.push((exp_ts, chain));
    }

    let (put_call_volume_ratio, put_call_oi_ratio) = compute_pc_ratios(&all_chains);
    let iv_term_structure = build_term_structure(&all_chains, spot);

    Ok(OptionsOutcome::Snapshot(OptionsSnapshot {
        spot_price: spot,
        atm_iv,
        iv_term_structure,
        put_call_volume_ratio,
        put_call_oi_ratio,
        max_pain_strike,
        near_term_expiration,
        near_term_strikes,
    }))
}

// ─── OptionsToolContext ───────────────────────────────────────────────────────

/// Write-once analysis-scoped cache for a prefetched [`OptionsOutcome`].
///
/// Mirrors [`OhlcvToolContext`](super::ohlcv::OhlcvToolContext) semantics: a single
/// `Arc<RwLock<Option<Arc<OptionsOutcome>>>>` provides shared ownership across cloned
/// tool instances; the inner `Arc<OptionsOutcome>` avoids re-cloning the heap data
/// on each read.
#[derive(Debug, Clone, Default)]
pub(crate) struct OptionsToolContext {
    outcome: Arc<RwLock<Option<Arc<OptionsOutcome>>>>,
}

impl OptionsToolContext {
    #[must_use]
    #[allow(dead_code)] // used by TechnicalAnalyst::run() in follow-on task
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Store an [`OptionsOutcome`] in the context.
    ///
    /// Write-once: returns [`TradingError::SchemaViolation`] if an outcome has
    /// already been stored, preventing the LLM from overwriting the first fetch
    /// with adversarial data on a second tool call.
    #[allow(dead_code)] // used by TechnicalAnalyst::run() in follow-on task
    pub(crate) async fn store(&self, outcome: OptionsOutcome) -> Result<(), TradingError> {
        let mut lock = self.outcome.write().await;
        if lock.is_some() {
            return Err(TradingError::SchemaViolation {
                message: "options snapshot has already been prefetched for this analysis; \
                          get_options_snapshot may only be stored once per analysis cycle"
                    .to_owned(),
            });
        }
        *lock = Some(Arc::new(outcome));
        Ok(())
    }

    /// Load the prefetched [`OptionsOutcome`].
    ///
    /// Returns a cheap `Arc` clone. Returns [`TradingError::SchemaViolation`] if
    /// the context is empty (nothing has been stored yet).
    pub(crate) async fn load(&self) -> Result<Arc<OptionsOutcome>, TradingError> {
        self.outcome
            .read()
            .await
            .clone()
            .ok_or_else(|| TradingError::SchemaViolation {
                message: "options context is empty; options outcome was not prefetched".to_owned(),
            })
    }
}

// ─── Shared serialization helper ─────────────────────────────────────────────

/// Serialize an [`OptionsOutcome`] into the JSON shape expected by the `get_options_snapshot`
/// tool output contract.
///
/// - `Snapshot` variants are serialized as-is with no injected `reason`.
/// - All other variants have a human-readable `reason` field injected so the LLM
///   understands why live options data is absent.
fn serialize_options_outcome_for_tool(
    outcome: &OptionsOutcome,
) -> Result<serde_json::Value, TradingError> {
    let mut val = serde_json::to_value(outcome).map_err(|e| TradingError::SchemaViolation {
        message: format!("failed to serialize OptionsOutcome: {e}"),
    })?;

    if let serde_json::Value::Object(ref mut map) = val {
        let reason = match outcome {
            OptionsOutcome::NoListedInstrument => {
                Some("this symbol has no listed options on Yahoo")
            }
            OptionsOutcome::SparseChain => {
                Some("options exist but no usable contracts within \u{b1}20% of spot")
            }
            OptionsOutcome::HistoricalRun => Some(
                "target_date is not market-local US/Eastern today; live options intentionally skipped",
            ),
            OptionsOutcome::MissingSpot => {
                Some("no underlying close price available for target_date")
            }
            OptionsOutcome::Snapshot(_) => None,
        };
        if let Some(r) = reason {
            map.insert("reason".to_owned(), serde_json::Value::String(r.to_owned()));
        }
    }

    Ok(val)
}

// ─── rig::tool::Tool wrapper ──────────────────────────────────────────────────

/// Args for the `get_options_snapshot` tool call.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct OptionsSnapshotArgs {
    /// Ticker symbol, e.g. `"AAPL"`.
    pub symbol: String,
    /// ISO-8601 target date, e.g. `"2026-04-27"`.
    pub target_date: String,
}

/// `rig` tool: fetch a live options-chain snapshot for an equity symbol.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetOptionsSnapshot {
    #[serde(skip)]
    provider: Option<YFinanceOptionsProvider>,
    #[serde(skip)]
    allowed_symbol: Option<String>,
    #[serde(skip)]
    target_date: Option<String>,
    /// Prefetched context for replay; takes precedence over the live provider.
    #[serde(skip)]
    context: Option<OptionsToolContext>,
}

impl GetOptionsSnapshot {
    /// Create a fully-scoped tool for a specific symbol and date, backed by a
    /// live provider.
    #[must_use]
    pub fn scoped(
        provider: YFinanceOptionsProvider,
        symbol: impl Into<String>,
        target_date: impl Into<String>,
    ) -> Self {
        Self {
            provider: Some(provider),
            allowed_symbol: Some(symbol.into()),
            target_date: Some(target_date.into()),
            context: None,
        }
    }

    /// Create a fully-scoped tool that replays a prefetched [`OptionsOutcome`]
    /// from `context` without making any network calls.
    ///
    /// The `context` must have been populated via [`OptionsToolContext::store`]
    /// before any tool calls are made.
    #[must_use]
    #[allow(dead_code)] // used by TechnicalAnalyst::run() in follow-on task
    pub(crate) fn scoped_prefetched(
        symbol: impl Into<String>,
        target_date: impl Into<String>,
        context: OptionsToolContext,
    ) -> Self {
        Self {
            provider: None,
            allowed_symbol: Some(symbol.into()),
            target_date: Some(target_date.into()),
            context: Some(context),
        }
    }
}

impl Tool for GetOptionsSnapshot {
    const NAME: &'static str = "get_options_snapshot";

    type Error = TradingError;
    type Args = OptionsSnapshotArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let mut desc = "Fetch a live options-chain snapshot for an equity symbol from Yahoo \
                        Finance. Returns implied volatility, put/call ratios, max-pain strike, \
                        and near-term strike details. Only valid for today's US/Eastern date."
            .to_owned();

        if let Some(sym) = &self.allowed_symbol {
            desc.push_str(&format!(" The symbol MUST be exactly \"{sym}\"."));
        }
        if let Some(td) = &self.target_date {
            desc.push_str(&format!(" The target_date MUST be exactly \"{td}\"."));
        }

        let symbol_schema = match &self.allowed_symbol {
            Some(s) => json!({ "type": "string", "enum": [s] }),
            None => json!({ "type": "string", "description": "The equity ticker symbol" }),
        };
        let date_schema = match &self.target_date {
            Some(d) => json!({ "type": "string", "enum": [d] }),
            None => json!({ "type": "string", "description": "ISO-8601 date (YYYY-MM-DD)" }),
        };

        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: desc,
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": symbol_schema,
                    "target_date": date_schema
                },
                "required": ["symbol", "target_date"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Scope validation.
        if let Some(allowed) = &self.allowed_symbol
            && !args.symbol.eq_ignore_ascii_case(allowed)
        {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "get_options_snapshot is scoped to symbol {allowed}, got {}",
                    args.symbol
                ),
            });
        }
        if let Some(allowed_date) = &self.target_date
            && args.target_date != *allowed_date
        {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "get_options_snapshot is scoped to target_date {allowed_date}, got {}",
                    args.target_date
                ),
            });
        }

        // Precedence:
        // 1. Prefetched context (replay path) — no network call.
        // 2. Live provider — fetch from Yahoo Finance.
        // 3. Neither set — return Config error.
        let outcome = if let Some(ctx) = &self.context {
            // load() returns SchemaViolation if the context is empty.
            let arc = ctx.load().await?;
            (*arc).clone()
        } else if let Some(provider) = &self.provider {
            let symbol =
                Symbol::Equity(crate::domain::Ticker::parse(&args.symbol).map_err(|e| {
                    TradingError::SchemaViolation {
                        message: format!("invalid ticker {}: {e}", args.symbol),
                    }
                })?);
            provider.fetch_snapshot(&symbol, &args.target_date).await?
        } else {
            return Err(TradingError::Config(anyhow::anyhow!(
                "YFinanceOptionsProvider not set on GetOptionsSnapshot tool"
            )));
        };

        serialize_options_outcome_for_tool(&outcome)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;
    use chrono::TimeZone as _;
    use yfinance_rs::ticker::{OptionChain, OptionContract};

    use super::*;
    use crate::data::yfinance::ohlcv::{Candle, StubbedFinancialResponses, YFinanceClient};
    use crate::domain::{Symbol, Ticker};

    // ── Test helpers ──────────────────────────────────────────────────────

    fn today_eastern() -> String {
        let eastern_now = Eastern.from_utc_datetime(&chrono::Utc::now().naive_utc());
        eastern_now.date_naive().to_string()
    }

    fn yesterday_eastern() -> String {
        let eastern_now = Eastern.from_utc_datetime(&chrono::Utc::now().naive_utc());
        (eastern_now.date_naive() - chrono::Duration::days(1)).to_string()
    }

    fn aapl() -> Symbol {
        Symbol::Equity(Ticker::parse("AAPL").unwrap())
    }

    fn make_candle(close: f64) -> Candle {
        Candle {
            date: today_eastern(),
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: Some(1_000_000),
        }
    }

    /// Build a minimal `OptionContract` with just the fields used by the provider.
    fn make_contract(
        strike: f64,
        iv: Option<f64>,
        volume: Option<u64>,
        oi: Option<u64>,
        expiry: &str,
    ) -> OptionContract {
        use paft_money::{Currency, IsoCurrency, Money};
        use rust_decimal::Decimal;

        let d = Decimal::try_from(strike).unwrap();
        let money = Money::new(d, Currency::Iso(IsoCurrency::USD)).unwrap();

        let exp_date = NaiveDate::parse_from_str(expiry, "%Y-%m-%d").unwrap();

        OptionContract {
            contract_symbol: paft_domain::Symbol::new("AAPL240101C00100000").unwrap(),
            strike: money,
            price: None,
            bid: None,
            ask: None,
            volume,
            open_interest: oi,
            implied_volatility: iv,
            in_the_money: false,
            expiration_date: exp_date,
            expiration_at: None,
            last_trade_at: None,
            greeks: None,
        }
    }

    fn expiry_ts() -> i64 {
        // Use a deterministic future expiration: 2030-01-18
        NaiveDate::from_ymd_opt(2030, 1, 18)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn expiry_ts2() -> i64 {
        // Second expiration: 2030-02-15
        NaiveDate::from_ymd_opt(2030, 2, 15)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn provider_with_stub(stub: StubbedFinancialResponses) -> YFinanceOptionsProvider {
        let client = YFinanceClient::with_stubbed_financials(stub);
        YFinanceOptionsProvider::new(client)
    }

    fn sample_snapshot() -> OptionsOutcome {
        use crate::data::traits::options::{IvTermPoint, NearTermStrike, OptionsSnapshot};
        OptionsOutcome::Snapshot(OptionsSnapshot {
            spot_price: 150.0,
            atm_iv: 0.29,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2030-01-18".to_owned(),
                atm_iv: 0.29,
            }],
            put_call_volume_ratio: 0.8,
            put_call_oi_ratio: 1.1,
            max_pain_strike: 150.0,
            near_term_expiration: "2030-01-18".to_owned(),
            near_term_strikes: vec![NearTermStrike {
                strike: 150.0,
                call_iv: Some(0.30),
                put_iv: Some(0.28),
                call_volume: Some(100),
                put_volume: Some(80),
                call_oi: Some(500),
                put_oi: Some(400),
            }],
        })
    }

    // ── OptionsToolContext tests ───────────────────────────────────────────

    #[tokio::test]
    async fn options_tool_context_loads_prefetched_outcome() {
        let ctx = OptionsToolContext::new();
        ctx.store(OptionsOutcome::HistoricalRun)
            .await
            .expect("store once");
        assert_eq!(
            *ctx.load().await.expect("load stored outcome"),
            OptionsOutcome::HistoricalRun
        );
    }

    #[tokio::test]
    async fn options_tool_context_store_write_once_rejects_second_write() {
        let ctx = OptionsToolContext::new();
        ctx.store(OptionsOutcome::HistoricalRun)
            .await
            .expect("first store must succeed");
        let result = ctx.store(OptionsOutcome::MissingSpot).await;
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[tokio::test]
    async fn get_options_snapshot_replays_prefetched_snapshot_without_refetch() {
        let ctx = OptionsToolContext::new();
        ctx.store(sample_snapshot()).await.unwrap();

        let tool = GetOptionsSnapshot::scoped_prefetched("AAPL", today_eastern(), ctx.clone());
        let result = rig::tool::Tool::call(
            &tool,
            OptionsSnapshotArgs {
                symbol: "AAPL".to_owned(),
                target_date: today_eastern(),
            },
        )
        .await
        .expect("prefetched replay should succeed");

        assert_eq!(result["kind"], "snapshot");
    }

    #[tokio::test]
    async fn get_options_snapshot_replays_prefetched_historical_run_with_reason() {
        let ctx = OptionsToolContext::new();
        ctx.store(OptionsOutcome::HistoricalRun).await.unwrap();

        let tool = GetOptionsSnapshot::scoped_prefetched("AAPL", yesterday_eastern(), ctx.clone());
        let result = rig::tool::Tool::call(
            &tool,
            OptionsSnapshotArgs {
                symbol: "AAPL".to_owned(),
                target_date: yesterday_eastern(),
            },
        )
        .await
        .expect("prefetched replay should succeed");

        assert_eq!(result["kind"], "historical_run");
        assert!(result.get("reason").is_some());
    }

    #[tokio::test]
    async fn get_options_snapshot_replay_is_idempotent_across_multiple_calls() {
        // The LLM may invoke get_options_snapshot more than once in a run.
        // load() must be multi-read (not consuming): calling Tool::call() twice must both succeed
        // and return identical output.
        let ctx = OptionsToolContext::new();
        ctx.store(sample_snapshot()).await.unwrap();

        let tool = GetOptionsSnapshot::scoped_prefetched("AAPL", today_eastern(), ctx.clone());
        let result1 = rig::tool::Tool::call(
            &tool,
            OptionsSnapshotArgs {
                symbol: "AAPL".to_owned(),
                target_date: today_eastern(),
            },
        )
        .await
        .expect("first call should succeed");
        let result2 = rig::tool::Tool::call(
            &tool,
            OptionsSnapshotArgs {
                symbol: "AAPL".to_owned(),
                target_date: today_eastern(),
            },
        )
        .await
        .expect("second call should succeed");
        assert_eq!(result1, result2, "replay must be idempotent");
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn returns_snapshot_with_atm_iv_from_front_month_chain() {
        // spot = 150; ATM call IV = 0.30, ATM put IV = 0.28.
        // Include enough strikes (±5% band: 142.5..157.5) so NTM slice doesn't
        // trigger SparseChain: 2 below (145, 147.5) and 2 above (152.5, 155).
        let spot = 150.0;
        let call_iv = 0.30;
        let put_iv = 0.28;
        let expiry = "2030-01-18";
        let ts = expiry_ts();

        let mut chains = BTreeMap::new();
        chains.insert(
            ts,
            OptionChain {
                calls: vec![
                    make_contract(145.0, Some(0.32), Some(60), Some(300), expiry),
                    make_contract(147.5, Some(0.31), Some(80), Some(400), expiry),
                    make_contract(150.0, Some(call_iv), Some(100), Some(500), expiry),
                    make_contract(152.5, Some(0.29), Some(80), Some(400), expiry),
                    make_contract(155.0, Some(0.28), Some(60), Some(300), expiry),
                ],
                puts: vec![
                    make_contract(145.0, Some(0.32), Some(60), Some(300), expiry),
                    make_contract(147.5, Some(0.31), Some(80), Some(400), expiry),
                    make_contract(150.0, Some(put_iv), Some(80), Some(400), expiry),
                    make_contract(152.5, Some(0.27), Some(80), Some(400), expiry),
                    make_contract(155.0, Some(0.26), Some(60), Some(300), expiry),
                ],
            },
        );

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let result = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await;
        let outcome = result.expect("should succeed");

        if let OptionsOutcome::Snapshot(s) = outcome {
            let expected_iv = (call_iv + put_iv) / 2.0;
            assert!(
                (s.atm_iv - expected_iv).abs() < 1e-9,
                "ATM IV should be average of call/put IVs: expected {expected_iv}, got {}",
                s.atm_iv
            );
            assert!((s.spot_price - spot).abs() < 1e-9);
        } else {
            panic!("expected Snapshot, got {outcome:?}");
        }
    }

    #[tokio::test]
    async fn snapshot_includes_put_call_ratios_over_all_strikes() {
        let spot = 100.0;
        let ts1 = expiry_ts();
        let ts2 = expiry_ts2();
        let expiry1 = "2030-01-18";
        let expiry2 = "2030-02-15";

        // Chain 1: 200 call vol, 300 put vol; 1000 call OI, 2000 put OI (at 100).
        // Extra strikes at 95, 105, 110 with zero vol/OI so the ratios are unaffected.
        // NTM band: ≥2 below (95, 100 ≤ spot=100) and ≥2 above (105, 110 > spot=100)
        // both within ±5% initial band gives 95..105; expand to ±20% cap (80..120)
        // picks up 110 as a second strike above.
        let chain1 = OptionChain {
            calls: vec![
                make_contract(95.0, Some(0.27), Some(0), Some(0), expiry1),
                make_contract(100.0, Some(0.25), Some(200), Some(1000), expiry1),
                make_contract(105.0, Some(0.23), Some(0), Some(0), expiry1),
                make_contract(110.0, Some(0.22), Some(0), Some(0), expiry1),
            ],
            puts: vec![
                make_contract(95.0, Some(0.27), Some(0), Some(0), expiry1),
                make_contract(100.0, Some(0.25), Some(300), Some(2000), expiry1),
                make_contract(105.0, Some(0.23), Some(0), Some(0), expiry1),
                make_contract(110.0, Some(0.22), Some(0), Some(0), expiry1),
            ],
        };
        // Chain 2: 400 call vol, 100 put vol; 500 call OI, 250 put OI
        let chain2 = OptionChain {
            calls: vec![make_contract(
                100.0,
                Some(0.30),
                Some(400),
                Some(500),
                expiry2,
            )],
            puts: vec![make_contract(
                100.0,
                Some(0.30),
                Some(100),
                Some(250),
                expiry2,
            )],
        };

        let mut chains = BTreeMap::new();
        chains.insert(ts1, chain1);
        chains.insert(ts2, chain2);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts1, ts2]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should succeed");

        if let OptionsOutcome::Snapshot(s) = outcome {
            // Total call vol = 600, total put vol = 400 → P/C vol ratio = 400/600 ≈ 0.667
            let expected_vol_ratio = 400.0 / 600.0;
            assert!(
                (s.put_call_volume_ratio - expected_vol_ratio).abs() < 1e-6,
                "P/C volume ratio: expected {expected_vol_ratio}, got {}",
                s.put_call_volume_ratio
            );
            // Total call OI = 1500, total put OI = 2250 → P/C OI ratio = 2250/1500 = 1.5
            let expected_oi_ratio = 2250.0 / 1500.0;
            assert!(
                (s.put_call_oi_ratio - expected_oi_ratio).abs() < 1e-6,
                "P/C OI ratio: expected {expected_oi_ratio}, got {}",
                s.put_call_oi_ratio
            );
        } else {
            panic!("expected Snapshot, got {outcome:?}");
        }
    }

    #[tokio::test]
    async fn snapshot_max_pain_uses_front_month_only() {
        // Front-month: max pain at $150 (call OI at $155 is large, put OI at $145 is large)
        // Second month: different structure.
        let spot = 150.0;
        let ts1 = expiry_ts();
        let ts2 = expiry_ts2();
        let expiry1 = "2030-01-18";
        let expiry2 = "2030-02-15";

        // Front-month: strikes 140, 145, 150, 155, 160
        // Design so that max pain is at 150: heavy put OI at 140, 145, 150; heavy call OI at 150, 155, 160
        let front_chain = OptionChain {
            calls: vec![
                make_contract(140.0, Some(0.20), Some(10), Some(100), expiry1),
                make_contract(145.0, Some(0.22), Some(20), Some(200), expiry1),
                make_contract(150.0, Some(0.25), Some(50), Some(1000), expiry1),
                make_contract(155.0, Some(0.28), Some(30), Some(500), expiry1),
                make_contract(160.0, Some(0.30), Some(10), Some(100), expiry1),
            ],
            puts: vec![
                make_contract(140.0, Some(0.20), Some(10), Some(100), expiry1),
                make_contract(145.0, Some(0.22), Some(20), Some(200), expiry1),
                make_contract(150.0, Some(0.25), Some(50), Some(1000), expiry1),
                make_contract(155.0, Some(0.28), Some(30), Some(100), expiry1),
                make_contract(160.0, Some(0.30), Some(10), Some(50), expiry1),
            ],
        };

        // Second month: structure that would put max pain at 130 (very different).
        let second_chain = OptionChain {
            calls: vec![
                make_contract(130.0, Some(0.40), Some(10), Some(5000), expiry2),
                make_contract(140.0, Some(0.38), Some(10), Some(100), expiry2),
            ],
            puts: vec![
                make_contract(130.0, Some(0.40), Some(10), Some(100), expiry2),
                make_contract(140.0, Some(0.38), Some(10), Some(100), expiry2),
            ],
        };

        let mut chains = BTreeMap::new();
        chains.insert(ts1, front_chain.clone());
        chains.insert(ts2, second_chain);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts1, ts2]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should succeed");

        if let OptionsOutcome::Snapshot(s) = outcome {
            // max pain from front-month should be the strike with minimum total pain.
            let expected_max_pain = compute_max_pain(&front_chain, spot);
            assert!(
                (s.max_pain_strike - expected_max_pain).abs() < 1e-6,
                "max pain should use front-month chain: expected {expected_max_pain}, got {}",
                s.max_pain_strike
            );
        } else {
            panic!("expected Snapshot, got {outcome:?}");
        }
    }

    #[tokio::test]
    async fn snapshot_near_term_slice_uses_band_then_min_strikes_fallback() {
        // spot = $100; initial band ±5% = [95, 105]
        // Strikes at 90, 95, 105, 110 → 95 is in [95, 105], 105 is in [95, 105]
        // So initial band gives 1 below (95) and 1 above (105) — not enough (need 2 each side).
        // With expansion: add 90 below (90 >= 80 cap) and 110 above (110 <= 120 cap) → 2 each side.
        let spot = 100.0;
        let ts = expiry_ts();
        let expiry = "2030-01-18";

        let chain = OptionChain {
            calls: vec![
                make_contract(90.0, Some(0.30), Some(50), Some(200), expiry),
                make_contract(95.0, Some(0.27), Some(80), Some(300), expiry),
                make_contract(105.0, Some(0.23), Some(80), Some(300), expiry),
                make_contract(110.0, Some(0.25), Some(50), Some(200), expiry),
            ],
            puts: vec![
                make_contract(90.0, Some(0.30), Some(50), Some(200), expiry),
                make_contract(95.0, Some(0.27), Some(80), Some(300), expiry),
                make_contract(105.0, Some(0.23), Some(80), Some(300), expiry),
                make_contract(110.0, Some(0.25), Some(50), Some(200), expiry),
            ],
        };

        let mut chains = BTreeMap::new();
        chains.insert(ts, chain);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should succeed");

        if let OptionsOutcome::Snapshot(s) = outcome {
            // Should include at least 4 strikes (90, 95, 105, 110) after expansion.
            assert!(
                s.near_term_strikes.len() >= 2,
                "should have strikes from expanded band, got: {:?}",
                s.near_term_strikes
            );
            // Verify that strikes from the expanded band are included.
            let strikes: Vec<f64> = s.near_term_strikes.iter().map(|s| s.strike).collect();
            assert!(
                strikes.contains(&95.0) || strikes.contains(&90.0),
                "should contain below-spot strikes"
            );
            assert!(
                strikes.contains(&105.0) || strikes.contains(&110.0),
                "should contain above-spot strikes"
            );
        } else {
            panic!("expected Snapshot, got {outcome:?}");
        }
    }

    #[tokio::test]
    async fn returns_no_listed_instrument_when_expirations_empty() {
        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(150.0)]),
            option_expirations: Some(vec![]),
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should not error");
        assert_eq!(outcome, OptionsOutcome::NoListedInstrument);
    }

    #[tokio::test]
    async fn returns_sparse_chain_when_band_and_fallback_yield_nothing() {
        // spot = $100; all strikes are far outside ±20% band (below $80 or above $120)
        let spot = 100.0;
        let ts = expiry_ts();
        let expiry = "2030-01-18";

        let chain = OptionChain {
            calls: vec![
                make_contract(50.0, Some(0.60), Some(10), Some(100), expiry),
                make_contract(150.0, Some(0.55), Some(10), Some(100), expiry),
            ],
            puts: vec![
                make_contract(50.0, Some(0.60), Some(10), Some(100), expiry),
                make_contract(150.0, Some(0.55), Some(10), Some(100), expiry),
            ],
        };

        let mut chains = BTreeMap::new();
        chains.insert(ts, chain);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should not error");
        assert_eq!(outcome, OptionsOutcome::SparseChain);
    }

    #[tokio::test]
    async fn near_term_slice_returns_sparse_chain_when_capped_expansion_still_short() {
        // spot = $1.50; strikes at $0.50, $1.00, $5.00
        // Cap low = 1.50 * 0.80 = 1.20; cap high = 1.50 * 1.20 = 1.80
        // Below: $0.50 (< 1.20 cap), $1.00 (>= 1.20 cap) → only 1 strike within cap
        // Above: $5.00 (> 1.80 cap) → 0 strikes within cap
        // Both sides < 2 minimum → SparseChain
        let spot = 1.50;
        let ts = expiry_ts();
        let expiry = "2030-01-18";

        let chain = OptionChain {
            calls: vec![
                make_contract(0.50, Some(0.80), Some(10), Some(100), expiry),
                make_contract(1.00, Some(0.70), Some(10), Some(100), expiry),
                make_contract(5.00, Some(0.50), Some(10), Some(100), expiry),
            ],
            puts: vec![
                make_contract(0.50, Some(0.80), Some(10), Some(100), expiry),
                make_contract(1.00, Some(0.70), Some(10), Some(100), expiry),
                make_contract(5.00, Some(0.50), Some(10), Some(100), expiry),
            ],
        };

        let mut chains = BTreeMap::new();
        chains.insert(ts, chain);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should not error");
        assert_eq!(outcome, OptionsOutcome::SparseChain);
    }

    #[tokio::test]
    async fn returns_historical_run_when_target_date_is_not_market_local_today() {
        // A clearly past date — should always return HistoricalRun.
        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(150.0)]),
            option_expirations: Some(vec![expiry_ts()]),
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), "2020-01-01")
            .await
            .expect("should not error");
        assert_eq!(outcome, OptionsOutcome::HistoricalRun);
    }

    #[tokio::test]
    async fn returns_missing_spot_when_get_latest_close_is_none() {
        // Empty ohlcv → no spot price available.
        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![]), // empty → MissingSpot
            option_expirations: Some(vec![expiry_ts()]),
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should not error");
        assert_eq!(outcome, OptionsOutcome::MissingSpot);
    }

    #[tokio::test]
    async fn returns_err_when_expiration_lookup_fails() {
        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(150.0)]),
            option_expirations_error: Some("network failure".to_owned()),
            ..StubbedFinancialResponses::default()
        });

        let result = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await;
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), TradingError::NetworkTimeout { .. }),
            "should be NetworkTimeout"
        );
    }

    #[tokio::test]
    async fn returns_err_when_option_chain_fetch_fails() {
        let ts = expiry_ts();

        let mut chain_errors = BTreeMap::new();
        chain_errors.insert(ts, "chain error".to_owned());

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(150.0)]),
            option_expirations: Some(vec![ts]),
            option_chain_errors: chain_errors,
            ..StubbedFinancialResponses::default()
        });

        let result = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await;
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), TradingError::NetworkTimeout { .. }),
            "should be NetworkTimeout"
        );
    }

    #[tokio::test]
    async fn ignores_missing_greeks_and_skips_true_skew_metric() {
        // All contracts have greeks: None (as is the case from Yahoo v7 API).
        // The snapshot should succeed — absence of greeks does not cause failure.
        // Use 4 strikes around spot so NTM band has ≥2 per side: 95, 98 (≤ spot),
        // 102, 105 (> spot) — all within ±5% of 100.
        let spot = 100.0;
        let ts = expiry_ts();
        let expiry = "2030-01-18";

        let chain = OptionChain {
            calls: vec![
                make_contract(95.0, Some(0.25), Some(100), Some(500), expiry),
                make_contract(98.0, Some(0.24), Some(150), Some(700), expiry),
                make_contract(102.0, Some(0.23), Some(200), Some(1000), expiry),
                make_contract(105.0, Some(0.22), Some(100), Some(500), expiry),
            ],
            puts: vec![
                make_contract(95.0, Some(0.26), Some(80), Some(400), expiry),
                make_contract(98.0, Some(0.25), Some(120), Some(600), expiry),
                make_contract(102.0, Some(0.24), Some(150), Some(800), expiry),
                make_contract(105.0, Some(0.23), Some(80), Some(400), expiry),
            ],
        };

        // Verify all greeks are None (as created by make_contract).
        for c in chain.calls.iter().chain(chain.puts.iter()) {
            assert!(
                c.greeks.is_none(),
                "greeks should be None in test contracts"
            );
        }

        let mut chains = BTreeMap::new();
        chains.insert(ts, chain);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(spot)]),
            option_expirations: Some(vec![ts]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        let outcome = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should succeed even with no greeks");

        assert!(
            matches!(outcome, OptionsOutcome::Snapshot(_)),
            "should be Snapshot even without greeks, got {outcome:?}"
        );

        // Verify no skew field in serialized output.
        let val = serde_json::to_value(&outcome).expect("serialize");
        let val_str = val.to_string();
        assert!(
            !val_str.contains("skew_25d"),
            "serialized snapshot must not contain skew_25d field"
        );
    }

    #[tokio::test]
    async fn target_date_uses_market_local_us_eastern_not_utc() {
        // Pass today's US/Eastern date → should NOT return HistoricalRun.
        // Pass yesterday's US/Eastern date → should return HistoricalRun.
        let ts = expiry_ts();
        let expiry = "2030-01-18";

        let chain = OptionChain {
            calls: vec![
                make_contract(95.0, Some(0.25), Some(100), Some(500), expiry),
                make_contract(100.0, Some(0.23), Some(200), Some(1000), expiry),
                make_contract(105.0, Some(0.22), Some(100), Some(500), expiry),
            ],
            puts: vec![
                make_contract(95.0, Some(0.26), Some(80), Some(400), expiry),
                make_contract(100.0, Some(0.24), Some(150), Some(800), expiry),
                make_contract(105.0, Some(0.23), Some(80), Some(400), expiry),
            ],
        };

        let mut chains = BTreeMap::new();
        chains.insert(ts, chain);

        let provider = provider_with_stub(StubbedFinancialResponses {
            ohlcv: Some(vec![make_candle(100.0)]),
            option_expirations: Some(vec![ts]),
            option_chains: chains,
            ..StubbedFinancialResponses::default()
        });

        // Today's Eastern date → should not be HistoricalRun.
        let outcome_today = provider
            .fetch_snapshot_impl(&aapl(), &today_eastern())
            .await
            .expect("should not error");
        assert_ne!(
            outcome_today,
            OptionsOutcome::HistoricalRun,
            "today's US/Eastern date should not return HistoricalRun, got {outcome_today:?}"
        );

        // Yesterday's Eastern date → should be HistoricalRun.
        let outcome_yesterday = provider
            .fetch_snapshot_impl(&aapl(), &yesterday_eastern())
            .await
            .expect("should not error");
        assert_eq!(
            outcome_yesterday,
            OptionsOutcome::HistoricalRun,
            "yesterday's US/Eastern date should return HistoricalRun"
        );
    }
}
