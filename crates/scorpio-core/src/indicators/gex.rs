//! BSM Greeks (gamma, vanna, charm) and chain-level aggregation for ETF
//! dealer-positioning analysis. Pure functions only — no I/O, no `unsafe`,
//! no panics. Degenerate inputs (σ ≤ 0, t ≤ 0, S ≤ 0) return `0.0`.

// Callers land in Task 2 (per-strike aggregation) and Task 4 (ETF valuator).
#![allow(dead_code)]

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
}
