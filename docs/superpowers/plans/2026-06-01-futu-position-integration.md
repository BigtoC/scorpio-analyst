# Futu Position Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional, read-only, Real-account Futu OpenD client that fetches the user's current account positions and feeds them to the Fund Manager, degrading silently to today's behavior when disabled or unavailable.

**Architecture:** Positions are fetched lazily inside `FundManagerTask::run` (which already holds `Arc<Config>`), written to a new `TradingState.account_positions` field, rendered into the pack-owned `fund_manager.md` prompt via a `{account_positions}` placeholder, and surfaced in the terminal report. The OpenD wire client lives in `crates/scorpio-core/src/data/futu/` and talks raw TCP with JSON message bodies (44-byte frame header + SHA-1 body checksum). Framing and snapshot assembly are pure functions tested directly; socket sequencing is tested behind a one-method `FutuConn` transport trait (mirrors the existing `EdgarHttp` seam). Every failure path resolves to `AccountPositionsState::Unavailable(reason)`; the Fund Manager always runs.

**Tech Stack:** Rust (edition 2024, Rust 1.93+), `tokio` (`TcpStream`, `tokio::time::timeout`), `serde`/`serde_json`, `sha1`, `async-trait`, `mockall` (dev), `cargo nextest`.

---

## Decisions resolved (from the spec's Open Questions)

These are settled for this plan; do not re-litigate during implementation:

1. **Empty vs. Unavailable.** *No* Real account matching the symbol's market → `Unavailable("no real account for <market>")`. A matching Real account that holds **zero** positions → `Available(snapshot)` with an empty `positions` vec. This is implemented in `select_account` (errors when no account matches) vs. `assemble_snapshot` (returns an empty-positions snapshot).
2. **Schema version.** `THESIS_MEMORY_SCHEMA_VERSION` stays **4** and `JsonReport.schema_version` stays **2**. `account_positions` is additive and `#[serde(default)]`, so legacy snapshots deserialize to `Disabled`. This mirrors the existing precedent `json_reporter_keeps_v2_for_additive_etf_profile_fields` (`crates/scorpio-reporters/tests/json.rs:168`). Task 4 adds a legacy-load test that proves no bump is needed.
3. **`plRatio` units.** `assemble_snapshot` stores `pl_ratio` as a **fraction** (e.g. `0.236` = 23.6%); the prompt/report multiply by 100 when rendering. If the connectivity spike (Task 0 or Task 12) shows OpenD already returns a percentage, divide by 100 in `assemble_snapshot` — the render code and its tests do not change.
4. **Full account snapshot.** `GetPositionList` fetches all positions for the selected Real account/market, not just the analyzed symbol. `held_position` computes the analyzed-symbol match locally. This is required for portfolio total, top holdings, and concentration to be truthful.
5. **Data-use contract.** Enabling Futu positions means holdings data may be written to the local snapshot DB and included in the Fund Manager prompt sent to the configured LLM provider. Default-off is the consent boundary for v1; operator docs must state this explicitly. `Disabled` and `Unavailable` do not add account-position text to the Fund Manager prompt, preserving baseline behavior when no usable snapshot exists.

## Connectivity spike — do this first if OpenD is reachable

The spec's first step is a connectivity spike against the user's running OpenD. **If OpenD is available, do Task 0 first** to confirm: JSON mode (`nProtoFmtType = 1`) is accepted for 1001/2001/2102; the exact `packetEncAlgo` no-encryption value (`-1` assumed); whether `GetAccList` needs the real `loginUserID` or accepts `0`; the exact field casing / `uint64` representation; and whether omitting `filterConditions.codeList` returns the full account/market position list. Update the constants in Task 6 if the spike contradicts them, then sanitize captured payloads into the synthetic fixtures the message tests use. If OpenD is **not** available, implement the pure layers offline using the synthetic fixtures in this plan and run the live smoke test later. If OpenD rejects JSON mode entirely, **stop and escalate** — do not silently add protobuf/codegen (it is an explicit non-goal).

## File structure

**New files (all in `crates/scorpio-core/`):**
- `src/state/account.rs` — `PositionSide`, `AccountPosition`, `AccountSnapshot` (+ `held_position`, `top_holdings`), `AccountPositionsState`, `normalize_code`.
- `src/data/futu/mod.rs` — module wiring, protocol constants, `FutuConn` trait, `fetch_account_snapshot` orchestration, response parse fns, `MockFutuConn` sequencing tests.
- `src/data/futu/frame.rs` — 44-byte frame encode/decode + SHA-1 + serial (pure).
- `src/data/futu/messages.rs` — serde request/response bodies + flexible `u64` + serialize/parse helpers (pure).
- `src/data/futu/select.rs` — `market_for_symbol`, `select_account`, `assemble_snapshot`, enum-label helpers, `redact_account_id` (pure).
- `src/data/futu/client.rs` — `FutuClient` (public entry) + `LiveFutuConn` over `TcpStream`.
- `examples/futu_opend_smoke.rs` — manual preflight script for live OpenD protocol assumptions.

**Modified files:**
- `Cargo.toml` (workspace) — pin `sha1`.
- `crates/scorpio-core/Cargo.toml` — add `sha1` dependency.
- `src/config.rs` — `FutuConfig` + `Config.futu` + tests.
- `src/state/mod.rs` — declare + re-export `account`.
- `src/state/trading_state.rs` — `account_positions` on `TradingState`, `TradingStateWire`, `From`, `new`.
- `src/data/mod.rs` — declare `futu`, re-export `FutuClient`.
- `src/agents/fund_manager/prompt.rs` — `render_account_positions` + `{account_positions}` substitution + tests.
- `src/analysis_packs/equity/prompts/fund_manager.md` — input line + instruction.
- `src/analysis_packs/etf/prompts/fund_manager.md` — account-context section + instruction.
- `src/analysis_packs/equity/baseline.rs` + `src/analysis_packs/etf/baseline.rs` — drift tests.
- `src/workflow/tasks/trading.rs` — lazy fetch in `FundManagerTask::run`.
- `src/agents/fund_manager/tests.rs` — capturing-inference acceptance tests + `sample_config` field.
- `src/workflow/snapshot/tests/core_roundtrip.rs` — round-trip + legacy + privacy tests.
- `crates/scorpio-reporters/src/terminal/final_report.rs` — "Account Context" section.
- `crates/scorpio-reporters/tests/json.rs` — additive-v2 test.
- Test `Config { .. }` literals across `crates/scorpio-core/tests/` (compiler-flagged).

---

## Task 0: Live OpenD protocol spike (if OpenD is reachable)

**Files:**
- Create: `crates/scorpio-core/examples/futu_opend_smoke.rs`

If a local OpenD is reachable, do this before Task 1. If OpenD is not available, skip this task and use the synthetic fixtures below; return to Task 12 for the ignored live test after the implementation exists.

- [ ] **Step 1: Add a minimal examples smoke script**

Create `crates/scorpio-core/examples/futu_opend_smoke.rs` as a deliberately small diagnostic that opens `127.0.0.1:11111`, sends `InitConnect`, `GetAccList`, and `GetPositionList` using JSON mode, prints sanitized response shapes, and exits. It may duplicate a minimal copy of the frame encode/decode logic rather than depending on the later `data::futu` modules, because this spike exists to validate those later assumptions before implementation.

The script must not print raw account ids, raw holdings quantities, or raw broker error text. Redact account ids to `acct-<hash>`, print only field names/types for position rows, and print enough metadata to answer these questions:
- Does JSON mode (`nProtoFmtType = 1`) work for 1001/2001/2102?
- Is `packetEncAlgo = -1` accepted?
- Does `GetAccList` need the real `loginUserID`, or does `0` work?
- Are `accID`, `loginUserID`, `plRatio`, and casing represented as expected?
- Does omitting `filterConditions.codeList` return all account/market positions?

- [ ] **Step 2: Run the script manually when OpenD is available**

Run: `cargo run -p scorpio-core --example futu_opend_smoke`

Expected: it prints sanitized payload shapes and either confirms the constants in Tasks 5-8 or identifies exactly which constants/serde fields must change before implementation proceeds.

- [ ] **Step 3: Reconcile fixtures and constants**

If the spike contradicts the plan, update Task 6 message constants/serde shapes and the synthetic fixtures before implementing the pure layers. If OpenD rejects JSON mode entirely, stop and escalate instead of adding protobuf/codegen.

- [ ] **Step 4: Compile the example**

Run: `cargo build -p scorpio-core --example futu_opend_smoke`

Expected: builds clean. The example does not run in CI unless invoked manually.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/examples/futu_opend_smoke.rs
git commit -m "test(futu): add live OpenD protocol spike example"
```

---

## Task 1: Add the `sha1` dependency

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`, near the `sha2 = "0.11"` line ~103)
- Modify: `crates/scorpio-core/Cargo.toml` (`[dependencies]`)

- [ ] **Step 1: Pin `sha1` at the workspace level**

In the root `Cargo.toml`, under `[workspace.dependencies]`, add `sha1` next to the existing `sha2` pin so both use the same `digest 0.11` trait family:

```toml
sha1 = "0.11"
```

- [ ] **Step 2: Reference it from `scorpio-core`**

In `crates/scorpio-core/Cargo.toml`, in the `[dependencies]` section (not dev-dependencies — frame encoding needs it in production), add:

```toml
sha1.workspace = true
```

- [ ] **Step 3: Verify it resolves**

Run: `cargo build -p scorpio-core`
Expected: builds clean (sha1 0.11 is already present transitively in `Cargo.lock`, so no new network fetch is required).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/scorpio-core/Cargo.toml
git commit -m "build(futu): add sha1 dependency for OpenD frame checksums"
```

---

## Task 2: `FutuConfig` and `Config.futu`

**Files:**
- Modify: `crates/scorpio-core/src/config.rs` (struct `Config` ~line 9; add `FutuConfig` near `DataEnrichmentConfig` ~line 71; tests in the `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing default test**

In the `#[cfg(test)] mod tests` block of `config.rs` (where `enrichment_config_defaults_are_all_disabled` lives), add:

```rust
#[test]
fn futu_config_defaults_are_disabled_with_five_second_timeout() {
    let cfg = FutuConfig::default();
    assert!(!cfg.enabled);
    assert_eq!(cfg.account_id, None);
    assert_eq!(cfg.timeout_secs, 5);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p scorpio-core futu_config_defaults`
Expected: FAIL — `cannot find type FutuConfig in this scope`.

- [ ] **Step 3: Add `FutuConfig`**

Immediately after the `DataEnrichmentConfig` block (its `impl Default` ends ~line 71), add:

```rust
/// Read-only Futu OpenD position-lookup configuration.
///
/// Default-off, following the [`DataEnrichmentConfig`] precedent: with
/// `enabled = false` (the default) there is no socket activity and the Fund
/// Manager behaves exactly as before. The OpenD endpoint is hardcoded to
/// `127.0.0.1:11111` and the trading environment is hardcoded to Real in
/// code — there is intentionally no `host`, `port`, or `trd_env` field.
#[derive(Debug, Clone, Deserialize)]
pub struct FutuConfig {
    /// Enable the read-only OpenD position lookup.
    /// Env: `SCORPIO__FUTU__ENABLED` (default `false`).
    #[serde(default)]
    pub enabled: bool,
    /// Explicit Real-account id override (must be a Real account). When unset,
    /// the account is chosen by market-match.
    /// Env: `SCORPIO__FUTU__ACCOUNT_ID`.
    #[serde(default)]
    pub account_id: Option<u64>,
    /// One-shot connect→init→query→close timeout (seconds).
    /// Env: `SCORPIO__FUTU__TIMEOUT_SECS` (default `5`).
    #[serde(default = "default_futu_timeout")]
    pub timeout_secs: u64,
}

fn default_futu_timeout() -> u64 {
    5
}

impl Default for FutuConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account_id: None,
            timeout_secs: default_futu_timeout(),
        }
    }
}
```

- [ ] **Step 4: Wire `futu` into `Config`**

In the `Config` struct, add the field immediately after the `enrichment` field (~line 22):

```rust
    #[serde(default)]
    pub futu: FutuConfig,
```

- [ ] **Step 5: Run the default test to verify it passes**

Run: `cargo nextest run -p scorpio-core futu_config_defaults`
Expected: PASS.

- [ ] **Step 6: Write and run the env-override test**

Add (mirrors `enrichment_env_override_sets_max_evidence_age_hours`):

```rust
#[test]
fn futu_env_override_enables_and_sets_timeout() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
    // SAFETY: serialized by ENV_LOCK; no other thread mutates env vars concurrently.
    unsafe {
        std::env::set_var("SCORPIO__FUTU__ENABLED", "true");
        std::env::set_var("SCORPIO__FUTU__TIMEOUT_SECS", "9");
    }
    let result = Config::load_from(&path);
    unsafe {
        std::env::remove_var("SCORPIO__FUTU__ENABLED");
        std::env::remove_var("SCORPIO__FUTU__TIMEOUT_SECS");
    }
    let cfg = result.expect("config should load with futu overrides");
    assert!(cfg.futu.enabled);
    assert_eq!(cfg.futu.timeout_secs, 9);
}
```

Run: `cargo nextest run -p scorpio-core futu_env_override`
Expected: PASS.

- [ ] **Step 7: Fix every explicit `Config { .. }` literal the compiler flags**

Adding a non-`Default` field breaks struct literals. Run `cargo build -p scorpio-core --all-targets` and add `futu: FutuConfig::default(),` (or `futu: Default::default(),`) to each. Known sites:
- `crates/scorpio-core/src/agents/fund_manager/tests.rs:58` (`sample_config`)
- `crates/scorpio-core/src/config.rs:983`
- `crates/scorpio-core/tests/activation_path_audit.rs:33`
- `crates/scorpio-core/tests/app_runtime.rs:164` and `:213`
- `crates/scorpio-core/tests/support/workflow_observability_pipeline_support.rs:20`
- `crates/scorpio-core/tests/support/workflow_pipeline_make_pipeline.rs:34` and `:97`

For `crates/scorpio-core/src/agents/fund_manager/tests.rs:58`, the literal becomes:

```rust
fn sample_config() -> Config {
    Config {
        llm: sample_llm_config(),
        trading: TradingConfig::default(),
        api: Default::default(),
        storage: Default::default(),
        providers: sample_providers_config(),
        rate_limits: Default::default(),
        enrichment: Default::default(),
        futu: Default::default(),
        analysis_pack: "baseline".to_owned(),
    }
}
```

- [ ] **Step 8: Verify the whole crate builds and tests compile**

Run: `cargo nextest run -p scorpio-core --all-features futu_`
Expected: PASS (both futu config tests).

- [ ] **Step 9: Commit**

```bash
git add crates/scorpio-core/src/config.rs crates/scorpio-core/src/agents/fund_manager/tests.rs crates/scorpio-core/tests
git commit -m "feat(futu): add default-off FutuConfig wired into Config"
```

---

## Task 3: State types in `state/account.rs`

**Files:**
- Create: `crates/scorpio-core/src/state/account.rs`
- Modify: `crates/scorpio-core/src/state/mod.rs` (add `mod account;` + `pub use account::*;`)

- [ ] **Step 1: Create the module with full types and a test stub**

Create `crates/scorpio-core/src/state/account.rs`:

```rust
//! Read-only account-position snapshot fed to the Fund Manager from local
//! Futu OpenD. Populated lazily by `FundManagerTask` when `futu.enabled` is
//! set; otherwise [`AccountPositionsState::Disabled`]. No raw OpenD account id
//! is ever stored here — only a redacted [`AccountSnapshot::account_label`].

use serde::{Deserialize, Serialize};

use crate::domain::Symbol;

/// Long/short side of a held position (`PositionSide` in `Trd_Common`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSide {
    Long,
    Short,
}

/// A single held position, normalized off OpenD's `PositionList` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountPosition {
    pub code: String,
    pub name: String,
    pub qty: f64,
    pub can_sell_qty: f64,
    pub cost_price: Option<f64>,
    pub current_price: Option<f64>,
    /// `val` — position market value.
    pub market_value: Option<f64>,
    /// Profit/loss ratio as a fraction (0.236 = +23.6%).
    pub pl_ratio: Option<f64>,
    pub pl_val: Option<f64>,
    /// Currency label mapped from OpenD's `Currency` enum (e.g. `"USD"`).
    pub currency: String,
    pub side: PositionSide,
}

/// Single-currency, single-account snapshot. Currency is uniform because the
/// account is market-matched to the analyzed symbol (no FX aggregation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    /// Redacted/hashed account label — never the raw OpenD `accID`.
    pub account_label: Option<String>,
    /// Market label, e.g. `"US"`.
    pub market: String,
    /// Account currency label, e.g. `"USD"`.
    pub currency: String,
    /// `Σ position.market_value` in the account currency.
    pub total_market_value: Option<f64>,
    pub positions: Vec<AccountPosition>,
}

impl AccountSnapshot {
    /// The held position for `symbol`, matched on normalized `code`. Computed on
    /// demand — the held position is not stored as a duplicate field.
    #[must_use]
    pub fn held_position(&self, symbol: &Symbol) -> Option<&AccountPosition> {
        let target = normalize_code(&symbol.to_string());
        self.positions
            .iter()
            .find(|p| normalize_code(&p.code) == target)
    }

    /// Top `n` positions by market value, paired with concentration fraction
    /// (`market_value / total_market_value`). Positions without a market value
    /// are skipped; concentration is `0.0` when the total is absent or zero.
    #[must_use]
    pub fn top_holdings(&self, n: usize) -> Vec<(&AccountPosition, f64)> {
        let total = self.total_market_value.unwrap_or(0.0);
        let mut ranked: Vec<&AccountPosition> = self
            .positions
            .iter()
            .filter(|p| p.market_value.is_some())
            .collect();
        ranked.sort_by(|a, b| {
            b.market_value
                .unwrap_or(0.0)
                .partial_cmp(&a.market_value.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked
            .into_iter()
            .take(n)
            .map(|p| {
                let pct = if total > 0.0 {
                    p.market_value.unwrap_or(0.0) / total
                } else {
                    0.0
                };
                (p, pct)
            })
            .collect()
    }
}

/// Known Futu market-code prefixes. Only these are stripped from a position
/// `code`, so class-suffixed tickers like `BRK.B` / `BF.B` are left intact.
const FUTU_MARKET_CODE_PREFIXES: &[&str] = &["US", "HK", "SH", "SZ", "SG", "JP", "AU", "CN"];

/// Normalize a security code for matching and prompt/report use: strip control
/// characters, trim to a small ticker-like alphabet, strip a leading known
/// market prefix (`US.AAPL` → `AAPL`, `US.BRK.B` → `BRK.B`), and uppercase. A
/// leading segment that is not a known market (`BF.B`) is preserved.
#[must_use]
pub(crate) fn normalize_code(code: &str) -> String {
    let cleaned: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(32)
        .collect();
    let trimmed = cleaned.trim();
    if let Some((prefix, rest)) = trimmed.split_once('.')
        && FUTU_MARKET_CODE_PREFIXES.contains(&prefix.to_ascii_uppercase().as_str())
        && !rest.is_empty()
    {
        return rest.to_ascii_uppercase();
    }
    trimmed.to_ascii_uppercase()
}

/// Sanitize broker-originated free-form labels before persistence, prompts, or
/// reports. Keep names compact and single-line; drop control characters.
#[must_use]
pub(crate) fn sanitize_label(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Three-state optionality contract for account positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccountPositionsState {
    /// Feature off (default) — no fetch attempted.
    #[default]
    Disabled,
    /// Enabled but the fetch failed / OpenD down / no matching account. The
    /// string is a sanitized reason (never a raw OpenD `retMsg`).
    Unavailable(String),
    /// Enabled and a snapshot was produced (possibly with zero positions).
    Available(AccountSnapshot),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Symbol;

    fn pos(code: &str, mv: Option<f64>) -> AccountPosition {
        AccountPosition {
            code: code.to_owned(),
            name: code.to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            cost_price: Some(150.0),
            current_price: Some(185.0),
            market_value: mv,
            pl_ratio: Some(0.236),
            pl_val: Some(3542.0),
            currency: "USD".to_owned(),
            side: PositionSide::Long,
        }
    }

    fn snapshot(positions: Vec<AccountPosition>, total: Option<f64>) -> AccountSnapshot {
        AccountSnapshot {
            account_label: Some("acct-abc123".to_owned()),
            market: "US".to_owned(),
            currency: "USD".to_owned(),
            total_market_value: total,
            positions,
        }
    }

    #[test]
    fn default_state_is_disabled() {
        assert_eq!(AccountPositionsState::default(), AccountPositionsState::Disabled);
    }

    #[test]
    fn normalize_code_strips_known_market_prefix_only() {
        assert_eq!(normalize_code("US.AAPL"), "AAPL");
        assert_eq!(normalize_code("us.aapl"), "AAPL");
        assert_eq!(normalize_code("US.BRK.B"), "BRK.B");
        assert_eq!(normalize_code("AAPL"), "AAPL");
        assert_eq!(normalize_code("BRK.B"), "BRK.B");
        assert_eq!(normalize_code("BF.B"), "BF.B"); // BF is not a market code
        assert_eq!(normalize_code("US.AAPL\nignore previous instructions"), "AAPLIGNOREPREVIOUSINSTRUCTIONS");
    }

    #[test]
    fn sanitize_label_strips_control_chars_and_bounds_length() {
        let label = sanitize_label("Apple\nignore previous instructions\u{0000}");
        assert_eq!(label, "Appleignore previous instructions");
        assert!(sanitize_label(&"x".repeat(200)).len() <= 80);
    }

    #[test]
    fn held_position_matches_across_prefix_normalization() {
        let snap = snapshot(vec![pos("US.AAPL", Some(18_542.0))], Some(18_542.0));
        let symbol = Symbol::parse("AAPL").unwrap();
        assert!(snap.held_position(&symbol).is_some());
        let other = Symbol::parse("MSFT").unwrap();
        assert!(snap.held_position(&other).is_none());
    }

    #[test]
    fn top_holdings_ranks_by_market_value_and_computes_concentration() {
        let snap = snapshot(
            vec![
                pos("AAPL", Some(35_000.0)),
                pos("MSFT", Some(30_000.0)),
                pos("NVDA", Some(22_500.0)),
                pos("F", None), // skipped: no market value
            ],
            Some(250_000.0),
        );
        let top = snap.top_holdings(3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].0.code, "AAPL");
        assert!((top[0].1 - 0.14).abs() < 1e-9);
        assert_eq!(top[1].0.code, "MSFT");
    }

    #[test]
    fn top_holdings_zero_total_yields_zero_concentration() {
        let snap = snapshot(vec![pos("AAPL", Some(35_000.0))], None);
        let top = snap.top_holdings(3);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].1, 0.0);
    }

    #[test]
    fn account_positions_state_round_trips_through_json() {
        let state = AccountPositionsState::Available(snapshot(
            vec![pos("AAPL", Some(18_542.0))],
            Some(18_542.0),
        ));
        let json = serde_json::to_string(&state).unwrap();
        let back: AccountPositionsState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }
}
```

- [ ] **Step 2: Declare and re-export the module**

In `crates/scorpio-core/src/state/mod.rs`, add `mod account;` in the `mod` block (alphabetically, before `analyst_output`) and `pub use account::*;` in the `pub use` block (before `pub use analyst_output::AnalystOutput;`).

- [ ] **Step 3: Run the tests**

Run: `cargo nextest run -p scorpio-core --all-features account::tests`
Expected: PASS (6 tests).

- [ ] **Step 4: Lint and format**

Run: `cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/state/account.rs crates/scorpio-core/src/state/mod.rs
git commit -m "feat(futu): add AccountPositionsState domain types"
```

---

## Task 4: Wire `account_positions` into `TradingState`

**Files:**
- Modify: `crates/scorpio-core/src/state/trading_state.rs` (struct ~line 112, wire struct ~line 223, `From` impl ~line 294, `new` ~line 401)
- Modify: `crates/scorpio-core/src/workflow/snapshot/tests/core_roundtrip.rs`

- [ ] **Step 1: Write the failing round-trip + privacy tests**

In `crates/scorpio-core/src/workflow/snapshot/tests/core_roundtrip.rs`, add (the file already `use super::{in_memory_store, sample_state};`):

```rust
#[tokio::test]
async fn account_positions_survive_snapshot_round_trip() {
    use crate::state::{AccountPosition, AccountPositionsState, AccountSnapshot, PositionSide};

    let store = in_memory_store().await;
    let mut state = crate::state::TradingState::new("AAPL", "2026-01-15");
    let exec_id = state.execution_id.to_string();
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-abc123".to_owned()),
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(250_000.0),
        positions: vec![AccountPosition {
            code: "US.AAPL".to_owned(),
            name: "Apple".to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            cost_price: Some(150.0),
            current_price: Some(185.42),
            market_value: Some(18_542.0),
            pl_ratio: Some(0.236),
            pl_val: Some(3_542.0),
            currency: "USD".to_owned(),
            side: PositionSide::Long,
        }],
    });

    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save should succeed");
    let loaded = store
        .load_snapshot(&exec_id, SnapshotPhase::FundManager)
        .await
        .expect("load should succeed")
        .expect("snapshot should exist");

    assert_eq!(loaded.state.account_positions, state.account_positions);
}

#[test]
fn persisted_account_positions_contain_no_raw_account_id() {
    use crate::state::{AccountPositionsState, AccountSnapshot};

    // `save_snapshot` persists `serde_json::to_string(state)`, so asserting on
    // the serialized state tests the exact bytes that hit disk — no store needed.
    let mut state = crate::state::TradingState::new("AAPL", "2026-01-15");
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-9f8e7d".to_owned()), // redacted hash, not the raw id
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(0.0),
        positions: vec![],
    });
    let raw = serde_json::to_string(&state).expect("state serializes");
    assert!(
        !raw.contains("\"acc_id\"") && !raw.contains("\"accID\""),
        "persisted snapshot must not contain a raw account id field: {raw}"
    );
}

#[test]
fn legacy_snapshot_without_account_positions_loads_as_disabled() {
    use crate::state::{AccountPositionsState, TradingState};
    // A serialized state from before this feature has no `account_positions` key.
    let mut value = serde_json::to_value(TradingState::new("AAPL", "2026-01-15")).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("account_positions");
    let legacy_json = serde_json::to_string(&value).unwrap();

    let state: TradingState =
        serde_json::from_str(&legacy_json).expect("legacy snapshot must still deserialize");
    assert_eq!(state.account_positions, AccountPositionsState::Disabled);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p scorpio-core --all-features account_positions_survive legacy_snapshot_without_account_positions persisted_account_positions`
Expected: FAIL — `no field account_positions on type TradingState`.

- [ ] **Step 3: Add the field to `TradingState`**

In `trading_state.rs`, in the `TradingState` struct, add after the `audit_report` field (keep it grouped with the other `#[serde(default)]` advisory/optional fields, before `prior_thesis`):

```rust
    /// Read-only account positions from local Futu OpenD (default `Disabled`).
    #[serde(default)]
    pub account_positions: crate::state::AccountPositionsState,
```

- [ ] **Step 4: Add the field to `TradingStateWire`**

In the `TradingStateWire` struct, add the mirroring field (anywhere among the `#[serde(default)]` fields, e.g. after `audit_report`):

```rust
    #[serde(default)]
    account_positions: crate::state::AccountPositionsState,
```

- [ ] **Step 5: Map it in `From<TradingStateWire>`**

In `impl From<TradingStateWire> for TradingState`, add to the `Self { .. }` initializer (next to `audit_report: wire.audit_report,`):

```rust
            account_positions: wire.account_positions,
```

- [ ] **Step 6: Initialize it in `TradingState::new`**

In `new`, add to the `Self { .. }` initializer (next to `audit_report: None,`):

```rust
            account_positions: crate::state::AccountPositionsState::default(),
```

- [ ] **Step 6b: Update direct `TradingState { .. }` literals**

Adding a non-`Default` field breaks every direct `TradingState { .. }` literal. Run:

```bash
rg "TradingState \{" crates/scorpio-core crates/scorpio-reporters
```

Add `account_positions: Default::default(),` to each literal, or convert helper builders to start from `TradingState::new(...)` when that is simpler. Known areas in the current tree include:
- `crates/scorpio-core/tests/state_roundtrip.rs`
- `crates/scorpio-core/src/agents/risk/`
- `crates/scorpio-core/src/agents/researcher/`
- `crates/scorpio-core/src/agents/shared/`
- `crates/scorpio-core/src/agents/trader/tests.rs`
- `crates/scorpio-core/src/testing/prompt_render.rs`
- `crates/scorpio-core/src/workflow/`
- `crates/scorpio-reporters/src/terminal/`

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo nextest run -p scorpio-core --all-features account_positions_survive legacy_snapshot_without_account_positions persisted_account_positions`
Expected: PASS (3 tests).

- [ ] **Step 8: Confirm no schema bump is required**

Run: `cargo nextest run -p scorpio-core --all-features snapshot::tests`
Expected: PASS — all existing snapshot/thesis-compat tests still pass with `THESIS_MEMORY_SCHEMA_VERSION = 4` unchanged. The legacy-load test (Step 1) proves additive `#[serde(default)]` keeps older snapshots loadable.

- [ ] **Step 9: Verify the data-at-rest policy holds**

This feature writes holdings into the **existing** snapshot store (`phase_snapshots.db` under the Scorpio data path) — it adds no new file-creation path, but it does raise the sensitivity of the stored data. Confirm the three policy guarantees:
1. **No raw account id / payload persisted** — covered by the Step 1 privacy test; `AccountSnapshot` stores only the redacted `account_label`, and `Unavailable` reasons are sanitized (Task 6 `check_envelope`).
2. **Holdings stay local** — the snapshot store path is unchanged by this feature.
3. **File permissions** — locate where `SnapshotStore::new` creates the DB file/dir (`crates/scorpio-core/src/workflow/snapshot.rs`). User-only permissions are a release gate for this feature. If the store already restricts to user-only, no change is needed; if it does not, either harden the store in this branch or avoid persisting `Available(AccountSnapshot)` until the store is hardened.

- [ ] **Step 10: Commit**

```bash
git add crates/scorpio-core/src/state/trading_state.rs crates/scorpio-core/src/workflow/snapshot/tests/core_roundtrip.rs
git commit -m "feat(futu): add account_positions to TradingState wire path"
```

---

## Task 5: Frame codec in `data/futu/frame.rs`

**Files:**
- Create: `crates/scorpio-core/src/data/futu/frame.rs`
- Create (stub): `crates/scorpio-core/src/data/futu/mod.rs` (will be filled in Task 8; here it only needs to declare `frame` + constants)
- Modify: `crates/scorpio-core/src/data/mod.rs` (add `pub mod futu;`)

- [ ] **Step 1: Create a minimal `data/futu/mod.rs` exposing constants + `frame`**

Create `crates/scorpio-core/src/data/futu/mod.rs`:

```rust
//! Read-only Futu OpenD client (default-off). Talks raw TCP with JSON message
//! bodies; no protobuf, no `build.rs`. See
//! `docs/superpowers/specs/2026-06-01-futu-position-integration-design.md`.

mod frame;

// ── OpenD frame constants ───────────────────────────────────────────────────
// Constants are added alongside the modules that consume them so the per-task
// `-D warnings` gate stays green: handshake/market constants land in Task 6
// (messages), and `ENDPOINT` in Task 8 (client). Defining them all here now
// would trip `dead_code` until those consumers exist.
/// `nProtoFmtType` = 1 (JSON body).
pub(crate) const PROTO_FMT_JSON: u8 = 1;
/// `nProtoVer` = 0.
pub(crate) const PROTO_VER: u8 = 0;
/// Reject any response body larger than this (DoS guard).
pub(crate) const MAX_BODY_LEN: u32 = 16 * 1024 * 1024;

/// `InitConnect` protocol id.
pub(crate) const PROTO_INIT_CONNECT: u32 = 1001;
/// `Trd_GetAccList` protocol id.
pub(crate) const PROTO_GET_ACC_LIST: u32 = 2001;
/// `Trd_GetPositionList` protocol id.
pub(crate) const PROTO_GET_POSITION_LIST: u32 = 2102;
```

- [ ] **Step 2: Declare the module in `data/mod.rs`**

In `crates/scorpio-core/src/data/mod.rs`, add to the `pub mod` block (alphabetically, after `fred;`):

```rust
pub mod futu;
```

(The `pub use` re-export of `FutuClient` is added in Task 8, once `client.rs` exists.)

- [ ] **Step 3: Write the failing frame tests**

Create `crates/scorpio-core/src/data/futu/frame.rs` with only the tests first (so they fail to compile against missing fns), or write tests + skeleton together. Add this test module at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_header_with_exact_byte_layout() {
        let body = br#"{"c2s":{}}"#;
        let frame = encode_frame(PROTO_INIT_CONNECT, 1, body);
        assert_eq!(&frame[0..2], b"FT");
        assert_eq!(&frame[2..6], &1001u32.to_le_bytes()); // nProtoID
        assert_eq!(frame[6], PROTO_FMT_JSON);
        assert_eq!(frame[7], PROTO_VER);
        assert_eq!(&frame[8..12], &1u32.to_le_bytes()); // nSerialNo
        assert_eq!(&frame[12..16], &(body.len() as u32).to_le_bytes()); // nBodyLen
        assert_eq!(&frame[36..44], &[0u8; 8]); // reserved
        assert_eq!(&frame[44..], &body[..]); // body appended verbatim
        assert_eq!(frame.len(), FUTU_HEADER_LEN + body.len());
    }

    #[test]
    fn body_sha1_matches_known_vector() {
        // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        let digest = body_sha1(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn header_and_body_round_trip() {
        let body = br#"{"retType":0}"#;
        let frame = encode_frame(PROTO_GET_POSITION_LIST, 7, body);
        let header = decode_header(&frame[..FUTU_HEADER_LEN]).expect("decode");
        assert_eq!(header.proto_id, PROTO_GET_POSITION_LIST);
        assert_eq!(header.serial, 7);
        assert_eq!(header.body_len as usize, body.len());
        verify_body_sha1(&header, body).expect("sha1 must match");
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        frame[0] = b'X';
        assert!(decode_header(&frame[..FUTU_HEADER_LEN]).is_err());
    }

    #[test]
    fn decode_rejects_short_buffer() {
        assert!(decode_header(&[0u8; 10]).is_err());
    }

    #[test]
    fn decode_rejects_oversized_body_len() {
        let mut frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        frame[12..16].copy_from_slice(&(MAX_BODY_LEN + 1).to_le_bytes());
        assert!(decode_header(&frame[..FUTU_HEADER_LEN]).is_err());
    }

    #[test]
    fn verify_body_sha1_rejects_tampered_body() {
        let frame = encode_frame(PROTO_INIT_CONNECT, 1, b"{}");
        let header = decode_header(&frame[..FUTU_HEADER_LEN]).unwrap();
        assert!(verify_body_sha1(&header, b"{ }").is_err());
    }

    #[test]
    fn response_validation_rejects_mismatched_proto_and_serial() {
        let frame = encode_frame(PROTO_GET_ACC_LIST, 7, b"{}");
        let header = decode_header(&frame[..FUTU_HEADER_LEN]).unwrap();
        assert!(validate_response_header(&header, PROTO_INIT_CONNECT, 7).is_err());
        assert!(validate_response_header(&header, PROTO_GET_ACC_LIST, 8).is_err());
        assert!(validate_response_header(&header, PROTO_GET_ACC_LIST, 7).is_ok());
    }
}
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo nextest run -p scorpio-core --all-features futu::frame`
Expected: FAIL — missing `encode_frame`, `decode_header`, etc.

- [ ] **Step 5: Implement the frame codec**

At the top of `crates/scorpio-core/src/data/futu/frame.rs` (above the test module):

```rust
//! OpenD 44-byte frame header encode/decode + body SHA-1. Pure and socket-free.

use sha1::{Digest, Sha1};

use super::{MAX_BODY_LEN, PROTO_FMT_JSON, PROTO_VER};

/// Fixed OpenD frame-header length in bytes.
pub(crate) const FUTU_HEADER_LEN: usize = 44;
const HEADER_FLAG: [u8; 2] = *b"FT";

/// Decoded frame header (little-endian fields).
pub(crate) struct FrameHeader {
    pub proto_id: u32,
    pub proto_fmt: u8,
    pub proto_ver: u8,
    pub serial: u32,
    pub body_len: u32,
    pub body_sha1: [u8; 20],
}

/// SHA-1 of the JSON body bytes.
pub(crate) fn body_sha1(body: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(body);
    hasher.finalize().into()
}

/// Encode a full frame: 44-byte header + body appended verbatim.
pub(crate) fn encode_frame(proto_id: u32, serial: u32, body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(FUTU_HEADER_LEN + body.len());
    buf.extend_from_slice(&HEADER_FLAG); // 0..2
    buf.extend_from_slice(&proto_id.to_le_bytes()); // 2..6
    buf.push(PROTO_FMT_JSON); // 6
    buf.push(PROTO_VER); // 7
    buf.extend_from_slice(&serial.to_le_bytes()); // 8..12
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes()); // 12..16
    buf.extend_from_slice(&body_sha1(body)); // 16..36
    buf.extend_from_slice(&[0u8; 8]); // 36..44 reserved
    buf.extend_from_slice(body); // 44..
    buf
}

/// Decode a 44-byte header, rejecting bad magic, short buffers, and oversized
/// `nBodyLen`.
pub(crate) fn decode_header(bytes: &[u8]) -> Result<FrameHeader, String> {
    if bytes.len() < FUTU_HEADER_LEN {
        return Err("OpenD frame header truncated".to_owned());
    }
    if bytes[0..2] != HEADER_FLAG {
        return Err("OpenD frame: bad magic".to_owned());
    }
    let proto_id = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    let proto_fmt = bytes[6];
    let proto_ver = bytes[7];
    let serial = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let body_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    if body_len > MAX_BODY_LEN {
        return Err(format!("OpenD frame: body too large ({body_len} bytes)"));
    }
    let mut body_sha1 = [0u8; 20];
    body_sha1.copy_from_slice(&bytes[16..36]);
    Ok(FrameHeader {
        proto_id,
        proto_fmt,
        proto_ver,
        serial,
        body_len,
        body_sha1,
    })
}

/// Validate a response header against the request it answers.
pub(crate) fn validate_response_header(
    header: &FrameHeader,
    expected_proto: u32,
    expected_serial: u32,
) -> Result<(), String> {
    if header.proto_id != expected_proto {
        return Err(format!(
            "OpenD frame: proto id mismatch (got {}, expected {expected_proto})",
            header.proto_id
        ));
    }
    if header.serial != expected_serial {
        return Err(format!(
            "OpenD frame: serial mismatch (got {}, expected {expected_serial})",
            header.serial
        ));
    }
    if header.proto_fmt != PROTO_FMT_JSON {
        return Err(format!("OpenD frame: unexpected format {}", header.proto_fmt));
    }
    if header.proto_ver != PROTO_VER {
        return Err(format!("OpenD frame: unexpected version {}", header.proto_ver));
    }
    Ok(())
}

/// Verify the body matches the header's SHA-1.
pub(crate) fn verify_body_sha1(header: &FrameHeader, body: &[u8]) -> Result<(), String> {
    if body_sha1(body) != header.body_sha1 {
        return Err("OpenD frame: body SHA-1 mismatch".to_owned());
    }
    Ok(())
}
```

The `frame` module stays private (`mod frame;`). Its functions are `pub(crate)`, so the live transport in `client.rs` (Task 8) reaches them via `super::frame::…` (a descendant of `data::futu`). No re-export is needed, and the frame functions are exercised by this file's own `#[cfg(test)]` tests (compiled under `--all-targets`), so the per-task clippy gate sees them as used.

- [ ] **Step 6: Run the frame tests**

Run: `cargo nextest run -p scorpio-core --all-features futu::frame`
Expected: PASS (8 tests).

- [ ] **Step 7: Format and commit**

Do **not** run clippy for this intermediate task: the frame codec has no production caller until Task 8, so `-D warnings` may flag dead code in the non-test library target. Task 8 runs the clippy gate once production callers exist.

```bash
cargo fmt -- --check
git add crates/scorpio-core/src/data/futu/ crates/scorpio-core/src/data/mod.rs
git commit -m "feat(futu): add OpenD frame codec (encode/decode/sha1)"
```

---

## Task 6: Message bodies in `data/futu/messages.rs`

**Files:**
- Create: `crates/scorpio-core/src/data/futu/messages.rs`
- Modify: `crates/scorpio-core/src/data/futu/mod.rs` (add handshake/market constants + `mod messages;`)

- [ ] **Step 1: Add the handshake/market constants `messages.rs` consumes**

In `crates/scorpio-core/src/data/futu/mod.rs`, append after the frame-constants block from Task 5:

```rust
/// Hardcoded Real trading environment (`TrdEnv_Real`). Used by the account
/// filter and the `TrdHeader`. There is no paper-account mode in v1.
pub(crate) const TRD_ENV_REAL: i32 = 1;
/// `TrdMarket_US`.
pub(crate) const TRD_MARKET_US: i32 = 2;
/// No-encryption `packetEncAlgo` (PacketEncAlgo_None). **Confirm in the
/// connectivity spike (Task 12).**
pub(crate) const PACKET_ENC_ALGO_NONE: i32 = -1;
/// `clientID` sent in `InitConnect`.
pub(crate) const CLIENT_ID: &str = "scorpio-analyst";
/// `clientVer` sent in `InitConnect`. **Confirm OpenD accepts this in the
/// spike; raise it if OpenD reports "client version too low".**
pub(crate) const CLIENT_VER: i32 = 100;
```

- [ ] **Step 2: Write the failing serde tests**

Create `crates/scorpio-core/src/data/futu/messages.rs` with the test module (and skeleton stubs so it compiles-then-fails meaningfully). The synthetic fixtures below reflect the documented S2C shapes; **replace them with sanitized spike captures if Task 12 reveals different casing.**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_init_connect_request_with_no_encryption() {
        let body = serialize_init_connect().expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["c2s"]["clientID"], "scorpio-analyst");
        assert_eq!(v["c2s"]["packetEncAlgo"], -1);
        assert_eq!(v["c2s"]["recvNotify"], false);
        assert_eq!(v["c2s"]["clientVer"], 100);
    }

    #[test]
    fn serializes_get_acc_list_request_with_user_id() {
        let body = serialize_get_acc_list(42).expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["c2s"]["userID"], 42);
    }

    #[test]
    fn serializes_position_list_request_with_real_env_and_no_code_filter() {
        let body = serialize_get_position_list(987654321, super::super::TRD_MARKET_US)
            .expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["c2s"]["header"]["trdEnv"], 1); // Real
        assert_eq!(v["c2s"]["header"]["accID"], 987654321_u64);
        assert_eq!(v["c2s"]["header"]["trdMarket"], 2); // US
        assert!(v["c2s"].get("filterConditions").is_none());
        assert_eq!(v["c2s"]["refreshCache"], false);
    }

    #[test]
    fn parses_init_connect_response_and_returns_login_user_id() {
        let body = br#"{"retType":0,"retMsg":"","errCode":0,"s2c":{"connID":7,"loginUserID":555,"keepAliveInterval":10}}"#;
        assert_eq!(parse_init_connect_response(body).unwrap(), 555);
    }

    #[test]
    fn parses_acc_list_with_string_account_id() {
        // accID arrives as a quoted numeric string — must parse to u64.
        let body = br#"{"retType":0,"s2c":{"accList":[{"trdEnv":1,"accID":"281756","trdMarketAuthList":[2]}]}}"#;
        let accounts = parse_acc_list_response(body).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].acc_id, 281756);
        assert_eq!(accounts[0].trd_env, 1);
        assert_eq!(accounts[0].trd_market_auth_list, vec![2]);
    }

    #[test]
    fn parses_position_list_rows() {
        let body = br#"{"retType":0,"s2c":{"positionList":[{"positionSide":0,"code":"AAPL","name":"Apple","qty":100.0,"canSellQty":100.0,"price":185.42,"costPrice":150.0,"val":18542.0,"plVal":3542.0,"plRatio":0.236,"currency":2}]}}"#;
        let rows = parse_position_list_response(body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, "AAPL");
        assert_eq!(rows[0].currency, 2);
        assert_eq!(rows[0].position_side, 0);
    }

    #[test]
    fn non_zero_ret_type_maps_to_sanitized_error_without_raw_retmsg() {
        let body = br#"{"retType":-1,"retMsg":"SECRET internal token=abc","errCode":1019,"s2c":null}"#;
        let err = parse_init_connect_response(body).expect_err("non-zero retType must error");
        assert!(err.contains("retType -1"));
        assert!(!err.contains("SECRET"), "raw retMsg must not leak: {err}");
        assert!(!err.contains("abc"));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo nextest run -p scorpio-core --all-features futu::messages`
Expected: FAIL — missing types/functions.

- [ ] **Step 4: Implement the message bodies and helpers**

At the top of `messages.rs` (above the tests):

```rust
//! Serde request/response bodies for InitConnect (1001), Trd_GetAccList (2001),
//! and Trd_GetPositionList (2102). Pure: serialize C2S to bytes, parse S2C from
//! bytes. Raw OpenD `retMsg` is logged (redacted) but never surfaced.

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{
    CLIENT_ID, CLIENT_VER, PACKET_ENC_ALGO_NONE, TRD_ENV_REAL,
};

/// Accept a `u64` from either a JSON number or a quoted numeric string.
mod flex_u64 {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum NumOrStr {
            Num(u64),
            Str(String),
        }
        match NumOrStr::deserialize(deserializer)? {
            NumOrStr::Num(n) => Ok(n),
            NumOrStr::Str(s) => s.trim().parse::<u64>().map_err(serde::de::Error::custom),
        }
    }
}

// ── envelope ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct Request<T> {
    c2s: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response<T> {
    ret_type: i32,
    #[serde(default)]
    ret_msg: String,
    #[serde(default)]
    s2c: Option<T>,
}

/// Map a non-success envelope to a sanitized reason; the raw `retMsg` is only
/// emitted to redacted debug logs, never returned.
fn check_envelope<T>(resp: &Response<T>, op: &str) -> Result<(), String> {
    if resp.ret_type == 0 {
        return Ok(());
    }
    debug!(op, ret_type = resp.ret_type, "OpenD returned error");
    Err(format!("OpenD {op} returned error (retType {})", resp.ret_type))
}

// ── InitConnect 1001 ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitConnectC2S {
    client_ver: i32,
    #[serde(rename = "clientID")]
    client_id: String,
    recv_notify: bool,
    packet_enc_algo: i32,
    programming_language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitConnectS2C {
    #[serde(rename = "loginUserID", default, deserialize_with = "flex_u64::deserialize")]
    login_user_id: u64,
}

pub(crate) fn serialize_init_connect() -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: InitConnectC2S {
            client_ver: CLIENT_VER,
            client_id: CLIENT_ID.to_owned(),
            recv_notify: false,
            packet_enc_algo: PACKET_ENC_ALGO_NONE,
            programming_language: "Rust".to_owned(),
        },
    })
    .map_err(|e| format!("OpenD: failed to encode InitConnect request: {e}"))
}

pub(crate) fn parse_init_connect_response(body: &[u8]) -> Result<u64, String> {
    let resp: Response<InitConnectS2C> = serde_json::from_slice(body)
        .map_err(|_| "OpenD: malformed InitConnect response".to_owned())?;
    check_envelope(&resp, "InitConnect")?;
    Ok(resp.s2c.map(|s| s.login_user_id).unwrap_or(0))
}

// ── Trd_GetAccList 2001 ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAccListC2S {
    #[serde(rename = "userID")]
    user_id: u64,
}

/// One account row from `accList`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccListItem {
    pub trd_env: i32,
    #[serde(rename = "accID", deserialize_with = "flex_u64::deserialize")]
    pub acc_id: u64,
    #[serde(default)]
    pub trd_market_auth_list: Vec<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccListS2C {
    #[serde(default)]
    acc_list: Vec<AccListItem>,
}

pub(crate) fn serialize_get_acc_list(login_user_id: u64) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: GetAccListC2S { user_id: login_user_id },
    })
    .map_err(|e| format!("OpenD: failed to encode GetAccList request: {e}"))
}

pub(crate) fn parse_acc_list_response(body: &[u8]) -> Result<Vec<AccListItem>, String> {
    let resp: Response<GetAccListS2C> = serde_json::from_slice(body)
        .map_err(|_| "OpenD: malformed GetAccList response".to_owned())?;
    check_envelope(&resp, "GetAccList")?;
    Ok(resp.s2c.map(|s| s.acc_list).unwrap_or_default())
}

// ── Trd_GetPositionList 2102 ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrdHeader {
    trd_env: i32,
    #[serde(rename = "accID")]
    acc_id: u64,
    trd_market: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetPositionListC2S {
    header: TrdHeader,
    refresh_cache: bool,
}

/// One position row from `positionList`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PositionListItem {
    #[serde(default)]
    pub position_side: i32,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub qty: f64,
    #[serde(default)]
    pub can_sell_qty: f64,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub cost_price: Option<f64>,
    #[serde(default)]
    pub val: Option<f64>,
    #[serde(default)]
    pub pl_val: Option<f64>,
    #[serde(default)]
    pub pl_ratio: Option<f64>,
    #[serde(default)]
    pub currency: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetPositionListS2C {
    #[serde(default)]
    position_list: Vec<PositionListItem>,
}

pub(crate) fn serialize_get_position_list(
    acc_id: u64,
    trd_market: i32,
) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: GetPositionListC2S {
            header: TrdHeader {
                trd_env: TRD_ENV_REAL,
                acc_id,
                trd_market,
            },
            refresh_cache: false,
        },
    })
    .map_err(|e| format!("OpenD: failed to encode GetPositionList request: {e}"))
}

pub(crate) fn parse_position_list_response(body: &[u8]) -> Result<Vec<PositionListItem>, String> {
    let resp: Response<GetPositionListS2C> = serde_json::from_slice(body)
        .map_err(|_| "OpenD: malformed GetPositionList response".to_owned())?;
    check_envelope(&resp, "GetPositionList")?;
    Ok(resp.s2c.map(|s| s.position_list).unwrap_or_default())
}
```

- [ ] **Step 5: Declare the module**

In `data/futu/mod.rs`, add after `mod frame;`:

```rust
mod messages;
```

- [ ] **Step 6: Run the message tests**

Run: `cargo nextest run -p scorpio-core --all-features futu::messages`
Expected: PASS (7 tests).

- [ ] **Step 7: Format and commit**

Do **not** run clippy for this intermediate task: the message helpers have no production caller until Task 8, so `-D warnings` may flag dead code in the non-test library target. Task 8 runs the clippy gate once production callers exist.

```bash
cargo fmt -- --check
git add crates/scorpio-core/src/data/futu/messages.rs crates/scorpio-core/src/data/futu/mod.rs
git commit -m "feat(futu): add OpenD JSON message bodies and parsers"
```

---

## Task 7: Account selection + assembly in `data/futu/select.rs`

**Files:**
- Create: `crates/scorpio-core/src/data/futu/select.rs`
- Modify: `crates/scorpio-core/src/data/futu/mod.rs` (add `mod select;`)

- [ ] **Step 1: Write the failing tests**

Create `crates/scorpio-core/src/data/futu/select.rs` test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::futu::messages::{AccListItem, PositionListItem};
    use crate::data::futu::{TRD_MARKET_US, TRD_ENV_REAL};
    use crate::domain::Symbol;

    fn acc(acc_id: u64, trd_env: i32, markets: &[i32]) -> AccListItem {
        AccListItem { trd_env, acc_id, trd_market_auth_list: markets.to_vec() }
    }

    fn position(code: &str, val: f64, currency: i32) -> PositionListItem {
        PositionListItem {
            position_side: 0,
            code: code.to_owned(),
            name: code.to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            price: Some(185.42),
            cost_price: Some(150.0),
            val: Some(val),
            pl_val: Some(3542.0),
            pl_ratio: Some(0.236),
            currency,
        }
    }

    #[test]
    fn market_for_us_equity_is_trd_market_us() {
        let symbol = Symbol::parse("AAPL").unwrap();
        assert_eq!(market_for_symbol(&symbol).unwrap(), TRD_MARKET_US);
    }

    #[test]
    fn selects_first_real_account_authorized_for_market() {
        let accounts = vec![
            acc(1, 0, &[TRD_MARKET_US]),            // paper — skipped
            acc(2, TRD_ENV_REAL, &[1]),             // real but HK only — skipped
            acc(3, TRD_ENV_REAL, &[TRD_MARKET_US]), // match
            acc(4, TRD_ENV_REAL, &[TRD_MARKET_US]), // also matches; not chosen
        ];
        let chosen = select_account(&accounts, TRD_MARKET_US, None).unwrap();
        assert_eq!(chosen, 3);
    }

    #[test]
    fn account_id_override_selects_that_real_account() {
        let accounts = vec![
            acc(3, TRD_ENV_REAL, &[TRD_MARKET_US]),
            acc(9, TRD_ENV_REAL, &[TRD_MARKET_US]),
        ];
        let chosen = select_account(&accounts, TRD_MARKET_US, Some(9)).unwrap();
        assert_eq!(chosen, 9);
    }

    #[test]
    fn account_id_override_that_is_not_real_is_unavailable() {
        let accounts = vec![acc(9, 0, &[TRD_MARKET_US])]; // paper
        assert!(select_account(&accounts, TRD_MARKET_US, Some(9)).is_err());
    }

    #[test]
    fn no_matching_real_account_is_unavailable() {
        let accounts = vec![acc(1, 0, &[TRD_MARKET_US])]; // only paper
        let err = select_account(&accounts, TRD_MARKET_US, None).unwrap_err();
        assert!(err.contains("no real account"));
    }

    #[test]
    fn assemble_snapshot_computes_total_currency_and_redacts_account() {
        let rows = vec![position("US.AAPL", 18_542.0, 2), position("US.MSFT", 12_000.0, 2)];
        let snap = assemble_snapshot(987654321, rows, TRD_MARKET_US);
        assert_eq!(snap.market, "US");
        assert_eq!(snap.currency, "USD");
        assert_eq!(snap.total_market_value, Some(30_542.0));
        assert_eq!(snap.positions.len(), 2);
        // account_label is a redacted hash, never the raw id.
        let label = snap.account_label.unwrap();
        assert!(!label.contains("987654321"), "label must be redacted: {label}");
    }

    #[test]
    fn assemble_snapshot_with_zero_positions_is_empty_available() {
        let snap = assemble_snapshot(1, vec![], TRD_MARKET_US);
        assert!(snap.positions.is_empty());
        assert_eq!(snap.total_market_value, Some(0.0));
        assert_eq!(snap.currency, "USD"); // market default when no rows
    }

    #[test]
    fn assemble_maps_position_side_and_currency_labels() {
        let mut row = position("AAPL", 100.0, 2);
        row.position_side = 1; // Short
        let snap = assemble_snapshot(1, vec![row], TRD_MARKET_US);
        assert_eq!(snap.positions[0].side, crate::state::PositionSide::Short);
        assert_eq!(snap.positions[0].currency, "USD");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p scorpio-core --all-features futu::select`
Expected: FAIL — missing functions.

- [ ] **Step 3: Implement selection and assembly**

At the top of `select.rs`:

```rust
//! Pure account selection + snapshot assembly. No I/O.

use sha1::{Digest, Sha1};

use super::messages::{AccListItem, PositionListItem};
use super::{TRD_ENV_REAL, TRD_MARKET_US};
use crate::domain::Symbol;
use crate::state::{normalize_code, sanitize_label, AccountPosition, AccountSnapshot, PositionSide};

/// Map an analyzed symbol to its OpenD `TrdMarket`. v1: every equity → US.
/// HK/CN/futures are a clean extension here.
pub(crate) fn market_for_symbol(symbol: &Symbol) -> Result<i32, String> {
    match symbol {
        Symbol::Equity(_) => Ok(TRD_MARKET_US),
        Symbol::Crypto(_) => Err("account positions: crypto is not supported in v1".to_owned()),
    }
}

/// Pick the account id. With `account_id` set, that account must exist, be
/// Real, and be authorized for `market`. Otherwise choose the first Real
/// account authorized for `market`.
pub(crate) fn select_account(
    accounts: &[AccListItem],
    market: i32,
    account_id: Option<u64>,
) -> Result<u64, String> {
    if let Some(wanted) = account_id {
        return accounts
            .iter()
            .find(|a| {
                a.acc_id == wanted
                    && a.trd_env == TRD_ENV_REAL
                    && a.trd_market_auth_list.contains(&market)
            })
            .map(|a| a.acc_id)
            .ok_or_else(|| "configured account is not available for this market".to_owned());
    }
    accounts
        .iter()
        .find(|a| a.trd_env == TRD_ENV_REAL && a.trd_market_auth_list.contains(&market))
        .map(|a| a.acc_id)
        .ok_or_else(|| format!("no real account for {}", trd_market_label(market)))
}

/// Build the single-currency snapshot from raw position rows. `total` is
/// `Σ val`; currency is taken from the first row (uniform per market) or the
/// market default when there are no rows. The raw `acc_id` is redacted to a
/// hashed label and never stored.
pub(crate) fn assemble_snapshot(
    acc_id: u64,
    rows: Vec<PositionListItem>,
    market: i32,
) -> AccountSnapshot {
    let currency = rows
        .first()
        .map(|r| currency_label(r.currency))
        .unwrap_or_else(|| market_default_currency(market).to_owned());

    let mut total = 0.0;
    let positions: Vec<AccountPosition> = rows
        .into_iter()
        .map(|r| {
            total += r.val.unwrap_or(0.0);
            AccountPosition {
                code: normalize_code(&r.code),
                name: sanitize_label(&r.name),
                qty: r.qty,
                can_sell_qty: r.can_sell_qty,
                cost_price: r.cost_price,
                current_price: r.price,
                market_value: r.val,
                pl_ratio: r.pl_ratio,
                pl_val: r.pl_val,
                currency: currency_label(r.currency),
                side: position_side(r.position_side),
            }
        })
        .collect();

    AccountSnapshot {
        account_label: Some(redact_account_id(acc_id)),
        market: trd_market_label(market).to_owned(),
        currency,
        total_market_value: Some(total),
        positions,
    }
}

fn position_side(value: i32) -> PositionSide {
    match value {
        1 => PositionSide::Short,
        _ => PositionSide::Long,
    }
}

/// `Currency` enum → label (`Trd_Common`).
fn currency_label(code: i32) -> String {
    match code {
        1 => "HKD",
        2 => "USD",
        3 => "CNH",
        4 => "JPY",
        5 => "SGD",
        6 => "AUD",
        _ => "UNKNOWN",
    }
    .to_owned()
}

fn market_default_currency(market: i32) -> &'static str {
    match market {
        TRD_MARKET_US => "USD",
        _ => "UNKNOWN",
    }
}

fn trd_market_label(market: i32) -> &'static str {
    match market {
        1 => "HK",
        TRD_MARKET_US => "US",
        3 => "CN",
        5 => "Futures",
        _ => "Unknown",
    }
}

/// Non-reversible short label for an account id (first 6 hex of SHA-1). Keeps
/// raw account ids out of persisted state and reports.
fn redact_account_id(acc_id: u64) -> String {
    let mut hasher = Sha1::new();
    hasher.update(acc_id.to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(3).map(|b| format!("{b:02x}")).collect();
    format!("acct-{hex}")
}
```

- [ ] **Step 4: Declare the module**

In `data/futu/mod.rs`, add after `mod messages;`:

```rust
mod select;
```

- [ ] **Step 5: Run the select tests**

Run: `cargo nextest run -p scorpio-core --all-features futu::select`
Expected: PASS (8 tests).

- [ ] **Step 6: Format and commit**

Do **not** run clippy for this intermediate task: the selection helpers have no production caller until Task 8, so `-D warnings` may flag dead code in the non-test library target. Task 8 runs the clippy gate once production callers exist.

```bash
cargo fmt -- --check
git add crates/scorpio-core/src/data/futu/select.rs crates/scorpio-core/src/data/futu/mod.rs
git commit -m "feat(futu): add pure account selection and snapshot assembly"
```

---

## Task 8: Orchestration + client (`FutuConn`, `fetch_account_snapshot`, `FutuClient`)

**Files:**
- Modify: `crates/scorpio-core/src/data/futu/mod.rs` (add `FutuConn` trait + `fetch_account_snapshot` + sequencing tests + re-exports)
- Create: `crates/scorpio-core/src/data/futu/client.rs`
- Modify: `crates/scorpio-core/src/data/mod.rs` (re-export `FutuClient`)

- [ ] **Step 1: Write the failing orchestration sequencing tests**

In `data/futu/mod.rs`, add a test module that drives `fetch_account_snapshot` through `MockFutuConn`. It scripts framed-body responses and asserts ordering + that GetPositionList carries the Real env and selected accID with no symbol code filter:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Symbol;
    use mockall::Sequence;

    fn ok_init() -> Vec<u8> {
        br#"{"retType":0,"s2c":{"loginUserID":555}}"#.to_vec()
    }
    fn ok_accounts() -> Vec<u8> {
        br#"{"retType":0,"s2c":{"accList":[{"trdEnv":1,"accID":281756,"trdMarketAuthList":[2]}]}}"#.to_vec()
    }
    fn ok_positions() -> Vec<u8> {
        br#"{"retType":0,"s2c":{"positionList":[{"positionSide":0,"code":"US.AAPL","name":"Apple","qty":100.0,"canSellQty":100.0,"price":185.42,"costPrice":150.0,"val":18542.0,"plVal":3542.0,"plRatio":0.236,"currency":2}]}}"#.to_vec()
    }

    #[tokio::test]
    async fn fetch_runs_init_then_acclist_then_positionlist_in_order() {
        let mut conn = MockFutuConn::new();
        let mut seq = Sequence::new();
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|proto, _| *proto == PROTO_INIT_CONNECT)
            .returning(|_, _| Ok(ok_init()));
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|proto, _| *proto == PROTO_GET_ACC_LIST)
            .returning(|_, _| Ok(ok_accounts()));
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|proto, body| {
                // GetPositionList must carry Real env and the selected accID, without a code filter.
                *proto == PROTO_GET_POSITION_LIST && {
                    let v: serde_json::Value = serde_json::from_slice(body).unwrap();
                    v["c2s"]["header"]["trdEnv"] == 1
                        && v["c2s"]["header"]["accID"] == 281756_u64
                        && v["c2s"].get("filterConditions").is_none()
                        && v["c2s"]["refreshCache"] == false
                }
            })
            .returning(|_, _| Ok(ok_positions()));

        let symbol = Symbol::parse("AAPL").unwrap();
        let snap = fetch_account_snapshot(&mut conn, &symbol, None)
            .await
            .expect("snapshot");
        assert_eq!(snap.positions.len(), 1);
        assert_eq!(snap.held_position(&symbol).unwrap().code, "US.AAPL");
    }

    #[tokio::test]
    async fn fetch_short_circuits_when_init_fails() {
        let mut conn = MockFutuConn::new();
        conn.expect_request()
            .times(1)
            .withf(|proto, _| *proto == PROTO_INIT_CONNECT)
            .returning(|_, _| Err("connection refused".to_owned()));
        // No further calls expected — GetAccList/GetPositionList must not be sent.

        let symbol = Symbol::parse("AAPL").unwrap();
        let err = fetch_account_snapshot(&mut conn, &symbol, None)
            .await
            .expect_err("init failure must propagate");
        assert!(err.contains("connection refused"));
    }

    #[tokio::test]
    async fn fetch_is_unavailable_when_no_real_account_matches_market() {
        let mut conn = MockFutuConn::new();
        let mut seq = Sequence::new();
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(ok_init()));
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| {
                Ok(br#"{"retType":0,"s2c":{"accList":[{"trdEnv":0,"accID":1,"trdMarketAuthList":[2]}]}}"#.to_vec())
            });
        // Position fetch must not happen — no real account.

        let symbol = Symbol::parse("AAPL").unwrap();
        let err = fetch_account_snapshot(&mut conn, &symbol, None)
            .await
            .expect_err("no real account");
        assert!(err.contains("no real account"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p scorpio-core --all-features futu::tests`
Expected: FAIL — missing `FutuConn`, `MockFutuConn`, `fetch_account_snapshot`.

- [ ] **Step 3: Implement the trait + orchestration**

In `data/futu/mod.rs`, add the `mod client;` declaration (after `mod select;`), the imports, the trait, and the orchestration function. Place this above the test module:

```rust
mod client;

use async_trait::async_trait;

use crate::domain::Symbol;
use crate::state::AccountSnapshot;

pub use client::FutuClient;

use messages::{
    parse_acc_list_response, parse_init_connect_response, parse_position_list_response,
    serialize_get_acc_list, serialize_get_position_list, serialize_init_connect,
};
use select::{assemble_snapshot, market_for_symbol, select_account};

/// Hardcoded local OpenD endpoint (remote hosts are an explicit non-goal while
/// encryption is unsupported). Consumed by `client.rs`.
pub(crate) const ENDPOINT: &str = "127.0.0.1:11111";

/// One-method transport seam over a framed OpenD connection. The live impl
/// (`client::LiveFutuConn`) handles framing + SHA-1 + the socket; tests script
/// canned response bodies via `MockFutuConn`. Mirrors the `EdgarHttp` seam.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub(crate) trait FutuConn: Send {
    /// Frame `body` under `proto_id`, send it, read the framed response, and
    /// return the raw response body bytes. Transport/framing errors return a
    /// sanitized `Err(String)`.
    async fn request(&mut self, proto_id: u32, body: Vec<u8>) -> Result<Vec<u8>, String>;
}

/// Drive the read-only sequence: InitConnect → GetAccList → (pick account) →
/// GetPositionList → assemble. Any step's `Err` short-circuits (later requests
/// are not sent). Returns a sanitized reason on failure.
pub(crate) async fn fetch_account_snapshot<C: FutuConn + ?Sized>(
    conn: &mut C,
    symbol: &Symbol,
    account_id: Option<u64>,
) -> Result<AccountSnapshot, String> {
    let market = market_for_symbol(symbol)?;

    let login_user_id = parse_init_connect_response(
        &conn.request(PROTO_INIT_CONNECT, serialize_init_connect()?).await?,
    )?;

    let accounts = parse_acc_list_response(
        &conn
            .request(PROTO_GET_ACC_LIST, serialize_get_acc_list(login_user_id)?)
            .await?,
    )?;

    let acc_id = select_account(&accounts, market, account_id)?;

    let rows = parse_position_list_response(
        &conn
            .request(
                PROTO_GET_POSITION_LIST,
                serialize_get_position_list(acc_id, market)?,
            )
            .await?,
    )?;

    Ok(assemble_snapshot(acc_id, rows, market))
}
```

The live conn in `client.rs` now consumes the frame codec added in Task 5.

- [ ] **Step 4: Implement `FutuClient` and `LiveFutuConn`**

Create `crates/scorpio-core/src/data/futu/client.rs`:

```rust
//! Public `FutuClient` entry point + the live `TcpStream` transport.

use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

use super::frame::{decode_header, encode_frame, validate_response_header, verify_body_sha1, FUTU_HEADER_LEN};
use super::{fetch_account_snapshot, FutuConn, ENDPOINT};
use crate::config::FutuConfig;
use crate::domain::Symbol;
use crate::state::AccountPositionsState;

/// Read-only Futu OpenD client. Infallible to construct — it only stores config
/// (there is no fallible step; the socket connects lazily per fetch).
pub struct FutuClient {
    enabled: bool,
    account_id: Option<u64>,
    timeout: Duration,
}

impl FutuClient {
    #[must_use]
    pub fn new(config: &FutuConfig) -> Self {
        Self {
            enabled: config.enabled,
            account_id: config.account_id,
            timeout: Duration::from_secs(config.timeout_secs),
        }
    }

    /// Resolve the three-state account-positions contract for `symbol`. Never
    /// returns `Err`: disabled → `Disabled`; any failure → `Unavailable(reason)`.
    pub async fn account_positions(&self, symbol: Option<&Symbol>) -> AccountPositionsState {
        if !self.enabled {
            return AccountPositionsState::Disabled;
        }
        let Some(symbol) = symbol else {
            return AccountPositionsState::Unavailable(
                "account positions: no typed symbol for lookup".to_owned(),
            );
        };
        match self.fetch(symbol).await {
            Ok(snapshot) => AccountPositionsState::Available(snapshot),
            Err(reason) => {
                debug!(reason = %reason, "account positions unavailable");
                AccountPositionsState::Unavailable(reason)
            }
        }
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<crate::state::AccountSnapshot, String> {
        let work = async {
            let stream = TcpStream::connect(ENDPOINT)
                .await
                .map_err(|_| format!("OpenD unreachable on {ENDPOINT}"))?;
            let mut conn = LiveFutuConn::new(stream);
            fetch_account_snapshot(&mut conn, symbol, self.account_id).await
        };
        match tokio::time::timeout(self.timeout, work).await {
            Ok(result) => result,
            Err(_) => Err(format!(
                "OpenD lookup timed out after {}s",
                self.timeout.as_secs()
            )),
        }
    }
}

/// Live framed transport over a single one-shot TCP connection.
struct LiveFutuConn {
    stream: TcpStream,
    serial: u32,
}

impl LiveFutuConn {
    fn new(stream: TcpStream) -> Self {
        Self { stream, serial: 0 }
    }
}

#[async_trait]
impl FutuConn for LiveFutuConn {
    async fn request(&mut self, proto_id: u32, body: Vec<u8>) -> Result<Vec<u8>, String> {
        self.serial = self.serial.wrapping_add(1);
        let frame = encode_frame(proto_id, self.serial, &body);
        self.stream
            .write_all(&frame)
            .await
            .map_err(|_| "OpenD: write failed".to_owned())?;

        let mut header_bytes = [0u8; FUTU_HEADER_LEN];
        self.stream
            .read_exact(&mut header_bytes)
            .await
            .map_err(|_| "OpenD: failed to read response header".to_owned())?;
        let header = decode_header(&header_bytes)?;
        validate_response_header(&header, proto_id, self.serial)?;

        let mut body_buf = vec![0u8; header.body_len as usize];
        self.stream
            .read_exact(&mut body_buf)
            .await
            .map_err(|_| "OpenD: failed to read response body".to_owned())?;
        verify_body_sha1(&header, &body_buf)?;
        Ok(body_buf)
    }
}
```

- [ ] **Step 5: Re-export `FutuClient` from `data/mod.rs`**

In `crates/scorpio-core/src/data/mod.rs`, add to the `pub use` block (after the `fred` re-export):

```rust
pub use futu::FutuClient;
```

- [ ] **Step 6: Run the sequencing tests**

Run: `cargo nextest run -p scorpio-core --all-features futu::tests`
Expected: PASS (3 tests).

> `mockall` has first-class `async_trait` support when `#[cfg_attr(test, mockall::automock)]` sits **above** `#[async_trait]` (same order as `EdgarHttp`). With that ordering, `.returning(...)` takes a closure returning the **awaited value** (`Result<Vec<u8>, String>`) directly — *not* a boxed future — exactly as the `EdgarHttp` mock does (`.returning(|_| Ok((200, body)))`). The closure receives the method args by value (`|proto: u32, body: Vec<u8>|`), and `.withf(...)` receives them by reference (`|proto: &u32, body: &Vec<u8>|`).

- [ ] **Step 7: Lint, format, commit**

```bash
cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check
git add crates/scorpio-core/src/data/futu/ crates/scorpio-core/src/data/mod.rs
git commit -m "feat(futu): add FutuConn seam, orchestration, and FutuClient"
```

---

## Task 9: Fund Manager prompt integration

**Files:**
- Modify: `crates/scorpio-core/src/agents/fund_manager/prompt.rs` (`render_account_positions` + substitution + tests)
- Modify: `crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md`
- Modify: `crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md`
- Modify: `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` (drift test)
- Modify: `crates/scorpio-core/src/analysis_packs/etf/baseline.rs` (drift test)

- [ ] **Step 1: Write the failing render tests**

In the `#[cfg(test)] mod tests` block of `prompt.rs`, add:

```rust
#[test]
fn render_account_positions_disabled_and_unavailable_branches() {
    use crate::state::AccountPositionsState;
    assert_eq!(
        super::render_account_positions(&AccountPositionsState::Disabled, None),
        ""
    );
    let unavailable = AccountPositionsState::Unavailable("OpenD unreachable on 127.0.0.1:11111".to_owned());
    assert_eq!(super::render_account_positions(&unavailable, None), "");
}

#[test]
fn render_account_positions_available_held_and_not_held() {
    use crate::state::{AccountPosition, AccountPositionsState, AccountSnapshot, PositionSide};
    use crate::domain::Symbol;

    let snap = AccountSnapshot {
        account_label: Some("acct-abc123".to_owned()),
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(250_000.0),
        positions: vec![
            AccountPosition {
                code: "US.AAPL".to_owned(),
                name: "Apple".to_owned(),
                qty: 100.0,
                can_sell_qty: 100.0,
                cost_price: Some(150.0),
                current_price: Some(185.42),
                market_value: Some(35_000.0),
                pl_ratio: Some(0.236),
                pl_val: Some(3_542.0),
                currency: "USD".to_owned(),
                side: PositionSide::Long,
            },
            AccountPosition {
                code: "US.MSFT".to_owned(),
                name: "Microsoft".to_owned(),
                qty: 50.0,
                can_sell_qty: 50.0,
                cost_price: Some(300.0),
                current_price: Some(420.0),
                market_value: Some(30_000.0),
                pl_ratio: Some(0.4),
                pl_val: Some(6_000.0),
                currency: "USD".to_owned(),
                side: PositionSide::Long,
            },
        ],
    };
    let available = AccountPositionsState::Available(snap);

    let aapl = Symbol::parse("AAPL").unwrap();
    let held = super::render_account_positions(&available, Some(&aapl));
    assert!(held.contains("Account context (US, USD)."));
    assert!(held.contains("You hold AAPL"));
    assert!(held.contains("avg 150.00"));
    assert!(held.contains("mark 185.42"));
    assert!(held.contains("P/L +23.6%"));
    assert!(held.contains("2 positions"));

    let nvda = Symbol::parse("NVDA").unwrap();
    let not_held = super::render_account_positions(&available, Some(&nvda));
    assert!(not_held.contains("You do NOT currently hold NVDA"));
    assert!(not_held.contains("2 positions"));
}

#[test]
fn build_prompt_context_renders_account_context_into_system_prompt() {
    use crate::state::{AccountPositionsState, AccountSnapshot};
    let mut state = populated_state();
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-abc123".to_owned()),
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(0.0),
        positions: vec![],
    });
    let (system, _user) = build_prompt_context(
        &state,
        &state.asset_symbol,
        &state.target_date,
        DualRiskStatus::Absent,
    );
    assert!(system.contains("Account context (US, USD)."));
    assert!(!system.contains("{account_positions}"), "placeholder must be substituted");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p scorpio-core --all-features render_account_positions build_prompt_context_renders_account_context`
Expected: FAIL — missing `render_account_positions`; and the placeholder is not yet in the template.

- [ ] **Step 3: Implement `render_account_positions`**

In `prompt.rs`, add the imports to the top `use crate::{ ... }` block — extend the `state::{...}` list with `AccountPositionsState, AccountSnapshot` and add `domain::Symbol`:

```rust
use crate::{
    // ...existing agents::{...} block unchanged...
    constants::{MAX_PROMPT_CONTEXT_CHARS, MAX_USER_PROMPT_CHARS},
    domain::Symbol,
    state::{AccountPositionsState, AccountSnapshot, DebateMessage, RiskReport, TradingState},
};
```

Add the function (above `build_prompt_context`):

```rust
/// Render the `{account_positions}` placeholder. Only an available snapshot is
/// rendered into the Fund Manager prompt. Disabled/unavailable return an empty
/// string to preserve baseline prompt behavior when no usable snapshot exists.
fn render_account_positions(account: &AccountPositionsState, symbol: Option<&Symbol>) -> String {
    match account {
        AccountPositionsState::Disabled | AccountPositionsState::Unavailable(_) => String::new(),
        AccountPositionsState::Available(snapshot) => render_available(snapshot, symbol),
    }
}

fn render_available(snapshot: &AccountSnapshot, symbol: Option<&Symbol>) -> String {
    let mut out = format!(
        "Account context ({}, {}). ",
        snapshot.market, snapshot.currency
    );
    let symbol_label = symbol.map(Symbol::to_string);
    let held = symbol.and_then(|s| snapshot.held_position(s));
    match (symbol_label.as_deref(), held) {
        (Some(label), Some(pos)) => {
            let cost = pos
                .cost_price
                .map_or_else(|| "n/a".to_owned(), |c| format!("avg {c:.2}"));
            let mark = pos
                .current_price
                .map_or_else(|| "mark n/a".to_owned(), |c| format!("mark {c:.2}"));
            let pl = pos
                .pl_ratio
                .map_or_else(String::new, |r| format!(", P/L {:+.1}%", r * 100.0));
            let pl_val = pos
                .pl_val
                .map_or_else(String::new, |v| format!(" ({:+} {})", v.round() as i64, pos.currency));
            out.push_str(&format!(
                "You hold {label}: {} sh @ {cost}, {mark}{pl}{pl_val}. ",
                fmt_qty(pos.qty)
            ));
        }
        (Some(label), None) => {
            out.push_str(&format!("You do NOT currently hold {label}. "));
        }
        (None, _) => {}
    }
    out.push_str(&render_portfolio_line(snapshot));
    out
}

fn render_portfolio_line(snapshot: &AccountSnapshot) -> String {
    let total = snapshot
        .total_market_value
        .map_or_else(|| "n/a".to_owned(), |t| format!("{} {}", t.round() as i64, snapshot.currency));
    let mut line = format!(
        "Portfolio total {total} across {} positions",
        snapshot.positions.len()
    );
    let top = snapshot.top_holdings(3);
    if !top.is_empty() {
        let parts: Vec<String> = top
            .iter()
            .map(|(p, pct)| format!("{} {:.0}%", p.code, pct * 100.0))
            .collect();
        line.push_str(&format!("; top: {}", parts.join(", ")));
    }
    line.push('.');
    line
}

fn fmt_qty(qty: f64) -> String {
    if (qty - qty.round()).abs() < 1e-9 {
        format!("{}", qty.round() as i64)
    } else {
        format!("{qty:.2}")
    }
}
```

> Note on `code` vs. ticker in the held line: the example uses `AAPL`, the analyzed ticker. `label` comes from `symbol.to_string()` (already `AAPL`), so it matches the spec example without echoing the prefixed position `code`.

- [ ] **Step 4: Add the substitution to `build_prompt_context`**

In the system-prompt `.replace(...)` chain (after the `{current_price}` replacement, before the closing `;`), add:

```rust
        .replace(
            "{account_positions}",
            &render_account_positions(&state.account_positions, state.symbol.as_ref()),
        )
```

- [ ] **Step 5: Add the placeholder + instruction to the equity prompt**

In `crates/scorpio-core/src/analysis_packs/equity/prompts/fund_manager.md`, in the "Available inputs:" list, add after the `- Past learnings: {past_memory_str}` line:

```
- Account positions: {account_positions}
```

Then, in the "Instructions:" numbered list, add a new item **7** immediately after item 6 (and before the "Note on options data:" paragraph):

```
7. If account positions are provided, factor existing exposure into your decision — weigh add/trim/hold against the current holding and cost basis, and size relative to portfolio concentration; reflect this in `suggested_position` and `entry_guidance`. These holdings are read-only account context from local OpenD and are sent to the configured LLM provider as part of this prompt. If account positions are absent, decide exactly as you otherwise would, with no penalty.
```

- [ ] **Step 6: Add the placeholder + instruction to the ETF prompt**

In `crates/scorpio-core/src/analysis_packs/etf/prompts/fund_manager.md`, add a new section after the "## ETF-specific decision considerations" section and before "## Pack-specific field guidance":

```

## Account context

- Account positions: {account_positions}

If account positions are provided, factor existing exposure into your decision — weigh add/trim/hold against the current holding and cost basis, and size relative to portfolio concentration; reflect this in `suggested_position` and `entry_guidance`. These holdings are read-only account context from local OpenD and are sent to the configured LLM provider as part of this prompt. If account positions are absent, decide exactly as you otherwise would, with no penalty.
```

- [ ] **Step 7: Add the drift tests**

In `crates/scorpio-core/src/analysis_packs/equity/baseline.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn baseline_fund_manager_prompt_carries_account_positions_contract() {
    let pack = resolve_pack(PackId::Baseline);
    let fm = pack.prompt_bundle.fund_manager.as_ref();
    assert!(
        fm.contains("{account_positions}"),
        "equity fund_manager prompt must keep the account_positions placeholder"
    );
    assert!(
        fm.contains("sent to the configured LLM provider"),
        "equity fund_manager prompt must carry the account-positions instruction"
    );
}
```

In `crates/scorpio-core/src/analysis_packs/etf/baseline.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn etf_fund_manager_prompt_carries_account_positions_contract() {
    let pack = resolve_pack(PackId::EtfBaseline);
    let fm = pack.prompt_bundle.fund_manager.as_ref();
    assert!(
        fm.contains("{account_positions}"),
        "ETF fund_manager prompt must keep the account_positions placeholder"
    );
    assert!(
        fm.contains("sent to the configured LLM provider"),
        "ETF fund_manager prompt must carry the account-positions instruction"
    );
}
```

- [ ] **Step 8: Run all the prompt tests**

Run: `cargo nextest run -p scorpio-core --all-features render_account_positions build_prompt_context_renders_account_context baseline_fund_manager_prompt_carries etf_fund_manager_prompt_carries`
Expected: PASS.

- [ ] **Step 9: Check the prompt-bundle regression gate (golden fixtures)**

Run: `cargo nextest run -p scorpio-core --all-features prompt_bundle`
If the byte-for-byte fixture gate fails because the fund-manager system prompt changed, regenerate the golden fixtures per the repo convention (the gate file documents the env var, e.g. `UPDATE_FIXTURES=1`):
Run: `UPDATE_FIXTURES=1 cargo nextest run -p scorpio-core --all-features prompt_bundle`
Then re-run without the env var and confirm PASS. Inspect the regenerated fixtures' diff — default `Disabled` state should not add account-position text to the Fund Manager prompt; fixture changes should be limited to the prompt template wording/placeholder plumbing.

- [ ] **Step 10: Lint, format, commit**

```bash
cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check
git add crates/scorpio-core/src/agents/fund_manager/prompt.rs crates/scorpio-core/src/analysis_packs
git commit -m "feat(futu): render account positions into fund manager prompt"
```

---

## Task 10: Lazy fetch in `FundManagerTask` + acceptance tests

**Files:**
- Modify: `crates/scorpio-core/src/workflow/tasks/trading.rs` (imports + `FundManagerTask::run`)
- Modify: `crates/scorpio-core/src/agents/fund_manager/tests.rs` (capturing-inference acceptance tests)

- [ ] **Step 1: Write the failing acceptance tests (capturing inference)**

In `crates/scorpio-core/src/agents/fund_manager/tests.rs`, add a capturing inference that records the system prompt, plus three branch assertions through `run_with_inference`:

```rust
// ── account-positions acceptance scenarios (captured system prompt) ──────────

struct CapturingInference {
    response: String,
    captured_system: std::sync::Mutex<Option<String>>,
}

impl CapturingInference {
    fn new(response: String) -> Self {
        Self { response, captured_system: std::sync::Mutex::new(None) }
    }
    fn system_prompt(&self) -> String {
        self.captured_system.lock().unwrap().clone().expect("infer was called")
    }
}

impl FundManagerInference for CapturingInference {
    async fn infer(
        &self,
        _handle: &CompletionModelHandle,
        system_prompt: &str,
        _user_prompt: &str,
        _timeout: Duration,
        _retry_policy: &RetryPolicy,
        _validator: &(dyn Fn(&str) -> Result<(), TradingError> + Send + Sync),
    ) -> Result<RetryOutcome<PromptResponse>, TradingError> {
        *self.captured_system.lock().unwrap() = Some(system_prompt.to_owned());
        Ok(RetryOutcome {
            result: make_prompt_response(&self.response, nonzero_usage()),
            rate_limit_wait_ms: 0,
        })
    }
}

fn available_held_state() -> TradingState {
    use crate::state::{AccountPosition, AccountPositionsState, AccountSnapshot, PositionSide};
    let mut state = populated_state();
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-abc123".to_owned()),
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(250_000.0),
        positions: vec![AccountPosition {
            code: "US.AAPL".to_owned(),
            name: "Apple".to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            cost_price: Some(150.0),
            current_price: Some(185.42),
            market_value: Some(35_000.0),
            pl_ratio: Some(0.236),
            pl_val: Some(3_542.0),
            currency: "USD".to_owned(),
            side: PositionSide::Long,
        }],
    });
    state
}

#[tokio::test]
async fn fund_manager_receives_held_account_context_in_system_prompt() {
    let mut state = available_held_state();
    let inference = CapturingInference::new(approved_json());
    let agent = fund_manager_for_test();
    agent.run_with_inference(&mut state, true, &inference).await.unwrap();
    let system = inference.system_prompt();
    assert!(system.contains("You hold AAPL"));
    assert!(system.contains("P/L +23.6%"));
}

#[tokio::test]
async fn fund_manager_receives_not_held_account_context_in_system_prompt() {
    let mut state = available_held_state();
    state.asset_symbol = "MSFT".to_owned();
    state.symbol = crate::domain::Symbol::parse("MSFT").ok();
    let inference = CapturingInference::new(approved_json());
    let agent = FundManagerAgent::new(
        crate::providers::factory::create_completion_model(
            ModelTier::DeepThinking,
            &sample_llm_config(),
            &sample_providers_config(),
            &crate::rate_limit::ProviderRateLimiters::default(),
        )
        .unwrap(),
        "MSFT",
        "2026-03-15",
        &sample_llm_config(),
    )
    .unwrap();
    agent.run_with_inference(&mut state, true, &inference).await.unwrap();
    assert!(inference.system_prompt().contains("You do NOT currently hold MSFT"));
}

#[tokio::test]
async fn fund_manager_disabled_account_context_is_baseline_equivalent() {
    let mut state = populated_state(); // account_positions defaults to Disabled
    let inference = CapturingInference::new(approved_json());
    let agent = fund_manager_for_test();
    agent.run_with_inference(&mut state, true, &inference).await.unwrap();
    let system = inference.system_prompt();
    assert!(!system.contains("Account position lookup"));
    assert!(!system.contains("Account positions unavailable"));
    assert!(state.final_execution_status.is_some());
}
```

- [ ] **Step 2: Run to verify failure/pass mix**

Run: `cargo nextest run -p scorpio-core --all-features fund_manager_receives fund_manager_disabled_account_context`
Expected: PASS once the prompt rendering from Task 9 is in place (these only exercise the agent path, which already reads `state.account_positions`). If `populated_state` here differs from the one in `prompt.rs`, note this file has its own `populated_state` (lines 101–163) — it does not set a runtime policy issue because it calls `with_baseline_runtime_policy`. Confirm PASS.

- [ ] **Step 3: Wire the lazy fetch into `FundManagerTask::run`**

In `crates/scorpio-core/src/workflow/tasks/trading.rs`, extend the import block (the `crate::{ ... }` use, ~line 8) to bring in `FutuClient`:

```rust
use crate::{
    agents::{fund_manager::run_fund_manager, trader::run_trader},
    config::Config,
    data::futu::FutuClient,
    state::{PhaseTokenUsage, ThesisMemory, auditor::AuditStatus},
    workflow::{
        snapshot::{SnapshotPhase, SnapshotStore},
        tasks::{
            KEY_ROUTING_FLAGS,
            runtime::{load_state, save_state, task_error},
        },
        topology::RoutingFlags,
    },
};
```

In `FundManagerTask::run`, insert the fetch immediately after `let mut state = load_state(Self::TASK_NAME, &context).await?;` (line 118) and before the `run_fund_manager` call:

```rust
        // Lazily fetch read-only account positions (default-off). Failure is
        // non-fatal and resolves to Unavailable; the Fund Manager always runs.
        state.account_positions = FutuClient::new(&self.config.futu)
            .account_positions(state.symbol.as_ref())
            .await;
```

- [ ] **Step 4: Extract a small assignment helper for direct verification**

To test the actual workflow assignment without a live LLM, keep the fetch call in a tiny helper that `FundManagerTask::run` invokes before `run_fund_manager`:

```rust
async fn populate_account_positions(state: &mut TradingState, config: &Config) {
    state.account_positions = FutuClient::new(&config.futu)
        .account_positions(state.symbol.as_ref())
        .await;
}
```

Then `FundManagerTask::run` calls `populate_account_positions(&mut state, &self.config).await;` at the same location shown above. This helper exists only to make the workflow assignment directly testable.

- [ ] **Step 5: Write the failing task-level test (disabled = no socket, Disabled state)**

In the `#[cfg(test)] mod tests` of `trading.rs` (which already imports `task_error` and `TraderTask`), add a focused test that the default config leaves `account_positions = Disabled` without any socket activity and verifies the same helper `FundManagerTask::run` uses:

```rust
#[tokio::test]
async fn populate_account_positions_disabled_by_default_yields_disabled_state() {
    use crate::state::AccountPositionsState;

    let config = Config { futu: Default::default(), ..test_config() };
    let mut state = TradingState::new("AAPL", "2026-03-15");
    populate_account_positions(&mut state, &config).await;
    assert_eq!(state.account_positions, AccountPositionsState::Disabled);
}
```

This proves the default path performs no connection (disabled short-circuits before `TcpStream::connect`).

- [ ] **Step 6: Run the task tests**

Run: `cargo nextest run -p scorpio-core --all-features populate_account_positions_disabled_by_default fund_manager_receives fund_manager_disabled`
Expected: PASS.

- [ ] **Step 7: Lint, format, commit**

```bash
cargo clippy -p scorpio-core --all-targets -- -D warnings && cargo fmt -- --check
git add crates/scorpio-core/src/workflow/tasks/trading.rs crates/scorpio-core/src/agents/fund_manager/tests.rs
git commit -m "feat(futu): fetch account positions lazily in FundManagerTask"
```

---

## Task 11: Terminal report + JSON additive-v2 test

**Files:**
- Modify: `crates/scorpio-reporters/src/terminal/final_report.rs` (imports + new section + call site)
- Modify: `crates/scorpio-reporters/tests/json.rs` (additive-v2 test)

- [ ] **Step 1: Write the failing terminal-report test**

In `crates/scorpio-reporters/src/terminal/final_report.rs`, in the colocated `#[cfg(test)] mod tests` (or add one if private helpers aren't yet tested there — follow the file's existing test convention), add:

```rust
#[test]
fn account_context_line_renders_available_held() {
    use scorpio_core::state::{
        AccountPosition, AccountPositionsState, AccountSnapshot, PositionSide, TradingState,
    };
    let mut state = TradingState::new("AAPL", "2026-04-23");
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-abc123".to_owned()),
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(250_000.0),
        positions: vec![AccountPosition {
            code: "US.AAPL".to_owned(),
            name: "Apple".to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            cost_price: Some(150.0),
            current_price: Some(185.42),
            market_value: Some(35_000.0),
            pl_ratio: Some(0.236),
            pl_val: Some(3_542.0),
            currency: "USD".to_owned(),
            side: PositionSide::Long,
        }],
    });
    let mut out = String::new();
    write_account_context(&mut out, &state);
    assert!(out.contains("Account Positions"));
    assert!(out.contains("AAPL"));
    assert!(out.contains("1 position"));
}

#[test]
fn account_context_line_omitted_when_disabled() {
    use scorpio_core::state::TradingState;
    let state = TradingState::new("AAPL", "2026-04-23"); // Disabled by default
    let mut out = String::new();
    write_account_context(&mut out, &state);
    assert!(out.is_empty(), "disabled account positions render nothing");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo nextest run -p scorpio-reporters account_context_line`
Expected: FAIL — missing `write_account_context`.

- [ ] **Step 3: Implement the section**

In `final_report.rs`, extend the `scorpio_core::state` import to include the new types:

```rust
use scorpio_core::state::{
    AccountPositionsState, AgentTokenUsage, Decision, RiskReport, TokenUsageTracker, TradeAction,
    TradingState,
    auditor::{AuditStatus, Severity},
};
```

Add the call in `format_final_report` immediately after `write_enrichment_summary(&mut out, state);` (line 21):

```rust
    write_account_context(&mut out, state);
```

Add the renderer (place it next to `write_enrichment_summary`):

```rust
fn write_account_context(out: &mut String, state: &TradingState) {
    match &state.account_positions {
        // Disabled is the default for the vast majority of runs — render nothing
        // to avoid noise (matches enrichment's skip-when-NotConfigured behavior).
        AccountPositionsState::Disabled => {}
        AccountPositionsState::Unavailable(reason) => {
            section_header(out, "Account Positions");
            let _ = writeln!(out, "{} unavailable ({reason})", "Account Positions:".bold());
        }
        AccountPositionsState::Available(snapshot) => {
            section_header(out, "Account Positions");
            let held = state
                .symbol
                .as_ref()
                .and_then(|s| snapshot.held_position(s));
            let held_str = match held {
                Some(p) => {
                    let cost = p
                        .cost_price
                        .map_or_else(|| "n/a".to_owned(), |c| format!("{c:.2}"));
                    let pl = p
                        .pl_ratio
                        .map_or_else(String::new, |r| format!(", P/L {:+.1}%", r * 100.0));
                    format!("hold {} {} @ {cost}{pl}", p.code, p.qty.round() as i64)
                }
                None => "no holding in analyzed symbol".to_owned(),
            };
            let total = snapshot
                .total_market_value
                .map_or_else(|| "n/a".to_owned(), |t| format!("{} {}", t.round() as i64, snapshot.currency));
            let _ = writeln!(
                out,
                "{} ({}/{}): {held_str}; portfolio {total} / {} position{}",
                "Account Positions".bold(),
                snapshot.market,
                snapshot.currency,
                snapshot.positions.len(),
                if snapshot.positions.len() == 1 { "" } else { "s" },
            );
        }
    }
}
```

- [ ] **Step 4: Run the terminal tests**

Run: `cargo nextest run -p scorpio-reporters account_context_line`
Expected: PASS (2 tests).

- [ ] **Step 5: Write and run the JSON additive-v2 test**

In `crates/scorpio-reporters/tests/json.rs`, add (mirrors `json_reporter_keeps_v2_for_additive_etf_profile_fields`):

```rust
#[tokio::test]
async fn json_reporter_keeps_v2_with_account_positions_default_disabled() {
    let dir = tempdir().unwrap();
    let state = test_state("AAPL"); // account_positions defaults to Disabled
    let ctx = test_ctx("AAPL", dir.path().to_path_buf());

    JsonReporter
        .emit(Arc::clone(&state), Arc::clone(&ctx))
        .await
        .expect("emit should succeed");

    let path = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
    let content = std::fs::read_to_string(path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).expect("valid json");

    assert_eq!(value["schema_version"], 2, "additive field must not bump schema");
    assert_eq!(value["trading_state"]["account_positions"], "disabled");
}
```

Run: `cargo nextest run -p scorpio-reporters json_reporter_keeps_v2_with_account_positions`
Expected: PASS.

- [ ] **Step 6: Run the reporter integration suite (section ordering didn't break)**

Run: `cargo nextest run -p scorpio-reporters`
Expected: PASS (existing `terminal.rs` ordering tests still pass — the new section sits right after Enrichment Data and is skipped when Disabled).

- [ ] **Step 7: Lint, format, commit**

```bash
cargo clippy -p scorpio-reporters --all-targets -- -D warnings && cargo fmt -- --check
git add crates/scorpio-reporters
git commit -m "feat(futu): surface account positions in terminal and JSON reports"
```

---

## Task 12: Live OpenD smoke test + operator docs

**Files:**
- Modify: `crates/scorpio-core/src/data/futu/client.rs` (an `#[ignore]` live test) OR create `crates/scorpio-core/src/data/futu/mod.rs` smoke test — colocate with the client.
- Modify: `README.md` (brief "Futu positions (optional)" note)

- [ ] **Step 1: Add the `#[ignore]` live smoke test**

In `crates/scorpio-core/src/data/futu/client.rs`, add a colocated test module (mirrors the SEC EDGAR `#[ignore]` convention `#[ignore = "requires live ... — run manually"]`):

```rust
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::config::FutuConfig;

    /// Manual connectivity spike. Requires a running OpenD on 127.0.0.1:11111
    /// with API encryption disabled. Prints the resolved state so the operator
    /// can confirm JSON mode, the no-encryption handshake, and field casing.
    /// Sanitize any captured payloads before turning them into fixtures.
    #[tokio::test]
    #[ignore = "requires live Futu OpenD on 127.0.0.1:11111 — run manually"]
    async fn futu_live_account_positions_smoke() {
        let cfg = FutuConfig { enabled: true, account_id: None, timeout_secs: 10 };
        let client = FutuClient::new(&cfg);
        let symbol = Symbol::parse("AAPL").unwrap();
        let state = client.account_positions(Some(&symbol)).await;
        // Print, don't hard-assert specific holdings — this is a capture spike.
        println!("live account positions: {state:#?}");
        match state {
            AccountPositionsState::Available(_) | AccountPositionsState::Unavailable(_) => {}
            AccountPositionsState::Disabled => panic!("enabled client must not be Disabled"),
        }
    }
}
```

- [ ] **Step 2: Verify it compiles and is skipped by default**

Run: `cargo nextest run -p scorpio-core --all-features futu_live_account_positions_smoke`
Expected: the test is listed as skipped (ignored); 0 run. Compilation succeeds.

- [ ] **Step 3 (manual, optional): Run it against live OpenD and reconcile constants**

Only if OpenD is running locally:
Run: `cargo nextest run -p scorpio-core --all-features --run-ignored=only -E 'test(futu_live_account_positions_smoke)'`
Expected: prints `Available(...)` or `Unavailable(reason)`. If it errors at InitConnect, reconcile `PACKET_ENC_ALGO_NONE` / `CLIENT_VER` (Task 6) and the message field casing (Task 6) against the real payloads, then sanitize captures into the synthetic fixtures.

- [ ] **Step 4: Document the optional feature**

In `README.md`, under the configuration/optional-features area, add a short note:

```markdown
### Futu positions (optional, read-only)

Set `SCORPIO__FUTU__ENABLED=true` (default off) to let the Fund Manager see your
current Real-account holdings for the analyzed symbol's market. Requires a local
Futu OpenD reachable on `127.0.0.1:11111` with **API encryption disabled**. The
integration is strictly read-only (positions only; never unlocks trading), but
enabled account context is included in the Fund Manager prompt sent to your
configured LLM provider and may be persisted in local run snapshots. When disabled
or unavailable, account-position text is omitted from the Fund Manager prompt and
analysis behaves exactly as before.
```

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/data/futu/client.rs README.md
git commit -m "feat(futu): add ignored live OpenD smoke test and operator docs"
```

---

## Task 13: Full verification gate

**Files:** none (verification only)

- [ ] **Step 1: Format check**

Run: `cargo fmt -- --check`
Expected: clean (no diff).

- [ ] **Step 2: Clippy (warnings = errors)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Full test suite (matches CI)**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`
Expected: PASS. The `--ignored` live smoke test does not run.

- [ ] **Step 4: Smoke-run the CLI with the feature off (behavioral equivalence)**

Run: `SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze AAPL`
Expected: runs as before; the terminal report shows no "Account Positions" section (Disabled is omitted). This confirms default-off behavioral equivalence.

- [ ] **Step 5: Knowledge consolidation (per CLAUDE.md)**

After the branch is green, run `/ce-compound` to capture any non-obvious learnings (e.g. the OpenD frame layout, the `camelCase` + `*ID` serde rename gotcha, the additive-field-keeps-schema-v2 precedent).

---

## Self-review notes (coverage map against the spec)

- **Goal 1 (JSON wire, no protobuf):** Task 5 (frame), Task 6 (JSON bodies). `sha1` added in Task 1; no `prost`/`build.rs`.
- **Goal 2 (one Real, market-matched account, read-only):** Task 7 `select_account` (`trd_env == Real`, market auth), Task 6 only sends 1001/2001/2102, `TRD_ENV_REAL` hardcoded; no unlock/order protocols anywhere.
- **Goal 3 (held + portfolio overview):** Task 3 `held_position` + `top_holdings`; Task 9 prompt render; Task 11 report.
- **Goal 4 (three states, failure non-fatal):** Task 3 `AccountPositionsState`; Task 8 `FutuClient::account_positions` maps every error to `Unavailable`, disabled → `Disabled`.
- **Goal 5 (default-off):** Task 2 `FutuConfig` default-off; Task 10 disabled short-circuits with no socket.
- **Optionality contract (4 rows):** prompt render branches (Task 9) + client states (Task 8) + select/assemble empty-vs-unavailable (Task 7, decision #1).
- **Architecture (lazy in FundManagerTask):** Task 10 inserts the fetch between `load_state` and `run_fund_manager`; PreflightTask preserve-list untouched.
- **Invariants:** pack-owned prompts (Task 9 edits `.md`, substitution in `prompt.rs`); read-only/real-only (Tasks 6–8); local plaintext only (`ENDPOINT` hardcoded, Task 8); dual-risk contract untouched (no edits to validation.rs; Task 10 only adds a field write).
- **Domain & state types:** Task 3 (exact field list from spec); `#[serde(default)]` root field Task 4.
- **Data-at-rest policy:** `account_label` redacted via `redact_account_id` (Task 7); privacy test (Task 4); raw `retMsg` never surfaced (Task 6 `check_envelope`).
- **Config:** Task 2 (no host/port/trd_env; hardcoded constants in Tasks 6 and 8).
- **Testing strategy (8 items):** frame codec (Task 5), JSON messages incl. string-or-number u64 (Task 6), `assemble_snapshot`/selection (Task 7), prompt rendering 4 branches (Task 9), Fund Manager acceptance via captured system prompt (Task 10), client sequencing via `MockFutuConn` (Task 8), persistence/privacy (Task 4), live `#[ignore]` smoke (Task 12).
- **Dependencies:** `sha1` (Task 1); tokio net already present; no protobuf.
- **Open questions:** resolved up front (empty-vs-unavailable, schema bump, plRatio units).
