#![cfg(feature = "test-helpers")]

mod workflow_test_support;

use workflow_test_support::run_stubbed_pipeline;

#[tokio::test]
async fn accounting_debate_rounds_have_correct_phase_names_and_agents() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(2, 1).await;

    for round in 1..=2u32 {
        let expected_name = format!("Researcher Debate Round {round}");
        let phase = final_state
            .token_usage
            .phase_usage
            .iter()
            .find(|p| p.phase_name == expected_name)
            .expect("debate round entry should exist");

        let agent_names: Vec<&str> = phase
            .agent_usage
            .iter()
            .map(|a| a.agent_name.as_str())
            .collect();
        assert_eq!(
            agent_names,
            vec!["Bullish Researcher", "Bearish Researcher"]
        );
        assert_eq!(phase.phase_duration_ms, 2);
    }
}

#[tokio::test]
async fn accounting_risk_rounds_have_correct_phase_names_and_agents() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(1, 2).await;

    for round in 1..=2u32 {
        let expected_name = format!("Risk Discussion Round {round}");
        let phase = final_state
            .token_usage
            .phase_usage
            .iter()
            .find(|p| p.phase_name == expected_name)
            .expect("risk round entry should exist");

        let agent_names: Vec<&str> = phase
            .agent_usage
            .iter()
            .map(|a| a.agent_name.as_str())
            .collect();
        assert_eq!(
            agent_names,
            vec!["Aggressive Risk", "Conservative Risk", "Neutral Risk"]
        );
        assert_eq!(phase.phase_duration_ms, 3);
    }
}

#[tokio::test]
async fn accounting_round_token_totals_reconcile_with_agents() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(2, 2).await;

    for phase in &final_state.token_usage.phase_usage {
        if phase.phase_name.contains("Round") {
            let sum_prompt: u64 = phase.agent_usage.iter().map(|a| a.prompt_tokens).sum();
            let sum_completion: u64 = phase.agent_usage.iter().map(|a| a.completion_tokens).sum();
            let sum_total: u64 = phase.agent_usage.iter().map(|a| a.total_tokens).sum();

            assert_eq!(phase.phase_prompt_tokens, sum_prompt);
            assert_eq!(phase.phase_completion_tokens, sum_completion);
            assert_eq!(phase.phase_total_tokens, sum_total);
        }
    }
}

#[tokio::test]
async fn accounting_tracker_totals_reconcile_with_all_phases() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(3, 2).await;

    let expected_prompt: u64 = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_prompt_tokens)
        .sum();
    let expected_completion: u64 = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_completion_tokens)
        .sum();
    let expected_total: u64 = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_total_tokens)
        .sum();

    assert_eq!(final_state.token_usage.total_prompt_tokens, expected_prompt);
    assert_eq!(
        final_state.token_usage.total_completion_tokens,
        expected_completion
    );
    assert_eq!(final_state.token_usage.total_tokens, expected_total);
}

#[tokio::test]
async fn accounting_round_entries_precede_moderation_entries() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(3, 2).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    let debate_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Moderation")
        .expect("debate moderation entry should exist");
    for round in 1..=3u32 {
        let name = format!("Researcher Debate Round {round}");
        let round_idx = phase_names
            .iter()
            .position(|n| *n == name.as_str())
            .expect("debate round should exist");
        assert!(round_idx < debate_mod_idx);
    }

    let risk_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Moderation")
        .expect("risk moderation entry should exist");
    for round in 1..=2u32 {
        let name = format!("Risk Discussion Round {round}");
        let round_idx = phase_names
            .iter()
            .position(|n| *n == name.as_str())
            .expect("risk round should exist");
        assert!(round_idx < risk_mod_idx);
    }
}

#[tokio::test]
async fn accounting_moderation_entries_are_structurally_correct() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(1, 1).await;

    let debate_mod = final_state
        .token_usage
        .phase_usage
        .iter()
        .find(|p| p.phase_name == "Researcher Debate Moderation")
        .expect("debate moderation entry should exist");
    assert_eq!(debate_mod.agent_usage.len(), 1);
    assert_eq!(debate_mod.agent_usage[0].agent_name, "Debate Moderator");
    assert_eq!(
        debate_mod.phase_prompt_tokens,
        debate_mod.agent_usage[0].prompt_tokens
    );
    assert_eq!(
        debate_mod.phase_total_tokens,
        debate_mod.agent_usage[0].total_tokens
    );

    let risk_mod = final_state
        .token_usage
        .phase_usage
        .iter()
        .find(|p| p.phase_name == "Risk Discussion Moderation")
        .expect("risk moderation entry should exist");
    assert_eq!(risk_mod.agent_usage.len(), 1);
    assert_eq!(risk_mod.agent_usage[0].agent_name, "Risk Moderator");
    assert_eq!(
        risk_mod.phase_prompt_tokens,
        risk_mod.agent_usage[0].prompt_tokens
    );
    assert_eq!(
        risk_mod.phase_total_tokens,
        risk_mod.agent_usage[0].total_tokens
    );
}
