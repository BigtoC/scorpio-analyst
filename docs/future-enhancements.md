# Future Enhancements

This document records intentionally deferred enhancements that appear in design specs but are out of scope for the
current implementation phase.

Use it to keep promising ideas visible without expanding MVP scope too early.

## When to update this doc

Update this file whenever a design spec explicitly calls out a future enhancement, deferred trade-off, or post-MVP
follow-up that should be revisited later.

## Deferred Enhancements

> Will be considered for implementation after the MVP is complete and stable enough to evaluate whether the added
> complexity is justified by the benefits.

### Per-agent provider overrides

- **Status**: Deferred until after the MVP is finished
- **Source**: `openspec/changes/add-llm-providers/design.md`
- **Current baseline**: The provider layer uses tier-level providers (`llm.quick_thinking_provider`,
  `llm.deep_thinking_provider`) and tier-level models (`QuickThinking` / `DeepThinking`), not per-agent overrides.
- **Why it was deferred**: Tier-level provider selection keeps config and key management simpler than fully per-agent
  routing while still allowing quick/deep tiers to use different backends.
- **Why revisit later**: Different agents may eventually benefit from different providers, cost/performance profiles, or
  provider-specific capabilities.
- **Intentionally deferred details**:
    - Exact config shape for per-agent overrides
    - Override precedence rules
    - Validation and fallback behavior
    - Any migration path from the MVP config model
- **Revisit trigger**: After the MVP provider, agent, and workflow layers are stable enough to evaluate whether
  mixed-provider routing is worth the added complexity

### Copilot heuristic token estimation

- **Status**: Deferred until after the MVP is finished
- **Source**: `openspec/changes/add-copilot-provider/design.md`
- **Current baseline**: GitHub Copilot via ACP does not expose authoritative provider token counts. MVP records
  authoritative latency, and token count fields are treated as unavailable/not reported metadata for Copilot-backed
  calls.
- **Why it was deferred**: A client-side estimate can only be derived from visible prompt/response text and would miss
  hidden system prompts, backend prompt rewrites, model/tokenizer differences, and other provider-side accounting.
- **Why revisit later**: Approximate token estimates may still be useful for rough budgeting or comparative UX if they
  are clearly labeled as heuristic-only.
- **Intentionally deferred details**:
    - Whether estimates should be shown in CLI/TUI/GPUI by default or only in verbose/debug views
    - Which tokenizer or model-family fallback to use when Copilot does not expose a stable backend model ID
    - How to separate approximate estimates from authoritative provider-reported counts in summaries and exports
    - Whether aggregate totals should exclude heuristic estimates by default to preserve auditability
- **Revisit trigger**: After the MVP token-usage reporting and Copilot provider behavior are stable enough to evaluate
  whether approximate estimates add enough value to justify the extra complexity and caveats
