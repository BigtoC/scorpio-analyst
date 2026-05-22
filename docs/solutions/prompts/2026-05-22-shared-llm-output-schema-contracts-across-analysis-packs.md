---
title: Centralize LLM output schema contracts across analysis packs
date: 2026-05-22
category: prompts
module: analysis_packs/common/prompts
problem_type: architecture_pattern
component: analysis_pack_prompts
severity: high
applies_when:
  - "Adding a new analysis pack that emits serde-typed JSON via LLM (RiskReport, TradeProposal, ExecutionStatus)"
  - "Authoring or editing prompts for risk, trader, or fund_manager roles"
  - "Changing a serde enum variant or struct field consumed by an LLM JSON response"
  - "Modifying validator logic that parses literal string prefixes out of an LLM rationale (e.g. `Dual-risk escalation:`)"
  - "Tempted to copy-paste a `Return ONLY a JSON object matching ...` block between prompt files"
related_components:
  - equity_baseline_pack
  - etf_baseline_pack
  - risk_report_schema
  - fund_manager_validator
tags:
  - prompt-engineering
  - schema-contract
  - analysis-packs
  - serde-json
  - llm-output-parsing
  - prompt-composition
  - risk-report
  - dual-risk-escalation
---

# Centralize LLM output schema contracts across analysis packs

## Context

While adding ETF Baseline as a second analysis pack alongside Equity Baseline, the ETF risk-stage cycle began aborting with a serde error mid-run:

```
graph-flow error in phase 'risk_discussion' task 'aggressive_risk':
schema violation: AggressiveRiskAgent: failed to parse RiskReport JSON:
unknown variant `high`, expected one of `Aggressive`, `Neutral`, `Conservative`
at line 2 column 22
```

The model was treating `RiskReport.risk_level` as a severity tag — the natural English reading — because the ETF pack's `aggressive_risk.md`, `neutral_risk.md`, and `conservative_risk.md` system prompts carried only pack-specific framing (ETF runtime contract, ETF failure modes, premium/discount anchors) and never restated the JSON schema. The equity pack's prompts had restated it inline; the ETF authors copied the framing pattern but not the schema block. The slot-completeness test only verified that each role had *a* prompt with the runtime placeholders — not that the prompt encoded the right output contract.

Investigating the failure surfaced a second drift of the same shape: the ETF `fund_manager.md` instructed the model to lead its output with `dual_risk_violation: <tag>` or `dual_risk_clear`, but the actual validator at `crates/scorpio-core/src/agents/fund_manager/validation.rs:175` parses `Dual-risk escalation: ...` as a prefix of `ExecutionStatus.rationale`. Nothing in code ever consumed the `dual_risk_violation:` strings. The prompt looked authoritative in review but had been decorative for the whole life of the ETF pack.

Both symptoms share one root cause: **structured-output contracts were duplicated (and silently diverged) across pack prompt bundles instead of living in one shared, composable snippet**.

A process root cause sits underneath it too (session history): the ETF baseline plan sequenced the "promote Tier-1/Tier-2 prompts to `common/`" tasks (Tasks 3-4) *after* all the feature tasks, treating shared-prompt extraction as cleanup rather than scaffolding. The 13 ETF prompt assets were therefore authored standalone, and the latent drift only surfaced once the pipeline actually ran against `SMH`. (session history)

## Guidance

Any serde-deserialized LLM output type shared across analysis packs (`RiskReport`, `TradeProposal`, `ExecutionStatus`, and anything that joins them later) must have **exactly one** prompt snippet describing its JSON shape, stored under `crates/scorpio-core/src/analysis_packs/common/prompts/`. Pack-specific prompts own only pack-specific framing — runtime contract, anchor sources, failure modes, role tone. The shared snippet is composed into each pack's `PromptBundle` at compile time via `include_str!` + `compose_prompt_sections`.

Snippet directory layout:

```
crates/scorpio-core/src/analysis_packs/common/prompts/
├── risk_report_output_contract.md          # {stance} placeholder
├── trade_proposal_output_contract.md
└── execution_status_output_contract.md     # incl. Dual-risk escalation prefix
```

When the contract has a per-call dimension (e.g. risk stance), use a placeholder substituted at **compose time**, not at LLM-runtime placeholder resolution. Compose-time substitution is what locks each slot to its actual variant — runtime placeholders are for values the agent code computes (ticker, current_date), not for stance labels that are bundle-static.

```rust
fn compose_equity_risk(raw: &'static str, stance: &str) -> Cow<'static, str> {
    let output_contract = RISK_REPORT_OUTPUT_CONTRACT.replace("{stance}", stance);
    Cow::Owned(compose_prompt_sections(raw, &[&output_contract]))
}

// PromptBundle wiring
aggressive_risk: compose_equity_risk(
    include_str!("prompts/aggressive_risk.md"),
    "Aggressive",
),
```

For prompts that consume the contract verbatim (trader, fund_manager), append via the existing section helper — no placeholder substitution needed:

```rust
trader: with_sections(
    include_str!("prompts/trader.md"),
    &[TRADE_PROPOSAL_OUTPUT_CONTRACT],
),
```

**Any literal string the validator parses out of an LLM response — rationale prefixes, sentinel tags, action keywords — is part of the contract and belongs in the shared snippet.** It does not belong in pack-specific framing, and it does not belong duplicated across packs.

## Why This Matters

Duplicated schema text is a silent-failure surface. Three failure modes compound:

- **Runtime serde aborts**: variant names drift to natural-English (`"high"` vs `Aggressive`) and the pipeline aborts mid-cycle, wasting an entire LLM debate round of tokens. The error only surfaces when the new pack actually runs end-to-end against a real ticker, not in unit tests.
- **Decorative prompt strings**: a prompt instructs the model to emit `dual_risk_violation:` and nothing parses it. Reviewers see "the prompt covers this," but the system behaves as if the instruction did not exist. Both the failure mode and the silent-success mode are invisible in completeness tests that only check `prompt.is_empty() == false`.
- **Bit rot across packs**: when the equity schema evolves (new field, renamed variant), the ETF pack stays on the old shape until a production failure surfaces the drift. Code review of one pack rarely flags the absence of a block in another.

Centralizing the contract turns these three failure classes into a one-line edit at a single path, plus a recompile that fans the change out to every pack. The companion lesson from session history (session history): treat shared-prompt extraction as **scaffolding**, not cleanup — do it before pack-specific prompts are authored, not after.

## When to Apply

Apply this pattern when **any one** of these holds:

- A serde-deserialized struct (or `serde(tag = ...)` enum) is emitted by an LLM and consumed by `serde_json::from_str` in agent code.
- Two or more analysis packs (or any prompt-bundle consumers) need to elicit the same output type.
- A validator parses a literal string prefix or sentinel out of an LLM response (e.g. `Dual-risk escalation:`, action keywords, status tags).
- A field has a closed enum variant set that does not match its English connotation (`risk_level`, `action`, `decision`).
- You are about to copy-paste a `Return ONLY a JSON object matching ...` block from one prompt file into another — stop and extract instead.

## Examples

**ETF `aggressive_risk.md` — before** (`crates/scorpio-core/src/analysis_packs/etf/prompts/aggressive_risk.md`): ETF runtime contract + ETF failure modes + premium/discount anchors. No schema block. Model emits `"risk_level": "high"`, serde rejects, cycle aborts.

**After**: same ETF-specific body, schema appended via compose:

```rust
aggressive_risk: compose_etf_risk(
    include_str!("prompts/aggressive_risk.md"),
    "Aggressive",
),
```

**`etf/baseline.rs` — before**:

```rust
fn compose_etf_risk(raw: &'static str) -> Cow<'static, str> {
    compose_etf_section(raw, ETF_RISK_DELTA)
}
```

**After** (`crates/scorpio-core/src/analysis_packs/etf/baseline.rs`):

```rust
fn compose_etf_risk(raw: &'static str, stance: &str) -> Cow<'static, str> {
    let contract = RISK_REPORT_OUTPUT_CONTRACT.replace("{stance}", stance);
    let body = compose_etf_section(raw, ETF_RISK_DELTA);
    Cow::Owned(compose_prompt_sections(&body, &[&contract]))
}
```

**Shared `risk_report_output_contract.md`** (`crates/scorpio-core/src/analysis_packs/common/prompts/risk_report_output_contract.md`, abbreviated):

```markdown
Return ONLY a JSON object matching `RiskReport`:
- `risk_level`: the exact string `{stance}` (NOT a severity like
  "high"/"medium"/"low" — this field names your debate stance)
- `assessment`: concise string explaining your view
- `recommended_adjustments`: array of concrete refinements
- `flags_violation`: boolean — true iff a hard rule is broken
```

**Fund-manager dual-risk drift — before** (ETF `fund_manager.md`):

```markdown
If both Conservative and Neutral flag a violation, lead with:
  dual_risk_violation: <tag>
Otherwise lead with: dual_risk_clear
```

Nothing parsed those strings. **After**, the shared `execution_status_output_contract.md` carries the validator-aligned form, and the ETF `fund_manager.md` keeps only the ETF-specific audit framing:

```markdown
When both Conservative and Neutral flag a violation, `rationale` MUST
start with the exact prefix `Dual-risk escalation: ` (one of: upheld,
deferred, overridden, indeterminate, stage-disabled). The validator at
agents/fund_manager/validation.rs:175 parses this prefix.
```

The slot-completeness test (`etf_baseline_populates_every_prompt_slot_with_runtime_placeholders` in `crates/scorpio-core/src/analysis_packs/etf/baseline.rs`) plus the 78-test `analysis_packs::` suite confirm composition preserves runtime placeholders (`{ticker}`, `{current_date}`) on every slot after the refactor.

## Related

- `docs/solutions/logic-errors/2026-04-21-fund-manager-dual-risk-prefix-contract-follow-up-fixes.md` — the single-instance precursor of the dual-risk prefix drift this learning generalizes. Treat that doc's prevention rule ("copy exact strings to prompt docs") as superseded by the architectural rule here: output-contract strings live in shared snippets, not duplicated in pack prompts.
- `docs/solutions/logic-errors/2026-04-25-prompt-bundle-centralization-runtime-contract.md` — centralized prompt *prose* via `AnalysisPackManifest.prompt_bundle` and `PreflightTask` authority. This learning extends the same DRY rule from prose to **serde output contracts**.
- `docs/solutions/logic-errors/2026-05-22-etf-runtime-policy-preseed-preflight-contract.md` — earlier ETF contract breach (preseed vs preflight ordering). Same family: pack-specific code paths must inherit cross-pack invariants, not redefine them.
- `docs/solutions/prompts/2026-05-10-anthropic-fsi-themes-port.md` — sibling prompt-pattern doc in the same directory; covers multi-role prompt consistency and falsifiability rather than output-schema contracts.
