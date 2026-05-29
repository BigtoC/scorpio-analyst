---
title: Make a constructor infallible when its only failure is process-fatal
date: 2026-05-29
category: design-patterns
module: scorpio-core
problem_type: design_pattern
component: service_object
severity: low
applies_when:
  - "Writing a client/service-object constructor whose only failure path is process-fatal (e.g. reqwest TLS backend init)"
  - "Tempted to return Result or hold an Option<Client> to gracefully degrade when the rest of the process is equally doomed"
  - "The constructor takes no required input (no API key) and has no other fallible step"
  - "An Option field or match fallback exists solely because a downstream constructor returns Result"
tags:
  - rust
  - constructor
  - infallible
  - error-handling
  - reqwest
  - api-design
  - simplicity
  - data-adapters
---

# Make a constructor infallible when its only failure is process-fatal

## Context

`SecEdgarClient::new(limiter) -> Result<Self, TradingError>` returned a `Result`
whose **only** failure path was `reqwest::Client::builder().build()` failing.
That build only fails when the system TLS backend can't initialize — virtually
impossible, and uniformly **process-fatal**: every other HTTP client in the
workspace (Finnhub, FRED, yfinance) fails the same way under that condition, and
SEC EDGAR requires no API key, so there was no other legitimate reason for `new`
to fail.

Returning that `Result` was not harmless. It forced defensive scaffolding at
call sites that modeled an unreachable failure as if it were recoverable:

- The catalyst provider carried `sec_edgar: Option<SecEdgar8kProvider>`, with a
  `None` "EDGAR-unavailable" branch in `build_catalyst_provider` (a `match` on
  the `Result`). That fallback was **illusory** — if EDGAR's reqwest client
  can't build, nothing in the process can make HTTP calls anyway.
- The codebase was already internally inconsistent: the graph path
  `build_default_sec_edgar_client` (`workflow/pipeline/mod.rs`) treated `new` as
  never-failing via `.expect(...)`, with a comment admitting it was "virtually
  impossible in practice." Two construction paths, one constructor,
  contradictory handling — the disagreement *was* the signal that the `Result`
  was wrong.

The pattern: **a `Result` whose only `Err` is a process-fatal, environment-level
impossibility is a false affordance. It advertises a recovery path that cannot
exist, and that phantom path metastasizes into `Option` fields and dead fallback
branches across every caller.**

## Guidance

When a constructor's *sole* failure mode is a process-fatal, environment-level
impossibility (TLS backend init, allocator failure) — and there is no key
validation, no parsing, no other fallible step — make it infallible. Return
`Self`, not `Result<Self, _>`.

The failure does not disappear; it collapses into the same fatal condition that
`reqwest::Client::new()` itself panics on. You are not hiding an error — you are
declining to model an unrecoverable, universal failure as a per-call-site
recoverable one.

```rust
// BEFORE — Result whose only Err is unreachable in practice
pub fn new(limiter: SharedRateLimiter) -> Result<Self, TradingError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(SEC_EDGAR_USER_AGENT)
        .build()
        .map_err(|e| {
            TradingError::Config(anyhow::anyhow!("SEC EDGAR reqwest client build: {e}"))
        })?;
    Ok(Self { http: Arc::new(ReqwestEdgarHttp { client }), limiter, /* ... */ })
}

// AFTER — infallible; the fatal condition stays fatal, but isn't modeled as recoverable
pub fn new(limiter: SharedRateLimiter) -> Self {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(SEC_EDGAR_USER_AGENT)
        .build()
        // Client::new() itself panics on a broken TLS stack — same fatal condition, no new risk.
        .unwrap_or_else(|_| reqwest::Client::new());
    Self { http: Arc::new(ReqwestEdgarHttp { client }), limiter, /* ... */ }
}
```

This matched an **existing convention in the same codebase**: `yfinance/summary`'s
`SummaryHttp::new()` (also no API key) already used
`.build().unwrap_or_else(|_| Client::new())` and returned `Self`. Prefer aligning
with a sibling that already made the right call over inventing a third style.

Removing the `Result` at the source lets the simplification cascade downstream —
the field stops being optional, the fallback branch disappears, and the builder
plus `?`/`.expect(...)` vanish from call sites:

```rust
// BEFORE — Option field + illusory fallback driven by the constructor's Result
pub struct CatalystProvider {
    sec_edgar: Option<SecEdgar8kProvider>,
    // ...
}

let provider = CatalystProvider::with_timeout(finnhub.clone(), fred.clone(), source_timeout);
match SecEdgarClient::new(SharedRateLimiter::new("sec-edgar", 10)) {
    Ok(edgar) => Arc::new(provider.with_sec_edgar(SecEdgar8kProvider::new(edgar))),
    Err(reason) => {
        // "graceful degradation" that can never run usefully — if reqwest can't
        // build here, no other provider can fetch either.
        info!(reason = %reason, "SEC EDGAR unavailable; Finnhub + FRED + yfinance only");
        Arc::new(provider)
    }
}

// AFTER — required field, no branch, no builder
pub struct CatalystProvider {
    sec_edgar: SecEdgar8kProvider,
    // ...
}

// `sec_edgar_limiter` is built once from RateLimitConfig and shared (cloned —
// it is Arc-backed) across both EDGAR clients; the constructor takes it as a
// parameter, so there is no hardcoded limiter and no fallback branch.
let sec_edgar = SecEdgar8kProvider::new(SecEdgarClient::new(sec_edgar_limiter));
Arc::new(CatalystProvider::with_timeout(finnhub.clone(), fred.clone(), sec_edgar, source_timeout))
```

The `with_sec_edgar` builder was deleted; `build_default_sec_edgar_client` dropped
its `.expect(...)`; ~14 call sites (tests, examples) dropped `?`/`.expect(...)`;
and the now-vacuous test `new_constructs_successfully_with_valid_limiter`
(asserting `.is_ok()`) was deleted — a test that can never observe `Err` tests
nothing.

## Why This Matters

- **A `Result` is an API contract that promises a meaningful `Err`.** When the
  only `Err` is "the universe is on fire," the contract lies. Every caller must
  handle a branch that can't happen, and the honest ones can't even write a real
  recovery — so they invent fake ones.
- **Phantom errors metastasize.** One unnecessary `Result` became an `Option`
  field, a `match` fallback, a builder method, `.expect(...)` in one path, `?` in
  ~14 others, and a vacuous test. Infallibility at the source deletes all of it.
- **Illusory fallbacks are a correctness hazard, not safety.** The
  `None`/"EDGAR-unavailable" branch looked like graceful degradation but could
  never run usefully. It added cognitive load and a dead path masquerading as
  resilience.
- **Internal inconsistency is a smell pointing at the root cause.** One path
  `.expect(...)`-ed while another `match`-ed the same constructor. Resolve such
  splits by fixing the source, not by picking a winner.

This is a concrete embodiment of CLAUDE.md §2 ("No error handling for impossible
scenarios"), and the error-handling sibling of
[[no-write-only-placeholder-fields]] — that rule deletes redundant *data* slots
nothing reads; this pattern deletes redundant *error-handling* slots no caller
can act on.

## When to Apply

Make the constructor infallible when **all** hold:

- The **only** failure path is a process-fatal, environment-level impossibility
  (HTTP/TLS client build, allocator) that uniformly dooms the whole process.
- There is **no other fallible step** — no required API-key validation, no
  parsing, no I/O that can legitimately fail per-instance.
- A **sibling in the same codebase** already treats the identical condition as
  infallible (here, `SummaryHttp::new`), or you are establishing that convention
  deliberately.

**Do NOT apply when the `Result` carries a real, recoverable `Err`.** `fred.rs`
and `alpha_vantage.rs` constructors legitimately return `Result` because a
required API key may be missing (`SCORPIO_FRED_API_KEY is not set`); the reqwest
build error simply folds into an already-needed `Result`. One real per-instance
failure reason justifies the `Result`, and the unreachable TLS error then rides
along for free.

**Watch for the test-isolation side effect.** Making an optional dependency
mandatory can pull network I/O into previously-isolated unit tests. Once
`sec_edgar` was always present, a full `fetch_catalysts` fanned out to SEC EDGAR
(network). Two tests had to stay network-free: the invalid-date test returns
before the `tokio::join!` fan-out is awaited, and
`calendar_catalysts_exclude_events_beyond_horizon_end` was rewritten to call the
pure `CatalystProvider::calendar_catalysts(...)` directly instead of routing
through `fetch_catalysts`. When you remove an `Option`, audit the unit tests that
relied on the dependency being absent — test the pure unit directly.

## Examples

**Example 1 — Apply (the SEC EDGAR case).** Sole failure is the unreachable TLS
build; no API key; a sibling (`SummaryHttp::new`) already returns `Self`. Make
`new` infallible (see Guidance) and let the `Option`/`match`/builder/`?`/`.expect`
cascade collapse.

**Example 2 — Do NOT apply (FRED, required key).** The `Result` is load-bearing
because the key check is a real per-instance failure:

```rust
// KEEP the Result — a missing key is a genuine, recoverable Err
pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError> {
    let key = api.fred_api_key.as_ref().ok_or_else(|| {
        TradingError::Config(anyhow::anyhow!("SCORPIO_FRED_API_KEY is not set"))
    })?;
    Ok(Self {
        http: Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| TradingError::Config(anyhow::anyhow!("reqwest client build: {e}")))?,
        api_key: key.clone(),
        limiter,
    })
}
```

**Example 3 — The resulting test-isolation fix.** Don't route a pure-logic
assertion through an entry point that now performs network fan-out; call the pure
method directly:

```rust
// BEFORE — went through fetch_catalysts, which now fans out to SEC EDGAR (network)
let events = provider
    .fetch_catalysts("AAPL", "2026-01-15", 30, Some(calendar))
    .await
    .expect("calendar mapping must succeed");

// AFTER — exercise the pure in-memory horizon filter, no network
let as_of = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
let horizon_end = as_of + Duration::days(30);
let events = CatalystProvider::calendar_catalysts(Some(&calendar), "AAPL", as_of, horizon_end);
```

**The diagnostic question:** *"If this `Err` fired, could any caller do something
other than crash?"* If the only honest answer is "no — and nothing else in the
process would work either," the `Result` is a false affordance. Remove it at the
source and let the call sites simplify.

## Related

- [[no-write-only-placeholder-fields]] — `.claude/rules/no-write-only-placeholder-fields.md`. Sibling rule: deletes redundant data slots nothing reads. This pattern is its error-handling analog.
- `docs/solutions/architecture-patterns/share-yfinance-info-across-pipeline.md` — same family of simplification: centralize at the source so downstream `Option`/duplicate slots disappear.
- `docs/solutions/data-sources/2026-05-10-catalyst-calendar.md` — the catalyst provider implementation. **Now stale**: it documents the `Tier1CatalystProvider`/`Tier2CatalystProvider` split (merged into a single `CatalystProvider`) and calls `SecEdgarClient::new` "virtually always succeeds" (now infallible). Needs a refresh.
- CLAUDE.md §2 "Simplicity First" — "No error handling for impossible scenarios."
