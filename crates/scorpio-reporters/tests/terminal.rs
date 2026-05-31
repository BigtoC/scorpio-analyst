use scorpio_core::state::TradingState;
use scorpio_reporters::terminal::render_final_report;

#[test]
fn render_final_report_keeps_core_sections_for_minimal_state() {
    let state = TradingState::new("AAPL", "2026-04-23");
    let report = render_final_report(&state);

    assert!(report.contains("AAPL"));
    assert!(report.contains("Scenario Valuation"));
    assert!(report.contains("Data Quality and Coverage"));
    assert!(report.contains("Evidence Provenance"));
}

#[test]
fn etf_terminal_renders_dealer_positioning_block_when_gex_present() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary, PremiumBand,
        PremiumSnapshot, ScenarioValuation, StrikeGex,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: Some(620.39),
                ask: Some(620.41),
                premium_pct: Some(0.06),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 2.84e9,
                gross_gex_usd_per_1pct_move: 7.12e9,
                call_put_oi_ratio: 1.31,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![
                    StrikeGex {
                        strike: 625.0,
                        net_gex_usd_per_1pct_move: 1.20e9,
                    },
                    StrikeGex {
                        strike: 615.0,
                        net_gex_usd_per_1pct_move: -0.84e9,
                    },
                    StrikeGex {
                        strike: 630.0,
                        net_gex_usd_per_1pct_move: 0.62e9,
                    },
                ],
                broad: None,
                vex_summary: None,
                cex_summary: None,
            }),
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(
        rendered.contains("DEALER POSITIONING"),
        "header missing: {rendered}"
    );
    assert!(
        rendered.contains("Near-term"),
        "near-term subheader missing: {rendered}"
    );
    assert!(
        rendered.contains("Summary"),
        "summary line missing: {rendered}"
    );
    assert!(
        rendered.contains("Gamma walls"),
        "gamma walls line missing: {rendered}"
    );
    assert!(
        rendered.contains("Max-pain"),
        "max-pain line missing: {rendered}"
    );
    // Stage 2 must NOT show secondary sensitivities or all-expirations rows.
    assert!(
        !rendered.contains("Secondary sensitivities"),
        "Stage 2 must omit VEX/CEX block: {rendered}"
    );
    assert!(
        !rendered.contains("All expirations"),
        "Stage 2 must omit broad GEX line: {rendered}"
    );
}

#[test]
fn etf_terminal_renders_dealer_positioning_absence_when_gex_absent() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
        PremiumSnapshot, ScenarioValuation,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(
        rendered.contains("Dealer positioning skipped")
            && rendered.contains("no usable options-derived overlay"),
        "absence notice must render when options_gex is None: {rendered}"
    );
}

#[test]
fn etf_terminal_emits_partial_data_note_for_missing_walls() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary, PremiumBand,
        PremiumSnapshot, ScenarioValuation,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 1.0e9,
                gross_gex_usd_per_1pct_move: 2.0e9,
                call_put_oi_ratio: 1.0,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![], // walls unavailable
                broad: None,
                vex_summary: None,
                cex_summary: None,
            }),
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(
        rendered.contains("gamma walls unavailable")
            || rendered.contains("gamma walls and broad GEX unavailable"),
        "missing partial-data note: {rendered}"
    );
}

#[test]
fn etf_terminal_renders_degraded_rate_banner_when_rate_unavailable() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
        PremiumSnapshot, ScenarioValuation,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = None;
    state.etf_risk_free_rate_source = None;
    // ETF scenario must be present for the degraded banner to fire — non-ETF
    // runs have rate fields at default None and must not trigger the warning.
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: None,
                market_price: 620.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(
        rendered.contains("⚠ Risk-free rate unavailable")
            && rendered.contains("dealer positioning unavailable"),
        "degraded-rate banner missing: {rendered}"
    );
}

#[test]
fn non_etf_terminal_does_not_show_degraded_rate_banner() {
    // Equity-only state. Both rate fields default to None. The banner must
    // stay silent — non-ETF reports should not be polluted with a warning
    // about a rate that has no meaning for equity analyses.
    let state = TradingState::new("AAPL".to_owned(), "2026-05-27".to_owned());
    let rendered = render_final_report(&state);
    assert!(
        !rendered.contains("Risk-free rate unavailable"),
        "non-ETF report must not show rate-unavailable banner: {rendered}"
    );
    assert!(
        !rendered.contains("dealer positioning unavailable"),
        "non-ETF report must not advertise dealer-positioning state: {rendered}"
    );
}

#[test]
fn etf_terminal_labels_yfinance_irx_rate_source_without_warning() {
    use scorpio_core::state::EtfRiskFreeRateSource;

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = Some(0.0433);
    state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::YFinanceIrx);

    let rendered = render_final_report(&state);
    assert!(rendered.contains("Risk-free rate    yfinance ^IRX"));
    assert!(
        !rendered.contains("Risk-free rate unavailable"),
        "^IRX fallback is a live source, not a hardcoded fallback warning: {rendered}"
    );
}

#[test]
fn etf_terminal_labels_fred_dgs3mo_rate_source_without_warning() {
    use scorpio_core::state::EtfRiskFreeRateSource;

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = Some(0.0427);
    state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FredDgs3Mo);

    let rendered = render_final_report(&state);
    assert!(rendered.contains("Risk-free rate    FRED DGS3MO"));
    assert!(
        !rendered.contains("Risk-free rate unavailable"),
        "FRED success must not show the degraded banner: {rendered}"
    );
}

#[test]
fn etf_terminal_renders_full_dealer_positioning_with_broad_and_secondary() {
    use scorpio_core::state::{
        AssetShape, BroadGex, CexSummary, DerivedValuation, EtfDataAvailability, EtfValuation,
        GexSummary, PremiumBand, PremiumSnapshot, ScenarioValuation, StrikeGex, TradingState,
        VexSummary,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: Some(620.39),
                ask: Some(620.41),
                premium_pct: Some(0.06),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 2.84e9,
                gross_gex_usd_per_1pct_move: 7.12e9,
                call_put_oi_ratio: 1.31,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![StrikeGex {
                    strike: 625.0,
                    net_gex_usd_per_1pct_move: 1.2e9,
                }],
                broad: Some(BroadGex {
                    net_gex_usd_per_1pct_move: 8.4e9,
                    gross_gex_usd_per_1pct_move: 22.1e9,
                    expirations_used: 5,
                    expirations_total_considered: 5,
                }),
                vex_summary: Some(VexSummary {
                    net_vex_usd_per_volpt: -1.2e9,
                    gross_vex_usd_per_volpt: 4.1e9,
                }),
                cex_summary: Some(CexSummary {
                    net_cex_usd_per_day: 0.45e9,
                    gross_cex_usd_per_day: 2.3e9,
                }),
            }),
            category: None,
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("Secondary sensitivities"));
    assert!(rendered.contains("Net VEX/volpt"));
    assert!(rendered.contains("Net CEX/day"));
    assert!(rendered.contains("All expirations  (5 used)"));
}

#[test]
fn etf_terminal_uses_partial_expirations_label_when_not_all_used() {
    use scorpio_core::state::{
        AssetShape, BroadGex, DerivedValuation, EtfDataAvailability, EtfValuation, GexSummary,
        PremiumBand, PremiumSnapshot, ScenarioValuation, TradingState,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.0,
                bid: None,
                ask: None,
                premium_pct: None,
                category_band: PremiumBand::Unknown,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 1.0e9,
                gross_gex_usd_per_1pct_move: 2.0e9,
                call_put_oi_ratio: 1.0,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 5, 23).unwrap(),
                strikes: vec![],
                broad: Some(BroadGex {
                    net_gex_usd_per_1pct_move: 3.0e9,
                    gross_gex_usd_per_1pct_move: 5.0e9,
                    expirations_used: 3,
                    expirations_total_considered: 5,
                }),
                vex_summary: None,
                cex_summary: None,
            }),
            category: None,
            leverage_factor: None,
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = scorpio_reporters::terminal::render_final_report(&state);
    assert!(rendered.contains("Partial expirations"));
    assert!(rendered.contains("3 used of 5"));
}

#[test]
fn etf_terminal_renders_profile_source_and_official_benchmark() {
    use scorpio_core::state::{
        AssetShape, BenchmarkSource, DerivedValuation, EtfComposition, EtfCompositionSource,
        EtfDataAvailability, EtfValuation, HoldingWeight, PremiumBand, PremiumSnapshot,
        ScenarioValuation, SectorWeight,
    };

    let mut state = TradingState::new("SOXX".to_owned(), "2026-05-30".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(240.0),
                market_price: 241.0,
                bid: Some(240.9),
                ask: Some(241.1),
                premium_pct: Some(0.42),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.08),
                as_of: chrono::Utc::now(),
            },
            composition: Some(EtfComposition {
                source: EtfCompositionSource::AlphaVantageEtfProfile,
                top_holdings: vec![HoldingWeight {
                    cusip: None,
                    ticker: Some("NVDA".to_owned()),
                    name: "NVIDIA Corp".to_owned(),
                    weight_pct: 8.4,
                    value_usd: None,
                }],
                top10_concentration_pct: 8.4,
                sector_weights: vec![SectorWeight {
                    sector: "Semiconductors".to_owned(),
                    weight_pct: 78.2,
                }],
                expense_ratio_pct: Some(0.0035),
                aum_usd: Some(12_300_000_000.0),
                fund_family: Some("iShares".to_owned()),
                distribution_yield_ttm_pct: Some(0.0061),
                holdings_filing_date: chrono::Utc::now().date_naive(),
                holdings_report_date: None,
                holdings_age_days: 0,
                portfolio_turnover_pct: Some(0.24),
                inception_date: Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap()),
            }),
            official_benchmark_name: Some("NYSE Semiconductor Index".to_owned()),
            official_benchmark_source: Some(BenchmarkSource::SecRiskReturn),
            official_benchmark_metadata_age_days: Some(316),
            options_gex: None,
            category: Some("Technology".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(
        rendered.contains("Composition source") && rendered.contains("Alpha Vantage ETF_PROFILE")
    );
    assert!(
        rendered.contains("Official benchmark") && rendered.contains("NYSE Semiconductor Index")
    );
    assert!(rendered.contains("SEC DERA Risk/Return Summary"));
    assert!(!rendered.contains("Tracking error"));
}

#[test]
fn etf_terminal_strips_control_sequences_from_provider_text() {
    use scorpio_core::state::{
        AssetShape, BenchmarkSource, DerivedValuation, EtfDataAvailability, EtfValuation,
        PremiumBand, PremiumSnapshot, ScenarioValuation,
    };

    let mut state = TradingState::new("SOXX".to_owned(), "2026-05-30".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(240.0),
                market_price: 241.0,
                bid: None,
                ask: None,
                premium_pct: Some(0.42),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: None,
                as_of: chrono::Utc::now(),
            },
            composition: None,
            official_benchmark_name: Some(
                "NYSE\u{001b}[31m Semiconductor\u{0007} Index".to_owned(),
            ),
            official_benchmark_source: Some(BenchmarkSource::SecRiskReturn),
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: Some("Technology".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(!rendered.contains('\u{001b}'));
    assert!(!rendered.contains('\u{0007}'));
    assert!(rendered.contains("NYSE[31m Semiconductor Index"));
}

#[test]
fn etf_terminal_renders_sec_nport_composition_source_with_report_and_filing_dates() {
    use scorpio_core::state::{
        AssetShape, DerivedValuation, EtfComposition, EtfCompositionSource, EtfDataAvailability,
        EtfValuation, HoldingWeight, PremiumBand, PremiumSnapshot, ScenarioValuation,
    };

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-30".to_owned());
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(620.0),
                market_price: 620.4,
                bid: Some(620.3),
                ask: Some(620.5),
                premium_pct: Some(0.06),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.03),
                as_of: chrono::Utc::now(),
            },
            composition: Some(EtfComposition {
                source: EtfCompositionSource::SecNport,
                top_holdings: vec![HoldingWeight {
                    cusip: None,
                    ticker: Some("AAPL".to_owned()),
                    name: "Apple Inc".to_owned(),
                    weight_pct: 7.1,
                    value_usd: None,
                }],
                top10_concentration_pct: 7.1,
                sector_weights: vec![],
                expense_ratio_pct: Some(0.0009),
                aum_usd: None,
                fund_family: None,
                distribution_yield_ttm_pct: None,
                holdings_filing_date: chrono::NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
                holdings_report_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 3, 31).unwrap()),
                holdings_age_days: 60,
                portfolio_turnover_pct: None,
                inception_date: None,
            }),
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let rendered = render_final_report(&state);
    assert!(rendered.contains("Composition source") && rendered.contains("SEC N-PORT"));
    assert!(rendered.contains("Report date") && rendered.contains("2026-03-31"));
    assert!(rendered.contains("Filing date") && rendered.contains("days old"));
}
