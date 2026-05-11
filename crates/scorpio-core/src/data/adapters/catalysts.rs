//! Catalyst calendar evidence contract and provider trait.
//!
//! Declares the [`CatalystEvent`] payload struct and the
//! [`CatalystCalendarProvider`] trait seam. The [`Tier1CatalystProvider`]
//! concrete implementation fans out to Finnhub, FRED, and yfinance with
//! fail-soft semantics: one source erroring zeros out only that source's
//! contribution; the others still flow through.

use async_trait::async_trait;
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::data::{
    FinnhubClient, FredClient,
    fred::release_id,
    yfinance::financials::TickerCalendar,
};
use crate::data::yfinance::ohlcv::YFinanceClient;
use crate::error::TradingError;
use crate::state::{CatalystCategory, ImpactLevel};

// ─── Payload ────────────────────────────────────────────────────────────────

/// A single forward-looking catalyst event for a ticker.
///
/// Distinct from `EventNewsEvidence` (which is *backward-looking* news that
/// already happened). `CatalystEvent` always has `event_date >= as_of_date`
/// at the time the provider returned it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CatalystEvent {
    /// Canonical uppercase ticker, or `"_MACRO"` for ticker-agnostic macro releases.
    pub symbol: String,
    /// ISO-8601 date `"YYYY-MM-DD"`. Time-of-day is intentionally omitted —
    /// providers disagree on it and the prompt only needs the day.
    pub event_date: String,
    pub category: CatalystCategory,
    pub impact: ImpactLevel,
    /// Short label, e.g. `"AAPL Q3 earnings"`, `"FOMC rate decision"`.
    pub headline: String,
    /// Optional canonical source URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Identifier of the upstream provider.
    pub source: String,
}

// ─── Trait ──────────────────────────────────────────────────────────────────

/// Contract for any provider that can supply [`CatalystEvent`]s.
#[async_trait]
pub trait CatalystCalendarProvider: Send + Sync {
    /// Fetch upcoming catalysts for `symbol` in the half-open window
    /// `[as_of_date, as_of_date + horizon_days)`. Returns an empty `Vec`
    /// rather than `Err` for "no events" so the analyst-context renderer
    /// treats absence as a domain-valid signal rather than a fetch failure.
    async fn fetch_catalysts(
        &self,
        symbol: &str,
        as_of_date: &str,
        horizon_days: u32,
    ) -> Result<Vec<CatalystEvent>, TradingError>;
}

// ─── Tier 1 provider ────────────────────────────────────────────────────────

/// Composes Finnhub earnings/IPO, FRED macro releases, and yfinance
/// ex-dividend calendars into a single fail-soft catalyst stream.
pub struct Tier1CatalystProvider {
    pub(crate) finnhub: FinnhubClient,
    pub(crate) fred: FredClient,
    pub(crate) yfinance: YFinanceClient,
}

impl Tier1CatalystProvider {
    pub fn new(finnhub: FinnhubClient, fred: FredClient, yfinance: YFinanceClient) -> Self {
        Self { finnhub, fred, yfinance }
    }

    /// Soft-fail: Finnhub earnings for a specific symbol.
    async fn try_finnhub_earnings(
        &self,
        symbol: &str,
        from: &str,
        to: &str,
    ) -> Vec<CatalystEvent> {
        match self.finnhub.fetch_earnings_calendar(from, to, Some(symbol)).await {
            Ok(rows) => rows
                .iter()
                .filter_map(|r| map_finnhub_earnings(symbol, r))
                .collect(),
            Err(err) => {
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "finnhub_earnings",
                    symbol,
                    error = %err,
                    "catalyst source failed; continuing with empty contribution"
                );
                Vec::new()
            }
        }
    }

    /// Soft-fail: Finnhub IPO calendar for the date window.
    async fn try_finnhub_ipo(&self, from: &str, to: &str) -> Vec<CatalystEvent> {
        match self.finnhub.fetch_ipo_calendar(from, to).await {
            Ok(rows) => rows.iter().filter_map(map_finnhub_ipo).collect(),
            Err(err) => {
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "finnhub_ipo",
                    error = %err,
                    "catalyst source failed; continuing with empty contribution"
                );
                Vec::new()
            }
        }
    }

    /// Soft-fail: FRED scheduled releases for all tracked high-impact IDs.
    async fn try_fred_releases(&self, from: &str, to: &str) -> Vec<CatalystEvent> {
        let release_ids = [
            (release_id::CPI, "CPI release", ImpactLevel::H),
            (release_id::NONFARM_PAYROLLS, "Nonfarm Payrolls", ImpactLevel::H),
            (release_id::FOMC_DECISION, "FOMC rate decision", ImpactLevel::H),
            (release_id::GDP, "GDP release", ImpactLevel::M),
            (release_id::ISM_MANUFACTURING, "ISM Manufacturing", ImpactLevel::M),
            (release_id::RETAIL_SALES, "Retail Sales", ImpactLevel::M),
        ];

        let mut events = Vec::new();
        for (id, label, impact) in release_ids {
            match self.fred.release_dates(id, from, to).await {
                Ok(dates) => {
                    for date in dates {
                        events.push(CatalystEvent {
                            symbol: "_MACRO".to_owned(),
                            event_date: date.format("%Y-%m-%d").to_string(),
                            category: CatalystCategory::MacroEvents,
                            impact,
                            headline: label.to_owned(),
                            source_url: None,
                            source: "fred".to_owned(),
                        });
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        kind = "catalyst_fetch_failed",
                        source = "fred",
                        release_id = id,
                        error = %err,
                        "FRED release dates failed; skipping this release"
                    );
                }
            }
        }
        events
    }

    /// Soft-fail: yfinance per-ticker ex-dividend date.
    async fn try_yfinance_calendar(&self, symbol: &str, as_of_date: &str) -> Vec<CatalystEvent> {
        let as_of = match NaiveDate::parse_from_str(as_of_date, "%Y-%m-%d") {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "invalid as_of_date for yfinance calendar");
                return Vec::new();
            }
        };

        let cal: TickerCalendar = match self.yfinance.fetch_calendar(symbol).await {
            Some(c) => c,
            None => {
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "yfinance_calendar",
                    symbol,
                    "yfinance calendar unavailable; continuing with empty contribution"
                );
                return Vec::new();
            }
        };

        let mut events = Vec::new();

        for date in &cal.earnings_dates {
            if *date >= as_of {
                events.push(CatalystEvent {
                    symbol: symbol.to_ascii_uppercase(),
                    event_date: date.format("%Y-%m-%d").to_string(),
                    category: CatalystCategory::EarningsAndFinancial,
                    impact: ImpactLevel::H,
                    headline: format!("{} earnings date", symbol.to_ascii_uppercase()),
                    source_url: None,
                    source: "yfinance".to_owned(),
                });
            }
        }

        if let Some(date) = cal.ex_dividend_date {
            if date >= as_of {
                events.push(CatalystEvent {
                    symbol: symbol.to_ascii_uppercase(),
                    event_date: date.format("%Y-%m-%d").to_string(),
                    category: CatalystCategory::EarningsAndFinancial,
                    impact: ImpactLevel::L,
                    headline: format!("{} ex-dividend date", symbol.to_ascii_uppercase()),
                    source_url: None,
                    source: "yfinance".to_owned(),
                });
            }
        }

        events
    }
}

#[async_trait]
impl CatalystCalendarProvider for Tier1CatalystProvider {
    async fn fetch_catalysts(
        &self,
        symbol: &str,
        as_of_date: &str,
        horizon_days: u32,
    ) -> Result<Vec<CatalystEvent>, TradingError> {
        let as_of = NaiveDate::parse_from_str(as_of_date, "%Y-%m-%d").map_err(|e| {
            TradingError::SchemaViolation {
                message: format!("invalid as_of_date '{as_of_date}': {e}"),
            }
        })?;
        let to = as_of + chrono::Duration::days(i64::from(horizon_days));
        let from_str = as_of.format("%Y-%m-%d").to_string();
        let to_str = to.format("%Y-%m-%d").to_string();

        let (earnings, ipos, macros, dividends) = tokio::join!(
            self.try_finnhub_earnings(symbol, &from_str, &to_str),
            self.try_finnhub_ipo(&from_str, &to_str),
            self.try_fred_releases(&from_str, &to_str),
            self.try_yfinance_calendar(symbol, as_of_date),
        );

        let total = earnings.len() + ipos.len() + macros.len() + dividends.len();
        let mut all = Vec::with_capacity(total);
        all.extend(earnings);
        all.extend(ipos);
        all.extend(macros);
        all.extend(dividends);

        all.sort_by(|a, b| {
            (&a.event_date, &a.symbol, &a.category)
                .cmp(&(&b.event_date, &b.symbol, &b.category))
        });
        all.dedup_by(|a, b| {
            a.event_date == b.event_date
                && a.symbol == b.symbol
                && a.category == b.category
        });

        Ok(all)
    }
}

// ─── Mapping helpers ─────────────────────────────────────────────────────────

/// Map a Finnhub `EarningsRelease` into a `CatalystEvent`.
///
/// Returns `None` when the upstream record lacks both a date and a symbol
/// (unusable for the calendar).
fn map_finnhub_earnings(
    queried_symbol: &str,
    r: &finnhub::models::calendar::EarningsRelease,
) -> Option<CatalystEvent> {
    let date = r.date.as_deref()?;
    let symbol = r
        .symbol
        .as_deref()
        .unwrap_or(queried_symbol)
        .to_ascii_uppercase();
    let label = match (r.year, r.quarter) {
        (Some(y), Some(q)) => format!("{symbol} Q{q} {y} earnings"),
        _ => format!("{symbol} earnings"),
    };
    Some(CatalystEvent {
        symbol,
        event_date: date.to_owned(),
        category: CatalystCategory::EarningsAndFinancial,
        impact: ImpactLevel::H,
        headline: label,
        source_url: None,
        source: "finnhub".to_owned(),
    })
}

/// Map a Finnhub `IPOEvent` into a `CatalystEvent`.
fn map_finnhub_ipo(r: &finnhub::models::calendar::IPOEvent) -> Option<CatalystEvent> {
    let date = r.date.as_deref()?;
    let symbol = r.symbol.as_deref().unwrap_or("_IPO").to_ascii_uppercase();
    let name = r.name.as_deref().unwrap_or(&symbol);
    Some(CatalystEvent {
        symbol: symbol.clone(),
        event_date: date.to_owned(),
        category: CatalystCategory::CorporateEvents,
        impact: ImpactLevel::M,
        headline: format!("IPO: {name}"),
        source_url: None,
        source: "finnhub".to_owned(),
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> CatalystEvent {
        CatalystEvent {
            symbol: "AAPL".to_owned(),
            event_date: "2026-06-01".to_owned(),
            category: CatalystCategory::EarningsAndFinancial,
            impact: ImpactLevel::H,
            headline: "AAPL Q2 earnings".to_owned(),
            source_url: None,
            source: "finnhub".to_owned(),
        }
    }

    // ── CatalystEvent serialization ──────────────────────────────────────

    #[test]
    fn catalyst_event_round_trip() {
        let event = sample_event();
        let json = serde_json::to_string(&event).expect("serialize");
        let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, recovered);
    }

    #[test]
    fn catalyst_event_with_source_url_round_trip() {
        let mut event = sample_event();
        event.source_url = Some("https://example.com/filing".to_owned());
        let json = serde_json::to_string(&event).expect("serialize");
        let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, recovered);
    }

    #[test]
    fn catalyst_event_source_url_omitted_when_none() {
        let event = sample_event();
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(!json.contains("source_url"), "source_url should be absent when None");
    }

    #[test]
    fn macro_sentinel_symbol_round_trip() {
        let event = CatalystEvent {
            symbol: "_MACRO".to_owned(),
            event_date: "2026-06-15".to_owned(),
            category: CatalystCategory::MacroEvents,
            impact: ImpactLevel::H,
            headline: "FOMC rate decision".to_owned(),
            source_url: None,
            source: "fred".to_owned(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, recovered);
    }

    #[test]
    fn all_impact_levels_round_trip() {
        for impact in [ImpactLevel::H, ImpactLevel::M, ImpactLevel::L] {
            let mut event = sample_event();
            event.impact = impact;
            let json = serde_json::to_string(&event).expect("serialize");
            let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(event, recovered);
        }
    }

    #[test]
    fn all_categories_round_trip() {
        for category in [
            CatalystCategory::EarningsAndFinancial,
            CatalystCategory::CorporateEvents,
            CatalystCategory::IndustryEvents,
            CatalystCategory::MacroEvents,
        ] {
            let mut event = sample_event();
            event.category = category;
            let json = serde_json::to_string(&event).expect("serialize");
            let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(event, recovered);
        }
    }

    // ── Mapping helper unit tests ────────────────────────────────────────

    #[test]
    fn map_finnhub_earnings_produces_h_impact_event() {
        let r = finnhub::models::calendar::EarningsRelease {
            symbol: Some("AAPL".to_owned()),
            date: Some("2026-07-15".to_owned()),
            hour: None,
            year: Some(2026),
            quarter: Some(3),
            eps_estimate: None,
            eps_actual: None,
            revenue_estimate: None,
            revenue_actual: None,
        };
        let event = map_finnhub_earnings("AAPL", &r).expect("should map");
        assert_eq!(event.symbol, "AAPL");
        assert_eq!(event.event_date, "2026-07-15");
        assert_eq!(event.category, CatalystCategory::EarningsAndFinancial);
        assert_eq!(event.impact, ImpactLevel::H);
        assert!(event.headline.contains("Q3"), "headline should include quarter");
        assert_eq!(event.source, "finnhub");
    }

    #[test]
    fn map_finnhub_earnings_returns_none_when_date_missing() {
        let r = finnhub::models::calendar::EarningsRelease {
            symbol: Some("AAPL".to_owned()),
            date: None,
            hour: None,
            year: None,
            quarter: None,
            eps_estimate: None,
            eps_actual: None,
            revenue_estimate: None,
            revenue_actual: None,
        };
        assert!(map_finnhub_earnings("AAPL", &r).is_none());
    }

    #[test]
    fn map_finnhub_earnings_falls_back_to_queried_symbol_when_symbol_absent() {
        let r = finnhub::models::calendar::EarningsRelease {
            symbol: None,
            date: Some("2026-07-15".to_owned()),
            hour: None,
            year: None,
            quarter: None,
            eps_estimate: None,
            eps_actual: None,
            revenue_estimate: None,
            revenue_actual: None,
        };
        let event = map_finnhub_earnings("MSFT", &r).expect("should map");
        assert_eq!(event.symbol, "MSFT");
    }

    #[test]
    fn map_finnhub_ipo_produces_m_impact_event() {
        let r = finnhub::models::calendar::IPOEvent {
            symbol: Some("NEWCO".to_owned()),
            date: Some("2026-06-10".to_owned()),
            exchange: None,
            name: Some("NewCo Inc.".to_owned()),
            status: Some("expected".to_owned()),
            price: None,
            number_of_shares: None,
            total_shares_value: None,
        };
        let event = map_finnhub_ipo(&r).expect("should map");
        assert_eq!(event.symbol, "NEWCO");
        assert_eq!(event.category, CatalystCategory::CorporateEvents);
        assert_eq!(event.impact, ImpactLevel::M);
        assert!(event.headline.contains("NewCo Inc."));
        assert_eq!(event.source, "finnhub");
    }

    #[test]
    fn map_finnhub_ipo_returns_none_when_date_missing() {
        let r = finnhub::models::calendar::IPOEvent {
            symbol: Some("NEWCO".to_owned()),
            date: None,
            exchange: None,
            name: None,
            status: None,
            price: None,
            number_of_shares: None,
            total_shares_value: None,
        };
        assert!(map_finnhub_ipo(&r).is_none());
    }

    // ── fetch_catalysts invalid date ─────────────────────────────────────

    #[tokio::test]
    async fn fetch_catalysts_returns_schema_error_for_invalid_date() {
        let provider = Tier1CatalystProvider::new(
            FinnhubClient::for_test(),
            FredClient::for_test(),
            YFinanceClient::new(crate::rate_limit::SharedRateLimiter::new("test-yf", 30)),
        );
        let result = provider.fetch_catalysts("AAPL", "not-a-date", 30).await;
        assert!(
            matches!(result, Err(TradingError::SchemaViolation { .. })),
            "invalid date must produce SchemaViolation, got {result:?}"
        );
    }

    // ── Composition: sources dedup and sort ──────────────────────────────

    #[test]
    fn dedup_by_symbol_date_category_removes_duplicates() {
        let mut events = vec![
            CatalystEvent {
                symbol: "AAPL".to_owned(),
                event_date: "2026-07-01".to_owned(),
                category: CatalystCategory::EarningsAndFinancial,
                impact: ImpactLevel::H,
                headline: "first".to_owned(),
                source_url: None,
                source: "finnhub".to_owned(),
            },
            CatalystEvent {
                symbol: "AAPL".to_owned(),
                event_date: "2026-07-01".to_owned(),
                category: CatalystCategory::EarningsAndFinancial,
                impact: ImpactLevel::H,
                headline: "duplicate".to_owned(),
                source_url: None,
                source: "yfinance".to_owned(),
            },
        ];
        events.sort_by(|a, b| {
            (&a.event_date, &a.symbol, &a.category)
                .cmp(&(&b.event_date, &b.symbol, &b.category))
        });
        events.dedup_by(|a, b| {
            a.event_date == b.event_date
                && a.symbol == b.symbol
                && a.category == b.category
        });
        assert_eq!(events.len(), 1, "duplicate should be removed");
        assert_eq!(events[0].headline, "first", "first occurrence kept");
    }
}
