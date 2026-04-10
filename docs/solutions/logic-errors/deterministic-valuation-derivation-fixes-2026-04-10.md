---
title: Fix deterministic valuation derivation ordering, PEG consistency, and runtime timeout wiring
date: 2026-04-10
category: logic-errors
module: scenario-valuation-runtime
problem_type: logic_error
component: service_object
symptoms:
  - derived valuation changed with Yahoo response row order instead of newest balance/share data
  - PEG could combine forward EPS and growth from different earnings-trend rows
  - analyst runtime valuation fetch timeout wiring used unnecessary helper indirection and a fixed timeout instead of the config-backed budget
  - AnalystSyncTask lacked deterministic success-path coverage with injected Yahoo financial responses
  - non-positive valuation inputs needed explicit guard coverage to prevent invalid derived metrics
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - testing_framework
  - tooling
tags:
  - valuation
  - determinism
  - analyst-runtime
  - yfinance
  - peg-ratio
  - rust
---

# Fix deterministic valuation follow-up regressions

## Problem
Chunk 3's deterministic valuation runtime still had follow-up review defects after the main implementation landed. The runtime timeout path was not fully config-backed, valuation math still depended on provider row ordering in some cases, and PEG derivation could combine EPS and growth from different earnings-trend rows.

Those issues were risky because they could silently distort derived valuation inputs that the trader and downstream phases rely on.

## Symptoms
- Changing timeout config did not cleanly control the valuation fetch budget used by `AnalystSyncTask::with_yfinance(...)`.
- DCF and EV/EBITDA could vary based on Yahoo row ordering instead of actual newest balance/share data.
- PEG could look valid while using forward EPS from one row and growth from another row.
- The no-network degradation path was covered, but the deterministic runtime success path was not.
- Non-positive FCF, operating income, price, forward EPS, or growth needed explicit regression coverage.

## What Didn't Work
- The first runtime cleanup kept an unnecessary wrapper around the valuation timeout expression and used a fixed `30` second duration. That duplicated policy outside config and added indirection without behavior.

```rust
// Rejected direction
AnalystSyncTask::with_yfinance(store, yfinance, Duration::from_secs(30))
```

- Earlier valuation selection logic implicitly trusted provider ordering with patterns like `find(...)` and `last()`, and the existing `AnalystSyncTask` integration test only covered the no-network degradation path. That left the real success path and ordering bugs unproven.

## Solution
### Wire the runtime timeout directly from config
Add `valuation_fetch_timeout_secs` to `LlmConfig`, set the default in `config.toml`, and pass it directly into runtime wiring instead of wrapping it in a helper:

```rust
let analyst_sync = AnalystSyncTask::with_yfinance(
    Arc::clone(&snapshot_store),
    yfinance.clone(),
    Duration::from_secs(config.llm.valuation_fetch_timeout_secs),
);
```

### Select newest balance and share data by domain keys, not list order
In `src/state/valuation_derive.rs`, choose the latest usable balance row by parsed statement period and choose the latest share count by max `date`:

```rust
fn select_latest_balance_row(
    rows: &[BalanceSheetRow],
    predicate: impl Fn(&BalanceSheetRow) -> bool,
) -> Option<&BalanceSheetRow> {
    rows
        .iter()
        .filter(|row| predicate(row))
        .filter_map(|row| parse_statement_period_key(&row.period.to_string()).map(|key| (key, row)))
        .max_by_key(|(key, _)| *key)
        .map(|(_, row)| row)
        .or_else(|| rows.iter().find(|row| predicate(row)))
}
```

```rust
shares
    .and_then(|share_counts| {
        share_counts
            .iter()
            .filter(|share_count| share_count.shares > 0)
            .max_by_key(|share_count| share_count.date)
    })
    .map(|sc| sc.shares)
    .filter(|&s| s > 0)
```

### Use one selected forward row for both forward P/E and PEG
Select the forward earnings row once and thread it through both computations so PEG cannot mix horizons:

```rust
let forward_row = earnings_trend.and_then(select_forward_eps_row);
let forward_pe = compute_forward_pe(forward_row, current_price);
let peg = compute_peg(forward_pe.as_ref(), forward_row);
```

```rust
fn compute_peg(
    forward_pe: Option<&ForwardPeValuation>,
    forward_row: Option<&EarningsTrendRow>,
) -> Option<PegValuation> {
    let pe = forward_pe?;
    let forward_row = forward_row?;

    let growth_decimal = forward_row
        .earnings_estimate
        .growth
        .or(forward_row.growth)
        .filter(|&g| g > 0.0)?;

    let growth_pct = growth_decimal * 100.0;
    let peg_ratio = pe.forward_pe / growth_pct;

    Some(PegValuation { peg_ratio })
}
```

### Add a narrow test-only Yahoo seam for deterministic runtime coverage
Add a test-only stub constructor in `src/data/yfinance/ohlcv.rs`, then short-circuit financial fetchers under `#[cfg(test)]` in `src/data/yfinance/financials.rs`:

```rust
#[cfg(test)]
pub fn with_stubbed_financials(responses: StubbedFinancialResponses) -> Self {
    Self {
        session: YfSession::default(),
        cache: Arc::new(RwLock::new(HashMap::new())),
        stubbed_financials: Some(Arc::new(responses)),
    }
}
```

That made it possible to cover `AnalystSyncTask::with_yfinance(...)` deterministically in `src/workflow/tasks/tests.rs` without live network calls.

### Lock in safe degradation for non-positive inputs
Keep the valuation guards explicit and covered by tests so invalid inputs degrade to `None` instead of producing bogus ratios.

## Why This Works
The fixes remove hidden policy and hidden assumptions. Timeout behavior now comes from config, recency comes from parsed period/date keys instead of provider ordering, and PEG uses one selected forward row for both EPS and growth.

The test-only Yahoo seam keeps production behavior unchanged while making the real runtime success path deterministic under test. Combined with the new non-positive input guards, that gives the valuation layer both safer math and better regression coverage.

## Prevention
- Keep runtime policy in config-backed fields instead of helper-wrapped literals.
- When using provider row collections, derive recency from parsed period/date keys rather than `first()`, `find()`, or `last()` assumptions.
- If two derived metrics must share a forecast horizon, select the source row once and pass it through both computations.
- Preserve regression tests for the exact failure modes:
  - `derive_valuation_uses_newest_balance_and_share_rows_regardless_of_provider_order`
  - `derive_valuation_does_not_mix_forward_eps_and_growth_from_different_trend_rows`
  - `analyst_sync_with_stubbed_yfinance_sets_corporate_equity_valuation_on_state`
  - non-positive input tests for DCF, EV/EBITDA, forward P/E, and PEG

## Related Issues
- Related learning: `docs/solutions/logic-errors/stale-trading-state-evidence-and-unavailable-data-quality-fallbacks-2026-04-07.md`
