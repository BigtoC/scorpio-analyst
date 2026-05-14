# Alpha Vantage Earnings Call Transcripts Integration

## Goal

Wire Alpha Vantage's `EARNINGS_CALL_TRANSCRIPT` API as a live `TranscriptProvider`, replacing the contract-only seam in `crates/scorpio-core/src/data/adapters/transcripts.rs`. This unblocks Theme C's full power (tone comparison between press releases and earnings calls) from the analytical themes port plan.

## Background

- `TranscriptEvidence` struct and `TranscriptProvider` trait exist as a contract-only seam — no provider is wired.
- `DataEnrichmentConfig.enable_transcripts` exists (default `false`).
- Theme C in the analytical themes port ships in degraded mode because transcripts aren't available, with a `TODO(transcripts)` marker waiting for this integration.
- The earnings calendar (catalyst-calendar Tier 1) is already wired, providing call dates.

## Design Decisions

| Decision                          | Choice                                                                                                                                                                                                                                                                                                                                                                                | Rationale                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
|-----------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| API key pattern                   | `SCORPIO_ALPHA_VANTAGE_API_KEY` env var, same as Finnhub/FRED                                                                                                                                                                                                                                                                                                                         | Consistent with existing data-source key management                                                                                                                                                                                                                                                                                                                                                                                                         |
| Multiple keys                     | Comma-separated in single config field, round-robin on rate limit                                                                                                                                                                                                                                                                                                                     | Free tier is 25 req/day; multiple keys multiply quota                                                                                                                                                                                                                                                                                                                                                                                                       |
| `call_date` / `as_of_date` format | `"YYYY-QN"` (e.g., `"2024Q1"`) at the **trait contract level**                                                                                                                                                                                                                                                                                                                        | API returns quarter, not date; caller resolves quarter so the trait contract aligns with Alpha Vantage's native granularity. Requires updating existing trait docstring, `TranscriptEvidence` fixtures, and any prompt-renderer that parses `call_date` as ISO.                                                                                                                                                                                             |
| Transcript fetching               | Single API call. The internal `enrich_transcript` task — not external callers of the client — determines the exact quarter from the earnings calendar and passes it to `fetch_transcript`. The client itself accepts a quarter parameter and does not perform calendar lookups.                                                                                                       | Avoids wasteful backward walk; earnings calendar already available; keeps the client narrow.                                                                                                                                                                                                                                                                                                                                                                |
| Content & segments                | `TranscriptEvidence` carries only `segments: Vec<TranscriptSegment>` (each with `speaker`, `title`, `content`, `sentiment`). The previous flat `content: String` is removed. A `rendered_content()` helper method produces a flat string on demand.                                                                                                                                   | Structured segments are the faithful representation; the flat `content` field had no live consumer (the trait was contract-only) and would have created drift risk between two representations.                                                                                                                                                                                                                                                             |
| Quarter semantics                 | Alpha Vantage's `quarter` field reflects the **issuer's fiscal quarter**, not the calendar quarter. The enrichment task resolves the quarter by reading a new `fiscal_period: Option<String>` field on `CatalystEvent` (populated by the Finnhub branch from `EarningsRelease`); falls back to a calendar-derived 6-week-lag heuristic when no past calendar entry carries the field. | `CatalystEvent.fiscal_period` does not exist today; this design adds it as an additive (`#[serde(default)]`) field. The Finnhub branch already has `year`/`quarter` available on `EarningsRelease`; the yfinance branch does not and leaves the field `None`. Most large-cap issuers have non-December fiscal years; without a structured field, calendar-quarter arithmetic would silently miss AAPL/MSFT/NVDA/etc. The heuristic fallback is best-effort. |
| Fetch-status preservation         | Add a sibling context key `KEY_TRANSCRIPT_FETCH_STATUS` carrying the `TranscriptFetch` variant name as a JSON string. Renderer reads both keys and produces distinct prompt language per variant.                                                                                                                                                                                     | Collapsing the four-variant enum into `Option<TranscriptEvidence>` at the context boundary would erase the stated benefit ("distinct prompt-layer language"). The sibling-key approach is additive and preserves the existing `KEY_CACHED_TRANSCRIPT` contract.                                                                                                                                                                                             |
| Sentiment mapping                 | Drop the unweighted aggregate. Pass per-segment scores into prompt context via `segments[].sentiment`. `TranscriptEvidence.sentiment_score` is removed from the struct.                                                                                                                                                                                                               | Unweighted mean of heterogeneous segments (operator boilerplate vs CFO Q&A) is statistically meaningless; per-segment scores are the faithful representation Alpha Vantage actually provides.                                                                                                                                                                                                                                                               |
| Enablement                        | Explicit opt-in via `enable_transcripts` flag                                                                                                                                                                                                                                                                                                                                         | Consistent with `enable_consensus_estimates` / `enable_event_news`                                                                                                                                                                                                                                                                                                                                                                                          |
| Fetch outcome                     | Trait returns `TranscriptFetch` enum: `Found(TranscriptEvidence)` / `NotPublished` / `Throttled` / `Unavailable` (network/HTTP errors map to `Err`). Each variant produces distinct prompt-layer language and audit-trail metadata.                                                                                                                                                   | Collapsing every non-Found state to `Ok(None)` would make the analyst LLM and operators blind to *why* a transcript is absent.                                                                                                                                                                                                                                                                                                                              |
| Key rotation (this slice)         | On `"Note"`/`"Information"` response, rotate to next key and retry within the same `fetch_transcript` call (up to `keys.len()` attempts). No persistent cooldown tracker.                                                                                                                                                                                                             | Multi-key remains in scope to multiply daily quota for production usage; persistent cooldown is deferred until usage patterns make it valuable.                                                                                                                                                                                                                                                                                                             |
| Daily-quota tracking              | Out of scope. Once 25/day is exhausted on every configured key, subsequent calls return `Throttled` per call (detected via `Information`/`Note` field) until next-day reset.                                                                                                                                                                                                          | Persistent SQLite-backed counter exceeds this slice's scope; per-call detection is sufficient for graceful degradation.                                                                                                                                                                                                                                                                                                                                     |
| Transcript-content sanitization   | Out of scope. Transcript content is injected into prompts without sanitization in this slice. `TODO(transcripts-sanitize)` and a tracked follow-up issue document the prompt-injection threat model.                                                                                                                                                                                  | Theme C ships faster with this risk explicitly deferred; Alpha Vantage is a low-likelihood adversarial source. Revisit when the threat surface grows.                                                                                                                                                                                                                                                                                                       |
| Rate limiting                     | `alpha_vantage_rps` in `RateLimitConfig`, default 1 rps                                                                                                                                                                                                                                                                                                                               | Smooths burst-spending of the daily quota; does NOT bound the 25/day cap.                                                                                                                                                                                                                                                                                                                                                                                   |
| Approach                          | New `AlphaVantageClient` in `data/` implementing `TranscriptProvider`                                                                                                                                                                                                                                                                                                                 | Follows existing `finnhub.rs` / `fred.rs` convention                                                                                                                                                                                                                                                                                                                                                                                                        |

## Architecture

```
User config / env vars
  └─ alpha_vantage_api_key = "KEY1,KEY2,KEY3"
  └─ [enrichment] enable_transcripts = true

                    ┌──────────────────────┐
                    │  AlphaVantageClient   │
                    │  ┌─────────────────┐  │
                    │  │ Vec<SecretKey>  │  │  ← comma-split at construction
                    │  │ AtomicIndex     │  │  ← round-robin rotation
                    │  │ RateLimiter     │  │  ← alpha_vantage_rps (default 1)
                    │  └─────────────────┘  │
                    │                       │
                    │  fetch_transcript(    │
                    │    symbol,            │
                    │    quarter "YYYY-QN"  │
                    │  ) -> TranscriptFetch │
                    │       │ Found(TE)    │
                    │       │ NotPublished │
                    │       │ Throttled    │
                    │       └ Unavailable  │
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │ TranscriptEvidence    │
                    │  symbol: String       │
                    │  call_date: "2024Q1"  │  ← YYYY-QN at trait level
                    │  segments: Vec<…>     │  ← structured per-segment data
                    │  rendered_content()   │  ← on-demand flat string
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │ Enrichment Pipeline   │
                    │ writes JSON           │
                    │  Option<TE>           │
                    │ → KEY_CACHED_TRANSCRIPT│  ← existing context-key seam
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │ Prompt Context        │
                    │ {transcript} block    │  ← Theme C; renders segments
                    └──────────────────────┘
```

## Components

### AlphaVantageClient

**File:** `crates/scorpio-core/src/data/alpha_vantage.rs`

```rust
pub struct AlphaVantageClient {
    keys: Vec<SecretString>,
    current_index: AtomicUsize,
    rate_limiter: SharedRateLimiter,
    http: reqwest::Client,
    base_url: String,  // default: "https://www.alphavantage.co/query"
}
```

**Constructor:** Calls `ApiConfig.alpha_vantage_api_key.expose_secret()` exactly once, immediately splits on commas, trims whitespace, and wraps each non-empty fragment in a fresh `SecretString`. Returns `Err(TradingError::Config)` with a **static** message (no raw-secret interpolation) if no key is configured.

**Key rotation:** Atomic index increments per call. On rate-limit response (Alpha Vantage returns `"Note"` or `"Information"` key in JSON), rotate to next key and retry within the same call (up to `keys.len()` attempts). All keys throttled within a single call → return `TranscriptFetch::Throttled`. `tracing::warn` emits **only** `provider = "alpha_vantage"`, `reason = "rate_limit"` — never key index, count, or any key material.

**Persistent cooldown is out of scope for this slice.** A key that returns `Note`/`Information` due to daily-quota exhaustion will be tried again on the next call and immediately re-throttled, wasting one HTTP request per call until the 24h reset. See [Out of Scope](#out-of-scope) for the deferred follow-up.

**Trait-level contract change.** The existing `TranscriptProvider` trait documents `as_of_date` as `"YYYY-MM-DD"` and `TranscriptEvidence.call_date` as ISO date. This integration changes both to `"YYYY-QN"` at the **trait** level — Alpha Vantage's native granularity flows through to the seam. Every downstream consumer that parses `call_date` as ISO must be audited and updated (see [Files to Modify](#files-to-modify)).

**Fetch outcome type.** A new `TranscriptFetch` enum replaces the prior `Option<TranscriptEvidence>` return shape:

```rust
pub enum TranscriptFetch {
    Found(TranscriptEvidence),
    NotPublished,   // API responded normally; no transcript for this symbol/quarter
    Throttled,      // every configured key returned Note/Information within this call
    Unavailable,    // HTTP 5xx or timeout persisted across 3 retries (RetryPolicy default)
}
```

**Boundary between `Throttled`, `Unavailable`, and `Err`:**

| Condition                                                                                                        | Outcome                                                                |
|------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------|
| `Note`/`Information` from every configured key after rotation                                                    | `Ok(Throttled)`                                                        |
| HTTP 5xx or connection/read timeout, persisted after 3 retries with exponential backoff (existing `RetryPolicy`) | `Ok(Unavailable)`                                                      |
| `Error Message` field in response                                                                                | `Err(TradingError::SchemaViolation)`                                   |
| HTTP 4xx other than 429 (e.g., 401/403 auth, 404 ticker not found)                                               | `Err(TradingError::Rig)` or `Err(TradingError::Config)` as appropriate |
| Invalid input to the function (bad symbol, malformed quarter)                                                    | `Err(TradingError::SchemaViolation)` before any HTTP call              |

The retry loop reuses the existing `RetryPolicy` (max 3 retries, base 500ms) so behavior matches other data clients.

```rust
#[async_trait]
impl TranscriptProvider for AlphaVantageClient {
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,  // "YYYY-QN" format, e.g., "2025Q1"
    ) -> Result<TranscriptFetch, TradingError> {
        // 0. Validate inputs before constructing URL:
        //    - symbol → existing `data::symbol::validate_symbol`
        //    - as_of_date → must match `^\d{4}Q[1-4]$`; otherwise
        //      `Err(TradingError::SchemaViolation)`
        // 1. Call Alpha Vantage API with symbol + as_of_date as quarter
        // 2. On Note/Information: rotate key, retry (up to keys.len() attempts).
        //    If every key throttled within this call → Ok(Throttled).
        // 3. Map response to TranscriptFetch:
        //    - Non-empty transcript → Found(TranscriptEvidence)
        //    - Missing/null/empty transcript array (and no error fields) → NotPublished
        // 4. Error Message field → Err(TradingError::SchemaViolation)
    }
}
```

**Caller responsibility — quarter resolution.** Alpha Vantage's `quarter` field reflects the **issuer's fiscal quarter**, not the calendar quarter — Apple's fiscal Q1 ends in late December, Microsoft's fiscal Q1 in September, Nvidia's fiscal Q1 in late April. The enrichment pipeline resolves the quarter using a deterministic three-step rule:

1. **Earnings calendar present AND the most recent past call carries fiscal metadata** → use it. Concretely: find the `CatalystEvent` with `event.symbol == symbol AND event.category == Earnings AND event.event_date ≤ analysis_date AND event.fiscal_period.is_some()`, take the one with the **maximum** `event_date`, and read the new `fiscal_period: Option<String>` field (format: `"YYYY-QN"`, e.g., `"2025Q1"`). This field is populated by the Finnhub branch of catalyst-calendar wiring when `EarningsRelease.year` and `EarningsRelease.quarter` are both present. This is the **preferred path** and works correctly for every fiscal-year convention.
2. **Earnings calendar present BUT no past entry has fiscal metadata** (e.g., the only past entry came from the yfinance branch, or the Finnhub `EarningsRelease` lacked year/quarter) → fall through to step 3. Emit `tracing::info` flagging the calendar-without-fiscal-metadata case.
3. **Earnings calendar absent OR step 2 fell through** → calendar-derived best-effort using a ~6-week reporting lag. Concretely:
   - Let `Q = calendar_quarter_of(as_of_date)` and `year_of_Q = year_of(as_of_date)`.
   - If `as_of_date` is ≥ 6 weeks past the start of Q, the most recent reportable quarter is **Q − 1** (rolling year back when Q − 1 < 1: e.g., Q = 1 → Q4 of previous year).
   - Otherwise it is **Q − 2** (same rollover rule).
   - Worked examples:
     - `2026-05-15` → Q2 2026 began 2026-04-01; ~6.5 weeks in → request `"2026Q1"`.
     - `2026-01-15` → Q1 2026 began 2026-01-01; 2 weeks in → request `"2025Q3"` (Q − 2 with year rollover). **Note:** this is the known-imprecise case — most Dec-FY large-caps will have reported Q4 2025 within ~2 weeks of mid-January; the heuristic accepts the false negative.
     - `2025-12-31` → Q4 2025 began 2025-10-01; ~13 weeks in → request `"2025Q3"`.
   - **Limitation:** the fallback is **calendar-based** and will request the wrong quarter for non-December-FY issuers when the calendar is missing. Returns `TranscriptFetch::NotPublished` for those tickers. `TODO(transcripts-fiscal-fallback)` tracks the eventual fix (probe issuer FY end from Finnhub profile and adjust).

This rule trades a small false-negative risk near boundaries (issuers that report late; non-Dec-FY companies when the calendar is missing) for avoiding wrong-quarter requests. Calendar-present is always preferred; the fallback heuristic is a last resort and should be observable in logs (the new enrichment task emits `tracing::info` distinguishing the two paths).

**Round-robin semantics — clarification.** `current_index` increments **once per `fetch_transcript` invocation** (at the start, modulo `keys.len()`), so successive analyses round-robin across all configured keys. Within a single invocation, key rotation on `Note`/`Information` retries uses `(current_index + retry_count) % keys.len()`. This makes the "multiple keys multiply quota" rationale (Design Decisions table row 19) hold under normal usage, not just in retry-failure mode.

### Response Mapping

**Alpha Vantage response:**

```json
{
  "symbol": "COIN",
  "quarter": "2024Q1",
  "transcript": [
    {
      "speaker": "Alesia Haas",
      "title": "Chief Financial Officer",
      "content": "Thank you, operator...",
      "sentiment": 0.85
    }
  ]
}
```

**Internal serde structs:**

```rust
#[derive(Deserialize)]
struct AlphaVantageTranscriptResponse {
    symbol: Option<String>,
    quarter: Option<String>,
    transcript: Option<Vec<TranscriptSegment>>,
    // Rate-limit / quota signals — Alpha Vantage uses multiple keys in the wild.
    // Any present non-None value is treated as a rate-limit signal that triggers
    // key rotation.
    #[serde(rename = "Note")]
    note: Option<String>,
    #[serde(rename = "Information")]
    information: Option<String>,
    // Per-request hard errors (bad symbol, malformed params) propagate as
    // `Err(TradingError::SchemaViolation)`, not key rotation.
    #[serde(rename = "Error Message")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct TranscriptSegment {
    speaker: String,
    title: String,
    content: String,
    sentiment: Option<f64>,
}
```

**Updated `TranscriptEvidence` schema:**

```rust
pub struct TranscriptEvidence {
    pub symbol: String,
    pub call_date: String,          // "YYYY-QN" at trait level
    pub segments: Vec<TranscriptSegment>,
}

pub struct TranscriptSegment {
    pub speaker: String,
    pub title: String,
    pub content: String,
    pub sentiment: Option<f64>,
}

impl TranscriptEvidence {
    /// Render all segments into a single string prefixed by `"{speaker} ({title}): "`,
    /// joined by `"\n\n"`. Provided for consumers (e.g., logging, debug snapshots)
    /// that want a flat representation; the canonical structure is `segments`.
    pub fn rendered_content(&self) -> String { /* … */ }
}
```

The previous `sentiment_score: Option<f64>` aggregate **and** the previous flat `content: String` field are both **removed**. `segments` is the single source of truth; `rendered_content()` is available when a flat string is needed without forcing it into the persistent schema.

> The internal serde struct `TranscriptSegment` used during deserialization (above) and the public `TranscriptSegment` carried in `TranscriptEvidence` are intentionally the same shape and may be collapsed into a single `pub` type at implementation time. The two definitions in this design highlight the response-vs-storage roles; they are not literally separate types.

**Mapping rules:**

| Alpha Vantage  | → TranscriptEvidence | Logic                                                                                                                                                                                                                       |
|----------------|----------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `symbol`       | `symbol`             | Direct copy, uppercase                                                                                                                                                                                                      |
| `quarter`      | `call_date`          | Direct copy (e.g., `"2024Q1"`)                                                                                                                                                                                              |
| `transcript[]` | `segments`           | One-to-one map. Each Alpha Vantage segment becomes one `TranscriptSegment` (`speaker`, `title`, `content`, `sentiment` copied as-is). No flat-`content` field on `TranscriptEvidence`; call `rendered_content()` on demand. |

**Edge cases:**
- Missing `transcript` field or `null` value (deserialized as `None`) → `Ok(TranscriptFetch::NotPublished)`
- Empty `transcript` array (`Vec::is_empty()`) → `Ok(TranscriptFetch::NotPublished)`
- Missing `sentiment` on some segments → `segments[i].sentiment = None`; the prompt layer renders the segment without a score
- `"Note"` or `"Information"` in response → rate-limit / daily-quota detected, triggers key rotation; if every key throttled within the call → `Ok(TranscriptFetch::Throttled)`
- `"Error Message"` in response → per-request hard error (invalid symbol/params) → `Err(TradingError::SchemaViolation)`, NOT key rotation
- Recoverable transient failures after retry exhaustion → `Ok(TranscriptFetch::Unavailable)` (lets the prompt layer say "we tried and failed" rather than "no transcript exists")
- Non-recoverable HTTP errors (network failures, 4xx/5xx not handled above) → `Err(TradingError)` propagated to caller
- `SharedRateLimiter` is an existing type used by `FinnhubClient` and `FredClient`; constructed via `SharedRateLimiter::alpha_vantage_from_config(&rate_limits)`

### Config Changes

**`crates/scorpio-core/src/config.rs`:**

```rust
pub struct ApiConfig {
    pub finnhub_api_key: Option<SecretString>,
    pub fred_api_key: Option<SecretString>,
    pub alpha_vantage_api_key: Option<SecretString>,  // NEW
}

pub struct RateLimitConfig {
    pub finnhub_rps: u32,
    pub fred_rps: u32,
    pub yahoo_finance_rps: u32,
    pub alpha_vantage_rps: u32,  // NEW, default 1
}
```

Env injection:

```rust
inject_env_override!(
    cfg.api.alpha_vantage_api_key,
    "SCORPIO_ALPHA_VANTAGE_API_KEY",
    "alpha_vantage"
);
```

**`crates/scorpio-core/src/settings.rs`:**

Add `alpha_vantage_api_key: Option<String>` to `PartialConfig` and `UserConfigFile`.

**Setup wizard** (`crates/scorpio-cli/src/cli/setup/steps.rs`):

New step: "Enter Alpha Vantage API key(s) (comma-separated for multiple keys)". Stores as comma-separated string in `PartialConfig.alpha_vantage_api_key`.

**`.env.example`:**

```
SCORPIO_ALPHA_VANTAGE_API_KEY=your-key-here
```

### Enrichment Integration

**Storage decision: extend the existing `KEY_CACHED_TRANSCRIPT` context-key seam, add a sibling status key.** `PreflightTask` already seeds `KEY_CACHED_TRANSCRIPT` to JSON `null` via `seed_if_absent` at `crates/scorpio-core/src/workflow/tasks/preflight.rs:274`. The new enrichment task overwrites that default on success. To preserve the `TranscriptFetch` variant for the prompt layer, a **new sibling context key** `KEY_TRANSCRIPT_FETCH_STATUS` carries the variant name as a JSON string:

```rust
/// Context key for the TranscriptFetch outcome. Written by `enrich_transcript`
/// or by preflight to `"NotPublished"` when transcripts are disabled / unavailable.
/// Always present after preflight.
pub const KEY_TRANSCRIPT_FETCH_STATUS: &str = "transcript_fetch_status";
```

Valid values: `"Found"`, `"NotPublished"`, `"Throttled"`, `"Unavailable"`. The prompt renderer reads both keys to differentiate "throttled — results may improve on retry" from "not yet published — results stable for now" from "fetch failed — degraded mode" in the prompt language.

This integration **extends the existing contract** rather than adding a `TradingState` field — no schema-evolution risk, no custom serde for `Arc<RwLock<…>>`, no dual-write coherence problem.

**Pipeline initialization:**
1. If `enable_transcripts` is true and `alpha_vantage_api_key` is present → construct `AlphaVantageClient`; the `enrich_transcript` task is registered into the workflow graph.
2. If key missing + `enable_transcripts` is true → `tracing::warn`, continue without transcripts. Preflight's seeded `null` + `"NotPublished"` status persists; the `enrich_transcript` task does not run.
3. Preflight is responsible for **seeding defaults**: writes JSON `null` to `KEY_CACHED_TRANSCRIPT` and `"NotPublished"` to `KEY_TRANSCRIPT_FETCH_STATUS` if those keys are absent. The new task is responsible for **overwriting** on success/throttle/unavailable.

**Phase 1 hydration:** The new enrichment task `crates/scorpio-core/src/workflow/tasks/enrich_transcript.rs` runs after preflight when `AlphaVantageClient` was constructed:

1. Read `analysis_date` (and earnings calendar if present) from context.
2. Resolve the target quarter using the fiscal-period-first rule documented under [Caller responsibility — quarter resolution](#components). Emit `tracing::info` distinguishing calendar-driven vs heuristic-driven resolution.
3. Call `client.fetch_transcript(symbol, quarter)` and write context per the `TranscriptFetch` variant:

   | Variant           | `KEY_CACHED_TRANSCRIPT`                 | `KEY_TRANSCRIPT_FETCH_STATUS` | tracing                                            |
   |-------------------|-----------------------------------------|-------------------------------|----------------------------------------------------|
   | `Found(evidence)` | `serde_json::to_value(Some(evidence))?` | `"Found"`                     | `info` with quarter + segment count                |
   | `NotPublished`    | `null`                                  | `"NotPublished"`              | `info` with reason                                 |
   | `Throttled`       | `null`                                  | `"Throttled"`                 | `warn` (rate-limited; transcripts absent this run) |
   | `Unavailable`     | `null`                                  | `"Unavailable"`               | `warn` with last HTTP status                       |

4. On `Err(TradingError)` → write `"Unavailable"` to status (the error already exited the API surface), keep `KEY_CACHED_TRANSCRIPT` at its `null` seed; emit `tracing::warn`; do **not** abort the pipeline (transcripts are enrichment, not a blocker).

**Phase 2 prompt rendering:**
- Renderers read `KEY_CACHED_TRANSCRIPT` (`Option<TranscriptEvidence>`) **and** `KEY_TRANSCRIPT_FETCH_STATUS` (variant name).
- `Found` → inject `{transcript}` block; renders `segments` with attribution and per-segment `sentiment` where present.
- `NotPublished` → existing Theme C degraded-mode language ("no transcript available for this quarter").
- `Throttled` → degraded-mode language with a hint to the analyst ("transcript data was rate-limited; this analysis may improve on retry").
- `Unavailable` → degraded-mode language indicating a transient failure ("transcript data could not be fetched this run").

> **No `TradingState` changes.** The struct gains no new field; phase snapshots remain backward-compatible.

## Files to Modify

| File                                                                      | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
|---------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/data/alpha_vantage.rs`                           | **NEW** — `AlphaVantageClient` struct, `TranscriptProvider` impl, serde structs, tests                                                                                                                                                                                                                                                                                                                                                                                                         |
| `crates/scorpio-core/src/data/mod.rs`                                     | Add `pub mod alpha_vantage;`                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `crates/scorpio-core/src/data/adapters/transcripts.rs`                    | **Contract change**: change return type to `Result<TranscriptFetch, TradingError>`; introduce `TranscriptFetch` enum; update `TranscriptEvidence` (drop `sentiment_score`, **drop** `content`, add `segments: Vec<TranscriptSegment>`, add `rendered_content()` helper method); change `as_of_date` / `call_date` doc comments to `"YYYY-QN"`; **update existing serialization-roundtrip test fixtures** (currently `"2025-01-30"`, `"2025-02-15"`) to use quarter format.                     |
| `crates/scorpio-core/src/data/adapters/catalysts.rs`                      | Add `pub fiscal_period: Option<String>` field to `CatalystEvent` (format: `"YYYY-QN"`). Use `#[serde(default)]` for backward-compat with existing serialized snapshots. Update the Finnhub builder to populate from `EarningsRelease.year` + `quarter` when both are `Some` (e.g., `Some("2025Q1")`); the yfinance builder leaves it `None`. Update existing `CatalystEvent` roundtrip / fixture tests to assert the new field.                                                                |
| `crates/scorpio-core/src/workflow/tasks/enrich_transcript.rs`             | **NEW** task: resolves quarter using the fiscal-period-first rule (reads the most-recent past `CatalystEvent.fiscal_period` for the symbol; falls back to the calendar-derived heuristic when none is available), calls `AlphaVantageClient::fetch_transcript`, writes JSON `Option<TranscriptEvidence>` into `KEY_CACHED_TRANSCRIPT` and the variant name into `KEY_TRANSCRIPT_FETCH_STATUS` per the table above. Wired into the workflow graph after preflight, behind `enable_transcripts`. |
| `crates/scorpio-core/src/workflow/tasks/preflight.rs`                     | Extend the existing `seed_if_absent` block near line 274 to also seed `KEY_TRANSCRIPT_FETCH_STATUS` with `"NotPublished"` as the default. **Note:** the existing `seed_if_absent(context, key)` helper is hardcoded to write the literal `"null"` and takes no value parameter. Either (a) generalize it to `seed_if_absent_with(context, key, default_value)` and update existing call sites, or (b) add a sibling helper `seed_status_if_absent` for status-string seeding.                  |
| `crates/scorpio-core/src/workflow/tasks/common.rs`                        | Add `pub const KEY_TRANSCRIPT_FETCH_STATUS: &str = "transcript_fetch_status";` and its doc comment. Update the `KEY_CACHED_TRANSCRIPT` doc comment to record that the contract is now "written by `enrich_transcript` (when `enable_transcripts` is true) or by preflight to `null` / `\"NotPublished\"` otherwise".                                                                                                                                                                           |
| Prompt rendering call sites (`agents/` analysts that consume transcripts) | Audit and update any code that parses `call_date` as ISO-8601; render `segments` rather than treating `content` as the only structured signal. Surface per-segment `sentiment` where present.                                                                                                                                                                                                                                                                                                  |
| `crates/scorpio-core/src/config.rs`                                       | Add `alpha_vantage_api_key` to `ApiConfig`, `alpha_vantage_rps` to `RateLimitConfig`. Env injection on **both** `load_effective_runtime` (via `inject_env_override!`) **and** `Config::load_from` (via `secret_from_env`). Extend the manual `Debug` impl for `ApiConfig` to redact the new field via `secret_display`.                                                                                                                                                                        |
| `crates/scorpio-core/src/rate_limit.rs`                                   | Add `SharedRateLimiter::alpha_vantage_from_config` constructor (new variant). If existing tests construct `RateLimitConfig { ... }` as a struct literal, add the new `alpha_vantage_rps` field there or switch to `..Default::default()`.                                                                                                                                                                                                                                                      |
| `crates/scorpio-core/src/settings.rs`                                     | Add `alpha_vantage_api_key` to `PartialConfig` + `UserConfigFile`, round-trip. Extend the manual `PartialConfig` `Debug` impl with `.field("alpha_vantage_api_key", &redact(&self.alpha_vantage_api_key))` following the existing `*_api_key` pattern.                                                                                                                                                                                                                                         |
| `crates/scorpio-cli/src/cli/setup/steps.rs`                               | Add Alpha Vantage API key input step. Position **after** the existing FRED step (preserves numbering for steps 1–N). Step is unconditional; users can leave it blank if they don't intend to enable transcripts.                                                                                                                                                                                                                                                                               |
| `.env.example`                                                            | Add `SCORPIO_ALPHA_VANTAGE_API_KEY`                                                                                                                                                                                                                                                                                                                                                                                                                                                            |

## Verification Strategy

**Unit tests** (in `alpha_vantage.rs`):
- `parse_transcript_response` — JSON → `TranscriptFetch::Found(TranscriptEvidence)` mapping; `segments` populated
- `parse_note_response_triggers_rotation` — `"Note"` field triggers rate-limit detection and key rotation
- `parse_information_response_triggers_rotation` — `"Information"` field also triggers rotation (Alpha Vantage daily-quota signal)
- `parse_error_message_returns_schema_violation` — `"Error Message"` field → `Err(TradingError::SchemaViolation)`, NOT rotation
- `parse_empty_transcript_array` → `Ok(TranscriptFetch::NotPublished)`
- `parse_missing_transcript_field` → `Ok(TranscriptFetch::NotPublished)`
- `parse_partial_sentiment` — segments without `sentiment` preserve `None`
- `key_rotation_on_rate_limit` — round-robin advances on `Note`/`Information`
- `all_keys_throttled_within_call_returns_throttled` — every configured key returns `Note` → `Ok(TranscriptFetch::Throttled)`
- `comma_split_keys` — comma-separated key parsing produces correct number of `SecretString` fragments
- `constructor_error_does_not_leak_secret` — when the constructor errors on a missing/empty key, the resulting `TradingError::Config` message contains no key material (regex assert: no commas, no fragment of a typical key shape)
- `invalid_quarter_format_rejected` — `as_of_date = "2025-Q1"` (or any non-matching string) → `Err(TradingError::SchemaViolation)` without an HTTP call
- `invalid_symbol_rejected` — bad symbol → `Err(TradingError::SchemaViolation)` without an HTTP call

**CatalystEvent schema tests** (in `catalysts.rs`):
- `fiscal_period_roundtrip` — `CatalystEvent { fiscal_period: Some("2025Q1"), .. }` serializes and deserializes intact.
- `fiscal_period_default_for_legacy_snapshots` — a JSON payload without `fiscal_period` deserializes successfully with `fiscal_period = None`.
- `finnhub_builder_populates_fiscal_period` — given an `EarningsRelease` with `year = Some(2025), quarter = Some(1)`, the produced `CatalystEvent.fiscal_period == Some("2025Q1")`.
- `finnhub_builder_leaves_fiscal_period_none_when_missing` — given an `EarningsRelease` with `year = None` (or `quarter = None`), the produced `CatalystEvent.fiscal_period == None`.
- `yfinance_builder_leaves_fiscal_period_none` — yfinance branch always produces `fiscal_period: None`.

**Quarter resolution tests** (in the new enrichment task):
- `calendar_present_uses_fiscal_period_field` — given a `CatalystEvent` with `event_date = 2025-01-30, fiscal_period = Some("2025Q1")`, `enrich_transcript` requests quarter `"2025Q1"` without calendar-quarter arithmetic.
- `calendar_present_non_dec_fy_issuer` — `CatalystEvent.fiscal_period = Some("2025Q2")` for an issuer with March fiscal-year end (`event_date = 2025-07-15`); resolves to `"2025Q2"` regardless of calendar-quarter math (the resolver does not re-interpret `event_date`).
- `calendar_present_but_no_fiscal_period_falls_back_to_heuristic` — a past calendar entry with `fiscal_period = None` (yfinance source, or Finnhub without quarter data) causes step 1 to fall through to step 3.
- `calendar_absent_mid_q2_dec_fy_resolves_to_q1` — `as_of = 2026-05-15` with no calendar → `"2026Q1"` (6-week-lag fallback).
- `calendar_absent_early_q2_resolves_to_q4_prior` — `as_of = 2026-04-05` (< 6 weeks into Q2) → `"2025Q4"` (Q − 2 with year rollover).
- `calendar_absent_mid_january_known_imprecise` — `as_of = 2026-01-15` (2 weeks into Q1) → `"2025Q3"` (Q − 2). Test asserts this is the documented behavior (known imprecise) and emits a `tracing::info` flagging the heuristic path.
- `calendar_absent_late_december` — `as_of = 2025-12-31` (13 weeks into Q4) → `"2025Q3"`.

**Status-key tests** (in `enrich_transcript.rs`):
- `found_writes_status_found_and_evidence` — successful fetch writes both keys with the expected values
- `notpublished_writes_status_only` — `KEY_CACHED_TRANSCRIPT` stays `null`; status is `"NotPublished"`
- `throttled_writes_status_only` — same; status is `"Throttled"`
- `unavailable_writes_status_only` — same; status is `"Unavailable"`
- `error_emits_warn_and_keeps_defaults` — `Err(TradingError)` writes `"Unavailable"` to status; the cached-transcript key stays at its `null` seed
- `successive_calls_round_robin_keys` — two back-to-back invocations of `fetch_transcript` use different keys (validates the `current_index` per-call increment)

**Config tests** (in `config.rs` / `settings.rs`):
- `alpha_vantage_key_from_env_load_effective_runtime` — `SCORPIO_ALPHA_VANTAGE_API_KEY` loads via `inject_env_override!` path
- `alpha_vantage_key_from_env_load_from` — same key loads via the `Config::load_from` `secret_from_env` path
- `alpha_vantage_rps_default` — default is 1
- `roundtrip_alpha_vantage_key` — survives `PartialConfig` → TOML → `PartialConfig`
- `debug_redacts_alpha_vantage_api_key_on_partial_config` — mirroring existing `debug_redacts_deepseek_api_key`
- `debug_redacts_alpha_vantage_api_key_on_api_config` — mirroring existing `finnhub`/`fred` redaction tests

**Fixture tests:**
- `prompt_bundle_regression_gate_found` — `{transcript}` block (rendered from `segments`) appears in `news_analyst` and `sentiment_analyst` prompts when `KEY_TRANSCRIPT_FETCH_STATUS = "Found"` and `KEY_CACHED_TRANSCRIPT` holds `Some(TranscriptEvidence)`.
- `prompt_bundle_renders_throttled_distinctly` — when status is `"Throttled"` and the cached key is `null`, the prompt language differs from `"NotPublished"` (specifically references rate limiting).
- `prompt_bundle_renders_unavailable_distinctly` — when status is `"Unavailable"`, prompt language references transient failure.
- `prompt_bundle_renders_notpublished_as_legacy_degraded` — when status is `"NotPublished"`, prompt language matches the existing Theme C degraded-mode output (regression gate for backward compat).

**Smoke test:**
```bash
SCORPIO_ALPHA_VANTAGE_API_KEY=your_key \
cargo run -p scorpio-cli -- analyze COIN --json
```

Verify: transcript evidence in JSON output; `segments` populated with per-speaker entries (each with its own `content` and optional `sentiment`); `call_date` is `"YYYY-QN"`. No flat `content` field at the `TranscriptEvidence` root; no aggregate `sentiment_score` at the root.

## Out of Scope

- **Our own sentiment NLP.** The per-segment sentiment from Alpha Vantage is pre-computed and we pass it through unaltered. We don't run our own NLP on transcript text in this slice. `TODO(transcripts-nlp)` marker in `enrich_transcript`.
- **Transcript caching.** No local cache for transcripts. Each run fetches fresh. Can be added later if quota becomes a concern.
- **Quarter backward walk.** The earnings calendar determines the exact quarter to fetch. No iterative walking. When the calendar is absent, the deterministic 6-week-lag heuristic in [Caller responsibility](#components) picks a single quarter; we do not retry adjacent quarters.
- **Q&A separation and speaker indexing.** `segments` preserves Alpha Vantage's segment ordering but we do not parse Q&A boundaries, attribute analyst-firm questions, or build a speaker dictionary.
- **Persistent daily-quota tracking.** No SQLite-backed counter, no cross-process awareness. Once 25/day is exhausted on every configured key, each call wastes one HTTP request to detect the throttled state and returns `TranscriptFetch::Throttled`. `TODO(transcripts-quota)` — revisit if usage demands it.
- **Persistent per-key cooldown.** The current slice rotates within a single `fetch_transcript` call but does not remember which keys were throttled across calls. A daily-keyed cooldown (24h on `Note`/`Information`) is the natural follow-up. `TODO(transcripts-cooldown)`.
- **Transcript content sanitization.** Free-text `speaker`/`title`/`content` fields from Alpha Vantage are injected into prompts **without** sanitization, length caps, or character-set restriction. The threat model: Alpha Vantage is a low-likelihood adversarial source, but transcript text reflects whatever the speaker said on the call (could include URLs, code-like strings, or quoted hostile copy). `TODO(transcripts-sanitize)` — pair with a tracked follow-up issue documenting the prompt-injection threat model and the eventual sanitization rule (likely mirroring the `{analysis_emphasis}` gate: length cap, control-char strip, optional tag handling).
- **Authoritative `TranscriptFetch::Unavailable` semantics.** This slice maps recoverable-after-retry transient failures to `Unavailable`, but the boundary between `Throttled` and `Unavailable` is not extensively codified. A future iteration may formalize the contract (e.g., retry-count exposed in the variant).

## Attribution

This integration uses the Alpha Vantage EARNINGS_CALL_TRANSCRIPT API. Free tier: 25 requests/day per key.
