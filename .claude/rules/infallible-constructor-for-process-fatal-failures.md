---
description: "Return `Self`, not `Result`, from a constructor whose only failure path is process-fatal (e.g. reqwest/TLS client build); degrade the impossible error to a default instead of threading Result/Option/fallbacks through every call site."
applyTo: "**/*.rs"
---

# Infallible Constructors for Process-Fatal-Only Failures

When a constructor's *only* failure path is a process-fatal, environment-level
impossibility — `reqwest::Client::builder().build()` failing because the system
TLS backend can't initialize, an allocator failure, and the like — **return
`Self`, not `Result<Self, _>`.** Degrade the unreachable error to a sane default
(`reqwest::Client::new()`, which itself panics on that same condition) rather
than modeling it as recoverable.

A `Result` whose only `Err` is "the universe is on fire" is a false affordance.
It advertises a recovery path that cannot exist, and that phantom path
metastasizes into `Option` fields, `match` fallbacks, builders, and
`?`/`.expect(...)` across every caller. The "graceful degradation" branches are
illusory: if this constructor's HTTP client can't build, every other HTTP client
in the process is equally doomed, so nothing downstream could run anyway.

## The rule

Make the constructor infallible (`-> Self`) when **all** are true:

1. **The only failure is process-fatal and environment-level** (TLS/HTTP client
   build, allocator). It uniformly dooms the whole process, so no caller can
   meaningfully recover.
2. **There is no other fallible step** — no required-input validation (API key),
   no parsing, no per-instance I/O that can legitimately fail for one caller
   while succeeding for another.
3. **A sibling in the same codebase already treats the identical condition as
   infallible**, or you are establishing that convention deliberately.

Degrade the impossible error in place
(`.build().unwrap_or_else(|_| reqwest::Client::new())`) and let the
simplification cascade: `Option` fields become required, `match`/fallback
branches and `?`/`.expect(...)` disappear at call sites, and any test that only
asserted `.is_ok()` is deleted (a test that can never observe `Err` tests
nothing).

## When NOT to make it infallible (keep the `Result`)

Keep `Result<Self, _>` when the constructor carries a **real, recoverable**
`Err`:

- A **required API key may be missing** (`FredClient`, `AlphaVantageClient` —
  `SCORPIO_FRED_API_KEY is not set`). The reqwest build error just folds into the
  already-needed `Result`; the key check is what justifies it.
- Any **per-instance validation, parsing, or I/O** that can legitimately fail
  for one caller while succeeding for another.

The distinguishing test: *"If this `Err` fired, could any caller do something
other than crash?"* If the only honest answer is "no — and nothing else in the
process would work either," the `Result` is a false affordance; remove it at the
source. One real per-instance failure reason justifies the `Result`, and the
unreachable build error then rides along for free.

## Worked example

`SecEdgarClient::new` returned `Result<Self, TradingError>` whose only `Err` was
the reqwest build. That forced `CatalystProvider` to carry
`sec_edgar: Option<SecEdgar8kProvider>` with a `None` "EDGAR-unavailable" `match`
fallback in `build_catalyst_provider` — while the graph path already
`.expect()`-ed the same constructor (the inconsistency *was* the signal). Making
`new` return `Self` (degrading via `.unwrap_or_else(|_| reqwest::Client::new())`)
removed the `Option`, the `match`, the `with_sec_edgar` builder, the `.expect()`,
`?`/`.expect(...)` at ~14 call sites, and a vacuous `.is_ok()` test.
`SummaryHttp::new` — another no-key client — was already infallible this way, so
this aligned with an existing sibling rather than inventing a third style.

**Watch for the test-isolation side effect:** making an optional dependency
mandatory can pull network I/O into previously-isolated unit tests. When you
remove an `Option`, audit tests that relied on the dependency being absent and
exercise the pure unit directly instead of the network-fanning entry point.

See `docs/solutions/design-patterns/infallible-constructor-when-failure-is-process-fatal.md`
for the full learning, the sibling rule [[no-write-only-placeholder-fields]]
(deletes redundant *data* slots; this deletes redundant *error-handling* slots),
and CLAUDE.md §2 "Simplicity First" — "No error handling for impossible
scenarios."
