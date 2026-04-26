#![cfg(feature = "test-helpers")]

//! Prompt-bundle regression gate.
//!
//! Captures the rendered output of every baseline prompt builder through the
//! test-only `RuntimePolicy` hydration path and asserts byte-for-byte equality
//! against on-disk golden fixtures under `tests/fixtures/prompt_bundle/`.
//! The gate exists to lock prompt behavior across the Unit 4a/4b runtime-
//! contract migration: prompt-builder signatures and wiring may change, but
//! the rendered baseline system prompts for the canonical fixture state must
//! remain byte-identical.
//!
//! **Updating fixtures:** when a baseline prompt template intentionally
//! changes, regenerate the golden bytes by setting `UPDATE_FIXTURES=1` and
//! re-running this test:
//!
//! ```bash
//! UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers
//! ```
//!
//! The regenerated files must then be reviewed in the PR — golden bytes are
//! the merge gate, not the harness code.

use std::fs;
use std::path::PathBuf;

use scorpio_core::{
    analysis_packs::{PackId, resolve_pack, validate_active_pack_completeness},
    testing::{
        PromptRenderScenario, canonical_fixture_identity, render_baseline_prompt_for_role,
        render_prompt_output_for_role, runtime_policy_from_manifest,
    },
    workflow::{Role, build_run_topology},
};

const LIVE_ROLES: [Role; 13] = [
    Role::FundamentalAnalyst,
    Role::SentimentAnalyst,
    Role::NewsAnalyst,
    Role::TechnicalAnalyst,
    Role::BullishResearcher,
    Role::BearishResearcher,
    Role::DebateModerator,
    Role::Trader,
    Role::AggressiveRisk,
    Role::ConservativeRisk,
    Role::NeutralRisk,
    Role::RiskModerator,
    Role::FundManager,
];

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("prompt_bundle")
}

fn fixture_path(role: Role) -> PathBuf {
    let filename = match role {
        Role::FundamentalAnalyst => "fundamental_analyst.txt",
        Role::SentimentAnalyst => "sentiment_analyst.txt",
        Role::NewsAnalyst => "news_analyst.txt",
        Role::TechnicalAnalyst => "technical_analyst.txt",
        Role::BullishResearcher => "bullish_researcher.txt",
        Role::BearishResearcher => "bearish_researcher.txt",
        Role::DebateModerator => "debate_moderator.txt",
        Role::Trader => "trader.txt",
        Role::AggressiveRisk => "aggressive_risk.txt",
        Role::ConservativeRisk => "conservative_risk.txt",
        Role::NeutralRisk => "neutral_risk.txt",
        Role::RiskModerator => "risk_moderator.txt",
        Role::FundManager => "fund_manager.txt",
    };
    fixtures_dir().join(filename)
}

fn user_prompt_fixture_path(role: Role, scenario: PromptRenderScenario) -> PathBuf {
    let filename = match (role, scenario) {
        (Role::Trader, PromptRenderScenario::AllInputsPresent) => {
            "trader_all_inputs_present_user.txt"
        }
        (Role::Trader, PromptRenderScenario::ZeroDebate) => "trader_zero_debate_user.txt",
        (Role::Trader, PromptRenderScenario::MissingAnalystData) => {
            "trader_missing_analyst_data_user.txt"
        }
        (Role::FundManager, PromptRenderScenario::AllInputsPresent) => {
            "fund_manager_all_inputs_present_user.txt"
        }
        (Role::FundManager, PromptRenderScenario::ZeroRisk) => "fund_manager_zero_risk_user.txt",
        (Role::FundManager, PromptRenderScenario::MissingAnalystData) => {
            "fund_manager_missing_analyst_data_user.txt"
        }
        _ => panic!("unsupported user-prompt fixture request for {role:?} / {scenario:?}"),
    };

    fixtures_dir().join("user").join(filename)
}

fn update_fixtures_enabled() -> bool {
    std::env::var("UPDATE_FIXTURES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn assert_or_update(role: Role, rendered: &str) {
    let path = fixture_path(role);
    if update_fixtures_enabled() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixtures dir");
        }
        fs::write(&path, rendered).expect("write fixture");
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing fixture for {role:?} at {}: {e}.\n\
             Generate it by running:\n  \
             UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers",
            path.display()
        )
    });
    assert_eq!(
        rendered,
        expected,
        "rendered baseline prompt for {role:?} drifted from golden bytes at {}.\n\
         If the change was intentional, regenerate fixtures with UPDATE_FIXTURES=1.",
        path.display()
    );
}

fn assert_or_update_user_prompt(role: Role, scenario: PromptRenderScenario, rendered: &str) {
    let path = user_prompt_fixture_path(role, scenario);
    if update_fixtures_enabled() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create user-prompt fixtures dir");
        }
        fs::write(&path, rendered).expect("write user-prompt fixture");
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing user-prompt fixture for {role:?} / {scenario:?} at {}: {e}.\n\
             Generate it by running:\n  \
             UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate --features test-helpers",
            path.display()
        )
    });

    assert_eq!(
        rendered,
        expected,
        "rendered user prompt for {role:?} / {scenario:?} drifted from golden bytes at {}.\n\
         If the change was intentional, regenerate fixtures with UPDATE_FIXTURES=1.",
        path.display()
    );
}

#[test]
fn baseline_pack_renders_match_golden_fixtures_all_inputs_present() {
    // The "all inputs present" scenario: every role under the fully-enabled
    // baseline topology renders through the real prompt-builder + RuntimePolicy
    // path and matches its on-disk golden bytes exactly.
    for role in LIVE_ROLES {
        let rendered =
            render_baseline_prompt_for_role(role, PromptRenderScenario::AllInputsPresent);
        assert_or_update(role, &rendered);
    }
}

#[test]
fn fixtures_contain_canonical_substitutions() {
    // Cross-check: every captured fixture must contain the canonical ticker and
    // date strings (and must not contain unrendered `{ticker}` /
    // `{current_date}` placeholders). This catches fixture-generation bugs
    // where prompt builders were bypassed or substitutions failed.
    if update_fixtures_enabled() {
        // Skip the cross-check when regenerating — the fixture files are
        // about to be overwritten in this run.
        return;
    }
    let (fixture_ticker, fixture_date) = canonical_fixture_identity();

    for role in LIVE_ROLES {
        let path = fixture_path(role);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("fixture missing for {role:?} at {}", path.display()));
        assert!(
            content.contains(fixture_ticker),
            "{role:?} fixture should contain the canonical ticker"
        );
        assert!(
            content.contains(fixture_date),
            "{role:?} fixture should contain the canonical date"
        );
        assert!(
            !content.contains("{ticker}"),
            "{role:?} fixture must not contain unrendered {{ticker}} placeholder"
        );
        assert!(
            !content.contains("{current_date}"),
            "{role:?} fixture must not contain unrendered {{current_date}} placeholder"
        );
    }

    for (role, scenario) in [
        (Role::Trader, PromptRenderScenario::AllInputsPresent),
        (Role::Trader, PromptRenderScenario::ZeroDebate),
        (Role::Trader, PromptRenderScenario::MissingAnalystData),
        (Role::FundManager, PromptRenderScenario::AllInputsPresent),
        (Role::FundManager, PromptRenderScenario::ZeroRisk),
        (Role::FundManager, PromptRenderScenario::MissingAnalystData),
    ] {
        let path = user_prompt_fixture_path(role, scenario);
        let content = fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "user-prompt fixture missing for {role:?} / {scenario:?} at {}",
                path.display()
            )
        });
        assert!(
            content.contains(fixture_ticker),
            "{role:?} / {scenario:?} user fixture should contain the canonical ticker"
        );
        assert!(
            content.contains(fixture_date),
            "{role:?} / {scenario:?} user fixture should contain the canonical date"
        );
        assert!(
            !content.contains("{ticker}"),
            "{role:?} / {scenario:?} user fixture must not contain unrendered {{ticker}} placeholder"
        );
        assert!(
            !content.contains("{current_date}"),
            "{role:?} / {scenario:?} user fixture must not contain unrendered {{current_date}} placeholder"
        );
    }
}

#[test]
fn trader_prompt_scenarios_capture_missing_input_states() {
    let all_inputs =
        render_baseline_prompt_for_role(Role::Trader, PromptRenderScenario::AllInputsPresent);
    let zero_debate =
        render_baseline_prompt_for_role(Role::Trader, PromptRenderScenario::ZeroDebate);
    let zero_risk = render_baseline_prompt_for_role(Role::Trader, PromptRenderScenario::ZeroRisk);
    let missing_analyst_data =
        render_baseline_prompt_for_role(Role::Trader, PromptRenderScenario::MissingAnalystData);

    assert_ne!(
        all_inputs, zero_debate,
        "missing debate consensus should change the trader system prompt"
    );
    assert!(
        zero_debate.contains("- Research consensus: null"),
        "zero-debate trader prompt should serialize the absent consensus as null"
    );

    assert_eq!(
        all_inputs, zero_risk,
        "trader system prompt should be independent of downstream risk-stage output"
    );

    assert_ne!(
        all_inputs, missing_analyst_data,
        "missing analyst inputs should change the trader system prompt"
    );
    assert!(
        missing_analyst_data.contains("- Data quality note: see user context"),
        "missing-analyst-data trader prompt should keep template-owned data-quality wording"
    );
    assert!(
        missing_analyst_data.contains("null"),
        "missing-analyst-data trader prompt should serialize absent analyst inputs explicitly"
    );
}

#[test]
fn trader_and_fund_manager_user_prompts_match_golden_fixtures() {
    for (role, scenario) in [
        (Role::Trader, PromptRenderScenario::AllInputsPresent),
        (Role::Trader, PromptRenderScenario::ZeroDebate),
        (Role::Trader, PromptRenderScenario::MissingAnalystData),
        (Role::FundManager, PromptRenderScenario::AllInputsPresent),
        (Role::FundManager, PromptRenderScenario::ZeroRisk),
        (Role::FundManager, PromptRenderScenario::MissingAnalystData),
    ] {
        let rendered = render_prompt_output_for_role(role, scenario)
            .user_prompt
            .expect("Trader and FundManager should expose user prompts");
        assert_or_update_user_prompt(role, scenario, &rendered);
    }
}

#[test]
fn all_inputs_present_user_prompts_capture_non_null_typed_evidence() {
    for role in [Role::Trader, Role::FundManager] {
        let rendered = render_prompt_output_for_role(role, PromptRenderScenario::AllInputsPresent)
            .user_prompt
            .expect("Trader and FundManager should expose user prompts");

        for label in ["fundamentals", "sentiment", "news", "technical"] {
            assert!(
                !rendered.contains(&format!("- {label}: null")),
                "all-inputs-present user prompt for {role:?} should serialize non-null {label} evidence"
            );
        }
    }
}

#[test]
fn fund_manager_missing_analyst_data_keeps_risk_inputs_present() {
    let rendered =
        render_prompt_output_for_role(Role::FundManager, PromptRenderScenario::MissingAnalystData)
            .user_prompt
            .expect("FundManager should expose a user prompt");

    assert!(
        rendered.contains("Dual-risk escalation: absent"),
        "missing-analyst-data scenario should keep the dual-risk signal tied to present risk reports"
    );
    assert!(
        rendered.contains("Aggressive risk report: {\"risk_level\":\"Aggressive\""),
        "missing-analyst-data scenario should preserve risk reports"
    );
    assert!(
        rendered.contains("Fundamental data: null"),
        "missing-analyst-data scenario should serialize absent analyst payloads as null"
    );
}

#[test]
fn user_prompt_inert_scenarios_match_happy_path() {
    let trader_all =
        render_prompt_output_for_role(Role::Trader, PromptRenderScenario::AllInputsPresent)
            .user_prompt
            .expect("Trader should expose a user prompt");
    let trader_zero_risk =
        render_prompt_output_for_role(Role::Trader, PromptRenderScenario::ZeroRisk)
            .user_prompt
            .expect("Trader should expose a user prompt");
    assert_eq!(
        trader_zero_risk, trader_all,
        "Trader user prompt should be unchanged by downstream risk-stage output"
    );

    let fund_manager_all =
        render_prompt_output_for_role(Role::FundManager, PromptRenderScenario::AllInputsPresent)
            .user_prompt
            .expect("FundManager should expose a user prompt");
    let fund_manager_zero_debate =
        render_prompt_output_for_role(Role::FundManager, PromptRenderScenario::ZeroDebate)
            .user_prompt
            .expect("FundManager should expose a user prompt");
    assert_eq!(
        fund_manager_zero_debate, fund_manager_all,
        "FundManager user prompt should be unchanged when only researcher debate output is absent"
    );
}

// Two vestigial tests were removed in Phase 7 of the prompt-bundle
// centralization migration:
//
// - `baseline_runtime_policy_and_legacy_fallback_system_prompts_match`
// - `blank_selected_prompt_slots_fall_back_to_legacy_rendering`
//
// Both asserted byte-equivalence between two renderer paths (runtime-policy
// vs. legacy-template fallback) that are now collapsed into a single path:
// the renderer requires `&RuntimePolicy` and has no fallback branch.
// `validate_active_pack_completeness` rejects packs whose required slots are
// empty before any renderer runs, so the legacy fallback is unreachable.
// The remaining golden-byte assertions still gate the merge: the rendered
// output for every role × scenario must match `tests/fixtures/prompt_bundle/`.

#[test]
fn baseline_manifest_is_complete_under_fully_enabled_topology() {
    // Sanity: the regression gate is only meaningful if the baseline pack
    // actually populates every required slot. This is also asserted in the
    // unit test in completeness.rs but mirrored here so a workspace-level
    // run still proves it.
    let manifest = resolve_pack(PackId::Baseline);
    let policy = runtime_policy_from_manifest(&manifest);
    let topology = build_run_topology(&manifest.required_inputs, 1, 1);
    let result = validate_active_pack_completeness(&policy, &topology);
    assert!(
        result.is_ok(),
        "baseline pack must be complete or the gate is meaningless: {result:?}"
    );
}
