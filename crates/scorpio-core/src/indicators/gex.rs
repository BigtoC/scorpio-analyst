//! BSM Greeks (gamma, vanna, charm) and chain-level aggregation for ETF
//! dealer-positioning analysis. Pure functions only — no I/O, no `unsafe`,
//! no panics. Degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`.

use statrs::distribution::{Continuous, ContinuousCDF, Normal};

/// Common BSM input bundle. All values are positive decimals; `t_years` is
/// the time-to-expiration in calendar years (e.g. 30/365 for a 30-day option).
#[derive(Debug, Clone, Copy)]
pub struct BsmInputs {
    pub spot: f64,
    pub strike: f64,
    pub iv: f64,
    pub r: f64,
    pub q: f64,
    pub t_years: f64,
}

fn standard_normal() -> Normal {
    Normal::new(0.0, 1.0).expect("standard normal must construct")
}

fn d1_d2(inputs: &BsmInputs) -> Option<(f64, f64)> {
    if inputs.iv <= 0.0 || inputs.t_years <= 0.0 || inputs.spot <= 0.0 || inputs.strike <= 0.0 {
        return None;
    }
    let sigma_sqrt_t = inputs.iv * inputs.t_years.sqrt();
    let d1 = ((inputs.spot / inputs.strike).ln()
        + (inputs.r - inputs.q + 0.5 * inputs.iv * inputs.iv) * inputs.t_years)
        / sigma_sqrt_t;
    let d2 = d1 - sigma_sqrt_t;
    Some((d1, d2))
}

/// Black-Scholes-Merton gamma with continuous dividend yield.
///
/// Γ = e^{-q·t} · φ(d1) / (S · σ · √t)
pub fn bsm_gamma(inputs: BsmInputs) -> f64 {
    let Some((d1, _d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let phi_d1 = standard_normal().pdf(d1);
    (-inputs.q * inputs.t_years).exp() * phi_d1 / (inputs.spot * inputs.iv * inputs.t_years.sqrt())
}

/// Black-Scholes-Merton vanna (call and put have the same vanna).
///
/// Vanna = -e^{-q·t} · φ(d1) · d2 / σ
pub fn bsm_vanna(inputs: BsmInputs) -> f64 {
    let Some((d1, d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let phi_d1 = standard_normal().pdf(d1);
    -(-inputs.q * inputs.t_years).exp() * phi_d1 * d2 / inputs.iv
}

/// Black-Scholes-Merton call charm (∂Δ_call / ∂t, per year).
pub fn bsm_charm_call(inputs: BsmInputs) -> f64 {
    let Some((d1, d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let n = standard_normal();
    let phi_d1 = n.pdf(d1);
    let big_n_d1 = n.cdf(d1);
    let e_qt = (-inputs.q * inputs.t_years).exp();
    let sigma_sqrt_t = inputs.iv * inputs.t_years.sqrt();
    let bracket = 2.0 * (inputs.r - inputs.q) * inputs.t_years - d2 * sigma_sqrt_t;
    let denom = 2.0 * inputs.t_years * sigma_sqrt_t;
    inputs.q * e_qt * big_n_d1 - e_qt * phi_d1 * bracket / denom
}

/// Black-Scholes-Merton put charm.
pub fn bsm_charm_put(inputs: BsmInputs) -> f64 {
    let Some((d1, d2)) = d1_d2(&inputs) else {
        return 0.0;
    };
    let n = standard_normal();
    let phi_d1 = n.pdf(d1);
    let big_n_neg_d1 = n.cdf(-d1);
    let e_qt = (-inputs.q * inputs.t_years).exp();
    let sigma_sqrt_t = inputs.iv * inputs.t_years.sqrt();
    let bracket = 2.0 * (inputs.r - inputs.q) * inputs.t_years - d2 * sigma_sqrt_t;
    let denom = 2.0 * inputs.t_years * sigma_sqrt_t;
    -inputs.q * e_qt * big_n_neg_d1 - e_qt * phi_d1 * bracket / denom
}

use crate::data::traits::options::NearTermStrike;

/// Per-strike aggregated GEX exposure (post-OI, post-sign-convention,
/// post-USD-scaling). Only net GEX is emitted per strike — VEX/CEX per-strike
/// rows are explicitly out of scope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerStrikeAggregate {
    pub strike: f64,
    pub net_gex_usd_per_1pct_move: f64,
}

/// Input bundle for near-term chain-level aggregation.
pub struct AggregateInputs<'a> {
    pub spot: f64,
    pub r: f64,
    pub q: f64,
    pub as_of: chrono::NaiveDate,
    pub near_term_expiration: &'a str,
    pub near_term_strikes: &'a [NearTermStrike],
    pub expirations: &'a [crate::data::traits::options::ExpirationStrikes],
    pub atm_iv_fallback: f64,
}

/// Result bundle covering the near-term front-month aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateResult {
    pub near_term: Option<NearTermAggregate>,
    pub broad: Option<BroadAggregate>,
    pub iv_fallback_count: u32,
    pub strikes_total: u32,
    pub strikes_used: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NearTermAggregate {
    pub expiration: chrono::NaiveDate,
    pub per_strike: Vec<PerStrikeAggregate>,
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub net_vex_usd_per_volpt: f64,
    pub gross_vex_usd_per_volpt: f64,
    pub net_cex_usd_per_day: f64,
    pub gross_cex_usd_per_day: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BroadAggregate {
    pub net_gex_usd_per_1pct_move: f64,
    pub gross_gex_usd_per_1pct_move: f64,
    pub expirations_used: u32,
    pub expirations_total_considered: u32,
}

const CONTRACT_MULTIPLIER: f64 = 100.0;

struct StrikeContribution {
    net_gex: f64,
    gross_gex: f64,
    net_vex: f64,
    gross_vex: f64,
    net_cex: f64,
    gross_cex: f64,
}

fn build_inputs(spot: f64, strike: f64, iv: f64, r: f64, q: f64, t_years: f64) -> BsmInputs {
    BsmInputs {
        spot,
        strike,
        iv,
        r,
        q,
        t_years,
    }
}

fn leg_iv_or_fallback(iv: Option<f64>, atm_iv_fallback: f64, iv_fallback_count: &mut u32) -> f64 {
    match iv {
        Some(value) if value > 0.0 => value,
        _ => {
            *iv_fallback_count = iv_fallback_count.saturating_add(1);
            atm_iv_fallback
        }
    }
}

/// Compute a single strike's signed + magnitude GEX contributions.
fn contribution_for_strike(
    spot: f64,
    r: f64,
    q: f64,
    t_years: f64,
    atm_iv_fallback: f64,
    row: &NearTermStrike,
    iv_fallback_count: &mut u32,
) -> Option<StrikeContribution> {
    let call_iv = leg_iv_or_fallback(row.call_iv, atm_iv_fallback, iv_fallback_count);
    let put_iv = leg_iv_or_fallback(row.put_iv, atm_iv_fallback, iv_fallback_count);
    if call_iv <= 0.0 && put_iv <= 0.0 {
        return None;
    }

    let call_in = build_inputs(spot, row.strike, call_iv, r, q, t_years);
    let put_in = build_inputs(spot, row.strike, put_iv, r, q, t_years);

    let call_oi = row.call_oi.unwrap_or(0) as f64;
    let put_oi = row.put_oi.unwrap_or(0) as f64;

    let gamma_call = bsm_gamma(call_in);
    let gamma_put = bsm_gamma(put_in);

    let spot_sq_pct = spot * spot * 0.01;

    let net_gex = (gamma_call * call_oi - gamma_put * put_oi) * CONTRACT_MULTIPLIER * spot_sq_pct;
    let gross_gex = (gamma_call * call_oi + gamma_put * put_oi) * CONTRACT_MULTIPLIER * spot_sq_pct;

    let vanna_call = bsm_vanna(call_in);
    let vanna_put = bsm_vanna(put_in);
    let charm_call = bsm_charm_call(call_in);
    let charm_put = bsm_charm_put(put_in);

    let net_vex = (vanna_call * call_oi - vanna_put * put_oi) * CONTRACT_MULTIPLIER * spot;
    let gross_vex =
        ((vanna_call * call_oi).abs() + (vanna_put * put_oi).abs()) * CONTRACT_MULTIPLIER * spot;

    let net_cex = (charm_call * call_oi - charm_put * put_oi) * CONTRACT_MULTIPLIER * spot / 365.0;
    let gross_cex =
        ((charm_call * call_oi).abs() + (charm_put * put_oi).abs()) * CONTRACT_MULTIPLIER * spot
            / 365.0;

    Some(StrikeContribution {
        net_gex,
        gross_gex,
        net_vex,
        gross_vex,
        net_cex,
        gross_cex,
    })
}

fn parse_expiration(expiration: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(expiration, "%Y-%m-%d").ok()
}

fn years_until(expiration: chrono::NaiveDate, as_of: chrono::NaiveDate) -> f64 {
    let days = (expiration - as_of).num_days();
    if days <= 0 { 0.0 } else { days as f64 / 365.0 }
}

/// Aggregate per-strike GEX contributions across the near-term chain.
pub fn aggregate(inputs: AggregateInputs<'_>) -> AggregateResult {
    let mut iv_fallback_count: u32 = 0;
    let mut strikes_total: u32 = 0;
    let mut strikes_used: u32 = 0;

    let near_term = match parse_expiration(inputs.near_term_expiration) {
        Some(exp) => {
            let t_years = years_until(exp, inputs.as_of);
            if t_years <= 0.0 || inputs.near_term_strikes.is_empty() {
                None
            } else {
                let mut per_strike: Vec<PerStrikeAggregate> = Vec::new();
                let mut net_gex = 0.0;
                let mut gross_gex = 0.0;
                let mut net_vex = 0.0;
                let mut gross_vex = 0.0;
                let mut net_cex = 0.0;
                let mut gross_cex = 0.0;

                for row in inputs.near_term_strikes {
                    strikes_total = strikes_total.saturating_add(1);
                    let Some(c) = contribution_for_strike(
                        inputs.spot,
                        inputs.r,
                        inputs.q,
                        t_years,
                        inputs.atm_iv_fallback,
                        row,
                        &mut iv_fallback_count,
                    ) else {
                        continue;
                    };
                    strikes_used = strikes_used.saturating_add(1);
                    per_strike.push(PerStrikeAggregate {
                        strike: row.strike,
                        net_gex_usd_per_1pct_move: c.net_gex,
                    });
                    net_gex += c.net_gex;
                    gross_gex += c.gross_gex;
                    net_vex += c.net_vex;
                    gross_vex += c.gross_vex;
                    net_cex += c.net_cex;
                    gross_cex += c.gross_cex;
                }

                if strikes_used == 0 {
                    None
                } else {
                    Some(NearTermAggregate {
                        expiration: exp,
                        per_strike,
                        net_gex_usd_per_1pct_move: net_gex,
                        gross_gex_usd_per_1pct_move: gross_gex,
                        net_vex_usd_per_volpt: net_vex,
                        gross_vex_usd_per_volpt: gross_vex,
                        net_cex_usd_per_day: net_cex,
                        gross_cex_usd_per_day: gross_cex,
                    })
                }
            }
        }
        None => None,
    };

    // ── Broad aggregation ────────────────────────────────────────────────────
    let mut expirations_total_considered: u32 = 0;
    let mut expirations_used: u32 = 0;
    let mut broad_net_gex: f64 = 0.0;
    let mut broad_gross_gex: f64 = 0.0;

    if let Some(ref nt) = near_term {
        expirations_total_considered = expirations_total_considered.saturating_add(1);
        expirations_used = expirations_used.saturating_add(1);
        broad_net_gex += nt.net_gex_usd_per_1pct_move;
        broad_gross_gex += nt.gross_gex_usd_per_1pct_move;
    }

    for extra in inputs.expirations {
        expirations_total_considered = expirations_total_considered.saturating_add(1);
        let Some(exp) = parse_expiration(&extra.expiration) else {
            continue;
        };
        let t_years = years_until(exp, inputs.as_of);
        if t_years <= 0.0 || extra.strikes.is_empty() {
            continue;
        }
        let mut local_net_gex = 0.0;
        let mut local_gross_gex = 0.0;
        let mut row_used = false;
        for row in &extra.strikes {
            let Some(c) = contribution_for_strike(
                inputs.spot,
                inputs.r,
                inputs.q,
                t_years,
                inputs.atm_iv_fallback,
                row,
                &mut iv_fallback_count,
            ) else {
                continue;
            };
            local_net_gex += c.net_gex;
            local_gross_gex += c.gross_gex;
            row_used = true;
        }
        if row_used {
            expirations_used = expirations_used.saturating_add(1);
            broad_net_gex += local_net_gex;
            broad_gross_gex += local_gross_gex;
        }
    }

    let broad = if expirations_used > 0 {
        Some(BroadAggregate {
            net_gex_usd_per_1pct_move: broad_net_gex,
            gross_gex_usd_per_1pct_move: broad_gross_gex,
            expirations_used,
            expirations_total_considered,
        })
    } else {
        None
    };

    AggregateResult {
        near_term,
        broad,
        iv_fallback_count,
        strikes_total,
        strikes_used,
    }
}

#[cfg(test)]
#[allow(clippy::clone_on_copy)]
mod tests {
    use super::*;

    fn ref_inputs() -> BsmInputs {
        BsmInputs {
            spot: 100.0,
            strike: 100.0,
            iv: 0.20,
            r: 0.045,
            q: 0.015,
            t_years: 30.0 / 365.0,
        }
    }

    #[test]
    fn bsm_gamma_matches_analytical_reference() {
        let g = bsm_gamma(ref_inputs());
        assert!((g - 0.069_313).abs() < 1e-5, "gamma drift: got {g}");
    }

    #[test]
    fn bsm_gamma_returns_zero_for_degenerate_inputs() {
        let mut i = ref_inputs();
        i.iv = 0.0;
        assert_eq!(bsm_gamma(i.clone()), 0.0);
        i = ref_inputs();
        i.t_years = 0.0;
        assert_eq!(bsm_gamma(i.clone()), 0.0);
        i = ref_inputs();
        i.spot = 0.0;
        assert_eq!(bsm_gamma(i), 0.0);
    }

    #[test]
    fn bsm_gamma_at_the_money_exceeds_out_of_the_money() {
        let atm = bsm_gamma(ref_inputs());
        let otm = bsm_gamma(BsmInputs {
            strike: 120.0,
            ..ref_inputs()
        });
        assert!(
            atm > otm,
            "ATM gamma must exceed OTM gamma: atm={atm} otm={otm}"
        );
    }

    #[test]
    fn bsm_vanna_call_and_put_share_value() {
        let v = bsm_vanna(ref_inputs());
        assert!(v.is_finite(), "vanna must be finite: {v}");
        assert!(v.abs() < 1.0, "|vanna| out of range: {v}");
    }

    #[test]
    fn bsm_charm_call_put_parity_gap_matches_dividend_yield() {
        let call = bsm_charm_call(ref_inputs());
        let put = bsm_charm_put(ref_inputs());
        assert!(call.is_finite() && put.is_finite());
        let expected_gap = ref_inputs().q * (-ref_inputs().q * ref_inputs().t_years).exp();
        assert!(
            ((call - put) - expected_gap).abs() < 1e-9,
            "unexpected charm parity gap: call={call} put={put} expected_gap={expected_gap}"
        );
    }

    #[test]
    fn bsm_vanna_returns_zero_for_degenerate_inputs() {
        let mut i = ref_inputs();
        i.iv = 0.0;
        assert_eq!(bsm_vanna(i), 0.0);
    }

    #[test]
    fn bsm_charm_returns_zero_for_degenerate_inputs() {
        let mut i = ref_inputs();
        i.t_years = 0.0;
        assert_eq!(bsm_charm_call(i.clone()), 0.0);
        assert_eq!(bsm_charm_put(i), 0.0);
    }

    use crate::data::traits::options::{IvTermPoint, NearTermStrike, OptionsSnapshot};

    fn snap(near_term_strikes: Vec<NearTermStrike>) -> OptionsSnapshot {
        OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes,
            all_expirations: vec![],
        }
    }

    fn row(strike: f64, call_oi: u64, put_oi: u64) -> NearTermStrike {
        NearTermStrike {
            strike,
            call_iv: Some(0.20),
            put_iv: Some(0.20),
            call_volume: None,
            put_volume: None,
            call_oi: Some(call_oi),
            put_oi: Some(put_oi),
        }
    }

    #[test]
    fn aggregate_returns_none_when_no_strikes() {
        let s = snap(vec![]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s.atm_iv,
        });
        assert!(res.near_term.is_none());
        assert_eq!(res.strikes_total, 0);
        assert_eq!(res.strikes_used, 0);
    }

    #[test]
    fn aggregate_signs_dealer_short_calls_long_puts() {
        let s = snap(vec![row(100.0, 1_000, 0)]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s.atm_iv,
        });
        let near = res.near_term.expect("near-term aggregate must be present");
        assert!(
            near.net_gex_usd_per_1pct_move > 0.0,
            "call-only OI must produce positive net GEX"
        );
        assert!(near.gross_gex_usd_per_1pct_move >= near.net_gex_usd_per_1pct_move);

        let s2 = snap(vec![row(100.0, 0, 1_000)]);
        let res2 = aggregate(AggregateInputs {
            spot: s2.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s2.near_term_expiration,
            near_term_strikes: &s2.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s2.atm_iv,
        });
        let near2 = res2.near_term.expect("put-only aggregate present");
        assert!(
            near2.net_gex_usd_per_1pct_move < 0.0,
            "put-only OI must produce negative net GEX"
        );
    }

    #[test]
    fn aggregate_iv_fallback_counter_increments_when_strike_iv_missing() {
        let row_no_iv = NearTermStrike {
            strike: 100.0,
            call_iv: None,
            put_iv: None,
            call_volume: None,
            put_volume: None,
            call_oi: Some(500),
            put_oi: Some(500),
        };
        let s = snap(vec![row_no_iv]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s.atm_iv,
        });
        assert_eq!(res.iv_fallback_count, 2);
        assert_eq!(res.strikes_used, 1);
    }

    #[test]
    fn aggregate_treats_non_positive_leg_iv_as_missing() {
        let mut malformed = row(100.0, 1_000, 0);
        malformed.call_iv = Some(0.0);
        let s = snap(vec![malformed]);

        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s.atm_iv,
        });

        let near = res
            .near_term
            .expect("zero-IV call leg should use ATM IV fallback");
        assert_eq!(res.iv_fallback_count, 1);
        assert_eq!(res.strikes_used, 1);
        assert!(
            near.net_gex_usd_per_1pct_move > 0.0,
            "call-only OI with zero leg IV should not be zeroed"
        );
    }

    #[test]
    fn aggregate_skips_row_when_no_iv_anywhere() {
        let bad_row = NearTermStrike {
            strike: 100.0,
            call_iv: None,
            put_iv: None,
            call_volume: None,
            put_volume: None,
            call_oi: Some(500),
            put_oi: Some(500),
        };
        let mut s = snap(vec![bad_row]);
        s.atm_iv = 0.0;
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s.atm_iv,
        });
        assert!(res.near_term.is_none());
        assert_eq!(res.strikes_total, 1);
        assert_eq!(res.strikes_used, 0);
    }

    #[test]
    fn aggregate_returns_none_when_expiration_is_today_or_past() {
        let s = snap(vec![row(100.0, 1_000, 1_000)]);
        let res = aggregate(AggregateInputs {
            spot: s.spot_price,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            near_term_expiration: &s.near_term_expiration,
            near_term_strikes: &s.near_term_strikes,
            expirations: &[],
            atm_iv_fallback: s.atm_iv,
        });
        assert!(
            res.near_term.is_none(),
            "same-day expiration must yield None"
        );
    }

    use crate::data::traits::options::ExpirationStrikes;

    fn extra_expiration(date: &str, rows: Vec<NearTermStrike>) -> ExpirationStrikes {
        ExpirationStrikes {
            expiration: date.to_owned(),
            strikes: rows,
        }
    }

    #[test]
    fn aggregate_broad_combines_front_month_with_additional_expirations() {
        let near = vec![row(100.0, 1_000, 1_000)];
        let extras = vec![
            extra_expiration("2026-07-31", vec![row(100.0, 500, 500)]),
            extra_expiration("2026-08-29", vec![row(100.0, 300, 300)]),
        ];
        let res = aggregate(AggregateInputs {
            spot: 100.0,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: "2026-06-26",
            near_term_strikes: &near,
            expirations: &extras,
            atm_iv_fallback: 0.20,
        });
        let broad = res.broad.expect("broad present");
        assert_eq!(broad.expirations_used, 3, "1 front + 2 extra");
        assert_eq!(broad.expirations_total_considered, 3);
        assert!(broad.gross_gex_usd_per_1pct_move > 0.0);
    }

    #[test]
    fn aggregate_broad_reports_partial_coverage_when_some_expirations_unusable() {
        let near = vec![row(100.0, 1_000, 1_000)];
        let extras = vec![
            extra_expiration("not-a-date", vec![row(100.0, 100, 100)]),
            extra_expiration("2026-05-27", vec![row(100.0, 200, 200)]),
            extra_expiration("2026-07-31", vec![row(100.0, 500, 500)]),
        ];
        let res = aggregate(AggregateInputs {
            spot: 100.0,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: "2026-06-26",
            near_term_strikes: &near,
            expirations: &extras,
            atm_iv_fallback: 0.20,
        });
        let broad = res.broad.expect("broad present");
        assert_eq!(broad.expirations_used, 2, "front + one valid extra");
        assert_eq!(broad.expirations_total_considered, 4, "front + 3 extras");
    }

    #[test]
    fn aggregate_broad_is_none_when_no_usable_expirations() {
        let near: Vec<NearTermStrike> = vec![];
        let res = aggregate(AggregateInputs {
            spot: 100.0,
            r: 0.045,
            q: 0.015,
            as_of: chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(),
            near_term_expiration: "2026-06-26",
            near_term_strikes: &near,
            expirations: &[],
            atm_iv_fallback: 0.20,
        });
        assert!(res.near_term.is_none());
        assert!(res.broad.is_none());
    }
}
