//! Prompt-bundle regression gate.
//!
//! Captures the rendered output of every baseline prompt template after
//! applying canonical placeholder substitutions (`{ticker}`, `{current_date}`)
//! and asserts byte-for-byte equality against on-disk golden fixtures under
//! `tests/fixtures/prompt_bundle/`. The gate exists to lock prompt-asset
//! content across the Unit 4a/4b runtime-contract migration: the renderer
//! signature changes (legacy `&str` template → `&RuntimePolicy`), but the
//! *template render* must remain byte-identical.
//!
//! **Updating fixtures:** when a baseline prompt template intentionally
//! changes, regenerate the golden bytes by setting `UPDATE_FIXTURES=1` and
//! re-running this test:
//!
//! ```bash
//! UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate
//! ```
//!
//! The regenerated files must then be reviewed in the PR — golden bytes are
//! the merge gate, not the harness code.

use std::fs;
use std::path::PathBuf;

use scorpio_core::analysis_packs::{PackId, resolve_pack};
use scorpio_core::testing::baseline_pack_prompt_for_role;
use scorpio_core::workflow::topology::Role;

/// Canonical placeholder values used when capturing fixtures. Stable across
/// runs so the same input always produces the same output bytes.
const FIXTURE_TICKER: &str = "AAPL";
const FIXTURE_DATE: &str = "2026-04-25";

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

/// Render the baseline pack's prompt for `role` with canonical placeholder
/// substitutions applied. The signature is intentionally minimal — Unit 4a
/// rewrites this helper to take `&RuntimePolicy` instead, but the rendered
/// bytes that result must continue to match the golden fixtures.
fn render_baseline_prompt(role: Role) -> String {
    let template = baseline_pack_prompt_for_role(role);
    template
        .replace("{ticker}", FIXTURE_TICKER)
        .replace("{current_date}", FIXTURE_DATE)
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
             UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --test prompt_bundle_regression_gate",
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

#[test]
fn baseline_pack_renders_match_golden_fixtures_all_inputs_present() {
    // The "all inputs present" scenario: every role under the fully-enabled
    // baseline topology renders with canonical placeholder substitutions and
    // matches its on-disk golden bytes exactly.
    for role in [
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
    ] {
        let rendered = render_baseline_prompt(role);
        assert_or_update(role, &rendered);
    }
}

#[test]
fn fixtures_contain_canonical_substitutions() {
    // Cross-check: every captured fixture must contain the canonical
    // `FIXTURE_TICKER` and `FIXTURE_DATE` strings (and must not contain the
    // unrendered `{ticker}` / `{current_date}` placeholders). This catches
    // fixture-generation bugs where placeholders were not substituted.
    if update_fixtures_enabled() {
        // Skip the cross-check when regenerating — the fixture files are
        // about to be overwritten in this run.
        return;
    }
    for role in [Role::Trader, Role::FundManager] {
        let path = fixture_path(role);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("fixture missing for {role:?} at {}", path.display()));
        assert!(
            content.contains(FIXTURE_TICKER),
            "{role:?} fixture should contain the canonical ticker"
        );
        assert!(
            content.contains(FIXTURE_DATE),
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
}

#[test]
fn baseline_manifest_is_complete_under_fully_enabled_topology() {
    // Sanity: the regression gate is only meaningful if the baseline pack
    // actually populates every required slot. This is also asserted in the
    // unit test in completeness.rs but mirrored here so a workspace-level
    // run still proves it.
    use scorpio_core::analysis_packs::validate_active_pack_completeness;
    use scorpio_core::workflow::topology::build_run_topology;

    let manifest = resolve_pack(PackId::Baseline);
    let topology = build_run_topology(&manifest.required_inputs, 1, 1);
    let result = validate_active_pack_completeness(&manifest, &topology);
    assert!(
        result.is_ok(),
        "baseline pack must be complete or the gate is meaningless: {result:?}"
    );
}
