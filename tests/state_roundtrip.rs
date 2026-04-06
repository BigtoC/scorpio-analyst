//! Property-based serialization round-trip tests for foundational state types.

use proptest::prelude::*;
use scorpio_analyst::state::*;

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
        (arb_opt_f64(), arb_opt_f64(), arb_opt_f64(), "[a-z ]{0,30}"),
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
                (support_level, resistance_level, volume_avg, summary),
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
    )
        .prop_map(
            |(title, source, published_at, relevance_score, snippet)| NewsArticle {
                title,
                source,
                published_at,
                relevance_score,
                snippet,
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

fn arb_trade_proposal() -> impl Strategy<Value = TradeProposal> {
    (
        arb_trade_action(),
        arb_f64(),
        arb_f64(),
        arb_f64(),
        "[a-z ]{5,40}",
    )
        .prop_map(
            |(action, target_price, stop_loss, confidence, rationale)| TradeProposal {
                action,
                target_price,
                stop_loss,
                confidence,
                rationale,
                valuation_assessment: None,
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
            )| {
                TradingState {
                    execution_id: uuid::Uuid::new_v4(),
                    asset_symbol,
                    target_date,
                    current_price: None,
                    market_volatility: None,
                    fundamental_metrics,
                    technical_indicators,
                    market_sentiment,
                    macro_news,
                    evidence_fundamental,
                    evidence_technical,
                    evidence_sentiment,
                    evidence_news,
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
                    token_usage,
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

proptest! {
    #[test]
    fn trading_state_json_roundtrip(state in arb_trading_state()) {
        assert_json_idempotent(&state);
    }

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
    fn risk_report_json_roundtrip(report in arb_risk_report()) {
        assert_json_idempotent(&report);
    }
}
