# Future Enhancements

This document records intentionally deferred enhancements that appear in design specs but are out of scope for the
current implementation phase.

Use it to keep promising ideas visible without expanding MVP scope too early.

## When to update this doc

Update this file whenever a design spec explicitly calls out a future enhancement, deferred trade-off, or post-MVP
follow-up that should be revisited later.

## Deferred Enhancements

### Per-agent provider overrides

- **Status**: Deferred until after the MVP is finished
- **Source**: `openspec/changes/add-llm-providers/design.md`
- **Current baseline**: The provider layer uses one `llm.default_provider` and selects models by tier (`QuickThinking` /
  `DeepThinking`), not by agent.
- **Why it was deferred**: A single provider keeps config, key management, testing, and provider-factory behavior
  simpler while the MVP is being established.
- **Why revisit later**: Different agents may eventually benefit from different providers, cost/performance profiles, or
  provider-specific capabilities.
- **Intentionally deferred details**:
    - Exact config shape for per-agent overrides
    - Override precedence rules
    - Validation and fallback behavior
    - Any migration path from the MVP config model
- **Revisit trigger**: After the MVP provider, agent, and workflow layers are stable enough to evaluate whether
  mixed-provider routing is worth the added complexity
