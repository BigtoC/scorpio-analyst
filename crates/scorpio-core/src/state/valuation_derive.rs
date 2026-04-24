//! Deterministic valuation derivation from financial statement inputs.
//!
//! Provides [`derive_valuation`], a pure function that computes a structured
//! [`DerivedValuation`] from financial statement data fetched from Yahoo Finance.
//! This runs **before** trader inference so the LLM can reason from computed
//! intrinsic values rather than free-text approximations.
//!
//! # Design invariants
//!
//! - Never coerces absent fields into fake values. A metric is only computed when
//!   all of its required inputs are present and positive.
//! - Fund-style profiles produce [`ScenarioValuation::NotAssessed`] with reason
//!   `"fund_style_asset"` rather than a broken equity path.
//! - Absent profile + no corporate-equity data signals produce
//!   [`AssetShape::Unknown`] and [`ScenarioValuation::NotAssessed`].
//! - Absent profile + any corporate-equity data present falls back to
//!   [`AssetShape::CorporateEquity`] via data-shape detection.
//! - When at least one metric can be computed, the result is
//!   [`ScenarioValuation::CorporateEquity`] (possibly with `None` sub-fields).
//! - When no metric can be computed from available corporate data, the result is
//!   [`ScenarioValuation::NotAssessed`] with reason
//!   `"insufficient_corporate_fundamentals"`.

use num_traits::ToPrimitive as _;
use yfinance_rs::{
    analysis::EarningsTrendRow,
    fundamentals::{BalanceSheetRow, CashflowRow, IncomeStatementRow, ShareCount},
    profile::Profile,
};

use super::{
    AssetShape, CorporateEquityValuation, DcfValuation, DerivedValuation, EvEbitdaValuation,
    ForwardPeValuation, PegValuation, ScenarioValuation,
};

// ─── Public entry point ────────────────────────────────────────────────────────

/// Derive a structured valuation from raw financial statement inputs.
///
/// All slice arguments are optional — absent data is treated as unavailable, not as
/// zero. Metrics that cannot be computed from the available inputs are returned as
/// `None` within the [`CorporateEquityValuation`] container.
///
/// # Asset-shape routing
///
/// 1. `Some(Profile::Fund(_))` → immediately returns `NotAssessed { reason: "fund_style_asset" }`.
/// 2. `Some(Profile::Company(_))` → proceeds to corporate-equity valuation.
/// 3. `None` (profile unavailable) → falls back to data-shape detection:
///    - Any of `cashflow_rows`, `balance_rows`, or `income_rows` being `Some(_)` signals
///      a corporate-equity-like data shape → [`AssetShape::CorporateEquity`].
///    - If all three are `None`, the shape is [`AssetShape::Unknown`] and the result is
///      `NotAssessed { reason: "unknown_asset_shape" }`.
///
/// # Metrics computed
///
/// | Metric    | Required inputs                                                |
/// |-----------|----------------------------------------------------------------|
/// | DCF       | `cashflow_rows` with positive FCF + shares outstanding        |
/// | EV/EBITDA | `balance_rows` with cash/debt/shares + `income_rows` with     |
/// |           | operating income + positive `current_price`                   |
/// | Forward P/E | `earnings_trend` with forward EPS + positive `current_price` |
/// | PEG       | Forward P/E (above) + earnings growth rate from trend data    |
#[must_use]
pub fn derive_valuation(
    profile: Option<Profile>,
    cashflow_rows: Option<&[CashflowRow]>,
    balance_rows: Option<&[BalanceSheetRow]>,
    income_rows: Option<&[IncomeStatementRow]>,
    shares: Option<&[ShareCount]>,
    earnings_trend: Option<&[EarningsTrendRow]>,
    current_price: Option<f64>,
) -> DerivedValuation {
    // ── 1. Determine asset shape ──────────────────────────────────────────────
    let asset_shape = match &profile {
        Some(Profile::Company(_)) => AssetShape::CorporateEquity,
        Some(Profile::Fund(_)) => AssetShape::Fund,
        None => {
            // Data-shape detection: any corporate-equity statement data present
            // is sufficient to assume a corporate equity instrument.
            if has_non_empty_rows(cashflow_rows)
                || has_non_empty_rows(balance_rows)
                || has_non_empty_rows(income_rows)
                || earnings_trend.is_some_and(|rows| !rows.is_empty())
            {
                AssetShape::CorporateEquity
            } else {
                AssetShape::Unknown
            }
        }
    };

    // ── 2. Short-circuit for non-corporate shapes ─────────────────────────────
    match &asset_shape {
        AssetShape::Fund => {
            return DerivedValuation {
                asset_shape,
                scenario: ScenarioValuation::NotAssessed {
                    reason: "fund_style_asset".to_owned(),
                },
            };
        }
        AssetShape::Unknown => {
            return DerivedValuation {
                asset_shape,
                scenario: ScenarioValuation::NotAssessed {
                    reason: "unknown_asset_shape".to_owned(),
                },
            };
        }
        AssetShape::CorporateEquity => {}
        // Crypto variants are placeholders in this slice; they resolve to
        // `NotAssessed` so the corporate-equity pipeline cannot be fed crypto
        // inputs by accident.
        _ => {
            return DerivedValuation {
                asset_shape,
                scenario: ScenarioValuation::NotAssessed {
                    reason: "unsupported_asset_shape".to_owned(),
                },
            };
        }
    }

    // ── 3. Compute each metric ────────────────────────────────────────────────
    let dcf = compute_dcf(cashflow_rows, balance_rows, shares);
    let ev_ebitda = compute_ev_ebitda(balance_rows, income_rows, current_price);
    let forward_row = earnings_trend.and_then(select_forward_eps_row);
    let forward_pe = compute_forward_pe(forward_row, current_price);
    let peg = compute_peg(forward_pe.as_ref(), forward_row);

    // ── 4. If no metric is computable, emit NotAssessed ───────────────────────
    if dcf.is_none() && ev_ebitda.is_none() && forward_pe.is_none() && peg.is_none() {
        return DerivedValuation {
            asset_shape,
            scenario: ScenarioValuation::NotAssessed {
                reason: "insufficient_corporate_fundamentals".to_owned(),
            },
        };
    }

    DerivedValuation {
        asset_shape,
        scenario: ScenarioValuation::CorporateEquity(CorporateEquityValuation {
            dcf,
            ev_ebitda,
            forward_pe,
            peg,
        }),
    }
}

// ─── DCF ─────────────────────────────────────────────────────────────────────

/// Fixed discount rate used in the DCF perpetuity model (10 %).
const DCF_DISCOUNT_RATE_PCT: f64 = 10.0;

/// Compute a perpetuity-based DCF intrinsic value per share.
///
/// Returns `None` when free cash flow or share count is unavailable, zero, or
/// negative — negative FCF cannot be meaningfully extrapolated.
fn compute_dcf(
    cashflow_rows: Option<&[CashflowRow]>,
    balance_rows: Option<&[BalanceSheetRow]>,
    shares: Option<&[ShareCount]>,
) -> Option<DcfValuation> {
    let cashflow_rows = cashflow_rows?;

    let fcf = trailing_quarter_sum(
        cashflow_rows,
        |row| row.period.to_string(),
        get_cashflow_fcf,
    )?;

    if fcf <= 0.0 {
        return None;
    }

    let shares_count = get_shares(balance_rows, shares)?;
    if shares_count == 0 {
        return None;
    }

    let intrinsic_value_per_share = (fcf / (DCF_DISCOUNT_RATE_PCT / 100.0)) / (shares_count as f64);

    Some(DcfValuation {
        free_cash_flow: fcf,
        discount_rate_pct: DCF_DISCOUNT_RATE_PCT,
        intrinsic_value_per_share,
    })
}

// ─── EV/EBITDA ────────────────────────────────────────────────────────────────

/// Compute the EV/EBITDA multiple using operating income as an EBITDA proxy.
///
/// Enterprise Value = (shares_outstanding × current_price) + long_term_debt − cash.
/// EBITDA ≈ operating_income (depreciation & amortisation not separately available).
///
/// Returns `None` when any required input is absent, zero, or negative.
fn compute_ev_ebitda(
    balance_rows: Option<&[BalanceSheetRow]>,
    income_rows: Option<&[IncomeStatementRow]>,
    current_price: Option<f64>,
) -> Option<EvEbitdaValuation> {
    let balance_rows = balance_rows?;
    let income_rows = income_rows?;
    let price = current_price.filter(|&p| p > 0.0)?;

    let row = select_latest_balance_row(balance_rows, |row| {
        row.shares_outstanding.is_some_and(|shares| shares > 0)
            && row.cash.is_some()
            && row.long_term_debt.is_some()
    })?;

    let shares = row.shares_outstanding? as f64;
    let cash = row.cash.as_ref().and_then(|m| m.amount().to_f64())?;
    let debt = row
        .long_term_debt
        .as_ref()
        .and_then(|m| m.amount().to_f64())?;

    let market_cap = shares * price;
    let ev = market_cap + debt - cash;
    if ev <= 0.0 {
        return None;
    }

    let ebitda = trailing_quarter_sum(
        income_rows,
        |row| row.period.to_string(),
        get_operating_income,
    )?;
    if ebitda <= 0.0 {
        return None;
    }

    let ev_ebitda_ratio = ev / ebitda;

    Some(EvEbitdaValuation {
        ev_ebitda_ratio,
        // Implied value per share requires a sector benchmark multiple which is
        // not available in this step; left None for downstream enrichment.
        implied_value_per_share: None,
    })
}

// ─── Forward P/E ─────────────────────────────────────────────────────────────

/// Compute forward P/E from analyst earnings-trend data and current price.
///
/// Picks the first trend row that carries a non-None, positive `earnings_estimate.avg`
/// as the forward EPS. Returns `None` when either EPS or price is absent/non-positive.
fn compute_forward_pe(
    forward_row: Option<&EarningsTrendRow>,
    current_price: Option<f64>,
) -> Option<ForwardPeValuation> {
    let price = current_price.filter(|&p| p > 0.0)?;

    let forward_row = forward_row?;
    let forward_eps = forward_row
        .earnings_estimate
        .avg
        .as_ref()
        .and_then(|m| m.amount().to_f64())
        .filter(|&eps| eps > 0.0)?;

    let forward_pe = price / forward_eps;

    Some(ForwardPeValuation {
        forward_eps,
        forward_pe,
    })
}

// ─── PEG ─────────────────────────────────────────────────────────────────────

/// Compute the PEG ratio: forward P/E divided by the expected EPS growth rate (%).
///
/// Growth rate is sourced from `earnings_estimate.growth` (preferred) or row-level
/// `growth`, expressed as a decimal (e.g., `0.08` = 8 %). Converted to percent
/// before dividing into forward P/E.
///
/// Returns `None` when forward P/E is unavailable or growth is absent/non-positive.
fn compute_peg(
    forward_pe: Option<&ForwardPeValuation>,
    forward_row: Option<&EarningsTrendRow>,
) -> Option<PegValuation> {
    let pe = forward_pe?;
    let forward_row = forward_row?;

    let growth_decimal = forward_row
        .earnings_estimate
        .growth
        .or(forward_row.growth)
        .filter(|&g| g > 0.0)?;

    let growth_pct = growth_decimal * 100.0;
    let peg_ratio = pe.forward_pe / growth_pct;

    Some(PegValuation { peg_ratio })
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn has_non_empty_rows<T>(rows: Option<&[T]>) -> bool {
    rows.is_some_and(|rows| !rows.is_empty())
}

fn trailing_quarter_sum<T, FPeriod, FValue>(
    rows: &[T],
    mut period_for: FPeriod,
    mut value_for: FValue,
) -> Option<f64>
where
    FPeriod: FnMut(&T) -> String,
    FValue: FnMut(&T) -> Option<f64>,
{
    let mut quarterly_values: Vec<((i32, u8), &T)> = Vec::new();
    for row in rows {
        if let Some(key) = parse_quarter_key(&period_for(row)) {
            quarterly_values.push((key, row));
        }
    }

    if quarterly_values.len() < 4 {
        return None;
    }

    quarterly_values.sort_by(|(a, _), (b, _)| b.cmp(a));

    let selected: Vec<_> = quarterly_values.into_iter().take(4).collect();
    if !selected
        .windows(2)
        .all(|pair| is_previous_quarter(pair[0].0, pair[1].0))
    {
        return None;
    }

    let mut sum = 0.0;
    for ((_, _), row) in selected {
        sum += value_for(row)?;
    }

    Some(sum)
}

fn parse_quarter_key(period: &str) -> Option<(i32, u8)> {
    let period = period.trim().to_ascii_uppercase();
    let (year, quarter) = period.split_once('Q')?;
    Some((year.parse().ok()?, quarter.parse().ok()?))
}

fn parse_statement_period_key(period: &str) -> Option<(i32, u8)> {
    parse_quarter_key(period).or_else(|| {
        let period = period.trim().to_ascii_uppercase();
        if is_annual_period(&period) {
            Some((period.parse().ok()?, 0))
        } else {
            None
        }
    })
}

fn is_previous_quarter(current: (i32, u8), next: (i32, u8)) -> bool {
    match current {
        (year, quarter @ 2..=4) => next == (year, quarter - 1),
        (year, 1) => next == (year - 1, 4),
        _ => false,
    }
}

fn is_annual_period(period: &str) -> bool {
    let period = period.trim();
    period.len() == 4 && period.chars().all(|ch| ch.is_ascii_digit())
}

fn annual_period_priority(period: &str) -> Option<u8> {
    let canonical = period.trim().to_ascii_uppercase();
    match canonical.as_str() {
        "1Y" | "+1Y" => Some(0),
        "0Y" => Some(1),
        other if is_annual_period(other) => Some(2),
        _ => None,
    }
}

fn select_forward_eps_row(trend: &[EarningsTrendRow]) -> Option<&EarningsTrendRow> {
    trend
        .iter()
        .filter_map(|row| {
            annual_period_priority(&row.period.to_string())
                .filter(|_| row.earnings_estimate.avg.is_some())
                .map(|priority| (priority, row))
        })
        .min_by_key(|(priority, _)| *priority)
        .map(|(_, row)| row)
        .or_else(|| trend.iter().find(|row| row.earnings_estimate.avg.is_some()))
}

fn select_latest_balance_row(
    rows: &[BalanceSheetRow],
    predicate: impl Fn(&BalanceSheetRow) -> bool,
) -> Option<&BalanceSheetRow> {
    rows.iter()
        .filter(|row| predicate(row))
        .filter_map(|row| parse_statement_period_key(&row.period.to_string()).map(|key| (key, row)))
        .max_by_key(|(key, _)| *key)
        .map(|(_, row)| row)
        .or_else(|| rows.iter().find(|row| predicate(row)))
}

fn get_cashflow_fcf(row: &CashflowRow) -> Option<f64> {
    row.free_cash_flow
        .as_ref()
        .and_then(|money| money.amount().to_f64())
}

fn get_operating_income(row: &IncomeStatementRow) -> Option<f64> {
    row.operating_income
        .as_ref()
        .and_then(|money| money.amount().to_f64())
}

/// Resolve shares outstanding.
///
/// Prefers `BalanceSheetRow.shares_outstanding` (most recent non-None, positive).
/// Falls back to the last entry in `ShareCount` if balance-sheet data is absent.
fn get_shares(
    balance_rows: Option<&[BalanceSheetRow]>,
    shares: Option<&[ShareCount]>,
) -> Option<u64> {
    if let Some(rows) = balance_rows
        && let Some(s) = select_latest_balance_row(rows, |row| {
            row.shares_outstanding.is_some_and(|shares| shares > 0)
        })
        .and_then(|row| row.shares_outstanding)
    {
        return Some(s);
    }

    shares
        .and_then(|share_counts| {
            share_counts
                .iter()
                .filter(|share_count| share_count.shares > 0)
                .max_by_key(|share_count| share_count.date)
        })
        .map(|sc| sc.shares)
        .filter(|&s| s > 0)
}
