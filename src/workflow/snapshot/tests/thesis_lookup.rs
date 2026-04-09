use chrono::Utc;

use super::{in_memory_store, sample_thesis};
use crate::state::TradingState;
use crate::workflow::snapshot::SnapshotPhase;

#[tokio::test]
async fn load_prior_thesis_returns_none_when_no_prior_snapshot() {
    let store = in_memory_store().await;

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert!(result.is_none(), "no prior snapshot should yield None");
}

#[tokio::test]
async fn load_prior_thesis_returns_none_when_no_phase5_snapshot() {
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-07");
    state.current_thesis = Some(sample_thesis());
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert!(
        result.is_none(),
        "no phase-5 snapshot means no prior thesis"
    );
}

#[tokio::test]
async fn load_prior_thesis_returns_thesis_from_phase5_snapshot() {
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-07");
    let thesis = sample_thesis();
    state.current_thesis = Some(thesis.clone());
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    let loaded_thesis = result.expect("prior thesis should be found");
    assert_eq!(loaded_thesis.symbol, "AAPL");
    assert_eq!(loaded_thesis.action, "Buy");
    assert_eq!(loaded_thesis.decision, "Approved");
    assert_eq!(loaded_thesis.rationale, "Strong fundamentals.");
}

#[tokio::test]
async fn load_prior_thesis_returns_most_recent_when_multiple_runs() {
    let store = in_memory_store().await;

    let mut old_state = TradingState::new("AAPL", "2026-01-01");
    old_state.current_thesis = Some(crate::state::ThesisMemory {
        symbol: "AAPL".to_owned(),
        action: "Hold".to_owned(),
        decision: "Rejected".to_owned(),
        rationale: "Old rationale.".to_owned(),
        summary: None,
        execution_id: "exec-old".to_owned(),
        target_date: "2026-01-01".to_owned(),
        captured_at: Utc::now() - chrono::Duration::hours(2),
    });
    store
        .save_snapshot(
            &old_state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &old_state,
            None,
        )
        .await
        .expect("save old run");
    sqlx::query(
        "UPDATE phase_snapshots SET created_at = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind((Utc::now() - chrono::Duration::hours(2)).to_rfc3339())
    .bind(old_state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("timestamp update for old run");

    let mut new_state = TradingState::new("AAPL", "2026-04-07");
    new_state.current_thesis = Some(crate::state::ThesisMemory {
        symbol: "AAPL".to_owned(),
        action: "Buy".to_owned(),
        decision: "Approved".to_owned(),
        rationale: "New rationale.".to_owned(),
        summary: None,
        execution_id: "exec-new".to_owned(),
        target_date: "2026-04-07".to_owned(),
        captured_at: Utc::now() - chrono::Duration::hours(1),
    });
    store
        .save_snapshot(
            &new_state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &new_state,
            None,
        )
        .await
        .expect("save new run");
    sqlx::query(
        "UPDATE phase_snapshots SET created_at = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind((Utc::now() - chrono::Duration::hours(1)).to_rfc3339())
    .bind(new_state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("timestamp update for new run");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    let thesis = result.expect("should find prior thesis");
    assert_eq!(thesis.action, "Buy", "newest thesis must win");
    assert_eq!(thesis.rationale, "New rationale.");
}

#[tokio::test]
async fn load_prior_thesis_checks_beyond_five_ineligible_recent_rows() {
    let store = in_memory_store().await;

    for i in 0..5 {
        let state = TradingState::new("AAPL", format!("2026-04-0{}", i + 1));
        store
            .save_snapshot(
                &state.execution_id.to_string(),
                SnapshotPhase::FundManager,
                &state,
                None,
            )
            .await
            .expect("save ineligible run");
    }

    let mut eligible_state = TradingState::new("AAPL", "2026-04-07");
    eligible_state.current_thesis = Some(crate::state::ThesisMemory {
        symbol: "AAPL".to_owned(),
        action: "Buy".to_owned(),
        decision: "Approved".to_owned(),
        rationale: "Eligible older thesis.".to_owned(),
        summary: None,
        execution_id: "exec-eligible".to_owned(),
        target_date: "2026-04-07".to_owned(),
        captured_at: Utc::now() - chrono::Duration::hours(6),
    });
    store
        .save_snapshot(
            &eligible_state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &eligible_state,
            None,
        )
        .await
        .expect("save eligible run");
    sqlx::query(
        "UPDATE phase_snapshots SET created_at = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind((Utc::now() - chrono::Duration::hours(6)).to_rfc3339())
    .bind(eligible_state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("timestamp update for eligible run");

    let thesis = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed")
        .expect("eligible thesis should still be found");

    assert_eq!(thesis.action, "Buy");
    assert_eq!(thesis.rationale, "Eligible older thesis.");
}

#[tokio::test]
async fn load_prior_thesis_returns_none_for_different_symbol() {
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-07");
    state.current_thesis = Some(sample_thesis());

    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &state,
            None,
        )
        .await
        .expect("save should succeed");

    let result = store
        .load_prior_thesis_for_symbol("TSLA", 30)
        .await
        .expect("query should succeed");

    assert!(
        result.is_none(),
        "TSLA lookup should not return AAPL thesis"
    );
}
