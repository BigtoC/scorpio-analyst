//! Property-based serialization round-trip tests for foundational state types.

use proptest::prelude::*;
use scorpio_core::data::adapters::{
    EnrichmentStatus,
    estimates::{ConsensusEvidence, PriceTargetSummary, RecommendationsSummary},
    events::EventNewsEvidence,
};
use scorpio_core::data::traits::options::{OptionsOutcome, OptionsSnapshot};
use scorpio_core::state::*;
use scorpio_core::workflow::{SnapshotPhase, SnapshotStore};
use serde::Deserialize;
use tempfile::tempdir;

fn arb_enrichment_status() -> impl Strategy<Value = EnrichmentStatus> {
    prop_oneof![
        Just(EnrichmentStatus::Disabled),
        Just(EnrichmentStatus::NotConfigured),
        Just(EnrichmentStatus::NotAvailable),
        "[a-z ]{5,30}".prop_map(EnrichmentStatus::FetchFailed),
        Just(EnrichmentStatus::Available),
    ]
}

fn arb_event_news_evidence() -> impl Strategy<Value = EventNewsEvidence> {
    (
        "[A-Z]{1,5}",
        "2024-0[1-9]-[0-2][0-9]T[0-2][0-9]:[0-5][0-9]:[0-5][0-9]Z",
        "[a-z_]{5,20}",
        "[A-Za-z ]{5,40}",
        proptest::option::of(prop::sample::select(vec![
            "positive".to_owned(),
            "negative".to_owned(),
            "neutral".to_owned(),
        ])),
    )
        .prop_map(|(symbol, event_timestamp, event_type, headline, impact)| {
            EventNewsEvidence {
                symbol,
                event_timestamp,
                event_type,
                headline,
                impact,
            }
        })
}

fn arb_price_target_summary() -> impl Strategy<Value = PriceTargetSummary> {
    (
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        proptest::option::of(0u32..200u32),
    )
        .prop_map(|(mean, high, low, analyst_count)| PriceTargetSummary {
            mean,
            high,
            low,
            analyst_count,
        })
}

fn arb_recommendations_summary() -> impl Strategy<Value = RecommendationsSummary> {
    (
        proptest::option::of(0u32..50u32),
        proptest::option::of(0u32..50u32),
        proptest::option::of(0u32..50u32),
        proptest::option::of(0u32..50u32),
        proptest::option::of(0u32..50u32),
    )
        .prop_map(
            |(strong_buy, buy, hold, sell, strong_sell)| RecommendationsSummary {
                strong_buy,
                buy,
                hold,
                sell,
                strong_sell,
            },
        )
}

fn arb_consensus_evidence() -> impl Strategy<Value = ConsensusEvidence> {
    (
        "[A-Z]{1,5}",
        arb_opt_f64(),
        arb_opt_f64(),
        proptest::option::of(0u32..100u32),
        "2024-0[1-9]-[0-2][0-9]",
        proptest::option::of(arb_price_target_summary()),
        proptest::option::of(arb_recommendations_summary()),
        0u32..10u32,
    )
        .prop_map(
            |(
                symbol,
                eps_estimate,
                revenue_estimate_m,
                analyst_count,
                as_of_date,
                price_target,
                recommendations,
                consecutive_provider_degraded_cycles,
            )| ConsensusEvidence {
                symbol,
                eps_estimate,
                revenue_estimate_m,
                analyst_count,
                as_of_date,
                price_target,
                recommendations,
                consecutive_provider_degraded_cycles,
            },
        )
}

fn arb_enrichment_state<T: Strategy>(payload: T) -> impl Strategy<Value = EnrichmentState<T::Value>>
where
    T::Value: Clone,
{
    (arb_enrichment_status(), proptest::option::of(payload))
        .prop_map(|(status, payload)| EnrichmentState { status, payload })
}

// ── Helpers ────────────────────────────────────────────────────────

/// Generate f64 values that survive a JSON text round-trip without precision loss.
fn arb_f64() -> impl Strategy<Value = f64> {
    -1e10f64..1e10f64
}

fn arb_opt_f64() -> impl Strategy<Value = Option<f64>> {
    proptest::option::of(arb_f64())
}

// ── Proptest strategies ────────────────────────────────────────────

fn arb_insider_transaction() -> impl Strategy<Value = InsiderTransaction> {
    (
        "[a-zA-Z ]{1,20}",
        arb_f64(),
        "2024-0[1-9]-[0-2][0-9]",
        prop::sample::select(vec![
            TransactionType::S,
            TransactionType::P,
            TransactionType::Other,
        ]),
    )
        .prop_map(|(name, share_change, transaction_date, transaction_type)| {
            InsiderTransaction {
                name,
                share_change,
                transaction_date,
                transaction_type,
            }
        })
}

fn arb_fundamental_data() -> impl Strategy<Value = FundamentalData> {
    (
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        proptest::collection::vec(arb_insider_transaction(), 0..3),
        "[a-z ]{0,40}",
    )
        .prop_map(
            |(
                revenue_growth_pct,
                pe_ratio,
                eps,
                current_ratio,
                debt_to_equity,
                gross_margin,
                net_income,
                insider_transactions,
                summary,
            )| {
                FundamentalData {
                    revenue_growth_pct,
                    pe_ratio,
                    eps,
                    current_ratio,
                    debt_to_equity,
                    gross_margin,
                    net_income,
                    insider_transactions,
                    summary,
                }
            },
        )
}

fn arb_macd_values() -> impl Strategy<Value = MacdValues> {
    (arb_f64(), arb_f64(), arb_f64()).prop_map(|(macd_line, signal_line, histogram)| MacdValues {
        macd_line,
        signal_line,
        histogram,
    })
}

fn arb_technical_options_context() -> impl Strategy<Value = Option<TechnicalOptionsContext>> {
    prop_oneof![
        Just(None),
        "[a-z ]{0,40}".prop_map(|reason| Some(TechnicalOptionsContext::FetchFailed { reason })),
        Just(Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::HistoricalRun,
        })),
        Just(Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::NoListedInstrument,
        })),
        Just(Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::SparseChain,
        })),
        Just(Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::MissingSpot,
        })),
        Just(Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(OptionsSnapshot {
                spot_price: 180.0,
                atm_iv: 0.28,
                iv_term_structure: vec![],
                put_call_volume_ratio: 1.1,
                put_call_oi_ratio: 1.0,
                max_pain_strike: 180.0,
                near_term_expiration: "2026-01-17".to_owned(),
                near_term_strikes: vec![],
                all_expirations: vec![],
            }),
        })),
    ]
}

fn arb_technical_data() -> impl Strategy<Value = TechnicalData> {
    (
        arb_opt_f64(),
        proptest::option::of(arb_macd_values()),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        arb_opt_f64(),
        (
            arb_opt_f64(),
            arb_opt_f64(),
            arb_opt_f64(),
            "[a-z ]{0,30}",
            proptest::option::of("[A-Za-z0-9 :;./%-]{0,80}"),
            arb_technical_options_context(),
        ),
    )
        .prop_map(
            |(
                rsi,
                macd,
                atr,
                sma_20,
                sma_50,
                ema_12,
                ema_26,
                bollinger_upper,
                bollinger_lower,
                (
                    support_level,
                    resistance_level,
                    volume_avg,
                    summary,
                    options_summary,
                    options_context,
                ),
            )| {
                TechnicalData {
                    rsi,
                    macd,
                    atr,
                    sma_20,
                    sma_50,
                    ema_12,
                    ema_26,
                    bollinger_upper,
                    bollinger_lower,
                    support_level,
                    resistance_level,
                    volume_avg,
                    summary,
                    options_summary,
                    options_context,
                }
            },
        )
}

fn arb_sentiment_source() -> impl Strategy<Value = SentimentSource> {
    ("[a-z]{3,10}", arb_f64(), 0..10_000u64).prop_map(|(source_name, score, sample_size)| {
        SentimentSource {
            source_name,
            score,
            sample_size,
        }
    })
}

fn arb_engagement_peak() -> impl Strategy<Value = EngagementPeak> {
    (
        "2024-0[1-9]-[0-2][0-9]T[01][0-9]:[0-5][0-9]",
        prop::sample::select(vec!["twitter", "reddit", "stocktwits"]),
        arb_f64(),
    )
        .prop_map(|(timestamp, platform, intensity)| EngagementPeak {
            timestamp,
            platform: platform.to_owned(),
            intensity,
        })
}

fn arb_sentiment_data() -> impl Strategy<Value = SentimentData> {
    (
        arb_f64(),
        proptest::collection::vec(arb_sentiment_source(), 0..3),
        proptest::collection::vec(arb_engagement_peak(), 0..3),
        "[a-z ]{0,30}",
    )
        .prop_map(
            |(overall_score, source_breakdown, engagement_peaks, summary)| SentimentData {
                overall_score,
                source_breakdown,
                engagement_peaks,
                summary,
            },
        )
}

fn arb_news_article() -> impl Strategy<Value = NewsArticle> {
    (
        "[A-Za-z ]{5,30}",
        "[a-z]{3,10}",
        "2024-0[1-9]-[0-2][0-9]",
        arb_opt_f64(),
        "[a-z ]{0,40}",
        proptest::option::of("https://[a-z]{3,10}\\.example\\.com/[a-z0-9-]{1,20}"),
    )
        .prop_map(
            |(title, source, published_at, relevance_score, snippet, url)| NewsArticle {
                title,
                source,
                published_at,
                relevance_score,
                snippet,
                url,
            },
        )
}

fn arb_macro_event() -> impl Strategy<Value = MacroEvent> {
    (
        "[a-z ]{5,20}",
        prop::sample::select(vec![
            ImpactDirection::Positive,
            ImpactDirection::Negative,
            ImpactDirection::Neutral,
            ImpactDirection::Mixed,
            ImpactDirection::Uncertain,
        ]),
        arb_f64(),
    )
        .prop_map(|(event, impact_direction, confidence)| MacroEvent {
            event,
            impact_direction,
            confidence,
        })
}

fn arb_news_data() -> impl Strategy<Value = NewsData> {
    (
        proptest::collection::vec(arb_news_article(), 0..3),
        proptest::collection::vec(arb_macro_event(), 0..3),
        "[a-z ]{0,30}",
    )
        .prop_map(|(articles, macro_events, summary)| NewsData {
            articles,
            macro_events,
            summary,
        })
}

fn arb_trade_action() -> impl Strategy<Value = TradeAction> {
    prop::sample::select(vec![TradeAction::Buy, TradeAction::Sell, TradeAction::Hold])
}

fn arb_scenario_valuation() -> impl Strategy<Value = ScenarioValuation> {
    prop_oneof![
        "[a-z_]{3,30}".prop_map(|reason| ScenarioValuation::NotAssessed { reason }),
        Just(ScenarioValuation::CorporateEquity(
            CorporateEquityValuation {
                dcf: None,
                ev_ebitda: None,
                forward_pe: None,
                peg: None,
            }
        )),
    ]
}

fn arb_trade_proposal() -> impl Strategy<Value = TradeProposal> {
    (
        arb_trade_action(),
        arb_f64(),
        arb_f64(),
        arb_f64(),
        "[a-z ]{5,40}",
        proptest::option::of(arb_scenario_valuation()),
    )
        .prop_map(
            |(action, target_price, stop_loss, confidence, rationale, scenario_valuation)| {
                TradeProposal {
                    action,
                    target_price,
                    stop_loss,
                    confidence,
                    rationale,
                    valuation_assessment: None,
                    scenario_valuation,
                }
            },
        )
}

fn arb_risk_level() -> impl Strategy<Value = RiskLevel> {
    prop::sample::select(vec![
        RiskLevel::Aggressive,
        RiskLevel::Neutral,
        RiskLevel::Conservative,
    ])
}

fn arb_risk_report() -> impl Strategy<Value = RiskReport> {
    (
        arb_risk_level(),
        "[a-z ]{5,30}",
        proptest::collection::vec("[a-z ]{3,15}", 0..3),
        proptest::bool::ANY,
    )
        .prop_map(
            |(risk_level, assessment, recommended_adjustments, flags_violation)| RiskReport {
                risk_level,
                assessment,
                recommended_adjustments,
                flags_violation,
            },
        )
}

fn arb_decision() -> impl Strategy<Value = Decision> {
    prop::sample::select(vec![Decision::Approved, Decision::Rejected])
}

fn arb_execution_status() -> impl Strategy<Value = ExecutionStatus> {
    (
        arb_decision(),
        arb_trade_action(),
        "[a-z ]{5,30}",
        "2024-0[1-9]-[0-2][0-9]",
    )
        .prop_map(
            |(decision, action, rationale, decided_at)| ExecutionStatus {
                decision,
                action,
                rationale,
                decided_at,
                entry_guidance: None,
                suggested_position: None,
            },
        )
}

fn arb_debate_message() -> impl Strategy<Value = DebateMessage> {
    ("[a-z]{3,10}", "[a-z ]{5,40}").prop_map(|(role, content)| DebateMessage { role, content })
}

fn arb_agent_token_usage() -> impl Strategy<Value = AgentTokenUsage> {
    (
        "[a-z_]{3,15}",
        "[a-z0-9-]{3,15}",
        any::<bool>(),
        0..10_000u64,
        0..10_000u64,
        0..20_000u64,
        0..5_000u64,
        0..60_000u64,
    )
        .prop_map(
            |(
                agent_name,
                model_id,
                token_counts_available,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                latency_ms,
                rate_limit_wait_ms,
            )| {
                AgentTokenUsage {
                    agent_name,
                    model_id,
                    token_counts_available,
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    latency_ms,
                    rate_limit_wait_ms,
                }
            },
        )
}

fn arb_phase_token_usage() -> impl Strategy<Value = PhaseTokenUsage> {
    (
        "[a-z_]{3,15}",
        proptest::collection::vec(arb_agent_token_usage(), 0..3),
        0..50_000u64,
        0..50_000u64,
        0..100_000u64,
        0..30_000u64,
    )
        .prop_map(
            |(
                phase_name,
                agent_usage,
                phase_prompt_tokens,
                phase_completion_tokens,
                phase_total_tokens,
                phase_duration_ms,
            )| {
                PhaseTokenUsage {
                    phase_name,
                    agent_usage,
                    phase_prompt_tokens,
                    phase_completion_tokens,
                    phase_total_tokens,
                    phase_duration_ms,
                }
            },
        )
}

fn arb_token_usage_tracker() -> impl Strategy<Value = TokenUsageTracker> {
    (
        proptest::collection::vec(arb_phase_token_usage(), 0..5),
        0..500_000u64,
        0..500_000u64,
        0..1_000_000u64,
    )
        .prop_map(
            |(phase_usage, total_prompt_tokens, total_completion_tokens, total_tokens)| {
                TokenUsageTracker {
                    phase_usage,
                    total_prompt_tokens,
                    total_completion_tokens,
                    total_tokens,
                }
            },
        )
}

fn arb_evidence_source() -> impl Strategy<Value = EvidenceSource> {
    (
        "[a-z]{3,12}",
        proptest::collection::vec("[a-z_]{3,20}", 0..3),
        1u32..=9u32,
        1u32..=28u32,
        0u32..=23u32,
        0u32..=59u32,
        0u32..=59u32,
    )
        .prop_map(|(provider, datasets, month, day, hour, minute, second)| {
            let fetched_at = format!("2024-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
            EvidenceSource {
                provider,
                datasets,
                fetched_at: chrono::DateTime::parse_from_rfc3339(&fetched_at)
                    .expect("valid rfc3339")
                    .with_timezone(&chrono::Utc),
                effective_at: None,
                url: None,
                citation: None,
            }
        })
}

fn arb_evidence_record<T: Strategy>(
    kind: EvidenceKind,
    payload: T,
) -> impl Strategy<Value = EvidenceRecord<T::Value>>
where
    T::Value: Clone,
{
    (
        payload,
        proptest::collection::vec(arb_evidence_source(), 0..2),
    )
        .prop_map(move |(payload, sources)| EvidenceRecord {
            kind: kind.clone(),
            payload,
            sources,
            quality_flags: vec![],
        })
}

fn arb_data_coverage_report() -> impl Strategy<Value = DataCoverageReport> {
    proptest::collection::vec(
        prop::sample::select(vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ]),
        0..4,
    )
    .prop_map(|missing_inputs| DataCoverageReport {
        required_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
        missing_inputs,
    })
}

fn arb_provenance_summary() -> impl Strategy<Value = ProvenanceSummary> {
    proptest::collection::vec("[a-z]{3,12}", 0..4)
        .prop_map(|providers_used| ProvenanceSummary { providers_used })
}

fn arb_thesis_memory() -> impl Strategy<Value = ThesisMemory> {
    (
        "[A-Z]{1,5}",
        prop::sample::select(vec!["Buy".to_owned(), "Sell".to_owned(), "Hold".to_owned()]),
        prop::sample::select(vec!["Approved".to_owned(), "Rejected".to_owned()]),
        "[a-z ]{5,40}",
        "[a-z]{8,12}",
        "2024-0[1-9]-[0-2][0-9]",
    )
        .prop_map(
            |(symbol, action, decision, rationale, execution_id, target_date)| ThesisMemory {
                symbol,
                action,
                decision,
                rationale,
                summary: None,
                execution_id,
                target_date,
                captured_at: chrono::DateTime::parse_from_rfc3339("2026-01-15T10:00:00Z")
                    .expect("valid rfc3339")
                    .with_timezone(&chrono::Utc),
            },
        )
}

fn arb_trading_state() -> impl Strategy<Value = TradingState> {
    (
        "[A-Z]{1,5}",
        "2024-0[1-9]-[0-2][0-9]",
        (
            proptest::option::of(arb_fundamental_data()),
            proptest::option::of(arb_technical_data()),
            proptest::option::of(arb_sentiment_data()),
            proptest::option::of(arb_news_data()),
        ),
        (
            proptest::option::of(arb_evidence_record(
                EvidenceKind::Fundamental,
                arb_fundamental_data(),
            )),
            proptest::option::of(arb_evidence_record(
                EvidenceKind::Technical,
                arb_technical_data(),
            )),
            proptest::option::of(arb_evidence_record(
                EvidenceKind::Sentiment,
                arb_sentiment_data(),
            )),
            proptest::option::of(arb_evidence_record(EvidenceKind::News, arb_news_data())),
            proptest::option::of(arb_data_coverage_report()),
            proptest::option::of(arb_provenance_summary()),
        ),
        (
            proptest::collection::vec(arb_debate_message(), 0..4),
            proptest::option::of("[a-z ]{5,30}"),
        ),
        (
            proptest::option::of(arb_trade_proposal()),
            proptest::collection::vec(arb_debate_message(), 0..4),
            proptest::option::of(arb_risk_report()),
            proptest::option::of(arb_risk_report()),
            proptest::option::of(arb_risk_report()),
            proptest::option::of(arb_execution_status()),
            arb_token_usage_tracker(),
        ),
        (
            proptest::option::of(arb_thesis_memory()),
            proptest::option::of(arb_thesis_memory()),
        ),
        (
            arb_enrichment_state(proptest::collection::vec(arb_event_news_evidence(), 0..3)),
            arb_enrichment_state(arb_consensus_evidence()),
        ),
    )
        .prop_map(
            |(
                asset_symbol,
                target_date,
                (fundamental_metrics, technical_indicators, market_sentiment, macro_news),
                (
                    evidence_fundamental,
                    evidence_technical,
                    evidence_sentiment,
                    evidence_news,
                    data_coverage,
                    provenance_summary,
                ),
                (debate_history, consensus_summary),
                (
                    trader_proposal,
                    risk_discussion_history,
                    aggressive_risk_report,
                    neutral_risk_report,
                    conservative_risk_report,
                    final_execution_status,
                    token_usage,
                ),
                (prior_thesis, current_thesis),
                (enrichment_event_news, enrichment_consensus),
            )| {
                TradingState {
                    execution_id: uuid::Uuid::new_v4(),
                    asset_symbol,
                    symbol: None,
                    target_date,
                    current_price: None,
                    equity: Some(EquityState {
                        fundamental_metrics,
                        technical_indicators,
                        market_sentiment,
                        macro_news,
                        evidence_fundamental,
                        evidence_technical,
                        evidence_sentiment,
                        evidence_news,
                        market_volatility: None,
                        derived_valuation: None,
                    }),
                    crypto: None,
                    enrichment_event_news,
                    enrichment_consensus,
                    enrichment_catalysts: Default::default(),
                    yfinance_info: None,
                    data_coverage,
                    provenance_summary,
                    debate_history,
                    consensus_summary,
                    trader_proposal,
                    risk_discussion_history,
                    aggressive_risk_report,
                    neutral_risk_report,
                    conservative_risk_report,
                    final_execution_status,
                    prior_thesis,
                    current_thesis,
                    token_usage,
                    analysis_pack_name: None,
                    analysis_runtime_policy: None,
                    etf_routing_fallback_reason: None,
                    etf_risk_free_rate: None,
                    etf_risk_free_rate_source: None,
                    audit_status: scorpio_core::state::auditor::AuditStatus::Disabled,
                    audit_report: None,
                }
            },
        )
}

// ── Round-trip tests ───────────────────────────────────────────────
//
// Property: after one normalize step (serialize → deserialize), subsequent
// roundtrips are **idempotent** — proving serde pipelines are stable and no
// data is silently lost or mutated between encode/decode cycles.
//
// We allow the first serialize→deserialize to "normalize" floating-point
// representations (e.g., `4118174664.6630635` → `4118174664.663063`), but
// once normalized, additional roundtrips must produce identical results.

fn assert_json_idempotent<T>(val: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json1 = serde_json::to_string(val).expect("serialize");
    let back1: T = serde_json::from_str(&json1).expect("deserialize");
    let json2 = serde_json::to_string(&back1).expect("re-serialize");
    let back2: T = serde_json::from_str(&json2).expect("re-deserialize");
    assert_eq!(
        json2,
        serde_json::to_string(&back2).expect("third serialize"),
        "serialization is not stable after normalization"
    );
    assert_eq!(
        back1, back2,
        "deserialized values differ after normalization"
    );
}

#[test]
fn trading_state_json_roundtrip() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            proptest!(|(state in arb_trading_state())| {
                assert_json_idempotent(&state);
            });
        })
        .expect("failed to spawn thread with larger stack")
        .join()
        .expect("test thread panicked");
}

proptest! {
    #[test]
    fn token_usage_tracker_json_roundtrip(tracker in arb_token_usage_tracker()) {
        assert_json_idempotent(&tracker);
    }

    #[test]
    fn fundamental_data_json_roundtrip(data in arb_fundamental_data()) {
        assert_json_idempotent(&data);
    }

    #[test]
    fn trade_proposal_json_roundtrip(proposal in arb_trade_proposal()) {
        assert_json_idempotent(&proposal);
    }

    #[test]
    fn scenario_valuation_json_roundtrip(val in arb_scenario_valuation()) {
        assert_json_idempotent(&val);
    }

    #[test]
    fn risk_report_json_roundtrip(report in arb_risk_report()) {
        assert_json_idempotent(&report);
    }

    #[test]
    fn thesis_memory_json_roundtrip(thesis in arb_thesis_memory()) {
        assert_json_idempotent(&thesis);
    }
}

// ── Backward-compatibility tests ───────────────────────────────────
//
// These verify that JSON snapshots produced *before* Chunk 1 (which added
// `scenario_valuation` on TradeProposal and `derived_valuation` on
// TradingState) still deserialize cleanly.  New optional fields should
// silently default to `None` rather than causing a parse error.

#[test]
fn trade_proposal_without_scenario_valuation_deserializes_as_none() {
    // Simulates a JSON snapshot produced before `scenario_valuation` was added.
    let json = r#"{"action":"Buy","target_price":185.5,"stop_loss":178.0,"confidence":0.8,"rationale":"Growth outlook"}"#;
    let proposal: TradeProposal =
        serde_json::from_str(json).expect("old snapshot must deserialize");
    assert!(proposal.scenario_valuation.is_none());
    assert_eq!(proposal.action, TradeAction::Buy);
}

#[test]
fn trading_state_without_derived_valuation_deserializes_as_none() {
    // Build a valid TradingState, serialize it, remove the new field added in
    // Chunk 1, then verify it still deserializes cleanly (simulating a
    // pre-Chunk-1 snapshot stored in SQLite before the field existed).
    let state = TradingState::new("AAPL", "2026-03-15");
    let mut json: serde_json::Value = serde_json::to_value(&state).expect("serialize");
    json.as_object_mut()
        .expect("json is object")
        .remove("derived_valuation");
    let back: TradingState = serde_json::from_value(json).expect("old snapshot must deserialize");
    assert!(back.derived_valuation().is_none());
    assert_eq!(back.asset_symbol, "AAPL");
}

#[test]
fn trading_state_with_legacy_null_enrichment_fields_deserializes_to_default_state() {
    let mut json: serde_json::Value =
        serde_json::to_value(TradingState::new("AAPL", "2026-03-15")).expect("serialize");
    let object = json.as_object_mut().expect("json is object");
    object.insert("enrichment_event_news".to_owned(), serde_json::Value::Null);
    object.insert("enrichment_consensus".to_owned(), serde_json::Value::Null);

    let back: TradingState =
        serde_json::from_value(json).expect("legacy null snapshot must deserialize");
    assert_eq!(
        back.enrichment_event_news.status,
        EnrichmentStatus::NotConfigured
    );
    assert!(back.enrichment_event_news.payload.is_none());
    assert_eq!(
        back.enrichment_consensus.status,
        EnrichmentStatus::NotConfigured
    );
    assert!(back.enrichment_consensus.payload.is_none());
}

#[test]
fn trading_state_with_legacy_payload_enrichment_fields_deserializes_as_available() {
    let mut json: serde_json::Value =
        serde_json::to_value(TradingState::new("AAPL", "2026-03-15")).expect("serialize");
    let object = json.as_object_mut().expect("json is object");
    object.insert(
        "enrichment_event_news".to_owned(),
        serde_json::json!([
            {
                "symbol": "AAPL",
                "event_timestamp": "2026-03-14T12:00:00Z",
                "event_type": "guidance_update",
                "headline": "Apple raises guidance",
                "impact": "positive"
            }
        ]),
    );
    object.insert(
        "enrichment_consensus".to_owned(),
        serde_json::json!({
            "symbol": "AAPL",
            "eps_estimate": 2.5,
            "revenue_estimate_m": 95000.0,
            "analyst_count": 35,
            "as_of_date": "2026-03-15"
        }),
    );

    let back: TradingState =
        serde_json::from_value(json).expect("legacy payload snapshot must deserialize");
    assert_eq!(
        back.enrichment_event_news.status,
        EnrichmentStatus::Available
    );
    assert_eq!(
        back.enrichment_event_news.payload.as_ref().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        back.enrichment_consensus.status,
        EnrichmentStatus::Available
    );
    assert_eq!(
        back.enrichment_consensus
            .payload
            .as_ref()
            .and_then(|payload| payload.analyst_count),
        Some(35)
    );
}

#[test]
fn trading_state_without_analysis_pack_name_deserializes_as_none() {
    let state = TradingState::new("AAPL", "2026-03-15");
    let mut json: serde_json::Value = serde_json::to_value(&state).expect("serialize");
    json.as_object_mut()
        .expect("json is object")
        .remove("analysis_pack_name");
    let back: TradingState = serde_json::from_value(json).expect("old snapshot must deserialize");
    assert!(
        back.analysis_pack_name.is_none(),
        "pre-pack snapshots should have analysis_pack_name = None"
    );
    assert_eq!(back.asset_symbol, "AAPL");
}

#[test]
fn trading_state_with_legacy_root_equity_fields_deserializes_into_equity_substate() {
    let mut json = serde_json::to_value(TradingState::new("AAPL", "2026-03-15"))
        .expect("serialize baseline state");
    let object = json.as_object_mut().expect("json is object");
    object.insert("current_price".to_owned(), serde_json::json!(185.5));
    object.insert(
        "fundamental_metrics".to_owned(),
        serde_json::json!({
            "revenue_growth_pct": 12.5,
            "pe_ratio": 24.5,
            "eps": 6.05,
            "current_ratio": 1.8,
            "debt_to_equity": 0.42,
            "gross_margin": 55.0,
            "net_income": 123456789.0,
            "insider_transactions": [],
            "summary": "legacy fundamentals"
        }),
    );
    object.insert(
        "technical_indicators".to_owned(),
        serde_json::json!({
            "rsi": 55.0,
            "macd": null,
            "atr": 2.1,
            "sma_20": 180.0,
            "sma_50": 175.0,
            "ema_12": 181.0,
            "ema_26": 179.0,
            "bollinger_upper": 190.0,
            "bollinger_lower": 170.0,
            "support_level": 176.0,
            "resistance_level": 188.0,
            "volume_avg": 1234567.0,
            "summary": "legacy technical"
        }),
    );
    object.insert(
        "market_sentiment".to_owned(),
        serde_json::json!({
            "overall_score": 0.72,
            "source_breakdown": [],
            "engagement_peaks": [],
            "summary": "legacy sentiment"
        }),
    );
    object.insert(
        "macro_news".to_owned(),
        serde_json::json!({
            "articles": [],
            "macro_events": [],
            "summary": "legacy news"
        }),
    );
    object.insert(
        "market_volatility".to_owned(),
        serde_json::json!({
            "vix_level": 19.76,
            "vix_sma_20": 22.03,
            "vix_trend": "falling",
            "vix_regime": "normal",
            "fetched_at": "2026-03-15"
        }),
    );
    object.insert(
        "derived_valuation".to_owned(),
        serde_json::json!({
            "asset_shape": "CorporateEquity",
            "scenario": {
                "corporate_equity": {
                    "dcf": {
                        "free_cash_flow": 1000000.0,
                        "discount_rate_pct": 10.0,
                        "intrinsic_value_per_share": 190.0
                    },
                    "ev_ebitda": null,
                    "forward_pe": null,
                    "peg": null
                }
            }
        }),
    );

    let back: TradingState =
        serde_json::from_value(json).expect("legacy root-shaped snapshot must deserialize");

    assert_eq!(back.asset_symbol, "AAPL");
    assert_eq!(
        back.fundamental_metrics().map(|data| data.summary.as_str()),
        Some("legacy fundamentals")
    );
    assert_eq!(
        back.technical_indicators()
            .map(|data| data.summary.as_str()),
        Some("legacy technical")
    );
    assert_eq!(
        back.market_sentiment().map(|data| data.summary.as_str()),
        Some("legacy sentiment")
    );
    assert_eq!(
        back.macro_news().map(|data| data.summary.as_str()),
        Some("legacy news")
    );
    assert_eq!(
        back.market_volatility()
            .map(|data| data.fetched_at.as_str()),
        Some("2026-03-15")
    );
    assert!(back.derived_valuation().is_some());
}

#[test]
fn trading_state_prefers_equity_substate_when_both_new_and_legacy_fields_are_present() {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    state.set_fundamental_metrics(FundamentalData {
        revenue_growth_pct: Some(99.0),
        pe_ratio: None,
        eps: None,
        current_ratio: None,
        debt_to_equity: None,
        gross_margin: None,
        net_income: None,
        insider_transactions: vec![],
        summary: "new equity payload".to_owned(),
    });
    let mut json = serde_json::to_value(state).expect("serialize current state");
    json.as_object_mut().expect("json is object").insert(
        "fundamental_metrics".to_owned(),
        serde_json::json!({
            "revenue_growth_pct": 12.5,
            "pe_ratio": 24.5,
            "eps": 6.05,
            "current_ratio": 1.8,
            "debt_to_equity": 0.42,
            "gross_margin": 55.0,
            "net_income": 123456789.0,
            "insider_transactions": [],
            "summary": "legacy root payload"
        }),
    );

    let back: TradingState =
        serde_json::from_value(json).expect("mixed-shape payload must deserialize");

    assert_eq!(
        back.fundamental_metrics().map(|data| data.summary.as_str()),
        Some("new equity payload")
    );
}

#[test]
fn trading_state_deserializes_old_snapshot_without_audit_report() {
    // Old snapshot JSON predates the audit_status / audit_report fields.
    // Serialize a current state, strip the new fields, verify backward compat.
    let state = TradingState::new("AAPL", "2026-05-10");
    let mut json: serde_json::Value = serde_json::to_value(&state).expect("serialize");
    let obj = json.as_object_mut().expect("json is object");
    obj.remove("audit_status");
    obj.remove("audit_report");
    let back: TradingState = serde_json::from_value(json).expect("old snapshot must deserialize");
    assert_eq!(
        back.audit_status,
        scorpio_core::state::auditor::AuditStatus::Disabled
    );
    assert!(back.audit_report.is_none());
}

// ── ETF variant + Phase-2 routing field backward-compat ────────────────
//
// These three tests cover the ETF baseline pack rollout:
//   - Task 1 added `ScenarioValuation::Etf(EtfValuation)` alongside the
//     existing `CorporateEquity` and `NotAssessed` variants. Old snapshots
//     of the surviving variants must still deserialize after that addition.
//   - Task 12 added `TradingState::etf_routing_fallback_reason: Option<String>`
//     with `#[serde(default)]`. Old snapshots without that field must
//     still deserialize cleanly.
//   - Round-tripping a `TradingState` containing the new `Etf` variant
//     exercises the full encode/decode path through `equity.derived_valuation`
//     so a future serde rename would fail loudly.

#[test]
fn trading_state_with_etf_variant_roundtrips() {
    let mut state = TradingState::new("SPY", "2026-05-21");
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(621.18),
                market_price: 621.40,
                bid: Some(621.39),
                ask: Some(621.41),
                premium_pct: Some(0.04),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            tracking_status: TrackingStatus::NotResolved,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });
    let json = serde_json::to_string(&state).expect("serialize");
    let back: TradingState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(state, back);
}

#[test]
fn legacy_snapshot_without_etf_routing_fallback_field_still_loads() {
    // Generate today's snapshot, then synthesise a "legacy" one by removing
    // the `etf_routing_fallback_reason` field (added in Task 12 with
    // `#[serde(default)]`). Using the round-trip-strip trick keeps this
    // robust against future additive fields on `TradingState`.
    let state = TradingState::new("AAPL", "2026-05-21");
    let mut value: serde_json::Value =
        serde_json::to_value(&state).expect("serialize current state");
    let removed = value
        .as_object_mut()
        .expect("json is object")
        .remove("etf_routing_fallback_reason");
    assert!(
        removed.is_some(),
        "etf_routing_fallback_reason must be present in current snapshots; \
         did the field get renamed?"
    );
    let legacy_json = serde_json::to_string(&value).expect("re-serialize legacy");

    let back: TradingState =
        serde_json::from_str(&legacy_json).expect("legacy snapshot must deserialize");
    assert_eq!(back.asset_symbol, "AAPL");
    assert!(back.etf_routing_fallback_reason.is_none());
}

#[test]
fn legacy_corporate_equity_snapshot_unchanged_after_etf_variant_added() {
    // A pre-ETF `ScenarioValuation::CorporateEquity` snapshot with all
    // metric sub-fields absent must still deserialize after the `Etf`
    // variant was added in Task 1. `CorporateEquityValuation` carries
    // `#[serde(default)]` on every metric, so an empty inner object is
    // the minimal legacy shape we expect to see in stored snapshots.
    let json = r#"{"corporate_equity":{}}"#;
    let back: ScenarioValuation =
        serde_json::from_str(json).expect("legacy variant must still parse");
    let inner = match back {
        ScenarioValuation::CorporateEquity(v) => v,
        other => panic!("expected CorporateEquity variant, got: {other:?}"),
    };
    assert!(inner.dcf.is_none());
    assert!(inner.ev_ebitda.is_none());
    assert!(inner.forward_pe.is_none());
    assert!(inner.peg.is_none());
}

#[tokio::test]
async fn etf_variant_requires_snapshot_schema_version_above_v3() {
    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    struct LegacySnapshotState {
        #[serde(default)]
        equity: Option<LegacyEquityState>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    struct LegacyEquityState {
        #[serde(default)]
        derived_valuation: Option<LegacyDerivedValuation>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    struct LegacyDerivedValuation {
        asset_shape: AssetShape,
        scenario: LegacyScenarioValuation,
    }

    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyScenarioValuation {
        CorporateEquity(CorporateEquityValuation),
        NotAssessed { reason: String },
    }

    let mut state = TradingState::new("SPY", "2026-05-21");
    state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::Etf(EtfValuation {
            premium: PremiumSnapshot {
                nav: Some(621.18),
                market_price: 621.40,
                bid: Some(621.39),
                ask: Some(621.41),
                premium_pct: Some(0.04),
                category_band: PremiumBand::Normal,
                bid_ask_spread_pct: Some(0.003),
                as_of: chrono::Utc::now(),
            },
            composition: None,
            tracking: None,
            tracking_status: TrackingStatus::NotResolved,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: None,
            category: Some("Large Blend".to_owned()),
            leverage_factor: Some(1.0),
            flags: EtfDataAvailability::default(),
        }),
    });

    let json = serde_json::to_string(&state).expect("serialize current ETF-bearing snapshot");
    let err = serde_json::from_str::<LegacySnapshotState>(&json)
        .expect_err("pre-ETF snapshot schema must reject the ETF variant");

    assert!(
        err.to_string().contains("unknown variant") && err.to_string().contains("etf"),
        "legacy decoder should fail on ETF variant tag: {err}"
    );

    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("state-roundtrip.db");
    let store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("snapshot store creation");
    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &state,
            None,
        )
        .await
        .expect("save ETF-bearing snapshot");

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rw", db_path.display()))
        .await
        .expect("sqlite open");
    let stored_schema_version: (i64,) = sqlx::query_as(
        "SELECT schema_version FROM phase_snapshots WHERE execution_id = ? AND phase_number = ?",
    )
    .bind(state.execution_id.to_string())
    .bind(SnapshotPhase::FundManager.number() as i64)
    .fetch_one(&pool)
    .await
    .expect("schema version query");

    assert!(
        stored_schema_version.0 > 3,
        "ETF-bearing snapshots are not reverse-compatible with schema v3 readers"
    );
}

#[test]
fn etf_valuation_with_populated_gex_strikes_roundtrips_through_trading_state() {
    let mut state = TradingState::new("SPY", "2026-05-27");
    state.equity = None;

    let derived = DerivedValuation {
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
            tracking: None,
            tracking_status: TrackingStatus::NotResolved,
            official_benchmark_name: None,
            official_benchmark_source: None,
            official_benchmark_metadata_age_days: None,
            options_gex: Some(GexSummary {
                net_gex_usd_per_1pct_move: 1.2e9,
                gross_gex_usd_per_1pct_move: 3.4e9,
                call_put_oi_ratio: 1.25,
                max_pain_strike: 620.0,
                near_term_expiration: chrono::NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
                strikes: vec![
                    StrikeGex {
                        strike: 620.0,
                        net_gex_usd_per_1pct_move: 0.6e9,
                    },
                    StrikeGex {
                        strike: 615.0,
                        net_gex_usd_per_1pct_move: -0.4e9,
                    },
                    StrikeGex {
                        strike: 625.0,
                        net_gex_usd_per_1pct_move: 0.2e9,
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
    };
    state.set_derived_valuation(derived);

    let json = serde_json::to_string(&state).expect("serialize");
    let back: TradingState = serde_json::from_str(&json).expect("deserialize");
    match back.derived_valuation().map(|d| &d.scenario) {
        Some(ScenarioValuation::Etf(etf)) => {
            let g = etf.options_gex.as_ref().expect("gex");
            assert_eq!(g.strikes.len(), 3);
        }
        other => panic!("expected ETF scenario with gex, got {other:?}"),
    }
}

#[test]
fn legacy_etf_snapshot_without_profile_quality_fields_deserializes_with_defaults() {
    let json = r#"{
        "etf": {
            "premium": {
                "nav": 100.0,
                "market_price": 100.1,
                "bid": null,
                "ask": null,
                "premium_pct": 0.1,
                "category_band": "normal",
                "bid_ask_spread_pct": null,
                "as_of": "2026-05-28T12:00:00Z"
            },
            "composition": null,
            "tracking": null,
            "options_gex": null,
            "category": "Technology",
            "leverage_factor": 1.0,
            "flags": {}
        }
    }"#;

    let scenario: ScenarioValuation = serde_json::from_str(json).expect("legacy ETF scenario");
    let ScenarioValuation::Etf(etf) = scenario else {
        panic!("expected ETF scenario");
    };

    assert!(etf.official_benchmark_name.is_none());
    assert!(etf.official_benchmark_source.is_none());
    assert_eq!(etf.tracking_status, TrackingStatus::NotResolved);
}

#[test]
fn etf_composition_profile_quality_fields_roundtrip() {
    let comp = EtfComposition {
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
        holdings_filing_date: chrono::NaiveDate::from_ymd_opt(2026, 5, 28).unwrap(),
        holdings_report_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 30).unwrap()),
        holdings_age_days: 0,
        portfolio_turnover_pct: Some(0.24),
        inception_date: Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap()),
    };

    let json = serde_json::to_string(&comp).expect("serialize");
    let back: EtfComposition = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, comp);
}

#[test]
fn legacy_etf_composition_without_profile_quality_fields_deserializes_with_defaults() {
    // A pre-profile-quality EtfComposition object (as found in stored snapshots)
    // lacks source/holdings_report_date/portfolio_turnover_pct/inception_date;
    // the #[serde(default)] attributes must fill them. This guards the nested
    // composition default path that the EtfValuation-level legacy test does not
    // (it uses "composition": null).
    let json = r#"{
        "top_holdings": [],
        "top10_concentration_pct": 0.0,
        "sector_weights": [],
        "holdings_filing_date": "2026-03-31",
        "holdings_age_days": 12
    }"#;
    let comp: EtfComposition = serde_json::from_str(json).expect("legacy composition");
    assert_eq!(comp.source, EtfCompositionSource::SecNport);
    assert!(comp.holdings_report_date.is_none());
    assert!(comp.portfolio_turnover_pct.is_none());
    assert!(comp.inception_date.is_none());
    assert!(comp.expense_ratio_pct.is_none());
}

#[test]
fn trading_state_etf_risk_free_rate_fields_roundtrip_with_serde_default() {
    use scorpio_core::state::{EtfRiskFreeRateSource, TradingState};

    let mut state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    state.etf_risk_free_rate = Some(0.0427);
    state.etf_risk_free_rate_source = Some(EtfRiskFreeRateSource::FredDgs3Mo);

    let json = serde_json::to_string(&state).expect("serialize");
    let back: TradingState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.etf_risk_free_rate, Some(0.0427));
    assert_eq!(
        back.etf_risk_free_rate_source,
        Some(EtfRiskFreeRateSource::FredDgs3Mo)
    );
}

#[test]
fn legacy_trading_state_without_yfinance_info_field_deserializes() {
    // `yfinance_info` was added with `#[serde(default)]`; snapshots predating
    // the shared-Info change must still deserialize, defaulting it to `None`.
    let state = TradingState::new("AAPL", "2026-05-29");
    let mut value: serde_json::Value =
        serde_json::to_value(&state).expect("serialize current state");
    let removed = value
        .as_object_mut()
        .expect("json is object")
        .remove("yfinance_info");
    assert!(
        removed.is_some(),
        "yfinance_info must be present in current snapshots; did the field get renamed?"
    );
    let back: TradingState =
        serde_json::from_value(value).expect("legacy snapshot must deserialize");
    assert!(back.yfinance_info.is_none());
    assert_eq!(back.asset_symbol, "AAPL");
}

#[test]
fn legacy_trading_state_without_etf_risk_free_rate_fields_deserializes() {
    // Strip the new fields from a current snapshot to simulate a legacy
    // snapshot that predates the etf_risk_free_rate fields.
    let state = TradingState::new("SPY".to_owned(), "2026-05-27".to_owned());
    let mut value: serde_json::Value =
        serde_json::to_value(&state).expect("serialize current state");
    let obj = value.as_object_mut().expect("json is object");
    obj.remove("etf_risk_free_rate");
    obj.remove("etf_risk_free_rate_source");

    let back: scorpio_core::state::TradingState =
        serde_json::from_value(value).expect("legacy snapshot must deserialize");
    assert!(back.etf_risk_free_rate.is_none());
    assert!(back.etf_risk_free_rate_source.is_none());
}

#[test]
fn legacy_etf_snapshot_without_phase2_gex_strikes_still_deserializes() {
    let json = r#"{
        "net_gex_usd_per_1pct_move": 100.0,
        "gross_gex_usd_per_1pct_move": 200.0,
        "call_put_oi_ratio": 1.0,
        "max_pain_strike": 100.0,
        "near_term_expiration": "2026-06-26"
    }"#;
    let summary: GexSummary = serde_json::from_str(json).expect("legacy summary must deserialize");
    assert!(summary.strikes.is_empty());
}
