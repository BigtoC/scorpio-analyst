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

const LIVE_ROLES: [Role; 14] = [
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
    Role::Auditor,
];

struct ThemeCoverageCase {
    theme: &'static str,
    roles: &'static [Role],
    required_markers: &'static [&'static str],
}

const ANALYTICAL_THEME_PORT_COVERAGE: &[ThemeCoverageCase] = &[
    ThemeCoverageCase {
        theme: "Theme H sourcing hierarchy + injection defense",
        roles: &[
            Role::FundamentalAnalyst,
            Role::NewsAnalyst,
            Role::SentimentAnalyst,
            Role::TechnicalAnalyst,
            Role::Trader,
        ],
        required_markers: &[
            "Data Sourcing Hierarchy",
            "[UNSOURCED]",
            "Untrusted External Content",
        ],
    },
    ThemeCoverageCase {
        theme: "Theme E researcher falsifiable structure",
        roles: &[Role::BullishResearcher, Role::BearishResearcher],
        required_markers: &["Pillars (3", "Thesis breakers (3"],
    },
    ThemeCoverageCase {
        theme: "Theme E moderator falsifiability synthesis",
        roles: &[Role::DebateModerator],
        required_markers: &["falsifi", "Surviving pillars", "unresolved uncertainty"],
    },
    ThemeCoverageCase {
        theme: "Theme E neutral risk falsifiability check",
        roles: &[Role::NeutralRisk],
        required_markers: &["Falsifiability Check"],
    },
    ThemeCoverageCase {
        theme: "Theme A valuation sanity bands",
        roles: &[Role::FundamentalAnalyst, Role::ConservativeRisk],
        required_markers: &["Valuation Sanity Bands"],
    },
    ThemeCoverageCase {
        theme: "Theme B industry KPI matrix",
        roles: &[Role::FundamentalAnalyst],
        required_markers: &["Industry KPI Matrix"],
    },
    ThemeCoverageCase {
        theme: "Theme C management red flags degraded mode",
        roles: &[
            Role::NewsAnalyst,
            Role::SentimentAnalyst,
            Role::ConservativeRisk,
        ],
        required_markers: &[
            "Management Commentary Red Flags",
            "degraded mode: transcript unavailable",
        ],
    },
    ThemeCoverageCase {
        theme: "Theme G catalyst taxonomy degraded mode",
        roles: &[Role::NewsAnalyst],
        required_markers: &[
            "Catalyst Taxonomy",
            "degraded mode: news-discovered events only",
        ],
    },
    ThemeCoverageCase {
        theme: "Theme F contrarian catalyst rule",
        roles: &[Role::BullishResearcher, Role::AggressiveRisk],
        required_markers: &["Contrarian Position Rule"],
    },
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
        Role::Auditor => "auditor.txt",
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
fn branching_prompts_reference_options_context_field_path() {
    // Every branching role must tell the agent to inspect options_context and
    // treat options_summary as supplemental, not authoritative.
    for role in [
        Role::BullishResearcher,
        Role::BearishResearcher,
        Role::DebateModerator,
        Role::Trader,
        Role::AggressiveRisk,
        Role::ConservativeRisk,
        Role::NeutralRisk,
        Role::RiskModerator,
    ] {
        let rendered =
            render_baseline_prompt_for_role(role, PromptRenderScenario::AllInputsPresent);
        assert!(
            rendered.contains("options_context"),
            "{role:?} prompt must reference options_context"
        );
        assert!(
            rendered.contains("outcome.kind"),
            "{role:?} prompt must tell the agent to branch on outcome.kind"
        );
        assert!(
            rendered.contains("supplemental") || rendered.contains("not authority"),
            "{role:?} prompt must treat options_summary as supplemental"
        );
    }
}

#[test]
fn branching_prompts_name_all_outcome_kind_values() {
    let outcome_kind_tokens = [
        "snapshot",
        "no_listed_instrument",
        "sparse_chain",
        "historical_run",
        "missing_spot",
    ];
    let status_tokens = ["fetch_failed", "available"];

    for role in [
        Role::BullishResearcher,
        Role::BearishResearcher,
        Role::DebateModerator,
        Role::Trader,
        Role::AggressiveRisk,
        Role::ConservativeRisk,
        Role::NeutralRisk,
        Role::RiskModerator,
        Role::FundManager,
    ] {
        let rendered =
            render_baseline_prompt_for_role(role, PromptRenderScenario::AllInputsPresent);
        for token in outcome_kind_tokens {
            assert!(
                rendered.contains(token),
                "{role:?} prompt must mention outcome.kind value {token}"
            );
        }
        for token in status_tokens {
            assert!(
                rendered.contains(token),
                "{role:?} prompt must mention status value {token}"
            );
        }
    }
}

#[test]
fn prompt_bundle_has_auditor_slot() {
    let bundle = resolve_pack(PackId::Baseline).prompt_bundle;
    assert!(!bundle.auditor.is_empty(), "auditor slot must not be empty");
}

#[test]
fn baseline_pack_ships_with_auditor_enabled_by_default() {
    let manifest = resolve_pack(PackId::Baseline);
    assert!(
        manifest.auditor_enabled,
        "baseline must ship with auditor_enabled = true"
    );
}

#[test]
fn baseline_manifest_is_complete_under_fully_enabled_topology() {
    // Sanity: the regression gate is only meaningful if the baseline pack
    // actually populates every required slot. This is also asserted in the
    // unit test in completeness.rs but mirrored here so a workspace-level
    // run still proves it.
    let manifest = resolve_pack(PackId::Baseline);
    let policy = runtime_policy_from_manifest(&manifest);
    let topology = build_run_topology(&manifest.required_inputs, 1, 1, manifest.auditor_enabled);
    let result = validate_active_pack_completeness(&policy, &topology);
    assert!(
        result.is_ok(),
        "baseline pack must be complete or the gate is meaningless: {result:?}"
    );
}

#[test]
fn etf_baseline_passes_completeness_under_all_topology_shapes() {
    // ETF baseline pack must populate every required prompt slot across the
    // four runtime topology shapes preflight can build:
    //  - full          (debate + risk both > 0)
    //  - no_debate     (max_debate_rounds == 0)
    //  - no_risk       (max_risk_rounds == 0)
    //  - no_debate_no_risk (both == 0)
    //
    // Uses `runtime_policy_from_manifest` (test-helpers gated) because
    // `EtfBaseline` is not user-selectable via `resolve_runtime_policy` —
    // `PackId::from_str` rejects it so the runtime classifier (not the
    // string config) is the only production hydration path.
    let manifest = resolve_pack(PackId::EtfBaseline);
    let policy = runtime_policy_from_manifest(&manifest);

    let shapes = [
        (1_u32, 1_u32, "full"),
        (0, 1, "no_debate"),
        (1, 0, "no_risk"),
        (0, 0, "no_debate_no_risk"),
    ];
    for (max_debate, max_risk, label) in shapes {
        let topology = build_run_topology(
            &manifest.required_inputs,
            max_debate,
            max_risk,
            manifest.auditor_enabled,
        );
        let result = validate_active_pack_completeness(&policy, &topology);
        assert!(
            result.is_ok(),
            "ETF baseline pack must be complete in shape '{label}' \
             (debate={max_debate}, risk={max_risk}, auditor={}): {result:?}",
            manifest.auditor_enabled,
        );
    }
}

#[test]
fn equity_baseline_prompt_bundle_still_includes_moved_files() {
    // Tier-1 and Tier-2 prompts were physically relocated to
    // `analysis_packs/common/prompts/` in Tasks 3 and 4. This is a low-cost
    // byte-identity sanity check: the equity baseline pack must still
    // include the moved files (i.e. the `include_str!` paths still resolve
    // to non-empty content that mentions the role-specific subject matter).
    //
    // The golden-byte fixtures in `tests/fixtures/prompt_bundle/` are the
    // real merge gate — this test is a fast pre-check that catches a
    // missing file before the full render diff fires.
    let pack = resolve_pack(PackId::Baseline);

    // Tier-1 files moved to common/ in Task 3 — verbatim reuse.
    assert!(
        !pack.prompt_bundle.debate_moderator.is_empty(),
        "debate_moderator must be non-empty"
    );
    assert!(
        !pack.prompt_bundle.risk_moderator.is_empty(),
        "risk_moderator must be non-empty"
    );
    assert!(
        !pack.prompt_bundle.bullish_researcher.is_empty(),
        "bullish_researcher must be non-empty"
    );
    assert!(
        !pack.prompt_bundle.bearish_researcher.is_empty(),
        "bearish_researcher must be non-empty"
    );

    // Tier-2 files moved to common/ in Task 4 — composed with equity-pack
    // deltas. Loose `contains` because the equity pack appends additional
    // sections; we only verify the right base file is being included.
    assert!(
        pack.prompt_bundle
            .news_analyst
            .to_lowercase()
            .contains("news"),
        "news_analyst should reference news"
    );
    assert!(
        pack.prompt_bundle
            .technical_analyst
            .to_lowercase()
            .contains("technical"),
        "technical_analyst should reference technical analysis"
    );
    assert!(
        pack.prompt_bundle.auditor.to_lowercase().contains("audit"),
        "auditor should reference auditing"
    );
}

#[test]
fn etf_baseline_prompt_bundle_still_includes_moved_files() {
    // Mirror of `equity_baseline_prompt_bundle_still_includes_moved_files`
    // for the ETF pack — the ETF pack reuses the same `common/prompts/`
    // files for Tier-1 (verbatim) and Tier-2 (composed with ETF deltas).
    let pack = resolve_pack(PackId::EtfBaseline);

    // Tier-1 verbatim reuse.
    assert!(
        !pack.prompt_bundle.debate_moderator.is_empty(),
        "debate_moderator must be non-empty"
    );
    assert!(
        !pack.prompt_bundle.risk_moderator.is_empty(),
        "risk_moderator must be non-empty"
    );
    assert!(
        !pack.prompt_bundle.bullish_researcher.is_empty(),
        "bullish_researcher must be non-empty"
    );
    assert!(
        !pack.prompt_bundle.bearish_researcher.is_empty(),
        "bearish_researcher must be non-empty"
    );

    // Tier-2 composed with ETF deltas.
    assert!(
        pack.prompt_bundle
            .news_analyst
            .to_lowercase()
            .contains("news"),
        "news_analyst should reference news"
    );
    assert!(
        pack.prompt_bundle
            .technical_analyst
            .to_lowercase()
            .contains("technical"),
        "technical_analyst should reference technical analysis"
    );
    assert!(
        pack.prompt_bundle.auditor.to_lowercase().contains("audit"),
        "auditor should reference auditing"
    );
}

// --- Analytical Themes Port assertions (Tasks 2-9 of 2026-05-10-004) ---

#[test]
fn analytical_theme_port_coverage_matrix_remains_intact() {
    for case in ANALYTICAL_THEME_PORT_COVERAGE {
        for role in case.roles {
            let rendered =
                render_baseline_prompt_for_role(*role, PromptRenderScenario::AllInputsPresent);
            for marker in case.required_markers {
                assert!(
                    rendered.contains(marker),
                    "{} marker {:?} missing from {:?}",
                    case.theme,
                    marker,
                    role
                );
            }
        }
    }
}
