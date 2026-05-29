---
description: "Remove struct fields that nothing reads when their value is already centralized elsewhere; don't keep them as perpetual `None` placeholders."
applyTo: "**/*.rs"
---

# No Write-Only Placeholder Fields

When data is centralized onto a single shared source (e.g. the per-cycle
`TradingState.yfinance_info` snapshot), **delete the now-redundant field from
the structs that used to carry it** instead of leaving it set to `None`.

A field that no code ever reads is dead code wearing a placeholder costume. It
is worse than ordinary dead code, because the next engineer who sees an empty
`Option` field naturally tries to "fill it in" — and the cheapest way to fill it
is to re-introduce the exact duplicate fetch that the centralization removed.
The `None` slot is a trap that quietly reverses the optimization.

## The rule

Remove the field when **both** are true:

1. **No code reads it.** Grep the workspace — every occurrence is the field
   definition plus `field: None` construction sites (production and tests). No
   `.field` read, no pattern match, no serialization consumer.
2. **The value is already available elsewhere.** It lives on a shared/centralized
   struct, so the field is a redundant duplicate slot, not a missing capability.

## When NOT to remove (keep the `None`)

This rule does **not** apply to genuine "upstream doesn't expose this yet"
placeholders:

- Fields on a **raw upstream struct** (e.g. `yfinance_rs::ticker::Info`) carried
  for future use — keep the whole raw struct intact; don't prune it. See
  [[prefers-simple-shared-structs]].
- Fields with **real readers** that are merely `None` when a best-effort
  secondary fetch fails (e.g. `EtfQuote::{nav, bid, ask}` — filled from the
  `quoteSummary` endpoint when available, read by the ETF valuator). These are
  optional data, not dead slots.

The distinguishing test is criterion #1: *does anything read it?* If a reader
exists, the `None` is legitimate optional data. If nothing reads it and the
value lives elsewhere, delete it.

## Worked example

`EtfQuote::market_cap` was left as `None` after market cap moved to the shared
`Info` snapshot (`key_statistics.market_cap`). Nothing read `EtfQuote.market_cap`
— it was the field definition, one `market_cap: None` in `get_quote`, and two
test constructors. It was removed entirely; the `get_quote` doc now points
readers to `state.yfinance_info.key_statistics.market_cap`.

See `docs/solutions/architecture-patterns/share-yfinance-info-across-pipeline.md`
for the centralization pattern this rule complements, and CLAUDE.md §2
"Simplicity First" / §3 "Surgical Changes".
