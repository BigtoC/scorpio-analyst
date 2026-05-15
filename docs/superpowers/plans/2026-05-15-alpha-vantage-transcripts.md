# Alpha Vantage Earnings Call Transcripts Integration — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Alpha Vantage's `EARNINGS_CALL_TRANSCRIPT` API as a live `TranscriptProvider`, replacing the contract-only seam in `transcripts.rs`, so Theme C can compare tone between press releases and earnings calls.

**Architecture:** New `AlphaVantageClient` in `data/alpha_vantage.rs` implements the updated `TranscriptProvider` trait (returns `TranscriptFetch` enum). A new `hydrate_transcript` function in the enrichment pipeline resolves the fiscal quarter from the earnings calendar, calls the client, and writes results to context keys (`KEY_CACHED_TRANSCRIPT` + sibling `KEY_TRANSCRIPT_FETCH_STATUS`). Prompt renderers read both keys to produce distinct language per fetch outcome. No `TradingState` schema changes — context keys only.

**Tech Stack:** Rust (edition 2024), `reqwest` (HTTP), `serde`/`serde_json` (JSON), `secrecy` (SecretString), `async-trait`, `tokio`, `tracing`, `graph_flow` (context keys), existing `SharedRateLimiter` pattern.

---

## File Structure

| File                                                                                    | Action     | Responsibility                                                                                                                            |
|-----------------------------------------------------------------------------------------|------------|-------------------------------------------------------------------------------------------------------------------------------------------|
| `crates/scorpio-core/src/config.rs`                                                     | Modify     | Add `alpha_vantage_api_key` to `ApiConfig`, `alpha_vantage_rps` to `RateLimitConfig`, env injection                                       |
| `crates/scorpio-core/src/settings.rs`                                                   | Modify     | Add `alpha_vantage_api_key` to `PartialConfig` + `UserConfigFile`, round-trip, Debug redaction                                            |
| `crates/scorpio-core/src/rate_limit.rs`                                                 | Modify     | Add `SharedRateLimiter::alpha_vantage_from_config`                                                                                        |
| `.env.example`                                                                          | Modify     | Add `SCORPIO_ALPHA_VANTAGE_API_KEY`                                                                                                       |
| `crates/scorpio-cli/src/cli/setup/steps.rs`                                             | Modify     | Add Alpha Vantage API key wizard step                                                                                                     |
| `crates/scorpio-core/src/data/adapters/transcripts.rs`                                  | Modify     | Update `TranscriptEvidence` (segments, drop content/sentiment_score), add `TranscriptFetch` enum, change `TranscriptProvider` return type |
| `crates/scorpio-core/src/data/adapters/catalysts.rs`                                    | Modify     | Add `fiscal_period: Option<String>` to `CatalystEvent`, update Finnhub builder                                                            |
| `crates/scorpio-core/src/data/alpha_vantage.rs`                                         | **Create** | `AlphaVantageClient`, serde structs, `TranscriptProvider` impl, key rotation, validation                                                  |
| `crates/scorpio-core/src/data/mod.rs`                                                   | Modify     | Add `pub mod alpha_vantage;`                                                                                                              |
| `crates/scorpio-core/src/workflow/tasks/common.rs`                                      | Modify     | Add `KEY_TRANSCRIPT_FETCH_STATUS` constant                                                                                                |
| `crates/scorpio-core/src/workflow/pipeline/runtime.rs`                                  | Modify     | Add `hydrate_transcript` fn, wire into enrichment section of `run_analysis_cycle`                                                         |
| `crates/scorpio-core/src/workflow/pipeline/mod.rs`                                      | Modify     | Add `alpha_vantage: Option<AlphaVantageClient>` to `TradingPipeline`                                                                      |
| `crates/scorpio-core/src/app/mod.rs`                                                    | Modify     | Construct `AlphaVantageClient` conditionally, pass to pipeline                                                                            |
| `crates/scorpio-core/src/agents/shared/prompt.rs`                                       | Modify     | Update `build_enrichment_context` to render transcript segments + fetch status                                                            |
| `crates/scorpio-core/src/analysis_packs/equity/prompts/theme_c_management_red_flags.md` | Modify     | Remove `TODO(transcripts)` marker, add transcript-aware language                                                                          |
| `crates/scorpio-core/src/analysis_packs/equity/prompts/conservative_risk.md`            | Modify     | Remove `TODO(transcripts)` marker                                                                                                         |
| `crates/scorpio-core/tests/fixtures/prompt_bundle/*.txt`                                | Modify     | Update fixture files to match new prompt output                                                                                           |

---

## Chunk 1: Config & Settings Foundation

### Task 1: Add `alpha_vantage_api_key` to `ApiConfig`

**Files:**
- Modify: `crates/scorpio-core/src/config.rs:157-164`

- [ ] **Step 1: Write the failing test**

Add a test in `crates/scorpio-core/src/config.rs` (in the existing `#[cfg(test)] mod tests` block) that verifies the new field exists and defaults to `None`:

```rust
#[test]
fn api_config_alpha_vantage_key_defaults_to_none() {
    let cfg = ApiConfig::default();
    assert!(cfg.alpha_vantage_api_key.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(api_config_alpha_vantage_key_defaults_to_none)'`
Expected: FAIL — `alpha_vantage_api_key` field does not exist on `ApiConfig`

- [ ] **Step 3: Add the field to `ApiConfig`**

In `crates/scorpio-core/src/config.rs`, add to the `ApiConfig` struct (after `fred_api_key`):

```rust
#[serde(skip)]
pub alpha_vantage_api_key: Option<SecretString>,
```

Also update the manual `Debug` impl for `ApiConfig` (search for `impl std::fmt::Debug for ApiConfig`) to redact the new field:

```rust
.field("alpha_vantage_api_key", &secret_display(&self.alpha_vantage_api_key))
```

Also add env injection in `load_effective_runtime` (after the `fred` line at ~line 574):

```rust
inject_env_override!(
    cfg.api.alpha_vantage_api_key,
    "SCORPIO_ALPHA_VANTAGE_API_KEY",
    "alpha_vantage"
);
```

And in `load_from` (after the `fred` line at ~line 615):

```rust
cfg.api.alpha_vantage_api_key = secret_from_env("SCORPIO_ALPHA_VANTAGE_API_KEY");
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(api_config_alpha_vantage_key_defaults_to_none)'`
Expected: PASS

- [ ] **Step 5: Run clippy and format**

Run: `cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "feat(config): add alpha_vantage_api_key to ApiConfig with env injection"
```

---

### Task 2: Add `alpha_vantage_rps` to `RateLimitConfig`

**Files:**
- Modify: `crates/scorpio-core/src/config.rs:314-325`
- Modify: `crates/scorpio-core/src/rate_limit.rs:89-97`

- [ ] **Step 1: Write the failing test**

Add a test in `crates/scorpio-core/src/config.rs`:

```rust
#[test]
fn rate_limit_config_alpha_vantage_rps_defaults_to_one() {
    // RateLimitConfig is deserialized from TOML; test the default function directly.
    assert_eq!(super::default_alpha_vantage_rps(), 1);
}
```

Add the default function (follow the existing pattern for `default_finnhub_rps`, `default_fred_rps`, `default_yahoo_finance_rps`):

```rust
fn default_alpha_vantage_rps() -> u32 {
    1
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(rate_limit_config_alpha_vantage_rps_defaults_to_one)'`
Expected: FAIL — `default_alpha_vantage_rps` not found

- [ ] **Step 3: Add the field to `RateLimitConfig`**

In `crates/scorpio-core/src/config.rs`, add to the `RateLimitConfig` struct:

```rust
#[serde(default = "default_alpha_vantage_rps")]
pub alpha_vantage_rps: u32,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(rate_limit_config_alpha_vantage_rps_defaults_to_one)'`
Expected: PASS

- [ ] **Step 5: Add `alpha_vantage_from_config` to `SharedRateLimiter`**

In `crates/scorpio-core/src/rate_limit.rs`, add after `yahoo_finance_from_config` (line ~97):

```rust
/// Create an Alpha Vantage rate limiter from `RateLimitConfig`.
///
/// Returns `None` when `cfg.alpha_vantage_rps == 0` (disabled).
pub fn alpha_vantage_from_config(cfg: &RateLimitConfig) -> Option<Self> {
    if cfg.alpha_vantage_rps == 0 {
        return None;
    }
    Some(Self::new("alpha_vantage", cfg.alpha_vantage_rps))
}
```

- [ ] **Step 6: Update any existing tests that construct `RateLimitConfig` as a struct literal**

Search for `RateLimitConfig {` in test code. Any struct literal that doesn't use `..Default::default()` will need the new `alpha_vantage_rps` field added. Run `cargo nextest run --workspace --all-features` to find compile errors.

- [ ] **Step 7: Run clippy and format**

Run: `cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/config.rs crates/scorpio-core/src/rate_limit.rs
git commit -m "feat(config): add alpha_vantage_rps to RateLimitConfig with default 1 rps"
```

---

### Task 3: Add `alpha_vantage_api_key` to `PartialConfig` and `UserConfigFile`

**Files:**
- Modify: `crates/scorpio-core/src/settings.rs:238-355`

- [ ] **Step 1: Write the failing test**

Add a test in `crates/scorpio-core/src/settings.rs` (in the existing `#[cfg(test)] mod tests` block):

```rust
#[test]
fn partial_config_alpha_vantage_key_roundtrip() {
    let mut partial = PartialConfig::default();
    partial.alpha_vantage_api_key = Some("test-key".to_owned());
    let toml = toml::to_string(&partial).expect("serialize");
    let recovered: PartialConfig = toml::from_str(&toml).expect("deserialize");
    assert_eq!(recovered.alpha_vantage_api_key, Some("test-key".to_owned()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(partial_config_alpha_vantage_key_roundtrip)'`
Expected: FAIL — `alpha_vantage_api_key` field not found on `PartialConfig`

- [ ] **Step 3: Add the field to `PartialConfig`**

In `crates/scorpio-core/src/settings.rs`, add to `PartialConfig` (after `fred_api_key`, around line 245):

```rust
/// Alpha Vantage API key(s) for earnings call transcripts (comma-separated for multiple keys).
#[serde(skip_serializing_if = "Option::is_none", default)]
pub alpha_vantage_api_key: Option<String>,
```

Update the `Debug` impl (around line 325) to redact:

```rust
.field("alpha_vantage_api_key", &redact(&self.alpha_vantage_api_key))
```

- [ ] **Step 4: Update `UserConfigFile` and its `From` impls**

Find the `UserConfigFile` struct (around line 19) and its `From<UserConfigFile> for PartialConfig` impl. Add the corresponding field and mapping. Also update `save_user_config_at` if it explicitly lists fields.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(partial_config_alpha_vantage_key_roundtrip)'`
Expected: PASS

- [ ] **Step 6: Add Debug redaction test**

```rust
#[test]
fn debug_redacts_alpha_vantage_api_key() {
    let mut partial = PartialConfig::default();
    partial.alpha_vantage_api_key = Some("secret-key".to_owned());
    let debug = format!("{:?}", partial);
    assert!(!debug.contains("secret-key"));
    assert!(debug.contains("[REDACTED]"));
}
```

- [ ] **Step 7: Wire into `Config::load_from_user_path`**

In `crates/scorpio-core/src/config.rs`, find `load_from_user_path` (or `load_effective_runtime`). After the line that maps `partial.fred_api_key` to `cfg.api.fred_api_key` (around line 520-522), add:

```rust
if let Some(k) = &partial.alpha_vantage_api_key {
    cfg.api.alpha_vantage_api_key = Some(SecretString::from(k.clone()));
}
```

- [ ] **Step 8: Run full test suite**

Run: `cargo nextest run --workspace --all-features --no-fail-fast`

- [ ] **Step 9: Commit**

```bash
git add crates/scorpio-core/src/settings.rs crates/scorpio-core/src/config.rs
git commit -m "feat(settings): add alpha_vantage_api_key to PartialConfig with round-trip and redaction"
```

---

### Task 4: Add setup wizard step and `.env.example`

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs:78-102`
- Modify: `.env.example`

- [ ] **Step 1: Add the wizard step function**

In `crates/scorpio-cli/src/cli/setup/steps.rs`, add after `step2_fred_api_key` (after line 102):

```rust
// ── Step 2b: Alpha Vantage API key ─────────────────────────────────────────

/// Prompt for the optional Alpha Vantage API key(s), preserving an existing saved value on empty input.
/// Multiple keys can be comma-separated to multiply the daily quota.
pub fn step2b_alpha_vantage_api_key(partial: &mut PartialConfig) -> Result<(), inquire::InquireError> {
    println!(
        "Alpha Vantage provides earnings call transcripts.\n\
         Get your free key at: https://www.alphavantage.co/support/#api-key\n\
         Multiple keys can be comma-separated to multiply the 25 req/day quota."
    );
    let existing = partial.alpha_vantage_api_key.clone();
    let mut prompt = inquire::Password::new("Alpha Vantage API key(s) (comma-separated):")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation();
    if existing.is_some() {
        prompt = prompt.with_help_message("[already set — press Enter to keep]");
    }
    let input = prompt.prompt()?;
    partial.alpha_vantage_api_key = apply_optional_secret(&input, existing);
    Ok(())
}
```

- [ ] **Step 2: Wire the step into the setup wizard**

Find where `step2_fred_api_key` is called in the setup flow (likely in `crates/scorpio-cli/src/cli/setup/mod.rs` or similar). Add `step2b_alpha_vantage_api_key(partial)?;` after the FRED step.

- [ ] **Step 3: Update `.env.example`**

Add after the `SCORPIO_FRED_API_KEY` line:

```
SCORPIO_ALPHA_VANTAGE_API_KEY=your-alpha-vantage-key-here
```

- [ ] **Step 4: Run clippy and format**

Run: `cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-cli/src/cli/setup/steps.rs .env.example
git commit -m "feat(setup): add Alpha Vantage API key wizard step and .env.example entry"
```

---

## Chunk 2: Data Contract Changes

### Task 5: Update `TranscriptEvidence` and introduce `TranscriptFetch`

**Files:**
- Modify: `crates/scorpio-core/src/data/adapters/transcripts.rs:1-74`

This is a **breaking change** to the contract-only types. No live provider was wired, so the only consumers are the existing roundtrip tests and the `TODO(transcripts)` prompt markers.

- [ ] **Step 1: Write the failing tests**

Replace the entire `#[cfg(test)] mod tests` block in `transcripts.rs` with tests that exercise the new schema:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_evidence_with_segments_roundtrips() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Tim Cook".to_owned(),
                    title: "Chief Executive Officer".to_owned(),
                    content: "We had a great quarter...".to_owned(),
                    sentiment: Some(0.85),
                },
                TranscriptSegment {
                    speaker: "Luca Maestri".to_owned(),
                    title: "Chief Financial Officer".to_owned(),
                    content: "Revenue grew 5%...".to_owned(),
                    sentiment: None,
                },
            ],
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: TranscriptEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn rendered_content_joins_segments() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Tim Cook".to_owned(),
                    title: "CEO".to_owned(),
                    content: "Hello everyone.".to_owned(),
                    sentiment: Some(0.5),
                },
                TranscriptSegment {
                    speaker: "Luca Maestri".to_owned(),
                    title: "CFO".to_owned(),
                    content: "Thanks Tim.".to_owned(),
                    sentiment: None,
                },
            ],
        };
        let rendered = evidence.rendered_content();
        assert!(rendered.contains("Tim Cook (CEO): Hello everyone."));
        assert!(rendered.contains("Luca Maestri (CFO): Thanks Tim."));
    }

    #[test]
    fn rendered_content_empty_segments() {
        let evidence = TranscriptEvidence {
            symbol: "COIN".to_owned(),
            call_date: "2024Q4".to_owned(),
            segments: vec![],
        };
        assert_eq!(evidence.rendered_content(), "");
    }

    #[test]
    fn transcript_fetch_found_serializes() {
        let fetch = TranscriptFetch::Found(TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![],
        });
        let json = serde_json::to_string(&fetch).expect("serialization");
        assert!(json.contains("Found"));
    }

    #[test]
    fn transcript_fetch_not_published_serializes() {
        let fetch: TranscriptFetch = TranscriptFetch::NotPublished;
        let json = serde_json::to_string(&fetch).expect("serialization");
        assert_eq!(json, "\"NotPublished\"");
    }

    #[test]
    fn call_date_is_quarter_format() {
        let evidence = TranscriptEvidence {
            symbol: "MSFT".to_owned(),
            call_date: "2025Q3".to_owned(),
            segments: vec![],
        };
        // Verify the doc-promised format is what we store
        assert!(evidence.call_date.contains('Q'));
        assert_eq!(evidence.call_date, "2025Q3");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(transcripts::)'`
Expected: FAIL — `TranscriptSegment` not found, `TranscriptFetch` not found, `rendered_content` not found

- [ ] **Step 3: Rewrite `transcripts.rs` with the new contract**

Replace the entire file content:

```rust
//! Transcript evidence contract and fetch-outcome types.
//!
//! [`TranscriptEvidence`] carries structured per-segment data from earnings
//! call transcripts. [`TranscriptFetch`] wraps the four possible outcomes
//! of a transcript fetch attempt: found, not published, throttled, or
//! unavailable.

use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// A single speaker segment within an earnings-call transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Speaker name (e.g., "Tim Cook").
    pub speaker: String,
    /// Speaker title (e.g., "Chief Executive Officer").
    pub title: String,
    /// Spoken content for this segment.
    pub content: String,
    /// Provider-computed sentiment score for this segment, if available (`-1.0` to `1.0`).
    pub sentiment: Option<f64>,
}

/// Structured earnings-call transcript evidence.
///
/// `call_date` uses `"YYYY-QN"` format (e.g., `"2025Q1"`) matching Alpha
/// Vantage's native quarter granularity. The canonical content is
/// `segments`; call [`rendered_content`](Self::rendered_content) for a
/// flat string when needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEvidence {
    /// Ticker symbol (canonical uppercase).
    pub symbol: String,
    /// Quarter identifier in `"YYYY-QN"` format (e.g., `"2025Q1"`).
    pub call_date: String,
    /// Per-speaker transcript segments.
    pub segments: Vec<TranscriptSegment>,
}

impl TranscriptEvidence {
    /// Render all segments into a single string.
    ///
    /// Each segment is formatted as `"{speaker} ({title}): {content}"` and
    /// joined by `"\n\n"`. Returns an empty string when `segments` is empty.
    pub fn rendered_content(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("{} ({}): {}", s.speaker, s.title, s.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Outcome of a transcript-fetch attempt.
///
/// Each variant produces distinct prompt-layer language and audit-trail
/// metadata. Network/HTTP errors that persist after retries map to
/// `Err(TradingError)`, not to these variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TranscriptFetch {
    /// Transcript found and parsed.
    Found(TranscriptEvidence),
    /// API responded normally; no transcript published for this symbol/quarter yet.
    NotPublished,
    /// Every configured key returned a rate-limit signal within this call.
    Throttled,
    /// Recoverable transient failure (HTTP 5xx / timeout) persisted after retries.
    Unavailable,
}

/// Contract for any provider that can supply earnings-call transcripts.
///
/// Implementations return a [`TranscriptFetch`] enum rather than
/// `Option<TranscriptEvidence>` so callers can distinguish "not published"
/// from "throttled" from "unavailable".
#[async_trait::async_trait]
pub trait TranscriptProvider: Send + Sync {
    /// Fetch the transcript for `symbol` in the quarter identified by
    /// `as_of_date` (format `"YYYY-QN"`, e.g., `"2025Q1"`).
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<TranscriptFetch, TradingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_evidence_with_segments_roundtrips() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Tim Cook".to_owned(),
                    title: "Chief Executive Officer".to_owned(),
                    content: "We had a great quarter...".to_owned(),
                    sentiment: Some(0.85),
                },
                TranscriptSegment {
                    speaker: "Luca Maestri".to_owned(),
                    title: "Chief Financial Officer".to_owned(),
                    content: "Revenue grew 5%...".to_owned(),
                    sentiment: None,
                },
            ],
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: TranscriptEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn rendered_content_joins_segments() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Tim Cook".to_owned(),
                    title: "CEO".to_owned(),
                    content: "Hello everyone.".to_owned(),
                    sentiment: Some(0.5),
                },
                TranscriptSegment {
                    speaker: "Luca Maestri".to_owned(),
                    title: "CFO".to_owned(),
                    content: "Thanks Tim.".to_owned(),
                    sentiment: None,
                },
            ],
        };
        let rendered = evidence.rendered_content();
        assert!(rendered.contains("Tim Cook (CEO): Hello everyone."));
        assert!(rendered.contains("Luca Maestri (CFO): Thanks Tim."));
    }

    #[test]
    fn rendered_content_empty_segments() {
        let evidence = TranscriptEvidence {
            symbol: "COIN".to_owned(),
            call_date: "2024Q4".to_owned(),
            segments: vec![],
        };
        assert_eq!(evidence.rendered_content(), "");
    }

    #[test]
    fn transcript_fetch_found_serializes() {
        let fetch = TranscriptFetch::Found(TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![],
        });
        let json = serde_json::to_string(&fetch).expect("serialization");
        assert!(json.contains("Found"));
    }

    #[test]
    fn transcript_fetch_not_published_serializes() {
        let fetch: TranscriptFetch = TranscriptFetch::NotPublished;
        let json = serde_json::to_string(&fetch).expect("serialization");
        assert_eq!(json, "\"NotPublished\"");
    }

    #[test]
    fn call_date_is_quarter_format() {
        let evidence = TranscriptEvidence {
            symbol: "MSFT".to_owned(),
            call_date: "2025Q3".to_owned(),
            segments: vec![],
        };
        assert!(evidence.call_date.contains('Q'));
        assert_eq!(evidence.call_date, "2025Q3");
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(transcripts::)'`
Expected: PASS

- [ ] **Step 5: Fix any downstream compile errors**

The `TranscriptEvidence` type change (removing `content` and `sentiment_score`, adding `segments`) will break any code that constructs or reads these fields. Search for all usages:

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | head -50
```

Likely breakage points:
- `KEY_CACHED_TRANSCRIPT` deserialization sites (these read `Option<TranscriptEvidence>` from context — the JSON shape changes)
- Prompt fixture files that reference `content` or `sentiment_score`
- The `TranscriptProvider` trait return type change from `Result<Option<TranscriptEvidence>, TradingError>` to `Result<TranscriptFetch, TradingError>` — but since no concrete impl exists yet, only the trait definition and any mock impls in tests break

Fix any compile errors found. Do NOT modify `alpha_vantage.rs` yet (it doesn't exist yet).

- [ ] **Step 6: Run full test suite**

Run: `cargo nextest run --workspace --all-features --no-fail-fast`

- [ ] **Step 7: Commit**

```bash
git add crates/scorpio-core/src/data/adapters/transcripts.rs
git commit -m "feat(transcripts): update TranscriptEvidence to segments, add TranscriptFetch enum"
```

---

### Task 6: Add `fiscal_period` to `CatalystEvent`

**Files:**
- Modify: `crates/scorpio-core/src/data/adapters/catalysts.rs:32-48`

- [ ] **Step 1: Write the failing tests**

Add tests in the existing `#[cfg(test)] mod tests` block in `catalysts.rs`:

```rust
#[test]
fn fiscal_period_roundtrip() {
    let event = CatalystEvent {
        symbol: "AAPL".to_owned(),
        event_date: "2025-01-30".to_owned(),
        category: CatalystCategory::EarningsAndFinancial,
        impact: ImpactLevel::H,
        headline: "AAPL Q1 2025 earnings".to_owned(),
        source_url: None,
        source: "finnhub".to_owned(),
        fiscal_period: Some("2025Q1".to_owned()),
    };
    let json = serde_json::to_string(&event).expect("serialization");
    let recovered: CatalystEvent = serde_json::from_str(&json).expect("deserialization");
    assert_eq!(event, recovered);
}

#[test]
fn fiscal_period_default_for_legacy_snapshots() {
    // A JSON payload without fiscal_period should deserialize with fiscal_period = None
    let json = r#"{
        "symbol": "AAPL",
        "event_date": "2025-01-30",
        "category": "EarningsAndFinancial",
        "impact": "H",
        "headline": "AAPL Q1 2025 earnings",
        "source": "finnhub"
    }"#;
    let event: CatalystEvent = serde_json::from_str(json).expect("deserialization");
    assert!(event.fiscal_period.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(fiscal_period)'`
Expected: FAIL — `fiscal_period` field not found on `CatalystEvent`

- [ ] **Step 3: Add the field to `CatalystEvent`**

In `crates/scorpio-core/src/data/adapters/catalysts.rs`, add to the `CatalystEvent` struct (after `source`):

```rust
/// Fiscal period in `"YYYY-QN"` format (e.g., `"2025Q1"`), populated when the
/// upstream provider supplies year+quarter metadata. `None` for providers that
/// lack this data (e.g., yfinance) or for non-earnings catalysts.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub fiscal_period: Option<String>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(fiscal_period)'`
Expected: PASS

- [ ] **Step 5: Update the Finnhub earnings builder**

In `map_finnhub_earnings` (around line 368), update the `CatalystEvent` construction to populate `fiscal_period` from `EarningsRelease.year` + `EarningsRelease.quarter`:

```rust
fn map_finnhub_earnings(queried_symbol: &str, r: &EarningsRelease) -> Option<CatalystEvent> {
    let date = r.date.as_deref()?;
    let symbol = r.symbol.as_deref().unwrap_or(queried_symbol).to_ascii_uppercase();
    let label = match (r.year, r.quarter) {
        (Some(y), Some(q)) => format!("{symbol} Q{q} {y} earnings"),
        _ => format!("{symbol} earnings"),
    };
    let fiscal_period = match (r.year, r.quarter) {
        (Some(y), Some(q)) => Some(format!("{y}Q{q}")),
        _ => None,
    };
    Some(CatalystEvent {
        symbol,
        event_date: date.to_owned(),
        category: CatalystCategory::EarningsAndFinancial,
        impact: ImpactLevel::H,
        headline: label,
        source_url: None,
        source: "finnhub".to_owned(),
        fiscal_period,
    })
}
```

- [ ] **Step 6: Add builder test**

```rust
#[test]
fn finnhub_builder_populates_fiscal_period() {
    use finnhub::models::calendar::EarningsRelease;
    let release = EarningsRelease {
        symbol: Some("AAPL".to_owned()),
        date: Some("2025-01-30".to_owned()),
        hour: Some("amc".to_owned()),
        year: Some(2025),
        quarter: Some(1),
        eps_estimate: Some(2.35),
        eps_actual: Some(2.40),
        revenue_estimate: Some(120_000.0),
        revenue_actual: Some(124_000.0),
    };
    let event = map_finnhub_earnings("AAPL", &release).expect("should map");
    assert_eq!(event.fiscal_period, Some("2025Q1".to_owned()));
}

#[test]
fn finnhub_builder_leaves_fiscal_period_none_when_missing() {
    use finnhub::models::calendar::EarningsRelease;
    let release = EarningsRelease {
        symbol: Some("AAPL".to_owned()),
        date: Some("2025-01-30".to_owned()),
        hour: None,
        year: None,
        quarter: Some(1),
        eps_estimate: None,
        eps_actual: None,
        revenue_estimate: None,
        revenue_actual: None,
    };
    let event = map_finnhub_earnings("AAPL", &release).expect("should map");
    assert!(event.fiscal_period.is_none());
}
```

- [ ] **Step 7: Update all other `CatalystEvent` constructions in the file**

Search for `CatalystEvent {` in `catalysts.rs` — every construction site needs `fiscal_period: None` (or the appropriate value). The yfinance and FRED builders should set `fiscal_period: None`. Run `cargo clippy` to find any remaining compile errors.

- [ ] **Step 8: Update existing CatalystEvent test fixtures**

Any existing test that constructs a `CatalystEvent` struct literal needs the new field. Search for `CatalystEvent {` in test code across the workspace and add `fiscal_period: None` (or use `..Default::default()` if `Default` is implemented).

- [ ] **Step 9: Run full test suite**

Run: `cargo nextest run --workspace --all-features --no-fail-fast`

- [ ] **Step 10: Commit**

```bash
git add crates/scorpio-core/src/data/adapters/catalysts.rs
git commit -m "feat(catalysts): add fiscal_period field to CatalystEvent with Finnhub builder population"
```

---

## Chunk 3: AlphaVantageClient Implementation

### Task 7: Create `AlphaVantageClient` with serde structs and validation

**Files:**
- Create: `crates/scorpio-core/src/data/alpha_vantage.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/scorpio-core/src/data/alpha_vantage.rs` with the test module first:

```rust
//! Alpha Vantage earnings-call transcript provider.
//!
//! Implements [`TranscriptProvider`] for Alpha Vantage's
//! `EARNINGS_CALL_TRANSCRIPT` API. Supports multiple comma-separated
//! API keys with round-robin rotation on rate-limit responses.

use std::sync::atomic::{AtomicUsize, Ordering};

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::{ApiConfig, RateLimitConfig};
use crate::data::adapters::transcripts::{
    TranscriptEvidence, TranscriptFetch, TranscriptProvider, TranscriptSegment,
};
use crate::error::TradingError;
use crate::rate_limit::SharedRateLimiter;

const BASE_URL: &str = "https://www.alphavantage.co/query";

/// Alpha Vantage API client for earnings-call transcripts.
///
/// Supports multiple comma-separated API keys with round-robin rotation
/// on rate-limit responses (`"Note"` / `"Information"` JSON fields).
#[derive(Clone)]
pub struct AlphaVantageClient {
    keys: Vec<SecretString>,
    current_index: AtomicUsize,
    rate_limiter: SharedRateLimiter,
    http: reqwest::Client,
    base_url: String,
}

impl std::fmt::Debug for AlphaVantageClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlphaVantageClient")
            .field("key_count", &self.keys.len())
            .field("current_index", &self.current_index.load(Ordering::Relaxed))
            .field("rate_limiter", &self.rate_limiter.label())
            .finish()
    }
}

/// Internal serde struct for Alpha Vantage transcript API responses.
#[derive(Deserialize)]
struct AlphaVantageTranscriptResponse {
    symbol: Option<String>,
    quarter: Option<String>,
    transcript: Option<Vec<TranscriptSegment>>,
    /// Rate-limit / daily-quota signal.
    #[serde(rename = "Note")]
    note: Option<String>,
    /// Alternative rate-limit / daily-quota signal.
    #[serde(rename = "Information")]
    information: Option<String>,
    /// Per-request hard error (bad symbol, malformed params).
    #[serde(rename = "Error Message")]
    error_message: Option<String>,
}

impl AlphaVantageClient {
    /// Construct a new client from the API config and rate limiter.
    ///
    /// Splits `alpha_vantage_api_key` on commas, trims whitespace, and wraps
    /// each non-empty fragment in a `SecretString`. Returns
    /// `Err(TradingError::Config)` if no key is configured.
    ///
    /// **Security:** The error message is a static string — no key material
    /// is interpolated.
    pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        let raw = api
            .alpha_vantage_api_key
            .as_ref()
            .ok_or_else(|| {
                TradingError::Config(anyhow::anyhow!(
                    "SCORPIO_ALPHA_VANTAGE_API_KEY is not set"
                ))
            })?
            .expose_secret();

        let keys: Vec<SecretString> = raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(SecretString::from)
            .collect();

        if keys.is_empty() {
            return Err(TradingError::Config(anyhow::anyhow!(
                "SCORPIO_ALPHA_VANTAGE_API_KEY is not set"
            )));
        }

        Ok(Self {
            keys,
            current_index: AtomicUsize::new(0),
            rate_limiter: limiter,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| TradingError::Config(anyhow::anyhow!("reqwest client build: {e}")))?,
            base_url: BASE_URL.to_owned(),
        })
    }

    /// Test-only constructor with a single dummy key.
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self {
            keys: vec![SecretString::from("test-dummy-key")],
            current_index: AtomicUsize::new(0),
            rate_limiter: SharedRateLimiter::new("test-alpha-vantage", 1),
            http: reqwest::Client::new(),
            base_url: BASE_URL.to_owned(),
        }
    }

    /// Validate the symbol format.
    fn validate_symbol(symbol: &str) -> Result<(), TradingError> {
        if symbol.is_empty() || symbol.len() > 10 || !symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
            return Err(TradingError::SchemaViolation {
                message: format!("invalid symbol: {symbol:?}"),
            });
        }
        Ok(())
    }

    /// Validate the quarter format (`"YYYY-QN"` where N is 1-4).
    fn validate_quarter(quarter: &str) -> Result<(), TradingError> {
        let re = regex::Regex::new(r"^\d{4}Q[1-4]$").unwrap();
        if !re.is_match(quarter) {
            return Err(TradingError::SchemaViolation {
                message: format!("invalid quarter format (expected YYYY-QN): {quarter:?}"),
            });
        }
        Ok(())
    }

    /// Select the current key index and advance for the next invocation.
    fn current_key_index(&self) -> usize {
        let idx = self.current_index.fetch_add(1, Ordering::Relaxed);
        idx % self.keys.len()
    }

    /// Build the request URL for a transcript fetch.
    fn build_url(&self, symbol: &str, quarter: &str) -> String {
        format!(
            "{}?function=EARNINGS_CALL_TRANSCRIPT&symbol={}&quarter={}",
            self.base_url, symbol, quarter
        )
    }

    /// Parse the raw JSON response into a `TranscriptFetch`.
    ///
    /// Handles: successful transcript, empty/null transcript, Note/Information
    /// rate-limit signals, and Error Message hard errors.
    fn parse_response(raw: &str) -> Result<TranscriptFetch, TradingError> {
        let resp: AlphaVantageTranscriptResponse = serde_json::from_str(raw).map_err(|e| {
            TradingError::SchemaViolation {
                message: format!("Alpha Vantage response deserialization failed: {e}"),
            }
        })?;

        // Hard error from the API (invalid symbol/params)
        if let Some(msg) = &resp.error_message {
            return Err(TradingError::SchemaViolation {
                message: format!("Alpha Vantage error: {msg}"),
            });
        }

        // Rate-limit / daily-quota signals
        if resp.note.is_some() || resp.information.is_some() {
            return Ok(TranscriptFetch::Throttled);
        }

        // Transcript data
        match resp.transcript {
            Some(segments) if !segments.is_empty() => {
                let symbol = resp.symbol.unwrap_or_default().to_uppercase();
                let call_date = resp.quarter.unwrap_or_default();
                Ok(TranscriptFetch::Found(TranscriptEvidence {
                    symbol,
                    call_date,
                    segments,
                }))
            }
            _ => Ok(TranscriptFetch::NotPublished),
        }
    }
}

#[async_trait::async_trait]
impl TranscriptProvider for AlphaVantageClient {
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<TranscriptFetch, TradingError> {
        Self::validate_symbol(symbol)?;
        Self::validate_quarter(as_of_date)?;

        let start_index = self.current_key_index();
        let num_keys = self.keys.len();

        for retry in 0..num_keys {
            let key_idx = (start_index + retry) % num_keys;
            let key = &keys[key_idx];

            self.rate_limiter.acquire().await;

            let url = self.build_url(symbol, as_of_date);
            let response = self
                .http
                .get(&url)
                .query(&[("apikey", key.expose_secret())])
                .send()
                .await;

            match response {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let body = resp.text().await.map_err(|e| {
                            TradingError::Config(anyhow::anyhow!("response read error: {e}"))
                        })?;
                        let result = Self::parse_response(&body)?;
                        match &result {
                            TranscriptFetch::Throttled => {
                                warn!(
                                    provider = "alpha_vantage",
                                    reason = "rate_limit",
                                    key_idx,
                                    "key throttled, rotating"
                                );
                                continue; // Try next key
                            }
                            _ => return Ok(result),
                        }
                    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        warn!(
                            provider = "alpha_vantage",
                            reason = "rate_limit",
                            key_idx,
                            "HTTP 429, rotating key"
                        );
                        continue;
                    } else {
                        return Err(TradingError::Config(anyhow::anyhow!(
                            "Alpha Vantage HTTP error: {status}"
                        )));
                    }
                }
                Err(e) => {
                    if e.is_timeout() || e.is_connect() {
                        return Ok(TranscriptFetch::Unavailable);
                    }
                    return Err(TradingError::Config(anyhow::anyhow!(
                        "Alpha Vantage request error: {e}"
                    )));
                }
            }
        }

        // Every key was throttled
        Ok(TranscriptFetch::Throttled)
    }
}

// Fix the borrow issue: need to access self.keys via &self.keys
// The above code has a bug: `keys` should be `self.keys`

#[cfg(test)]
mod tests {
    use super::*;

    // ── Comma-separated key parsing ────────────────────────────────────

    #[test]
    fn comma_split_keys() {
        let mut api = ApiConfig::default();
        api.alpha_vantage_api_key = Some(SecretString::from("key1, key2 ,key3"));
        let client = AlphaVantageClient::new(&api, SharedRateLimiter::disabled("test")).expect("construct");
        assert_eq!(client.keys.len(), 3);
    }

    #[test]
    fn single_key_no_split() {
        let mut api = ApiConfig::default();
        api.alpha_vantage_api_key = Some(SecretString::from("only-one-key"));
        let client = AlphaVantageClient::new(&api, SharedRateLimiter::disabled("test")).expect("construct");
        assert_eq!(client.keys.len(), 1);
    }

    #[test]
    fn constructor_error_does_not_leak_secret() {
        let api = ApiConfig::default(); // no key set
        let err = AlphaVantageClient::new(&api, SharedRateLimiter::disabled("test"))
            .unwrap_err();
        let msg = format!("{err}");
        // Must not contain any key-like material
        assert!(!msg.contains(','));
        assert!(!msg.contains("key"));
    }

    // ── Input validation ───────────────────────────────────────────────

    #[test]
    fn invalid_quarter_format_rejected() {
        let client = AlphaVantageClient::for_test();
        let err = tokio_test::block_on(client.fetch_transcript("AAPL", "2025-Q1"))
            .unwrap_err();
        assert!(format!("{err}").contains("invalid quarter format"));
    }

    #[test]
    fn invalid_symbol_rejected() {
        let client = AlphaVantageClient::for_test();
        let err = tokio_test::block_on(client.fetch_transcript("", "2025Q1"))
            .unwrap_err();
        assert!(format!("{err}").contains("invalid symbol"));
    }

    // ── Response parsing ───────────────────────────────────────────────

    #[test]
    fn parse_transcript_response() {
        let json = r#"{
            "symbol": "COIN",
            "quarter": "2024Q1",
            "transcript": [
                {
                    "speaker": "Alesia Haas",
                    "title": "Chief Financial Officer",
                    "content": "Thank you, operator...",
                    "sentiment": 0.85
                },
                {
                    "speaker": "Brian Armstrong",
                    "title": "CEO",
                    "content": "We had a strong quarter.",
                    "sentiment": null
                }
            ]
        }"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        match result {
            TranscriptFetch::Found(evidence) => {
                assert_eq!(evidence.symbol, "COIN");
                assert_eq!(evidence.call_date, "2024Q1");
                assert_eq!(evidence.segments.len(), 2);
                assert_eq!(evidence.segments[0].speaker, "Alesia Haas");
                assert_eq!(evidence.segments[0].sentiment, Some(0.85));
                assert!(evidence.segments[1].sentiment.is_none());
            }
            _ => panic!("expected Found"),
        }
    }

    #[test]
    fn parse_note_response_triggers_rotation() {
        let json = r#"{"Note": "Thank you for using Alpha Vantage..."}"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        assert_eq!(result, TranscriptFetch::Throttled);
    }

    #[test]
    fn parse_information_response_triggers_rotation() {
        let json = r#"{"Information": "You have exceeded the daily limit..."}"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        assert_eq!(result, TranscriptFetch::Throttled);
    }

    #[test]
    fn parse_error_message_returns_schema_violation() {
        let json = r#"{"Error Message": "Invalid API call"}"#;
        let err = AlphaVantageClient::parse_response(json).unwrap_err();
        assert!(format!("{err}").contains("Invalid API call"));
    }

    #[test]
    fn parse_empty_transcript_array() {
        let json = r#"{"symbol": "AAPL", "quarter": "2025Q1", "transcript": []}"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        assert_eq!(result, TranscriptFetch::NotPublished);
    }

    #[test]
    fn parse_missing_transcript_field() {
        let json = r#"{"symbol": "AAPL", "quarter": "2025Q1"}"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        assert_eq!(result, TranscriptFetch::NotPublished);
    }

    #[test]
    fn parse_partial_sentiment() {
        let json = r#"{
            "symbol": "AAPL",
            "quarter": "2025Q1",
            "transcript": [
                {"speaker": "A", "title": "B", "content": "C", "sentiment": 0.5},
                {"speaker": "D", "title": "E", "content": "F"}
            ]
        }"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        match result {
            TranscriptFetch::Found(evidence) => {
                assert_eq!(evidence.segments[0].sentiment, Some(0.5));
                assert!(evidence.segments[1].sentiment.is_none());
            }
            _ => panic!("expected Found"),
        }
    }

    // ── Key rotation ───────────────────────────────────────────────────

    #[test]
    fn key_rotation_on_rate_limit() {
        let mut api = ApiConfig::default();
        api.alpha_vantage_api_key = Some(SecretString::from("k1,k2,k3"));
        let client = AlphaVantageClient::new(&api, SharedRateLimiter::disabled("test")).expect("construct");
        // Verify that current_key_index advances
        let idx1 = client.current_key_index();
        let idx2 = client.current_key_index();
        assert_ne!(idx1, idx2);
    }

    // ── URL building ───────────────────────────────────────────────────

    #[test]
    fn build_url_format() {
        let client = AlphaVantageClient::for_test();
        let url = client.build_url("AAPL", "2025Q1");
        assert!(url.contains("symbol=AAPL"));
        assert!(url.contains("quarter=2025Q1"));
        assert!(url.contains("EARNINGS_CALL_TRANSCRIPT"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(alpha_vantage::)'`
Expected: FAIL — module not found

- [ ] **Step 3: Register the module**

In `crates/scorpio-core/src/data/mod.rs`, add:

```rust
pub mod alpha_vantage;
```

Add it near the other client modules (after `pub mod fred;`). Also add a re-export:

```rust
pub use alpha_vantage::AlphaVantageClient;
```

- [ ] **Step 4: Fix the `self.keys` borrow in `fetch_transcript`**

In the `fetch_transcript` method, the loop references `keys` instead of `self.keys`. Fix:

```rust
let key = &self.keys[key_idx];
```

- [ ] **Step 5: Add `regex` dependency if not present**

Check `crates/scorpio-core/Cargo.toml` for `regex`. If missing, add it. Also ensure `tokio-test` is in `[dev-dependencies]` for `block_on`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(alpha_vantage::)'`
Expected: PASS

- [ ] **Step 7: Run full test suite and clippy**

Run: `cargo fmt -- --check && cargo clippy --workspace --all-targets -- -D warnings`
Fix any warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/data/alpha_vantage.rs crates/scorpio-core/src/data/mod.rs
git commit -m "feat(data): add AlphaVantageClient with TranscriptProvider impl and key rotation"
```

---

## Chunk 4: Enrichment Pipeline Integration

### Task 8: Add context key constant and enrichment helper

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/common.rs:41-62`
- Create: `crates/scorpio-core/src/workflow/pipeline/runtime.rs` (add `hydrate_transcript` fn)

- [ ] **Step 1: Add `KEY_TRANSCRIPT_FETCH_STATUS` constant**

In `crates/scorpio-core/src/workflow/tasks/common.rs`, add after `KEY_CACHED_TRANSCRIPT`:

```rust
/// Context key for the `TranscriptFetch` outcome variant name.
/// Written by transcript enrichment or by preflight to `"NotPublished"`.
/// Always present after preflight.
pub const KEY_TRANSCRIPT_FETCH_STATUS: &str = "transcript_fetch_status";
```

- [ ] **Step 2: Write the `hydrate_transcript` function**

In `crates/scorpio-core/src/workflow/pipeline/runtime.rs`, add a new helper function (after `hydrate_consensus`):

```rust
/// Resolve the target fiscal quarter for transcript fetching.
///
/// Priority:
/// 1. Most recent past CatalystEvent with `fiscal_period` set (Finnhub source).
/// 2. Calendar-derived 6-week-lag heuristic.
fn resolve_transcript_quarter(
    catalyst_events: &[CatalystEvent],
    symbol: &str,
    as_of_date: &str,
) -> String {
    // Step 1: Look for the most recent past catalyst with fiscal_period
    let fiscal_match = catalyst_events
        .iter()
        .filter(|e| {
            e.symbol == symbol
                && e.category == CatalystCategory::EarningsAndFinancial
                && e.event_date <= as_of_date
                && e.fiscal_period.is_some()
        })
        .max_by_key(|e| &e.event_date);

    if let Some(event) = fiscal_match {
        info!(
            symbol,
            quarter = %event.fiscal_period.as_ref().unwrap(),
            source = "calendar_fiscal_period",
            "transcript quarter resolved from earnings calendar"
        );
        return event.fiscal_period.clone().unwrap();
    }

    // Step 2/3: Calendar-derived heuristic with 6-week reporting lag
    info!(
        symbol,
        "transcript quarter resolved via calendar heuristic (no fiscal_period on calendar)"
    );
    heuristic_quarter_from_date(as_of_date)
}

/// Compute the most likely reportable quarter from a calendar date using a
/// ~6-week reporting lag heuristic.
fn heuristic_quarter_from_date(as_of_date: &str) -> String {
    use chrono::NaiveDate;

    let date = NaiveDate::parse_from_str(as_of_date, "%Y-%m-%d")
        .unwrap_or_else(|_| chrono::Utc::now().date_naive());
    let month = date.month();
    let year = date.year();

    let (q, q_year) = if month <= 3 {
        // Q1 (Jan-Mar): 6 weeks in → Q4 prior year; early → Q3 prior year
        if date.day() >= 15 {
            (4, year - 1)
        } else {
            (3, year - 1)
        }
    } else if month <= 6 {
        // Q2 (Apr-Jun): 6 weeks in → Q1; early → Q4 prior year
        if date.day() >= 15 {
            (1, year)
        } else {
            (4, year - 1)
        }
    } else if month <= 9 {
        // Q3 (Jul-Sep): 6 weeks in → Q2; early → Q1
        if date.day() >= 15 {
            (2, year)
        } else {
            (1, year)
        }
    } else {
        // Q4 (Oct-Dec): 6 weeks in → Q3; early → Q2
        if date.day() >= 15 {
            (3, year)
        } else {
            (2, year)
        }
    };

    format!("{q_year}Q{q}")
}

/// Fetch transcript enrichment with a timeout boundary.
///
/// Resolves the fiscal quarter from the earnings calendar (or heuristic
/// fallback), calls the Alpha Vantage client, and writes results to
/// context keys.
async fn hydrate_transcript(
    client: &AlphaVantageClient,
    symbol: &str,
    as_of_date: &str,
    catalyst_events: &[CatalystEvent],
    timeout: std::time::Duration,
) -> (Option<TranscriptEvidence>, &'static str) {
    let quarter = resolve_transcript_quarter(catalyst_events, symbol, as_of_date);

    match tokio::time::timeout(
        timeout,
        client.fetch_transcript(symbol, &quarter),
    )
    .await
    {
        Ok(Ok(TranscriptFetch::Found(evidence))) => {
            info!(
                symbol,
                quarter = %quarter,
                segments = evidence.segments.len(),
                "transcript enrichment: available"
            );
            (Some(evidence), "Found")
        }
        Ok(Ok(TranscriptFetch::NotPublished)) => {
            info!(symbol, quarter = %quarter, "transcript enrichment: not published");
            (None, "NotPublished")
        }
        Ok(Ok(TranscriptFetch::Throttled)) => {
            warn!(symbol, "transcript enrichment: throttled on all keys");
            (None, "Throttled")
        }
        Ok(Ok(TranscriptFetch::Unavailable)) => {
            warn!(symbol, "transcript enrichment: unavailable (transient failure)");
            (None, "Unavailable")
        }
        Ok(Err(e)) => {
            warn!(symbol, error = %e, "transcript enrichment: fetch error (fail-open)");
            (None, "Unavailable")
        }
        Err(_) => {
            warn!(symbol, "transcript enrichment: timed out (fail-open)");
            (None, "Unavailable")
        }
    }
}
```

- [ ] **Step 3: Add required imports**

At the top of `runtime.rs`, add the new imports:

```rust
use crate::data::alpha_vantage::AlphaVantageClient;
use crate::data::adapters::transcripts::{TranscriptEvidence, TranscriptFetch};
use crate::data::adapters::catalysts::CatalystCategory;
use crate::workflow::tasks::KEY_TRANSCRIPT_FETCH_STATUS;
```

- [ ] **Step 4: Write quarter resolution tests**

Add a `#[cfg(test)]` module with tests for `heuristic_quarter_from_date`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_absent_mid_q2_dec_fy_resolves_to_q1() {
        // May 15 → Q2 started Apr 1, ~6.5 weeks in → Q1 same year
        assert_eq!(heuristic_quarter_from_date("2026-05-15"), "2026Q1");
    }

    #[test]
    fn calendar_absent_early_q2_resolves_to_q4_prior() {
        // Apr 5 → Q2 started Apr 1, < 6 weeks → Q4 prior year
        assert_eq!(heuristic_quarter_from_date("2026-04-05"), "2025Q4");
    }

    #[test]
    fn calendar_absent_mid_january_known_imprecise() {
        // Jan 15 → Q1 started Jan 1, 2 weeks → Q3 prior year (documented imprecise)
        assert_eq!(heuristic_quarter_from_date("2026-01-15"), "2025Q3");
    }

    #[test]
    fn calendar_absent_late_december() {
        // Dec 31 → Q4 started Oct 1, ~13 weeks → Q3 same year
        assert_eq!(heuristic_quarter_from_date("2025-12-31"), "2025Q3");
    }

    #[test]
    fn calendar_present_uses_fiscal_period_field() {
        let events = vec![CatalystEvent {
            symbol: "AAPL".to_owned(),
            event_date: "2025-01-30".to_owned(),
            category: CatalystCategory::EarningsAndFinancial,
            impact: crate::data::adapters::catalysts::ImpactLevel::H,
            headline: "AAPL Q1 earnings".to_owned(),
            source_url: None,
            source: "finnhub".to_owned(),
            fiscal_period: Some("2025Q1".to_owned()),
        }];
        let quarter = resolve_transcript_quarter(&events, "AAPL", "2025-03-15");
        assert_eq!(quarter, "2025Q1");
    }

    #[test]
    fn calendar_present_no_fiscal_period_falls_back_to_heuristic() {
        let events = vec![CatalystEvent {
            symbol: "AAPL".to_owned(),
            event_date: "2025-01-30".to_owned(),
            category: CatalystCategory::EarningsAndFinancial,
            impact: crate::data::adapters::catalysts::ImpactLevel::H,
            headline: "AAPL earnings".to_owned(),
            source_url: None,
            source: "yfinance".to_owned(),
            fiscal_period: None,
        }];
        let quarter = resolve_transcript_quarter(&events, "AAPL", "2026-05-15");
        // Falls through to heuristic: May 15 → 2026Q1
        assert_eq!(quarter, "2026Q1");
    }
}
```

- [ ] **Step 5: Run the new tests**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(heuristic_quarter)' -E 'test(resolve_transcript)'`
Expected: PASS

- [ ] **Step 6: Run full test suite**

Run: `cargo nextest run --workspace --all-features --no-fail-fast`

- [ ] **Step 7: Commit**

```bash
git add crates/scorpio-core/src/workflow/tasks/common.rs crates/scorpio-core/src/workflow/pipeline/runtime.rs
git commit -m "feat(enrichment): add transcript quarter resolution and hydrate_transcript helper"
```

---

### Task 9: Wire transcript enrichment into `run_analysis_cycle`

**Files:**
- Modify: `crates/scorpio-core/src/workflow/pipeline/runtime.rs:422-506`
- Modify: `crates/scorpio-core/src/workflow/pipeline/mod.rs:99-114`

- [ ] **Step 1: Add `alpha_vantage` field to `TradingPipeline`**

In `crates/scorpio-core/src/workflow/pipeline/mod.rs`, add to the `TradingPipeline` struct:

```rust
pub(super) alpha_vantage: Option<AlphaVantageClient>,
```

Add to the `Debug` impl:

```rust
.field("alpha_vantage", &self.alpha_vantage.as_ref().map(|c| format!("{:?}", c)))
```

Update `new`, `try_new`, `__from_parts`, and `build_graph` to accept and store the new field. The parameter should be `Option<AlphaVantageClient>` — `None` when the key is not configured.

- [ ] **Step 2: Add `alpha_vantage` parameter to `build_graph`**

In `runtime.rs`, update the `build_graph` function signature to accept `alpha_vantage: Option<&AlphaVantageClient>`. This is needed so the enrichment code can access the client.

Actually, looking at the architecture more carefully: the enrichment happens in `run_analysis_cycle`, not in graph tasks. The `AlphaVantageClient` should be stored on `TradingPipeline` and accessed in `run_analysis_cycle` directly (same pattern as `finnhub`, `fred`, `yfinance`).

- [ ] **Step 3: Wire the enrichment call in `run_analysis_cycle`**

In `runtime.rs`, in the enrichment hydration section (around line 422), add:

```rust
let (transcript_evidence, transcript_status) = if enrichment_intent.transcripts {
    if let Some(ref av_client) = pipeline.alpha_vantage {
        let catalyst_events = catalysts_result
            .payload
            .as_ref()
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        hydrate_transcript(
            av_client,
            &symbol,
            &date,
            catalyst_events,
            fetch_timeout,
        )
        .await
    } else {
        warn!("transcripts enabled but AlphaVantageClient not constructed");
        (None, "Unavailable")
    }
} else {
    (None, "NotPublished")
};
```

- [ ] **Step 4: Write context keys**

After the existing enrichment context-key writes (around line 506), add:

```rust
// ── Write transcript enrichment to context cache keys ─────────────
let transcript_json = match &transcript_evidence {
    Some(evidence) => serde_json::to_string(&Some(evidence)).unwrap_or_else(|_| "null".to_owned()),
    None => "null".to_owned(),
};
session.context.set(KEY_CACHED_TRANSCRIPT, transcript_json).await;
session.context.set(KEY_TRANSCRIPT_FETCH_STATUS, transcript_status.to_owned()).await;
```

- [ ] **Step 5: Update `app/mod.rs` to construct `AlphaVantageClient` conditionally**

In `crates/scorpio-core/src/app/mod.rs`, after the `yfinance` construction (around line 118), add:

```rust
let alpha_vantage = if cfg.enrichment.enable_transcripts && cfg.api.alpha_vantage_api_key.is_some() {
    let av_limiter = SharedRateLimiter::alpha_vantage_from_config(&cfg.rate_limits)
        .unwrap_or_else(|| SharedRateLimiter::disabled("alpha_vantage"));
    match AlphaVantageClient::new(&cfg.api, av_limiter) {
        Ok(client) => {
            info!("Alpha Vantage client constructed for transcript enrichment");
            Some(client)
        }
        Err(e) => {
            warn!(error = %e, "failed to construct Alpha Vantage client; transcripts disabled");
            None
        }
    }
} else {
    None
};
```

Pass `alpha_vantage` to `TradingPipeline::try_new`.

- [ ] **Step 6: Update preflight seeding**

In `crates/scorpio-core/src/workflow/tasks/preflight.rs`, extend the `seed_if_absent` block (around line 274) to also seed the status key. Since `seed_if_absent` always writes `"null"`, add a separate call:

```rust
// Seed transcript fetch status default
let existing_status: Option<String> = context.get(KEY_TRANSCRIPT_FETCH_STATUS).await;
if existing_status.is_none() {
    context.set(KEY_TRANSCRIPT_FETCH_STATUS, "NotPublished".to_owned()).await;
}
```

Add the import for `KEY_TRANSCRIPT_FETCH_STATUS` from `common.rs`.

- [ ] **Step 7: Run full test suite**

Run: `cargo nextest run --workspace --all-features --no-fail-fast`
Fix any compile errors from the `TradingPipeline` signature changes.

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/workflow/pipeline/mod.rs crates/scorpio-core/src/workflow/pipeline/runtime.rs crates/scorpio-core/src/app/mod.rs crates/scorpio-core/src/workflow/tasks/preflight.rs
git commit -m "feat(enrichment): wire transcript enrichment into run_analysis_cycle with context keys"
```

---

## Chunk 5: Prompt Rendering & Final Wiring

### Task 10: Update prompt rendering for transcript segments and fetch status

**Files:**
- Modify: `crates/scorpio-core/src/agents/shared/prompt.rs:258-406`

- [ ] **Step 1: Add transcript rendering to `build_enrichment_context`**

In `crates/scorpio-core/src/agents/shared/prompt.rs`, add a transcript section in `build_enrichment_context` (after the catalyst calendar section, around line 404). The function currently reads from `TradingState` fields; transcripts use context keys instead, so this requires a different approach.

Looking at the architecture: `build_enrichment_context` takes `&TradingState`, but transcript data lives in context keys, not on `TradingState`. Two options:

**Option A (preferred):** Add a new function `build_transcript_context` that takes the transcript evidence and status as parameters, and call it from the agent prompt builders that have access to context.

**Option B:** Add `transcript_evidence: Option<TranscriptEvidence>` and `transcript_status: &str` parameters to `build_enrichment_context`.

Go with **Option A** for clean separation:

```rust
/// Render transcript evidence context for prompts.
///
/// Returns distinct language per fetch outcome:
/// - `Found`: structured segment block with per-speaker attribution
/// - `NotPublished`: degraded-mode notice
/// - `Throttled`: degraded-mode with retry hint
/// - `Unavailable`: degraded-mode with transient failure notice
pub(crate) fn build_transcript_context(
    evidence: Option<&TranscriptEvidence>,
    status: &str,
) -> String {
    match status {
        "Found" => {
            if let Some(transcript) = evidence {
                let mut lines = vec![format!(
                    "Earnings call transcript ({}):\n",
                    transcript.call_date
                )];
                for segment in &transcript.segments {
                    let sentiment_str = segment
                        .sentiment
                        .map(|s| format!(" [sentiment: {:.2}]", s))
                        .unwrap_or_default();
                    lines.push(format!(
                        "  {} ({}):{} {}",
                        segment.speaker, segment.title, sentiment_str, segment.content
                    ));
                }
                lines.join("\n")
            } else {
                "(transcript data marked as found but evidence is missing)".to_owned()
            }
        }
        "Throttled" => {
            "Earnings call transcript: unavailable (rate-limited). \
             This analysis may improve on retry."
                .to_owned()
        }
        "Unavailable" => {
            "Earnings call transcript: unavailable (transient fetch failure)."
                .to_owned()
        }
        _ => {
            // "NotPublished" or unknown
            "Earnings call transcript: not yet published for this quarter."
                .to_owned()
        }
    }
}
```

- [ ] **Step 2: Write tests for the new function**

Add tests in the existing test module:

```rust
#[test]
fn transcript_context_renders_found() {
    use crate::data::adapters::transcripts::{TranscriptEvidence, TranscriptSegment};
    let evidence = TranscriptEvidence {
        symbol: "AAPL".to_owned(),
        call_date: "2025Q1".to_owned(),
        segments: vec![TranscriptSegment {
            speaker: "Tim Cook".to_owned(),
            title: "CEO".to_owned(),
            content: "Great quarter.".to_owned(),
            sentiment: Some(0.8),
        }],
    };
    let ctx = build_transcript_context(Some(&evidence), "Found");
    assert!(ctx.contains("Tim Cook"));
    assert!(ctx.contains("[sentiment: 0.80]"));
    assert!(ctx.contains("2025Q1"));
}

#[test]
fn transcript_context_renders_not_published() {
    let ctx = build_transcript_context(None, "NotPublished");
    assert!(ctx.contains("not yet published"));
}

#[test]
fn transcript_context_renders_throttled() {
    let ctx = build_transcript_context(None, "Throttled");
    assert!(ctx.contains("rate-limited"));
    assert!(ctx.contains("retry"));
}

#[test]
fn transcript_context_renders_unavailable() {
    let ctx = build_transcript_context(None, "Unavailable");
    assert!(ctx.contains("transient fetch failure"));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(transcript_context)'`
Expected: PASS

- [ ] **Step 4: Remove `TODO(transcripts)` markers from prompt templates**

Update the following files to remove the `<!-- TODO(transcripts) ... -->` HTML comments and replace with actual transcript-aware instructions:

- `crates/scorpio-core/src/analysis_packs/equity/prompts/theme_c_management_red_flags.md`
- `crates/scorpio-core/src/analysis_packs/equity/prompts/conservative_risk.md`

Replace the `TODO(transcripts)` comment blocks with instructions that reference the transcript data when available:

```markdown
When a transcript is available (status: Found), compare the tone and
language between the press release / headline and the earnings call
segments. Flag any divergence — e.g., optimistic press release language
paired with cautious or evasive call commentary.

When transcripts are unavailable (status: NotPublished / Throttled /
Unavailable), explicitly include the phrase
`degraded mode: transcript unavailable` in the affected summary.
```

- [ ] **Step 5: Update prompt bundle test fixtures**

Update the fixture files to reflect the new prompt text:
- `crates/scorpio-core/tests/fixtures/prompt_bundle/news_analyst.txt`
- `crates/scorpio-core/tests/fixtures/prompt_bundle/sentiment_analyst.txt`
- `crates/scorpio-core/tests/fixtures/prompt_bundle/conservative_risk.txt`

Replace `TODO(transcripts)` markers with the new transcript-aware language.

- [ ] **Step 6: Run prompt bundle regression tests**

Run: `cargo nextest run --workspace --all-features --no-fail-fast -E 'test(prompt_bundle)'`
Expected: PASS (fixtures match)

- [ ] **Step 7: Run full test suite**

Run: `cargo nextest run --workspace --all-features --no-fail-fast`

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/agents/shared/prompt.rs crates/scorpio-core/src/analysis_packs/equity/prompts/theme_c_management_red_flags.md crates/scorpio-core/src/analysis_packs/equity/prompts/conservative_risk.md crates/scorpio-core/tests/fixtures/prompt_bundle/
git commit -m "feat(prompts): add transcript segment rendering and fetch-status-aware prompt language"
```

---

### Task 11: Smoke test and final verification

- [ ] **Step 1: Run the full CI pipeline**

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

All three must pass.

- [ ] **Step 2: Run the smoke test with a real API key**

```bash
SCORPIO_ALPHA_VANTAGE_API_KEY=your_key \
cargo run -p scorpio-cli -- analyze COIN --json
```

Verify:
- Transcript evidence appears in JSON output
- `segments` populated with per-speaker entries
- Each segment has `speaker`, `title`, `content`, optional `sentiment`
- `call_date` is `"YYYY-QN"` format
- No flat `content` field at `TranscriptEvidence` root
- No aggregate `sentiment_score` at root

- [ ] **Step 3: Verify degraded mode works without API key**

```bash
cargo run -p scorpio-cli -- analyze AAPL --json
```

Verify:
- Pipeline completes without error (transcripts are fail-open)
- Transcript status is `"NotPublished"` in the output
- Prompt includes degraded-mode language

- [ ] **Step 4: Final commit (if any fixups needed)**

```bash
git add -A
git commit -m "fix: final adjustments from smoke testing"
```

---

## Out of Scope (Deferred)

These items are explicitly deferred per the design spec:

- **Persistent daily-quota tracking** — `TODO(transcripts-quota)`
- **Persistent per-key cooldown** — `TODO(transcripts-cooldown)`
- **Transcript content sanitization** — `TODO(transcripts-sanitize)`
- **Our own sentiment NLP** — `TODO(transcripts-nlp)`
- **Q&A separation and speaker indexing**
- **Quarter backward walk** (single quarter per call)
- **Transcript caching** (each run fetches fresh)
