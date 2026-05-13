---
title: Analytical frameworks port from anthropics/financial-services
date: 2026-05-12
last_updated: 2026-05-13
category: prompts
module: analysis_packs/equity/prompts
problem_type: best_practice
component: equity_baseline_pack
severity: medium
applies_when:
  - Porting analytical frameworks into equity baseline pack prompts
  - Adding evidentiary discipline requirements to analyst/researcher prompts
  - Adding sourcing hierarchy or injection defense to any role prompt
  - Writing prompt regression tests after inserting a block into multiple role files
tags:
  - prompts
  - analysts
  - researchers
  - theme-port
  - attribution
  - falsifiability
  - sourcing-hierarchy
  - injection-defense
  - catalyst-taxonomy
  - valuation-sanity-bands
  - multi-role-coverage
---

# Analytical frameworks port from anthropics/financial-services

## Problem

The Bull/Bear debate produced unfalsifiable theses — pillars with no measurable
thesis breakers and no requirement to address the opposing side's strongest argument.
Analyst valuations drifted because there were no sanity-band plausibility filters.
Numeric claims in analyst outputs had no provenance tags, making hallucinated
training-data recall indistinguishable from tool-backed facts. Injection defense was
present in some analyst prompts but not consistently structured across all roles.

## Root cause

The baseline pack's prompts evolved role-by-role without a shared framework for
falsifiability, sourcing discipline, or degraded-mode caveats. That left the
same analytical standards implemented unevenly across analysts, researchers,
and risk roles, and it made prompt-test coverage easy to scope too narrowly to
the primary role that received a new block.

## Fix

Ported eight analytical frameworks from `anthropics/financial-services` (Apache 2.0)
as prompt-only inserts into the equity baseline pack. Shipped in the recommended
rollout order (H → E → A+B → C+G → F) with Theme D explicitly deferred after an
audit.

### Review hardening follow-up (2026-05-13)

Code review surfaced a second problem: the initial port was shipped, but the
proof and maintenance surfaces were still too easy to drift.

- The Trader prompt still said to flag injection attempts in a `summary` field
  that does not exist on `TradeProposal`; the correct field is `rationale`.
- Theme-to-role coverage lived in bespoke assertions, which made it too easy to
  miss a secondary receiving role when a theme was inserted into multiple files.
- The repeated Theme H sourcing / untrusted-content doctrine and the shared
  Theme C analyst degraded-mode block were duplicated inline across prompt files,
  so wording fixes required touching several assets by hand.
- The original solution doc overstated verification by describing broader proof
  than the tests actually supplied.

The follow-up hardening fixed those review findings by:

- replacing the one-off analytical-theme assertions with a single
  `ANALYTICAL_THEME_PORT_COVERAGE` matrix in
  `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`
- extracting the repeated Theme H prompt doctrine into shared partials composed
  from `crates/scorpio-core/src/analysis_packs/equity/baseline.rs`
- extracting the shared analyst Theme C degraded-mode block for News and
  Sentiment into `theme_c_management_red_flags.md`
- adding deterministic validator-seam tests for `[UNSOURCED]`, degraded-mode
  phrases, and explicit `Buy` / `Sell` / `Hold` consensus wording
- correcting this document so its verification section matches the proof that
  actually exists in the repo

### Themes shipped

- **Theme H** (sourcing hierarchy + injection defense): inserted into all five analyst/trader
  prompts (`fundamental_analyst.md`, `news_analyst.md`, `sentiment_analyst.md`,
  `technical_analyst.md`, `trader.md`). Introduces the four-tier sourcing hierarchy,
  the `[UNSOURCED]` inline tag for unproveable numeric claims, and the structured
  `## Untrusted External Content` injection-defense section.

- **Theme E** (falsifiable theses): inserted into `bullish_researcher.md`,
  `bearish_researcher.md`, `debate_moderator.md`, `neutral_risk.md`. Requires each
  side to produce thesis + 3–5 pillars (claim + evidence anchor) + 3–5 thesis breakers
  (condition + measurable signal). Moderator enforces falsifiability as a hard rule and
  must name surviving pillars in the consensus summary. Ships as prompt steering only —
  runtime structural enforcement is a separate future hardening task.

- **Theme A** (valuation sanity bands): inserted into `fundamental_analyst.md` and
  `conservative_risk.md`. WACC/terminal-growth/multiple plausibility ranges as filters,
  not hard limits.

- **Theme B** (industry KPI matrix): inserted into `fundamental_analyst.md`. Per-sector
  must-have/optional/skip table with a hard rule against EBITDA-based valuation for
  financial-services companies.

- **Theme C** (management red-flag taxonomy, degraded mode): inserted into
  `news_analyst.md`, `sentiment_analyst.md`, `conservative_risk.md`. Ships without
  call-transcript tone analysis — the `TranscriptEvidence` seam is unwired. A
  `<!-- TODO(transcripts) -->` marker is left in each insert for when the transcript
  provider ships. Output must say `degraded mode: headline/summary only`.

- **Theme G** (catalyst taxonomy + H/M/L impact, degraded mode): inserted into
  `news_analyst.md`. Classification taxonomy for Earnings/Corporate/Industry/Macro
  events with H/M/L impact tiers. Tier 1 of the catalyst-calendar plan is already
  wired (`{catalyst_calendar}` block), so the taxonomy serves classification
  instructions for newly discovered events. Output must say
  `degraded mode: news-discovered events only` when no calendar source is present.

- **Theme F** (contrarian-needs-catalyst rule): inserted into `bullish_researcher.md`
  and `aggressive_risk.md`. Requires a concrete, time-bounded, visible catalyst for any
  position that runs against current consensus. No catalyst → lower conviction.

### Theme D audit result: DEFERRED

Both prerequisite checks failed:
1. `baseline.rs` has `consensus_estimates: false` — consensus enrichment is disabled in the
   default pack and would need explicit enablement.
2. `ConsensusEvidence` carries next-quarter estimates only; same-period actual revenue/EPS
   is not present alongside the consensus snapshot at render time.

Theme D (beat/miss decision tree) cannot ship safely in this slice. A future plan must
enable baseline consensus enrichment and verify that same-period actuals are present
before the exact-threshold classification rules are inserted.

## Verification

All themes were verified deterministically:
- `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs` covers the
  analytical themes port with the table-driven assertion
  `analytical_theme_port_coverage_matrix_remains_intact`, which proves the
  intended theme-to-role mapping across every receiving prompt.
- Output-shape proof lives at validator seams rather than in the prompt-byte
  gate. The specific deterministic tests are:
  `summary_accepts_unsourced_numeric_marker`,
  `summary_preserves_transcript_degraded_mode_notice`,
  `summary_preserves_news_discovered_catalyst_degraded_mode_notice`,
  `rationale_accepts_unsourced_numeric_marker`,
  `consensus_containing_buy_is_valid_content`,
  `validate_consensus_summary_accepts_hold`, and
  `consensus_containing_sell_is_valid_content`.
- Shared prompt partial extraction preserved rendered prompt bytes everywhere
  except the intentional Trader wording fix from `summary` to `rationale`; the
  only regenerated golden fixture in this follow-up was
  `crates/scorpio-core/tests/fixtures/prompt_bundle/trader.txt`.
- Fresh workspace verification for the review-hardening follow-up completed with:
  `cargo fmt -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo nextest run --workspace --all-features --locked --no-fail-fast`.

### Lesson: encode multi-role coverage once as a matrix

Three themes were inserted into multiple files, but the initial tests only covered the
primary role for each. Code review (ce:review) surfaced these as P1/P2 gaps:

| Theme | Files receiving the insert | Initial test | Gap |
|-------|---------------------------|-------------|-----|
| A (Valuation Sanity Bands) | `fundamental_analyst.md`, `conservative_risk.md` | FundamentalAnalyst only | ConservativeRisk uncovered |
| C (Management Red Flags) | `news_analyst.md`, `sentiment_analyst.md`, `conservative_risk.md` | NewsAnalyst only | SentimentAnalyst + ConservativeRisk uncovered |
| F (Contrarian Position Rule) | `bullish_researcher.md`, `aggressive_risk.md` | BullishResearcher only | AggressiveRisk uncovered |

**Rule:** when a plan entry specifies "insert X into file A AND file B", the test suite must
assert all receiving roles. Encode the mapping once as a coverage matrix so the plan remains
the authoritative checklist and new receiving roles extend one table instead of one-off tests:

```rust
const ANALYTICAL_THEME_PORT_COVERAGE: &[ThemeCoverageCase] = &[
    ThemeCoverageCase {
        theme: "Theme C management red flags degraded mode",
        roles: &[
            Role::NewsAnalyst,
            Role::SentimentAnalyst,
            Role::ConservativeRisk,
        ],
        required_markers: &[
            "Management Commentary Red Flags",
            "degraded mode: headline/summary only",
        ],
    },
];
```

A missing insertion into a secondary file is a **silent defect**: the build passes, tests
pass, and the agent simply runs with an incomplete system prompt — no compile-time or
runtime signal. The plan document is the authoritative mapping from theme to target files;
treat it as the test coverage checklist.

### Lesson: split prompt-byte proof from output-shape proof

Prompt-byte coverage and output-shape coverage are different failure surfaces.

- The prompt regression gate should answer: "did every receiving role get the
  required doctrine or taxonomy text?"
- Validator-seam tests should answer: "does downstream parsing and validation
  still accept the policy-required output phrases?"

That split keeps the prompt gate stable and cheap while still giving explicit
proof for phrases that matter at runtime, such as `[UNSOURCED]`, degraded-mode
disclosures, and `Buy` / `Sell` / `Hold` moderator summaries.

### Lesson: extract repeated doctrine into shared prompt partials

If the same prompt doctrine block is copied into multiple prompt files, pull it
into a shared partial and compose it from the pack builder.

In this slice:

- `theme_h_sourcing_and_untrusted.md` holds the shared Theme H doctrine, and
  `baseline.rs` substitutes the role-specific output field (`summary` vs
  `rationale`) at composition time
- `theme_c_management_red_flags.md` feeds News and Sentiment because those two
  prompts shared the exact same degraded-mode analyst wording

This keeps future wording fixes local to one file and prevents prompt doctrine
from drifting across roles that are supposed to stay identical.

See also: `docs/solutions/logic-errors/shared-options-evidence-regression-2026-04-29.md`
for the related fixture-regeneration mechanics.

## Open items

- Theme C ships degraded pending a future transcripts plan (Milestone 7 `TranscriptEvidence`
  provider). The `TODO(transcripts)` markers in the three affected prompts are the upgrade
  seam.
- Theme G ships with Tier 1 of the catalyst-calendar plan already wired. Tier 3
  (FDA AdComm, S-1 lockup, DEF M14A expected-close) remains deferred in
  `2026-05-10-003-catalyst-calendar-integration.md`.
- Theme D is explicitly deferred — see audit result above.
- Runtime enforcement of `[UNSOURCED]` and degraded-mode disclosures is intentionally
  out of scope; it is a separate renderer/hardening follow-up.
- Structured thesis-memory extensions (storing surviving pillars across runs in
  `ThesisMemory`) are explicitly out of scope and would require a
  `THESIS_MEMORY_SCHEMA_VERSION` bump.
