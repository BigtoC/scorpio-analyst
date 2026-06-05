//! Financial statement and profile fetchers built on top of [`YFinanceClient`].
//!
//! Exposes quarterly financial data (cashflow, balance sheet, income statement,
//! shares outstanding), earnings trend data, and Yahoo Finance profile information.
//! All functions degrade gracefully to `None` so the pipeline can continue when
//! upstream data is unavailable or a symbol is not a corporate equity.
//!
//! # Asset-shape detection
//!
//! [`YFinanceClient::get_profile`] returns the raw `Profile` enum from
//! `yfinance_rs`. Callers should treat `Profile::Fund(_)` as a signal that
//! corporate-equity valuation inputs may be structurally absent (not a data error).
//!
//! # Degradation contract
//!
//! Every method in this module returns `Option<T>`. A `None` result means the
//! data was unavailable or could not be parsed — not that the pipeline should
//! abort. Callers are responsible for deciding whether to emit `NotAssessed`.

use chrono::NaiveDate;
use yfinance_rs::fundamentals::Calendar;

/// Thin domain wrapper over the upstream yfinance `Calendar` type.
///
/// Uses `NaiveDate` (date-only) because providers disagree on time-of-day and
/// the catalyst calendar only needs the day.
#[derive(Debug, Clone, PartialEq)]
pub struct TickerCalendar {
    /// Upcoming or historical earnings dates (UTC).
    pub earnings_dates: Vec<NaiveDate>,
    /// Ex-dividend date, if declared.
    pub ex_dividend_date: Option<NaiveDate>,
    /// Dividend payment date, if declared.
    pub dividend_payment_date: Option<NaiveDate>,
}

impl From<Calendar> for TickerCalendar {
    fn from(c: Calendar) -> Self {
        Self {
            earnings_dates: c
                .earnings_dates
                .into_iter()
                .map(|dt| dt.date_naive())
                .collect(),
            ex_dividend_date: c.ex_dividend_date.map(|dt| dt.date_naive()),
            dividend_payment_date: c.dividend_payment_date.map(|dt| dt.date_naive()),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── TickerCalendar conversion tests ──────────────────────────────────

    #[test]
    fn ticker_calendar_from_upstream_with_all_fields() {
        use chrono::TimeZone;
        let upstream = yfinance_rs::fundamentals::Calendar {
            earnings_dates: vec![
                chrono::Utc.with_ymd_and_hms(2026, 7, 15, 20, 0, 0).unwrap(),
                chrono::Utc
                    .with_ymd_and_hms(2026, 10, 14, 20, 0, 0)
                    .unwrap(),
            ],
            ex_dividend_date: Some(chrono::Utc.with_ymd_and_hms(2026, 5, 9, 0, 0, 0).unwrap()),
            dividend_payment_date: Some(
                chrono::Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap(),
            ),
        };
        let cal = TickerCalendar::from(upstream);
        assert_eq!(cal.earnings_dates.len(), 2);
        assert_eq!(
            cal.earnings_dates[0],
            chrono::NaiveDate::from_ymd_opt(2026, 7, 15).unwrap()
        );
        assert_eq!(
            cal.ex_dividend_date,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 9).unwrap())
        );
        assert_eq!(
            cal.dividend_payment_date,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 15).unwrap())
        );
    }

    #[test]
    fn ticker_calendar_from_upstream_with_empty_fields() {
        let upstream = yfinance_rs::fundamentals::Calendar {
            earnings_dates: vec![],
            ex_dividend_date: None,
            dividend_payment_date: None,
        };
        let cal = TickerCalendar::from(upstream);
        assert!(cal.earnings_dates.is_empty());
        assert!(cal.ex_dividend_date.is_none());
        assert!(cal.dividend_payment_date.is_none());
    }
}
