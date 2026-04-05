## 1. Enrichment Config

- [ ] 1.1 Add `DataEnrichmentConfig` struct to `src/config.rs` immediately before or after `StorageConfig`:

  ```rust
  /// Data enrichment feature flags.
  ///
  /// All flags default to `false` so existing runs with no `[enrichment]` section
  /// in `config.toml` continue to behave identically.
  #[derive(Debug, Clone, Deserialize)]
  pub struct DataEnrichmentConfig {
      #[serde(default)]
      pub enable_transcripts: bool,
      #[serde(default)]
      pub enable_consensus_estimates: bool,
      #[serde(default)]
      pub enable_event_news: bool,
      #[serde(default = "default_max_evidence_age_hours")]
      pub max_evidence_age_hours: u64,
  }

  fn default_max_evidence_age_hours() -> u64 {
      48
  }

  impl Default for DataEnrichmentConfig {
      fn default() -> Self {
          Self {
              enable_transcripts: false,
              enable_consensus_estimates: false,
              enable_event_news: false,
              max_evidence_age_hours: default_max_evidence_age_hours(),
          }
      }
  }
  ```

- [ ] 1.2 Add `#[serde(default)] pub enrichment: DataEnrichmentConfig` field to the `Config` struct in `src/config.rs`.
- [ ] 1.3 Extend `Config::validate()` in `src/config.rs` to call
  `crate::data::symbol::validate_symbol(&self.trading.asset_symbol)` and map the error:

  ```rust
  crate::data::symbol::validate_symbol(&self.trading.asset_symbol)
      .map_err(|e| anyhow::anyhow!("config validation: {e}"))?;
  ```

  Place this call before the existing `has_any_llm_key()` warning so that format-invalid symbols abort before any
  provider check.
- [ ] 1.4 Add `[enrichment]` section to `config.toml`:

  ```toml
  [enrichment]
  enable_transcripts = false
  enable_consensus_estimates = false
  enable_event_news = false
  max_evidence_age_hours = 48
  ```

- [ ] 1.5 Add unit tests in `src/config.rs` under `#[cfg(test)]`:
  - `enrichment_config_defaults_all_false`: load `config.toml` and assert all three `enable_*` flags are `false`
    and `max_evidence_age_hours == 48`.
  - `enrichment_config_enable_transcripts_via_env`: set
    `SCORPIO__ENRICHMENT__ENABLE_TRANSCRIPTS=true`, load config, assert `enable_transcripts == true`, unset env var.
  - `enrichment_config_max_evidence_age_hours_via_env`: set
    `SCORPIO__ENRICHMENT__MAX_EVIDENCE_AGE_HOURS=72`, load config, assert `max_evidence_age_hours == 72`, unset.
  - `validate_rejects_format_invalid_symbol`: construct a `Config` with `trading.asset_symbol = "BAD SYMBOL"`,
    call `validate()`, assert `Err`.
  - `validate_accepts_lowercase_symbol`: construct a `Config` with `trading.asset_symbol = "nvda"`,
    call `validate()`, assert `Ok` (lowercase passes format check).
- [ ] 1.6 Verify the section is wired up with:
    `rg -n "DataEnrichmentConfig|enrichment" src/config.rs config.toml`
- [ ] 1.7 Commit: `git add src/config.rs config.toml && git commit -m "feat: add DataEnrichmentConfig and strengthen symbol validation in Config::validate"`

## 2. Entity Resolution

- [ ] 2.1 Create `src/data/entity.rs` with `ResolvedInstrument` and `resolve_symbol`:

  ```rust
  use serde::{Deserialize, Serialize};
  use crate::error::TradingError;
  use crate::data::symbol::validate_symbol;

  /// Canonical instrument identity record produced by `resolve_symbol`.
  ///
  /// In Stage 1, only `input_symbol` and `canonical_symbol` are populated.
  /// All metadata fields (`issuer_name`, `exchange`, `instrument_type`, `aliases`)
  /// are `None` / empty until a live-metadata enrichment provider is wired in.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct ResolvedInstrument {
      pub input_symbol: String,
      pub canonical_symbol: String,
      pub issuer_name: Option<String>,
      pub exchange: Option<String>,
      pub instrument_type: Option<String>,
      pub aliases: Vec<String>,
  }

  /// Validate and canonicalize a stock or index symbol.
  ///
  /// Delegates format validation to [`validate_symbol`] in `src/data/symbol.rs`,
  /// then uppercases the result to produce the canonical form.
  ///
  /// # Errors
  ///
  /// Returns [`TradingError::SchemaViolation`] when the symbol fails format validation.
  pub fn resolve_symbol(symbol: &str) -> Result<ResolvedInstrument, TradingError> {
      let validated = validate_symbol(symbol)?;
      let canonical = validated.to_ascii_uppercase();
      Ok(ResolvedInstrument {
          input_symbol: symbol.trim().to_owned(),
          canonical_symbol: canonical,
          issuer_name: None,
          exchange: None,
          instrument_type: None,
          aliases: vec![],
      })
  }
  ```

- [ ] 2.2 Add unit tests in `src/data/entity.rs` under `#[cfg(test)]`:
  - `resolve_symbol_uppercase_normalizes`: assert `resolve_symbol("nvda").unwrap().canonical_symbol == "NVDA"`.
  - `resolve_symbol_preserves_input`: assert `resolve_symbol("nvda").unwrap().input_symbol == "nvda"`.
  - `resolve_symbol_already_uppercase`: assert `resolve_symbol("AAPL").unwrap().canonical_symbol == "AAPL"`.
  - `resolve_symbol_accepts_dot_suffix`: assert `resolve_symbol("BRK.B").unwrap().canonical_symbol == "BRK.B"`.
  - `resolve_symbol_accepts_index`: assert `resolve_symbol("^GSPC").unwrap().canonical_symbol == "^GSPC"`.
  - `resolve_symbol_rejects_empty`: assert `resolve_symbol("").is_err()`.
  - `resolve_symbol_rejects_space`: assert `resolve_symbol("BAD SYMBOL").is_err()`.
  - `resolve_symbol_rejects_semicolon`: assert `resolve_symbol("DROP;TABLE").is_err()`.
  - `resolve_symbol_metadata_none_in_stage1`: call `resolve_symbol("AAPL")`, assert `issuer_name.is_none()`,
    `exchange.is_none()`, `instrument_type.is_none()`, `aliases.is_empty()`.
- [ ] 2.3 Export `entity` from `src/data/mod.rs` by adding `pub mod entity;`.
- [ ] 2.4 Commit: `git add src/data/entity.rs src/data/mod.rs && git commit -m "feat: add entity resolution module with ResolvedInstrument and resolve_symbol"`

## 3. Provider Capabilities

- [ ] 3.1 Create `src/data/adapters/` directory and `src/data/adapters/mod.rs` with `ProviderCapabilities`:

  ```rust
  use crate::config::DataEnrichmentConfig;
  use serde::{Deserialize, Serialize};

  /// Config-derived enrichment capability flags for the current run.
  ///
  /// In Stage 1, capabilities are config-derived only. Future milestones may
  /// upgrade specific fields to represent confirmed runtime availability.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct ProviderCapabilities {
      pub transcripts_enabled: bool,
      pub consensus_estimates_enabled: bool,
      pub event_news_enabled: bool,
  }

  impl ProviderCapabilities {
      /// Derive capability flags from the loaded enrichment configuration.
      ///
      /// This function is infallible: reading from a fully-loaded `DataEnrichmentConfig`
      /// cannot produce an error.
      pub fn from_config(cfg: &DataEnrichmentConfig) -> Self {
          Self {
              transcripts_enabled: cfg.enable_transcripts,
              consensus_estimates_enabled: cfg.enable_consensus_estimates,
              event_news_enabled: cfg.enable_event_news,
          }
      }
  }
  ```

- [ ] 3.2 Add unit tests in `src/data/adapters/mod.rs` under `#[cfg(test)]`:
  - `from_config_all_disabled`: use `DataEnrichmentConfig::default()`, assert all three fields are `false`.
  - `from_config_transcripts_enabled`: set `enable_transcripts: true`, assert `transcripts_enabled == true`,
    others `false`.
  - `from_config_all_enabled`: set all three `enable_*` to `true`, assert all three capability fields are `true`.
- [ ] 3.3 Export `adapters` from `src/data/mod.rs` by adding `pub mod adapters;`.
- [ ] 3.4 Commit: `git add src/data/adapters/mod.rs src/data/mod.rs && git commit -m "feat: add ProviderCapabilities stub derived from DataEnrichmentConfig"`

## 4. Workflow Context Keys

- [ ] 4.1 Create `src/workflow/tasks/common.rs` with the six Stage 1 context key constants:

  ```rust
  //! Shared context key constants for all Stage 1 workflow tasks.
  //!
  //! Every key listed here is written by [`super::preflight::PreflightTask`] before
  //! any analyst task runs. Consumers must treat a missing key as orchestration
  //! corruption, not normal data absence.

  /// Serialized [`crate::data::entity::ResolvedInstrument`] for the current run.
  pub const KEY_RESOLVED_INSTRUMENT: &str = "resolved_instrument";

  /// Serialized [`crate::data::adapters::ProviderCapabilities`] for the current run.
  pub const KEY_PROVIDER_CAPABILITIES: &str = "provider_capabilities";

  /// Serialized `Vec<String>` of required coverage input IDs for the current run.
  ///
  /// Stage 1 value: `["fundamentals", "sentiment", "news", "technical"]`.
  pub const KEY_REQUIRED_COVERAGE_INPUTS: &str = "required_coverage_inputs";

  /// Serialized `Option<TranscriptEvidence>` — JSON `null` until a transcript
  /// enrichment provider populates it.
  pub const KEY_CACHED_TRANSCRIPT: &str = "cached_transcript";

  /// Serialized `Option<ConsensusEvidence>` — JSON `null` until a consensus
  /// enrichment provider populates it.
  pub const KEY_CACHED_CONSENSUS: &str = "cached_consensus";

  /// Serialized `Option<Vec<EventNewsEvidence>>` — JSON `null` until an event-news
  /// enrichment provider populates it.
  pub const KEY_CACHED_EVENT_FEED: &str = "cached_event_feed";
  ```

- [ ] 4.2 Export `common` from `src/workflow/tasks/mod.rs` by adding `pub mod common;`.
- [ ] 4.3 Commit: `git add src/workflow/tasks/common.rs src/workflow/tasks/mod.rs && git commit -m "feat: add Stage 1 workflow context key constants"`

## 5. PreflightTask

- [ ] 5.1 Create `src/workflow/tasks/preflight.rs` implementing `PreflightTask`. The task must:
  - Accept `cfg: Arc<Config>` in its constructor (or hold the fields it needs: `enrichment: DataEnrichmentConfig`,
    `asset_symbol: String`).
  - In `async fn execute(&self, ctx: &mut Context) -> Result<(), anyhow::Error>`:
    1. Call `resolve_symbol(&self.asset_symbol)` and propagate any error immediately (hard fail).
    2. Serialize `resolved` to JSON and write to context under `KEY_RESOLVED_INSTRUMENT`.
    3. Construct `ProviderCapabilities::from_config(&self.enrichment)`, serialize, write under `KEY_PROVIDER_CAPABILITIES`.
    4. Serialize `vec!["fundamentals", "sentiment", "news", "technical"]`, write under `KEY_REQUIRED_COVERAGE_INPUTS`.
    5. Write the JSON literal `"null"` (the string, not an absent key) under `KEY_CACHED_TRANSCRIPT`, `KEY_CACHED_CONSENSUS`, `KEY_CACHED_EVENT_FEED`.
  - Derive/implement `Debug`. Add a doc comment.
- [ ] 5.2 Export `preflight` from `src/workflow/tasks/mod.rs` by adding `pub mod preflight;`.
- [ ] 5.3 Add unit tests in `src/workflow/tasks/preflight.rs` under `#[cfg(test)]`:
  - `preflight_task_writes_resolved_instrument`: construct a `PreflightTask` with `asset_symbol = "nvda"`, run it
    against a fresh `Context`, deserialize `KEY_RESOLVED_INSTRUMENT`, assert `canonical_symbol == "NVDA"`.
  - `preflight_task_writes_provider_capabilities`: same setup, deserialize `KEY_PROVIDER_CAPABILITIES`,
    assert all three flags are `false` (defaults).
  - `preflight_task_writes_required_coverage_inputs`: deserialize `KEY_REQUIRED_COVERAGE_INPUTS`, assert the
    resulting `Vec<String>` equals `["fundamentals", "sentiment", "news", "technical"]`.
  - `preflight_task_writes_null_cache_placeholders`: assert that `KEY_CACHED_TRANSCRIPT`, `KEY_CACHED_CONSENSUS`,
    and `KEY_CACHED_EVENT_FEED` are each present in context with value `"null"`.
  - `preflight_task_fails_on_invalid_symbol`: construct a `PreflightTask` with `asset_symbol = "DROP;TABLE"`,
    run it, assert `Err`.
- [ ] 5.4 Commit: `git add src/workflow/tasks/preflight.rs src/workflow/tasks/mod.rs && git commit -m "feat: add PreflightTask as Stage 1 graph preflight node"`

## 6. Pipeline Wiring

- [ ] 6.1 In `src/workflow/pipeline.rs`, import `PreflightTask` from `crate::workflow::tasks::preflight`.
- [ ] 6.2 Construct a `PreflightTask` instance from the config fields available in `TradingPipeline::new()` or
  `run_analysis_cycle()`.
- [ ] 6.3 Add `PreflightTask` as the first node in the graph builder. Add an edge from `PreflightTask` to the
  existing `analyst_fanout` node. Keep all subsequent edges unchanged.
- [ ] 6.4 Add an integration test (or extend an existing one in `tests/`) that runs a full pipeline with a valid
  symbol and asserts that `KEY_RESOLVED_INSTRUMENT` is present in the final context with the correct
  `canonical_symbol`. Use deterministic stubs for LLM and API calls so no network call is made.
- [ ] 6.5 Commit: `git add src/workflow/pipeline.rs && git commit -m "feat: wire PreflightTask as first graph node before analyst fan-out"`

## 7. Verification

- [ ] 7.1 Run `cargo fmt -- --check` and fix any formatting issues.
- [ ] 7.2 Run `cargo clippy --all-targets -- -D warnings` and resolve all warnings.
- [ ] 7.3 Run `cargo test` and confirm all tests pass, including the new unit tests from tasks 1.5, 2.2, 3.2,
  and the integration tests from 5.3 and 6.4.
- [ ] 7.4 Run `cargo build` and confirm clean compilation.
- [ ] 7.5 After all tasks complete, run `/opsx:verify`.
