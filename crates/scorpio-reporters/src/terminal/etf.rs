//! ETF Valuation Snapshot panel renderer.

use std::fmt::Write;

use scorpio_core::state::{
    EtfComposition, EtfValuation, GexSummary, HoldingsAgeBand, PremiumBand, ScenarioValuation,
    StrikeGex, TrackingError, TradingState,
};

/// Render policy. Picks glyphs + layout based on terminal capability.
/// Phase 1 keeps this minimal — three flavours only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPolicy {
    /// Default rendering: rich UTF-8 glyphs + multi-column rows.
    Rich,
    /// Narrow terminal (<60 cols): one-item-per-line for holdings + tracking.
    Narrow,
    /// ASCII-only fallback: replace `─`, `▲`, `⚠`, `✓/✗` with ASCII equivalents.
    Ascii,
}

impl RenderPolicy {
    pub(crate) fn rule_char(self) -> char {
        if self == Self::Ascii { '-' } else { '─' }
    }
    pub(crate) fn warn(self) -> &'static str {
        if self == Self::Ascii { "!" } else { "⚠" }
    }
    pub(crate) fn check(self, ok: bool) -> &'static str {
        if self == Self::Ascii {
            if ok { "[OK]" } else { "[X]" }
        } else if ok {
            "✓"
        } else {
            "✗"
        }
    }
    pub(crate) fn band_marker(self) -> &'static str {
        if self == Self::Ascii { "^" } else { "▲" }
    }
    pub(crate) fn holdings_separator(self) -> &'static str {
        if self == Self::Narrow { "\n" } else { " │ " }
    }
}

pub(crate) fn render_etf_panel(out: &mut String, state: &TradingState) {
    render_etf_panel_with_policy(out, state, RenderPolicy::Rich);
}

pub(crate) fn render_etf_panel_with_policy(
    out: &mut String,
    state: &TradingState,
    policy: RenderPolicy,
) {
    super::final_report::section_header(out, "ETF Valuation Snapshot");

    let Some(dv) = state.derived_valuation() else {
        let _ = writeln!(out, "Not computed for this run.");
        return;
    };

    let etf = match &dv.scenario {
        ScenarioValuation::Etf(e) => e,
        ScenarioValuation::NotAssessed { reason } => {
            let _ = writeln!(out, "ETF valuation    Not assessed");
            let _ = writeln!(out, "Reason           {reason}");
            return;
        }
        other => {
            let _ = writeln!(out, "Unexpected valuation variant for ETF panel: {other:?}");
            return;
        }
    };

    render_premium_block(out, etf, state, policy);
    match etf.composition.as_ref() {
        Some(comp) => render_composition_block(out, comp, policy),
        None => {
            let _ = writeln!(
                out,
                "{} Holdings unavailable — N-PORT-P data missing or too stale",
                policy.warn()
            );
        }
    }
    render_cost_block(out, etf);
    render_sector_summary_block(out, etf.composition.as_ref());
    match etf.tracking.as_ref() {
        Some(tr) => render_tracking_block(out, tr, policy),
        None => {
            let _ = writeln!(
                out,
                "{} Tracking error skipped — benchmark not resolved",
                policy.warn()
            );
        }
    }
    render_trust_signals(out, etf, policy);
    if let Some(gex) = etf.options_gex.as_ref() {
        render_dealer_positioning_block(out, gex);
    } else {
        let _ = writeln!(
            out,
            "{} Dealer positioning skipped — no usable options-derived overlay available",
            policy.warn()
        );
    }
}

fn render_premium_block(
    out: &mut String,
    etf: &EtfValuation,
    state: &TradingState,
    policy: RenderPolicy,
) {
    let _ = writeln!(out, "Analysis Pack    ETF Baseline");
    let _ = writeln!(out, "Symbol           {}", state.asset_symbol);
    if let Some(cat) = etf.category.as_deref() {
        let _ = writeln!(out, "Category         {cat}");
    }
    let _ = writeln!(out, "Market Price     ${:.2}", etf.premium.market_price);
    match etf.premium.nav {
        Some(nav) => {
            let _ = writeln!(
                out,
                "NAV              ${nav:.2}   (as of {})",
                etf.premium.as_of.format("%H:%M UTC")
            );
        }
        None => {
            let _ = writeln!(out, "NAV              unavailable");
        }
    }
    match etf.premium.premium_pct {
        Some(p) => {
            let _ = writeln!(
                out,
                "Premium          {p:+.2}%   Band  {} {}",
                policy.band_marker(),
                band_label(&etf.premium.category_band),
            );
        }
        None => {
            let _ = writeln!(out, "Premium          unavailable   Band  Unknown");
            let _ = writeln!(
                out,
                "{} Premium band unavailable — NAV missing from ETF quote payload",
                policy.warn()
            );
        }
    }
    match (
        etf.premium.bid,
        etf.premium.ask,
        etf.premium.bid_ask_spread_pct,
    ) {
        (Some(b), Some(a), Some(s)) => {
            let _ = writeln!(out, "Bid/Ask          ${b:.2}/${a:.2}   Spread {s:.3}%");
        }
        _ => {
            let _ = writeln!(out, "Bid/Ask          unavailable   Spread unavailable");
            let _ = writeln!(
                out,
                "{} Noise-floor check skipped — bid/ask unavailable",
                policy.warn()
            );
        }
    }
    if let Some(lev) = etf
        .leverage_factor
        .filter(|&l| (l - 1.0).abs() > f64::EPSILON)
    {
        let _ = writeln!(out, "Leverage         {lev:+.1}x");
    }
}

fn render_composition_block(out: &mut String, comp: &EtfComposition, policy: RenderPolicy) {
    let rule = policy.rule_char();
    let _ = writeln!(
        out,
        "{rule}{rule}{rule} COMPOSITION  (filing {}, {} days old) {rule}{rule}{rule}{rule}",
        comp.holdings_filing_date, comp.holdings_age_days,
    );
    let _ = writeln!(out, "Top-10 weight    {:.1}%", comp.top10_concentration_pct);
    if !comp.top_holdings.is_empty() {
        let pieces: Vec<String> = comp
            .top_holdings
            .iter()
            .take(5)
            .enumerate()
            .map(|(idx, h)| {
                let label = h.ticker.as_deref().unwrap_or(&h.name);
                format!("#{} {label}  {:.1}%", idx + 1, h.weight_pct)
            })
            .collect();
        let _ = writeln!(out, "{}", pieces.join(policy.holdings_separator()));
    }
    if comp.holdings_age_days > 90 {
        let _ = writeln!(
            out,
            "{} Holdings staleness — {} days old",
            policy.warn(),
            comp.holdings_age_days
        );
    }
}

fn render_tracking_block(out: &mut String, tr: &TrackingError, policy: RenderPolicy) {
    let rule = policy.rule_char();
    let _ = writeln!(
        out,
        "{rule}{rule}{rule} TRACKING vs {} {rule}{rule}{rule}{rule}",
        tr.benchmark_symbol
    );
    let _ = writeln!(
        out,
        "90d TE: {:.2}% annualised   |   1y TE: {:.2}% annualised  (n={} days)",
        tr.te_pct_90d, tr.te_pct_1y, tr.sample_days
    );
}

fn render_cost_block(out: &mut String, etf: &EtfValuation) {
    let Some(comp) = etf.composition.as_ref() else {
        return;
    };
    if comp.expense_ratio_pct.is_none()
        && comp.distribution_yield_ttm_pct.is_none()
        && comp.aum_usd.is_none()
    {
        return;
    }
    if let Some(er) = comp.expense_ratio_pct {
        let _ = writeln!(out, "Expense ratio    {:.2}%", er * 100.0);
    }
    if let Some(yld) = comp.distribution_yield_ttm_pct {
        let _ = writeln!(out, "Distribution TTM {:.2}%", yld * 100.0);
    }
    if let Some(aum) = comp.aum_usd {
        let _ = writeln!(out, "AUM              ${:.2}B", aum / 1e9);
    }
}

fn render_sector_summary_block(out: &mut String, comp: Option<&EtfComposition>) {
    let Some(comp) = comp else {
        return;
    };
    if comp.sector_weights.is_empty() {
        return;
    }
    let mut sorted: Vec<_> = comp.sector_weights.iter().collect();
    sorted.sort_by(|a, b| {
        b.weight_pct
            .partial_cmp(&a.weight_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top: Vec<String> = sorted
        .iter()
        .take(2)
        .map(|s| format!("{}: {:.1}%", s.sector, s.weight_pct))
        .collect();
    let _ = writeln!(out, "Sector tilt      {}", top.join("  |  "));
}

fn render_trust_signals(out: &mut String, etf: &EtfValuation, policy: RenderPolicy) {
    let rule = policy.rule_char();
    let _ = writeln!(
        out,
        "{rule}{rule}{rule} TRUST SIGNALS {rule}{rule}{rule}{rule}"
    );
    let _ = writeln!(
        out,
        "NAV: {}  Bid/Ask: {}  Holdings: {}  Benchmark: {}",
        policy.check(etf.flags.nav_available),
        policy.check(etf.flags.bid_ask_available),
        policy.check(etf.flags.holdings_present),
        policy.check(etf.flags.benchmark_resolved),
    );
    let _ = writeln!(
        out,
        "Holdings age band: {}",
        age_band_label(etf.flags.holdings_age_band)
    );
}

fn band_label(band: &PremiumBand) -> &'static str {
    match band {
        PremiumBand::Normal => "Normal",
        PremiumBand::Elevated => "Elevated",
        PremiumBand::Extreme => "Extreme",
        PremiumBand::Unknown => "Unknown",
    }
}

fn age_band_label(b: HoldingsAgeBand) -> &'static str {
    match b {
        HoldingsAgeBand::Fresh => "Fresh",
        HoldingsAgeBand::Aging => "Aging",
        HoldingsAgeBand::Stale => "Stale",
        HoldingsAgeBand::Unknown => "Unknown",
    }
}

fn render_dealer_positioning_block(out: &mut String, gex: &GexSummary) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  ─── DEALER POSITIONING ──────────────────────────────────────────────"
    );
    let _ = writeln!(out, "  Near-term  ({})", gex.near_term_expiration);

    let summary_line = build_dealer_summary_line(gex);
    let _ = writeln!(out, "    Summary         {summary_line}");
    let _ = writeln!(
        out,
        "    Net GEX/1%      {net}    Gross GEX/1%    {gross}",
        net = format_usd_signed(gex.net_gex_usd_per_1pct_move),
        gross = format_usd_magnitude(gex.gross_gex_usd_per_1pct_move),
    );
    let _ = writeln!(
        out,
        "    Call/Put OI     {cp:.2}      Max-pain        ${mp:.0}",
        cp = gex.call_put_oi_ratio,
        mp = gex.max_pain_strike,
    );

    if !gex.strikes.is_empty() {
        let walls = gex
            .strikes
            .iter()
            .map(format_strike_gex)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "    Gamma walls    {walls}");
    }

    let walls_missing = gex.strikes.is_empty();
    let broad_missing = gex.broad.is_none();
    if walls_missing && broad_missing {
        let _ = writeln!(
            out,
            "    Dealer positioning partial — gamma walls and broad GEX unavailable"
        );
    } else if walls_missing {
        let _ = writeln!(
            out,
            "    Dealer positioning partial — gamma walls unavailable"
        );
    } else if broad_missing {
        let _ = writeln!(
            out,
            "    Dealer positioning partial — broad GEX unavailable"
        );
    }

    if let (Some(v), Some(c)) = (gex.vex_summary.as_ref(), gex.cex_summary.as_ref()) {
        let _ = writeln!(out, "    Secondary sensitivities");
        let _ = writeln!(
            out,
            "      Net VEX/volpt {nv}    Gross VEX       {gv}",
            nv = format_usd_signed(v.net_vex_usd_per_volpt),
            gv = format_usd_magnitude(v.gross_vex_usd_per_volpt),
        );
        let _ = writeln!(
            out,
            "      Net CEX/day   {nc}    Gross CEX       {gc}",
            nc = format_usd_signed(c.net_cex_usd_per_day),
            gc = format_usd_magnitude(c.gross_cex_usd_per_day),
        );
    }

    if let Some(broad) = gex.broad.as_ref() {
        let _ = writeln!(out);
        if broad.expirations_used == broad.expirations_total_considered {
            let _ = writeln!(out, "  All expirations  ({} used)", broad.expirations_used);
        } else {
            let _ = writeln!(
                out,
                "  Partial expirations  ({} used of {})",
                broad.expirations_used, broad.expirations_total_considered
            );
        }
        let _ = writeln!(
            out,
            "    Net GEX/1%      {net}    Gross GEX/1%    {gross}",
            net = format_usd_signed(broad.net_gex_usd_per_1pct_move),
            gross = format_usd_magnitude(broad.gross_gex_usd_per_1pct_move),
        );
    }
}

fn format_strike_gex(s: &StrikeGex) -> String {
    format!(
        "{} @ ${:.0}",
        format_usd_signed(s.net_gex_usd_per_1pct_move),
        s.strike
    )
}

fn format_usd_signed(value: f64) -> String {
    let abs = value.abs();
    let (suffix, scaled) = scale_for_usd(abs);
    let sign = if value >= 0.0 { '+' } else { '-' };
    format!("{sign}${scaled:.2}{suffix}")
}

fn format_usd_magnitude(value: f64) -> String {
    let (suffix, scaled) = scale_for_usd(value.abs());
    format!("${scaled:.2}{suffix}")
}

fn scale_for_usd(value: f64) -> (&'static str, f64) {
    const B: f64 = 1.0e9;
    const M: f64 = 1.0e6;
    const K: f64 = 1.0e3;
    if value >= B {
        ("B", value / B)
    } else if value >= M {
        ("M", value / M)
    } else if value >= K {
        ("K", value / K)
    } else {
        ("", value)
    }
}

fn build_dealer_summary_line(gex: &GexSummary) -> String {
    let regime = if gex.net_gex_usd_per_1pct_move > 0.0 {
        "Dealer hedging likely dampens near-term moves"
    } else if gex.net_gex_usd_per_1pct_move < 0.0 {
        "Dealer hedging likely amplifies near-term moves"
    } else {
        "Dealer hedging is roughly neutral on near-term moves"
    };

    if gex.strikes.is_empty() {
        regime.to_owned()
    } else {
        let mut strikes_sorted: Vec<f64> = gex.strikes.iter().map(|w| w.strike).collect();
        strikes_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let lo = strikes_sorted.first().copied().unwrap_or(0.0);
        let hi = strikes_sorted.last().copied().unwrap_or(0.0);
        if (hi - lo).abs() < f64::EPSILON {
            format!("{regime}; gamma walls cluster near ${lo:.0}")
        } else {
            format!("{regime}; gamma walls cluster near ${lo:.0}-${hi:.0}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, PremiumSnapshot, TradingState,
    };

    fn etf_state_with(etf: EtfValuation) -> TradingState {
        let mut state = TradingState::new("SPY", "2026-05-21");
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::Etf(etf),
        });
        state
    }

    fn minimal_etf() -> EtfValuation {
        EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(621.18),
                market_price: 621.40,
                bid: Some(621.39),
                ask: Some(621.41),
                premium_pct: Some(0.04),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: Utc::now(),
            },
            composition: None,
            tracking: None,
            tracking_status: scorpio_core::state::TrackingStatus::NotResolved,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: Some("Large Blend".into()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability {
                nav_available: true,
                bid_ask_available: true,
                ..EtfDataAvailability::default()
            },
        }
    }

    #[test]
    fn renders_etf_panel_with_full_premium_snapshot() {
        let state = etf_state_with(minimal_etf());
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("ETF Valuation Snapshot"));
        assert!(out.contains("Market Price"));
        assert!(out.contains("Premium"));
        assert!(out.contains("Normal"));
    }

    #[test]
    fn renders_not_assessed_when_quote_unavailable() {
        let mut state = TradingState::new("BOGUS", "2026-05-21");
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::NotAssessed {
                reason: "etf_quote_unavailable".into(),
            },
        });
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("Not assessed"));
        assert!(out.contains("etf_quote_unavailable"));
    }

    #[test]
    fn renders_holdings_unavailable_warning_when_composition_missing() {
        let state = etf_state_with(minimal_etf());
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("Holdings unavailable"));
    }

    #[test]
    fn renders_premium_unknown_when_nav_missing() {
        let mut etf = minimal_etf();
        etf.premium.nav = None;
        etf.premium.premium_pct = None;
        etf.premium.category_band = PremiumBand::Unknown;
        etf.flags.nav_available = false;
        let state = etf_state_with(etf);
        let mut out = String::new();
        render_etf_panel(&mut out, &state);
        assert!(out.contains("NAV              unavailable"));
        assert!(out.contains("Premium band unavailable"));
    }

    #[test]
    fn ascii_policy_replaces_unicode_glyphs() {
        let state = etf_state_with(minimal_etf());
        let mut out = String::new();
        render_etf_panel_with_policy(&mut out, &state, RenderPolicy::Ascii);
        assert!(!out.contains('⚠'), "expected no unicode warn glyph");
        assert!(!out.contains('▲'), "expected no unicode band marker");
        assert!(!out.contains('─'), "expected no unicode rule");
        assert!(
            out.contains("[OK]") || out.contains("[X]"),
            "expected ASCII trust signal"
        );
    }

    #[test]
    fn narrow_policy_separates_holdings_with_newlines() {
        let mut etf = minimal_etf();
        etf.composition = Some(EtfComposition {
            source: scorpio_core::state::EtfCompositionSource::SecNport,
            top_holdings: vec![
                scorpio_core::state::HoldingWeight {
                    cusip: None,
                    ticker: Some("AAPL".into()),
                    name: "Apple".into(),
                    weight_pct: 7.5,
                    value_usd: None,
                },
                scorpio_core::state::HoldingWeight {
                    cusip: None,
                    ticker: Some("MSFT".into()),
                    name: "Microsoft".into(),
                    weight_pct: 6.9,
                    value_usd: None,
                },
            ],
            top10_concentration_pct: 14.4,
            sector_weights: vec![],
            expense_ratio_pct: None,
            aum_usd: None,
            fund_family: None,
            distribution_yield_ttm_pct: None,
            holdings_filing_date: NaiveDate::from_ymd_opt(2026, 4, 30).unwrap(),
            holdings_report_date: None,
            holdings_age_days: 21,
            portfolio_turnover_pct: None,
            inception_date: None,
        });
        let state = etf_state_with(etf);
        let mut out = String::new();
        render_etf_panel_with_policy(&mut out, &state, RenderPolicy::Narrow);
        // Narrow policy uses newline as holdings separator.
        let holdings_line = out
            .lines()
            .find(|l| l.contains("#1 AAPL"))
            .expect("holdings line present");
        // In Narrow policy the two holdings live on different lines.
        assert!(
            !holdings_line.contains("#2 MSFT"),
            "expected one holding per line in narrow policy"
        );
    }
}
