# Alpha Vantage Earnings Call Transcripts Integration

## Goal

Wire Alpha Vantage's `EARNINGS_CALL_TRANSCRIPT` API as a live `TranscriptProvider`, replacing the contract-only seam in `crates/scorpio-core/src/data/adapters/transcripts.rs`. This unblocks Theme C's full power (tone comparison between press releases and earnings calls) from the analytical themes port plan.

## Background

- `TranscriptEvidence` struct and `TranscriptProvider` trait exist as a contract-only seam — no provider is wired.
- `DataEnrichmentConfig.enable_transcripts` exists (default `false`).
- Theme C in the analytical themes port ships in degraded mode because transcripts aren't available, with a `TODO(transcripts)` marker waiting for this integration.
- The earnings calendar (catalyst-calendar Tier 1) is already wired, providing call dates.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| API key pattern | `SCORPIO_ALPHA_VANTAGE_API_KEY` env var, same as Finnhub/FRED | Consistent with existing data-source key management |
| Multiple keys | Comma-separated in single config field, round-robin on rate limit | Free tier is 25 req/day; multiple keys multiply quota |
| `call_date` format | `"YYYY-QN"` (e.g., `"2024Q1"`) | Preserves the actual quarter; API returns quarter, not exact date |
| Transcript fetching | Single API call; caller determines exact quarter from earnings calendar | Avoids wasteful backward walk; earnings calendar already available |
| Content mapping | Concatenate all segments with `"{speaker} ({title}): "` prefix | Preserves speaker attribution for LLM context |
| Sentiment mapping | Average all per-segment scores; `None` if no segments have scores | Simple, deterministic |
| Enablement | Explicit opt-in via `enable_transcripts` flag | Consistent with `enable_consensus_estimates` / `enable_event_news` |
| Key exhaustion | `Ok(None)` + `tracing::warn` | Graceful degradation; transcripts are enrichment, not a blocker |
| Rate limiting | `alpha_vantage_rps` in `RateLimitConfig`, default 1 rps | Conservative default for free tier |
| Approach | New `AlphaVantageClient` in `data/` implementing `TranscriptProvider` | Follows existing `finnhub.rs` / `fred.rs` convention |

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
                    │                      │
                    │  fetch_transcript(    │
                    │    symbol, as_of_date │
                    │  ) -> Option<TE>      │
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │ TranscriptEvidence    │
                    │  symbol: String       │
                    │  call_date: "2024Q1"  │  ← YYYY-QN directly
                    │  content: String      │  ← all segments concatenated
                    │  sentiment_score:     │
                    │    Option<f64>        │  ← averaged from segments
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │ Enrichment Pipeline   │
                    │ (existing hydration)  │
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │ Prompt Context        │
                    │ {transcript} block    │  ← Theme C full mode
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
    key_cooldowns: Vec<Mutex<Option<Instant>>>,  // per-key rate-limit cooldown tracker
}
```

**Key cooldown:** `key_cooldowns` is a `Vec<Mutex<Option<Instant>>>` with one entry per key. When a key hits a rate limit, the current `Instant` is stored. On the next call, keys with a cooldown `Instant` within the last 60 seconds are skipped.

**Constructor:** Splits `ApiConfig.alpha_vantage_api_key` on commas, trims whitespace, collects into `Vec<SecretString>`. Returns `Err(TradingError::Config)` if key is missing.

**Key rotation:** Atomic index increments per call. On rate-limit response (Alpha Vantage returns `"Note"` key in JSON), skip to next key. Per-key cooldown tracker: if a key hit rate limit within the last 60 seconds, skip immediately. All keys exhausted → `Ok(None)` + `tracing::warn`.

**`TranscriptProvider` implementation:**

The client does NOT depend on the earnings calendar. The caller (enrichment pipeline) determines the quarter from the earnings calendar and passes it as `as_of_date` in `"YYYY-QN"` format. The client fetches that exact quarter.

```rust
#[async_trait]
impl TranscriptProvider for AlphaVantageClient {
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,  // "YYYY-QN" format, e.g., "2025Q1"
    ) -> Result<Option<TranscriptEvidence>, TradingError> {
        // 1. Call Alpha Vantage API with symbol + as_of_date as quarter
        // 2. On rate limit: rotate key, retry (up to keys.len() attempts)
        // 3. Map response to TranscriptEvidence
        // 4. Ok(None) if no transcript exists or all keys exhausted
    }
}
```

**Caller responsibility:** The enrichment pipeline resolves the quarter before calling `fetch_transcript`:
- If earnings calendar data available → use most recent call date ≤ analysis date, convert to `"YYYY-QN"`
- If no earnings calendar data → derive quarter from analysis date (e.g., `"2025-05-15"` → `"2025Q2"`)

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
    #[serde(rename = "Note")]
    note: Option<String>,
}

#[derive(Deserialize)]
struct TranscriptSegment {
    speaker: String,
    title: String,
    content: String,
    sentiment: Option<f64>,
}
```

**Mapping rules:**

| Alpha Vantage | → TranscriptEvidence | Logic |
|---|---|---|
| `symbol` | `symbol` | Direct copy, uppercase |
| `quarter` | `call_date` | Direct copy (e.g., `"2024Q1"`) |
| `transcript[].content` | `content` | Concatenate with `\n\n`, prefixed by `"{speaker} ({title}): "` |
| `transcript[].sentiment` | `sentiment_score` | Average of all segment scores; `None` if none present |

**Edge cases:**
- Empty `transcript` array → `Ok(None)`
- Missing `sentiment` on some segments → average only present values; `None` if all missing
- `"Note"` in response → rate-limit detected, triggers key rotation
- Non-rate-limit HTTP errors (network failures, 4xx/5xx) → `Err(TradingError)` propagated to caller; enrichment pipeline handles gracefully (transcript absent, degraded mode)
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

**Pipeline initialization:**
1. If `enable_transcripts` is true and `alpha_vantage_api_key` is present → construct `AlphaVantageClient`
2. If key missing + `enable_transcripts` is true → `tracing::warn`, continue without transcripts

**Phase 1 hydration:**
1. Earnings calendar already resolved (catalyst-calendar Tier 1)
2. Pipeline determines most recent call date ≤ analysis date, converts to `"YYYY-QN"` quarter string
3. `client.fetch_transcript(symbol, quarter)` → `Ok(Some(evidence))` or `Ok(None)`
4. Store in `TradingState.transcript_evidence: Arc<RwLock<Option<TranscriptEvidence>>>`

**Phase 2 prompt rendering:**
- Transcript present → inject `{transcript}` block into news_analyst, sentiment_analyst prompts
- Transcript absent → prompts include "degraded mode: headline/summary only" (existing Theme C behavior)

**`TradingState` addition** (if not already present):

```rust
#[serde(default)]
pub transcript_evidence: Arc<RwLock<Option<TranscriptEvidence>>>,
```

## Files to Modify

| File | Change |
|------|--------|
| `crates/scorpio-core/src/data/alpha_vantage.rs` | **NEW** — `AlphaVantageClient` struct, `TranscriptProvider` impl, serde structs, tests |
| `crates/scorpio-core/src/data/mod.rs` | Add `pub mod alpha_vantage;` |
| `crates/scorpio-core/src/config.rs` | Add `alpha_vantage_api_key` to `ApiConfig`, `alpha_vantage_rps` to `RateLimitConfig`, env injection, debug redaction |
| `crates/scorpio-core/src/rate_limit.rs` | Add `SharedRateLimiter::alpha_vantage_from_config` constructor (new variant) |
| `crates/scorpio-core/src/settings.rs` | Add `alpha_vantage_api_key` to `PartialConfig` + `UserConfigFile`, round-trip |
| `crates/scorpio-core/src/data/adapters/transcripts.rs` | Update `call_date` doc comment to note `"YYYY-QN"` format; update `as_of_date` doc comment to note `"YYYY-QN"` format |
| `crates/scorpio-cli/src/cli/setup/steps.rs` | Add Alpha Vantage API key input step |
| `.env.example` | Add `SCORPIO_ALPHA_VANTAGE_API_KEY` |

## Verification Strategy

**Unit tests** (in `alpha_vantage.rs`):
- `parse_transcript_response` — JSON → `TranscriptEvidence` mapping
- `parse_rate_limit_response` — `"Note"` field triggers rate-limit detection
- `parse_empty_transcript` — empty array → `Ok(None)`
- `parse_partial_sentiment` — missing sentiment handling
- `key_rotation_on_rate_limit` — round-robin advances
- `all_keys_exhausted_returns_none` — all keys rate-limited → `Ok(None)`
- `comma_split_keys` — comma-separated key parsing

**Config tests** (in `config.rs` / `settings.rs`):
- `alpha_vantage_key_from_env` — env var loads correctly
- `alpha_vantage_rps_default` — default is 1
- `roundtrip_alpha_vantage_key` — survives `PartialConfig` → TOML → `PartialConfig`

**Fixture test:**
- `prompt_bundle_regression_gate` — transcript block appears in rendered prompts when `TranscriptEvidence` is present

**Smoke test:**
```bash
SCORPIO_ALPHA_VANTAGE_API_KEY=your_key \
cargo run -p scorpio-cli -- analyze COIN --json
```

Verify: transcript evidence in JSON output, content is concatenated segments, sentiment averaged, `call_date` is `"YYYY-QN"`.

## Out of Scope

- **Sentiment analysis beyond averaging.** The per-segment sentiment from Alpha Vantage is pre-computed. We don't run our own NLP on transcript text in this slice.
- **Transcript caching.** No local cache for transcripts. Each run fetches fresh. Can be added later if quota becomes a concern.
- **Quarter backward walk.** The earnings calendar determines the exact quarter to fetch. No guessing/walking. If calendar data is unavailable, derive quarter from `as_of_date` and make one call.
- **Structured transcript parsing.** We concatenate all segments into a single string. Structured Q&A separation, speaker indexing, etc. are follow-up enhancements.

## Attribution

This integration uses the Alpha Vantage EARNINGS_CALL_TRANSCRIPT API. Free tier: 25 requests/day per key.
