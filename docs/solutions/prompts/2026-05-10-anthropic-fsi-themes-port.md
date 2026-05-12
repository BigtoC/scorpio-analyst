---
title: Analytical frameworks port from anthropics/financial-services
date: 2026-05-12
last_updated: 2026-05-12
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
- 23 `#[test]` functions total live in `crates/scorpio-core/tests/prompt_bundle_regression_gate.rs`.
- 11 theme-port assertions in that file verify the exact required strings in each rendered
  role prompt (8 initial + 3 added by code review to close secondary-role coverage gaps —
  see lesson below).
- Golden fixtures regenerated with `UPDATE_FIXTURES=1` after all inserts.
- Full workspace test suite: 1803/1803 tests pass.

### Lesson: assert in every receiving role, not just the primary file

Three themes were inserted into multiple files, but the initial tests only covered the
primary role for each. Code review (ce:review) surfaced these as P1/P2 gaps:

| Theme | Files receiving the insert | Initial test | Gap |
|-------|---------------------------|-------------|-----|
| A (Valuation Sanity Bands) | `fundamental_analyst.md`, `conservative_risk.md` | FundamentalAnalyst only | ConservativeRisk uncovered |
| C (Management Red Flags) | `news_analyst.md`, `sentiment_analyst.md`, `conservative_risk.md` | NewsAnalyst only | SentimentAnalyst + ConservativeRisk uncovered |
| F (Contrarian Position Rule) | `bullish_researcher.md`, `aggressive_risk.md` | BullishResearcher only | AggressiveRisk uncovered |

**Rule:** when a plan entry specifies "insert X into file A AND file B", the test suite must
assert all receiving roles. Use a loop over all target roles to make coverage explicit and
easy to extend:

```rust
#[test]
fn sentiment_analyst_and_conservative_risk_prompts_include_management_red_flags() {
    for role in [Role::SentimentAnalyst, Role::ConservativeRisk] {
        let p = render_baseline_prompt_for_role(role, PromptRenderScenario::AllInputsPresent);
        assert!(
            p.contains("Management Commentary Red Flags"),
            "Theme C management red flags missing from {role:?}",
        );
    }
}
```

A missing insertion into a secondary file is a **silent defect**: the build passes, tests
pass, and the agent simply runs with an incomplete system prompt — no compile-time or
runtime signal. The plan document is the authoritative mapping from theme to target files;
treat it as the test coverage checklist.

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
