---
title: "Futu OpenD positions unavailable — GetAccList omits unified accounts and plRatio wire scale varies"
date: 2026-06-02
category: integration-issues
module: data/futu
problem_type: integration_issue
component: data_pipeline
symptoms:
  - "`analyze <SYMBOL>` reports \"Account Positions: unavailable (configured account is not available for this market)\" despite the Futu account holding that symbol"
  - "Configured `SCORPIO__FUTU__ACCOUNT` (uniCardNum shown in the Futu mobile app) never matches any account returned by OpenD"
  - "GetAccList (protocol 2001) returns only 15 legacy per-market accounts instead of the 18 visible in-app; none carry the US-authorized uniCardNum"
  - "Exhaustive probe of every visible account × market × currency finds zero positions/assets, wrongly suggesting the OpenD login holds nothing"
  - "plRatio rendered at the wrong magnitude (+23.6% would show as +0.236%) when unconditionally dividing by 100"
root_cause: wrong_api
resolution_type: code_fix
severity: high
related_components:
  - futu-opend
  - account-selector
tags: [futu, opend, integration, trade-api, wire-protocol, unified-account, pl-ratio, rust]
---

# Futu OpenD positions unavailable — GetAccList omits unified accounts and plRatio wire scale varies

## Problem

`cargo run -p scorpio-cli -- analyze <SYMBOL>` reported `Account Positions: unavailable (configured account is not available for this market)` even though the Futu account held the analyzed symbol and `SCORPIO__FUTU__ACCOUNT` was set to the universal account number. The `Trd_GetAccList` (2001) request omitted the optional `needGeneralSecAccount` flag, so OpenD silently excluded all unified (general securities) account rows — including the one whose `uniCardNum` matched the configured account. A second latent defect: `plRatio` was assumed to be percentage-scaled per the proto comment, but live unified accounts return a fraction, which would have rendered P/L 100× too small.

## Symptoms

- Report line: `Account Positions: unavailable (configured account is not available for this market)`.
- `SCORPIO__FUTU__ACCOUNT=<uniCardNum>` (the universal account number shown in the Futu mobile app) never resolved to an `accID`.
- `GetAccList` returned 15 account rows; the expected unified account (the row pairing the app-visible `uniCardNum` with its `accID`) was absent — 18 accounts exist, only 15 came back.

## What Didn't Work

- **Dead end #1 — blaming the selection gate.** The selector matched only a futures-only account (`uniCardNum` present, `trdMarketAuthList=[5]`), which the Real + US-authorization gate in `select_account` correctly skipped. Near-miss diagnostics added to the spike (`"matches SCORPIO__FUTU__ACCOUNT via uniCardNum, but skipped: not authorized for US"`) surfaced the symptom but not the cause — the *right* account simply wasn't in the list.
- **Dead end #2 — exhaustive row probing.** A live probe swept all 15 visible accounts (Real + simulated) × every authorized market + forced US/HK × every supported currency, plus the funds endpoint (2101). It found zero positions and zero assets everywhere, leading to the wrong conclusion "this OpenD login holds nothing." It failed because the account **list itself** was incomplete: probing the visible rows more deeply can never recover rows the list omitted. Depth cannot compensate for breadth missing at the list level.

## Solution

**Root cause #1 — always request unified accounts.** `GetAccListC2S` gained a `need_general_sec_account` field, hard-set to `true` (`crates/scorpio-core/src/data/futu/messages.rs`):

```rust
struct GetAccListC2S {
    #[serde(rename = "userID")]
    user_id: u64,
    /// Include unified (general securities) accounts — the rows whose
    /// `uniCardNum` matches the account number shown in the Futu app. Without
    /// this flag OpenD omits them, returning only legacy per-market accounts.
    need_general_sec_account: bool,
}

pub(crate) fn serialize_get_acc_list(login_user_id: u64) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: GetAccListC2S {
            user_id: login_user_id,
            need_general_sec_account: true,
        },
    })
    // ...
}
```

Before, the body was `{"c2s":{"userID":0}}`; after, `{"c2s":{"userID":0,"needGeneralSecAccount":true}}`. Live OpenD then returned the unified account (REAL, raw OpenD market-auth codes `[1,2,4,6,113,123,15]` — only 1/2/3/5 have labels in `trd_market_label`), the configured `uniCardNum` resolved to its `accID`, and `GetPositionList` returned the account's position rows with no `currency` parameter needed.

**Root cause #2 — disambiguate `plRatio` scale per row instead of trusting the proto** (`crates/scorpio-core/src/data/futu/select.rs`):

```rust
fn normalize_pl_ratio(row: &PositionListItem) -> Option<f64> {
    let pl_ratio = row.pl_ratio?;
    if let (Some(pl_val), Some(cost)) = (row.pl_val, row.cost_price) {
        let basis = cost * row.qty;
        if basis.abs() > f64::EPSILON {
            let computed = pl_val / basis;
            if (pl_ratio / 100.0 - computed).abs() < (pl_ratio - computed).abs() {
                return Some(pl_ratio / 100.0);
            }
        }
    }
    Some(pl_ratio) // no basis → keep raw as fraction (only scale observed live)
}
```

The earlier approach unconditionally divided by 100. The fix compares `plRatio` and `plRatio/100` against the unit-unambiguous `plVal / (costPrice × qty)` and keeps whichever is closer; with no cost basis it keeps the raw value as a fraction.

## Why This Works

- `needGeneralSecAccount` is documented on [Trd_GetAccList](https://openapi.futunn.com/futu-api-doc/trade/get-acc-list.html) as "whether to return unified accounts (HK/US/SG/AU systems)." Unified accounts are exactly the ones whose `uniCardNum` matches the mobile-app display — which is what users naturally configure. Omitting the flag made OpenD default to legacy per-market rows only, so the selector's lookup in `account_matches` (`uni_card_num == wanted || acc_id == wanted`) could never find the configured account. Sending the flag puts the target row in the list, and selection/positions then work end-to-end.
- For `plRatio`, the wire scale differs across account generations: the proto comment describes a percentage, but unified accounts observably return a fraction (every live row in the capture). `plVal / (costPrice × qty)` is a fraction by construction and independent of how `plRatio` is encoded, so it is a reliable arbiter of scale. The percent assumption had never been observed against live rows — all earlier captures had 0 rows, so it derived solely from the proto comment.

## Prevention

- **When a LIST endpoint seems to be missing expected rows, check for optional request flags/filters that gate row visibility** before exhaustively probing visible rows or concluding the data doesn't exist. Diagnose breadth (is the list complete?) before depth. The spike now sends the flag and documents the finding (`crates/scorpio-core/examples/futu_opend_smoke.rs` module doc).
- **Validate wire-field units against a derivable quantity rather than trusting doc/proto comments.** Scale can differ across API generations of the same endpoint. The spike emits a per-row vote count, never holdings:

  ```rust
  let computed = pl_val / basis; // a fraction by construction
  if (pl_ratio - computed).abs() < (pl_ratio / 100.0 - computed).abs() {
      fraction_votes += 1;
  } else {
      percent_votes += 1;
  }
  ```

- **Keep a sanitized live "spike" example in sync with production assumptions.** `examples/futu_opend_smoke.rs` prints field names/types and vote counts but never raw holdings; it caught both bugs and is the verification harness for them.
- **Tests cover all three `plRatio` branches** (`select.rs`): percent-scale rows convert to fraction (`23.6 → 0.236`), fraction-scale rows pass through unscaled (`0.236 → 0.236`), and the no-basis fallback keeps the raw value (`0.5 → 0.5`). The request serialization is pinned by `serializes_get_acc_list_request_with_user_id_and_general_sec_accounts`, which asserts `v["c2s"]["needGeneralSecAccount"] == true`.
- Verification gate: `cargo fmt` + `cargo clippy -D warnings` clean, `cargo nextest` 2201/2201, live spike (selector resolves the unified account; every position row voted fraction, `percent_votes=0`), and end-to-end `analyze <SYMBOL>` rendering a populated `Account Positions (US/USD): hold <SYMBOL> … P/L …` line with the correct percentage magnitude.

## Related Issues

- [finnhub-earnings-release-quarter-semantics](../logic-errors/finnhub-earnings-release-quarter-semantics-2026-05-16.md) — prior instance of the same anti-pattern: a comment asserted wrong field semantics, and the fix came from comparing real upstream data against a derivable ground truth. This doc's wire-unit lesson is the units-domain twin of that doc's quarter-semantics lesson.
- [catalyst-calendar](../data-sources/2026-05-10-catalyst-calendar.md) — vendor integration behind a fail-soft seam with the same "unavailable/degraded mode" framing.
- `.claude/rules/mock-at-the-right-seam-not-in-production.md` — `normalize_pl_ratio`, `select_account`, and `assemble_snapshot` are tested as pure functions (seam option 1), consistent with that rule.
- Planning artifacts superseded on these two points: `docs/superpowers/plans/2026-06-01-futu-position-integration.md` (blanket ÷100 plRatio note; spike GetAccList without the flag) and `docs/superpowers/specs/2026-06-01-futu-position-integration-design.md` (`Trd_GetAccList` C2S documented as just `userID`).
- Changed source: `crates/scorpio-core/src/data/futu/messages.rs`, `crates/scorpio-core/src/data/futu/select.rs`, `crates/scorpio-core/examples/futu_opend_smoke.rs`.
