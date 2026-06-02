# Futu Position Integration — Design

**Date:** 2026-06-01
**Status:** Approved (pending implementation plan)
**Scope:** Add an optional, **read-only, real-account** Futu OpenD client that fetches the user's current account positions and feeds them to the Fund Manager. When OpenD is enabled, reachable on `127.0.0.1:11111`, and returns usable account-position context for the analyzed symbol's market, the Fund Manager factors existing holdings and portfolio concentration into its final suggestion. When disabled or unavailable, the pipeline remains behaviorally equivalent to today: no account data is fetched, decision policy and routing are unchanged, and any account-position prompt/report text is explicitly neutral metadata.

**Motivation:** The Fund Manager currently decides in a vacuum about the user's actual book — it cannot see whether the user already holds the analyzed symbol, at what cost basis, or how concentrated the portfolio already is. Feeding live read-only account positions lets the final decision reason about add/trim/hold versus a real holding and size relative to existing exposure. The user already runs OpenD locally, so this is a local, low-latency lookup.

**V1 success:** With `futu.enabled = true`, OpenD reachable on `127.0.0.1:11111`, and a matching real account snapshot available, the Fund Manager's `suggested_position` and `entry_guidance` reflect the user's current holding in the analyzed symbol and overall portfolio concentration. With `futu.enabled = false` (default), or OpenD down/unavailable, the decision behavior is equivalent to today.

## Goals

1. Talk to OpenD directly from Rust over its TCP framing protocol, using **JSON message bodies** (no protobuf codegen, no `build.rs`, no `prost`).
2. Fetch positions for exactly one account — the **Real** (`TrdEnv_Real`) account whose authorized market matches the analyzed symbol's market — strictly read-only (no trade-unlock, no order capability, no trade password).
3. Surface to the Fund Manager **both** the held position in the analyzed symbol (qty, cost basis, mark, P/L) **and** a single-currency portfolio overview (total value, top holdings, concentration %).
4. Degrade gracefully through three explicit states (`Disabled` / `Unavailable(reason)` / `Available`). Any failure — disabled, connection refused, init rejected, parse error, timeout — leaves the Fund Manager proceeding normally with no account-position penalty.
5. Default-off. Existing configs and runs are unchanged.

## Non-goals (v1)

- **Paper-account mode.** `trd_env` is hardcoded to **Real** (`TrdEnv_Real`) for v1. There is no config or env var to select simulated trading.
- **Order placement / trade unlock.** Strictly position queries (`GetAccList`, `GetPositionList`). `Trd_UnlockTrade` is never called; no trade password is read or stored.
- **OpenD encryption.** v1 assumes OpenD's local API encryption is **off** (the localhost default). If an encryption key is configured, `InitConnect` would need an RSA/AES handshake — out of scope; the feature reports `Unavailable` and documents "disable OpenD encryption to use this feature."
- **Remote OpenD.** v1 connects only to `127.0.0.1:11111`. Host/port configuration is intentionally out of scope while encryption is unsupported.
- **Protobuf wire format.** JSON body only (`proto_fmt_type = 1`). The frame codec is format-agnostic, so a future protobuf swap is contained, but no protobuf machinery is built up-front.
- **Markets beyond US equity.** v1 maps US equities → `TrdMarket_US`. HK/CN/futures are a clean extension via the market-mapping function but are not wired in v1.
- **Cross-currency portfolio aggregation.** By selecting a single market-matched account, all positions share one currency; no FX conversion is performed. (This is why account scope is single-account, not "all accounts.")
- **Persistent connection / keep-alive / notifications.** Each fetch is a one-shot connect → init → query → close. No heartbeat loop, `recvNotify = false`.
- **Feeding positions to any agent other than the Fund Manager.** Analysts, researchers, trader, and risk agents do not see account data in v1.
- **Multi-account aggregation or account-picker UX.** Single account, chosen by market-match or an explicit configured `account_id`.

## Optionality contract

| Situation                                                       | `state.account_positions`                                                                              | Fund Manager behavior                                                                             |
|-----------------------------------------------------------------|--------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------|
| `futu.enabled = false` (default)                                | `Disabled`                                                                                             | No fetch; prompt/report may say lookup is not enabled, and the model proceeds normally            |
| Enabled, OpenD unreachable / init fails / parse fails / timeout | `Unavailable(reason)`                                                                                  | Prompt says "account positions unavailable" with a sanitized reason; decides as today, no penalty |
| Enabled, reachable, no matching real account or zero positions  | `Available(snapshot)` with empty positions (or `Unavailable` with a clear reason — see Open Questions) | Prompt says "no holdings found"; decides as today                                                 |
| Enabled, reachable, positions present                           | `Available(snapshot)`                                                                                  | Factors held position + concentration into `suggested_position` and `entry_guidance`              |

## Architecture

The Fund Manager is the sole consumer, so positions are fetched **lazily inside `FundManagerTask`** (which already holds `Arc<Config>`) rather than threaded through `run_analysis_cycle`'s prefetch fan-out and `PreflightTask`'s preserve-list. The task fetches, writes `state.account_positions`, then runs the agent; the terminal report reads the field back from the final state.

```
FundManagerTask::run(context)
   │  load TradingState from context
   │
   ├── if config.futu.enabled:
   │      │  FutuClient::new(config.futu)              // infallible: stores config only
   │      │  fetch_account_snapshot(symbol)            // timeout-bounded
   │      │     ├── TcpStream::connect(127.0.0.1:11111)
   │      │     ├── InitConnect      (proto 1001)      // handshake, plaintext
   │      │     ├── GetAccList        (proto 2001)      // filter: trdEnv==Real ∧ market(symbol) authorized
   │      │     └── GetPositionList   (proto 2102)      // header { trdEnv=Real, accID, trdMarket }
   │      │           ▼
   │      │     assemble_snapshot(accounts, positions, symbol)   // pure: pick acct, match held, total, concentration
   │      │           ▼
   │      │  Ok(snapshot)  → state.account_positions = Available(snapshot)
   │      │  Err(reason)   → state.account_positions = Unavailable(reason)   // sanitized, non-fatal
   │      └── else:           state.account_positions = Disabled
   │
   ├── run_fund_manager(&mut state, config, context)   // prompt.rs renders {account_positions}
   │
   ├── save FundManager snapshot (now includes account_positions)
   └── route to Auditor / End  (unchanged)
```

**Invariants preserved:**

- **Default-off optionality.** `futu.enabled` defaults to `false`; with it off there is no socket activity, `account_positions = Disabled`, and the rendered prompt/report only include neutral disabled metadata that instructs the model to proceed normally.
- **Read-only & real-account-only.** Only `InitConnect`, `GetAccList`, `GetPositionList` are ever sent. `trdEnv` is the hardcoded Real constant everywhere it appears (account filter + `TrdHeader`).
- **Local plaintext only.** Because OpenD encryption is out of scope, v1 hardcodes `127.0.0.1:11111` and rejects remote host configuration rather than sending account data over plaintext networks.
- **Pack-owned prompts.** The `{account_positions}` placeholder and its instruction live in each pack's `fund_manager.md` (equity + ETF), substituted in `prompt.rs`. No prompt text is hardcoded in agent logic.
- **Failure is never fatal.** The fetch is wrapped so every error path resolves to `Unavailable(reason)`; the Fund Manager always runs.
- **The dual-risk escalation contract is untouched.** Account positions are additive context only; the first-line rationale-prefix rules (`Dual-risk escalation: upheld/deferred/overridden because …`) and all existing `ExecutionStatus` validation are unchanged.
- **Mock at the right seam.** Framing and snapshot assembly are pure functions tested directly; socket sequencing is tested behind a one-method transport trait (mirrors the existing `EdgarHttp` seam). No `#[cfg(test)]` branches in production methods.

## Module layout

```
crates/scorpio-core/src/data/futu/
├── mod.rs        # pub use re-exports (FutuClient); FutuConn transport trait; fetch_account_snapshot orchestration
├── frame.rs      # OpenD 44-byte header encode/decode + body SHA1 + serial counter (pure, socket-free)
├── messages.rs   # serde structs for InitConnect / GetAccList / GetPositionList C2S/S2C/Response bodies
├── client.rs     # FutuClient over tokio::net::TcpStream; impl FutuConn for the live transport
└── select.rs     # assemble_snapshot(): pure account-pick + held-match + total + concentration

crates/scorpio-core/src/state/
└── account.rs    # AccountPosition, PositionSide, AccountSnapshot, AccountPositionsState (new)
```

## OpenD wire protocol (implementation reference)

All multi-byte integers are **little-endian**. The frame is identical for every request/response; only the body changes.

### Frame header — 44 bytes

| Offset | Size | Field                | Value                                 |
|--------|------|----------------------|---------------------------------------|
| 0      | 2    | `szHeaderFlag`       | ASCII `"FT"` (`0x46 0x54`)            |
| 2      | 4    | `nProtoID` (u32)     | protocol id (e.g. 1001)               |
| 6      | 1    | `nProtoFmtType` (u8) | **1 = JSON**                          |
| 7      | 1    | `nProtoVer` (u8)     | 0                                     |
| 8      | 4    | `nSerialNo` (u32)    | monotonic per connection, starts at 1 |
| 12     | 4    | `nBodyLen` (u32)     | byte length of the JSON body          |
| 16     | 20   | `arrBodySHA1`        | SHA-1 digest of the JSON body bytes   |
| 36     | 8    | `arrReserved`        | zeros                                 |

Response framing is the same; the client reads 44 header bytes, rejects any `nBodyLen` above the configured maximum, then reads the body and verifies the SHA-1. Each response must match the requested `nProtoID`, `nSerialNo`, expected format/version, and success envelope shape; any mismatch maps to `Unavailable`.

### Protocols used

| Name                  | `nProtoID` | Purpose                                                    |
|-----------------------|------------|------------------------------------------------------------|
| `InitConnect`         | 1001       | Handshake; must precede all trade calls                    |
| `Trd_GetAccList`      | 2001       | Enumerate accounts (to pick the real, market-matched one)  |
| `Trd_GetPositionList` | 2102       | Fetch positions for the chosen account                     |

Each body is `{"c2s": { ... }}` on request and `{"retType": int, "retMsg": string, "errCode": int, "s2c": { ... }}` on response. `retType == 0` is success; anything else maps to a sanitized `Unavailable(reason)` category. Raw OpenD `retMsg` values may be retained only in redacted debug logs, never in persisted state or terminal output.

### Message fields (JSON, camelCase mirrors of the proto field names)

**InitConnect 1001 — C2S:** `clientVer: i32` (e.g. `100`), `clientID: string` (`"scorpio-analyst"`), `recvNotify: false`, `packetEncAlgo: i32` (no-encryption value — confirm in spike), optional `programmingLanguage: "Rust"`.
**S2C (read):** `connID`, `loginUserID`, `keepAliveInterval` (ignored — one-shot).

**Trd_GetAccList 2001 — C2S:** `userID: u64` (from InitConnect `loginUserID`, or `0`).
**S2C (read):** `accList: [{ trdEnv: i32, accID: u64, trdMarketAuthList: [i32], accType, simAccType, … }]`.

**Trd_GetPositionList 2102 — C2S:** `header: { trdEnv: i32 (Real), accID: u64, trdMarket: i32 }`, optional `filterConditions: { codeList: [symbol_code], idList: [u64], ... }`, optional `refreshCache: false`, optional `assetCategory`, optional `currency`.
**S2C (read):** `positionList: [{ positionSide: i32, code: string, name: string, qty: f64, canSellQty: f64, price: f64, costPrice: f64, val: f64, plVal: f64, plRatio: f64, currency: i32, … }]`.

For OpenD JSON wire structs, handle protobuf `uint64`/`int64` fields consistently in request and response bodies. Fields such as `userID`, `accID`, and `idList` must serialize correctly and deserialize from either JSON numbers or quoted numeric strings before converting to domain `u64` values.

### Enum mappings (`Trd_Common`)

- **`TrdEnv`**: `Simulate(Paper) = 0`, `Real = 1`. v1 hardcodes `1`.
- **`TrdMarket`**: `Unknown = 0`, `HK = 1`, `US = 2`, `CN = 3`, `Futures = 5`, … v1 maps US equities → `2`.
- **`PositionSide`**: `Long = 0`, `Short = 1`.
- **`Currency`**: `Unknown = 0`, `HKD = 1`, `USD = 2`, `CNH = 3`, `JPY = 4`, `SGD = 5`, `AUD = 6`, … rendered to a string label for the prompt/report.

### Account selection

From `accList`, keep accounts where `trdEnv == 1` (Real) **and** `trdMarketAuthList` contains `market(symbol)`. If `config.futu.account_id` is set, select that `accID` instead (it must be a Real account; a mismatch surfaces as `Unavailable`). If multiple match, take the first. If none match, the result is `Unavailable("no real account for <market>")` (see Open Questions for the empty-vs-unavailable nuance).

### Held-position matching

Within the selected account, the held position is the one whose `code` equals the analyzed ticker after normalization (strip any `"US."`/market prefix, uppercase). `AccountSnapshot::held_position(symbol)` computes this on demand — the held position is **not** stored as a duplicate field. Portfolio total is `Σ position.val` (single currency); per-position concentration is `val / total`.

## Domain & state types (`state/account.rs`)

```rust
pub enum PositionSide { Long, Short }

pub struct AccountPosition {
    pub code: String,
    pub name: String,
    pub qty: f64,
    pub can_sell_qty: f64,
    pub cost_price: Option<f64>,
    pub current_price: Option<f64>,
    pub market_value: Option<f64>,   // `val`
    pub pl_ratio: Option<f64>,
    pub pl_val: Option<f64>,
    pub currency: String,            // mapped from Currency enum
    pub side: PositionSide,
}

pub struct AccountSnapshot {
    pub account_label: Option<String>, // redacted/hash label only; do not persist raw accID
    pub market: String,              // e.g. "US"
    pub currency: String,            // single currency for the account
    pub total_market_value: Option<f64>,
    pub positions: Vec<AccountPosition>,
}
impl AccountSnapshot {
    pub fn held_position(&self, symbol: &Symbol) -> Option<&AccountPosition>;
}

pub enum AccountPositionsState {
    Disabled,                  // feature off (default)
    Unavailable(String),       // enabled but fetch failed / OpenD down / no account
    Available(AccountSnapshot),
}
impl Default for AccountPositionsState { fn default() -> Self { Self::Disabled } }
```

All types derive `Debug, Clone, PartialEq, Serialize, Deserialize` (needed for the context blob and snapshot persistence). Note the snapshot records `market`/`currency` (read by the report to label positions as account context) — these are read, not write-only. Persisted state must not store raw account IDs; use a redacted or hashed `account_label` only if the report needs to distinguish accounts.

Because real account positions are sensitive local data, v1 must define a data-at-rest policy before implementation lands:

- Phase snapshots and context blobs may persist holdings needed to reproduce/report the run, but raw OpenD account IDs and raw protocol payloads must not be persisted.
- Snapshot storage remains local to the existing Scorpio data paths; implementation must verify user-only file permissions where the snapshot store creates files/directories.
- Tests must assert persisted snapshots do not contain raw `accID` values or unsanitized OpenD error/payload fragments.

On `TradingState`, one new root field:

```rust
#[serde(default)]
pub account_positions: AccountPositionsState,
```

`#[serde(default)]` keeps older snapshots loadable (they deserialize to `Disabled`). The purpose-built 3-state enum is preferred over reusing `EnrichmentState<T>` to avoid coupling account data to enrichment staleness semantics; during implementation, if `EnrichmentState`/`EnrichmentStatus` proves clean and gives free report-status rendering, it may be reused instead — this is the only representation detail allowed to flex, and it does not change behavior.

Implementation must update the complete `TradingState` wire path: `TradingState`, `TradingStateWire`, `From<TradingStateWire>`, `TradingState::new`, plus legacy-snapshot and round-trip tests.

> **Schema note:** `AccountPositionsState` is embedded in `TradingState`, which is persisted in phase snapshots. Because the FundManager snapshot now carries this field, confirm whether `THESIS_MEMORY_SCHEMA_VERSION` (currently 4) must bump per the snapshot-evolution rule. Since the field is `#[serde(default)]` and additive, a bump is likely unnecessary, but this must be verified against the snapshot load path during implementation.

## Configuration (`FutuConfig`, added as `config.futu`)

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct FutuConfig {
    #[serde(default)]
    pub enabled: bool,                       // SCORPIO__FUTU__ENABLED          (default false)
    #[serde(default)]
    pub account_id: Option<u64>,             // SCORPIO__FUTU__ACCOUNT_ID       (overrides market-match; must be Real)
    #[serde(default = "default_futu_timeout")]
    pub timeout_secs: u64,                   // SCORPIO__FUTU__TIMEOUT_SECS     (default 5)
}
impl Default for FutuConfig { /* enabled=false, account_id=None, timeout_secs=5 */ }
```

Added to top-level `Config` as `#[serde(default)] pub futu: FutuConfig`. Follows the `DataEnrichmentConfig` precedent: all fields `#[serde(default)]`, default-off, `SCORPIO__FUTU__*` env mapping via the existing `__` separator. **There is no `host`, `port`, or `trd_env` field** — the endpoint is hardcoded to `127.0.0.1:11111` and the trading environment is hardcoded to Real in code (a `const FUTU_TRD_ENV_REAL: i32 = 1;` used by the account filter and `TrdHeader`).

## Fund Manager prompt integration

- `agents/fund_manager/prompt.rs` renders the `{account_positions}` placeholder from `state.account_positions`:
  - **Available + held:** `Account context (US, USD). You hold AAPL: 100 sh @ avg 150.00, mark 185.42, P/L +23.6% (+3,542 USD). Portfolio total 250,000 USD across 8 positions; top: AAPL 14%, MSFT 12%, NVDA 9%.`
  - **Available + not held:** `Account context (US, USD). You do NOT currently hold AAPL. Portfolio total 250,000 USD across 8 positions; top: …`
  - **Unavailable / Disabled:** `Account positions unavailable (<reason>).` / `Account position lookup is not enabled.`
- `analysis_packs/equity/prompts/fund_manager.md` and `analysis_packs/etf/prompts/fund_manager.md` each gain:
  - one input line: `- Account positions: {account_positions}`
  - one instruction: *"If account positions are provided, factor existing exposure into your decision — weigh add/trim/hold against the current holding and cost basis, and size relative to portfolio concentration; reflect this in `suggested_position` and `entry_guidance`. These holdings are read-only account context from local OpenD. If positions are unavailable or null, decide exactly as you otherwise would, with no penalty."*
- Both prompt contracts get a drift test asserting the placeholder and instruction are present (consistent with how the repo tests prompt contracts).

## Reporting

A compact **Account Context** line in the terminal report (near the Enrichment section), driven by `state.account_positions`:
- `Disabled` → omitted, or a single greyed "Account positions: not enabled."
- `Unavailable(reason)` → "Account positions: unavailable (<reason>)."
- `Available` → "Account positions (US/USD): hold AAPL 100 @ 150.00, P/L +23.6%; portfolio 250,000 USD / 8 positions." (held one-liner + portfolio total).

Minimal, mirrors how enrichment status is surfaced today; it exists so the user can see whether positions actually informed the decision.

## Testing strategy (per `mock-at-the-right-seam-not-in-production`)

1. **Frame codec (pure):** header encodes to exact expected bytes; header+body round-trips; SHA-1 matches a known vector; decode rejects bad magic and short/truncated buffers; serial increments; response validation rejects mismatched proto ID, serial, format/version, envelope shape, and oversized body length.
2. **JSON messages (pure serde):** serialize each C2S and deserialize each S2C against sanitized captured sample payloads (success and `retType != 0` error envelope), including string-or-number handling for `uint64`/`int64` fields.
3. **`assemble_snapshot` (pure):** market-matched account selection; `account_id` override; held-position match incl. `US.`-prefix normalization; portfolio total + concentration; not-held case; zero-account / zero-position cases.
4. **Prompt rendering (pure):** all four branches (available+held, available+not-held, unavailable, disabled) produce the expected text.
5. **Fund Manager acceptance scenarios:** deterministic model-stub tests assert that available+held, available+not-held, overweight/concentrated, unavailable, and disabled contexts produce the expected `suggested_position` / `entry_guidance` deltas or baseline-equivalent behavior.
6. **Client sequencing (trait seam):** a scripted in-memory `FutuConn` returns canned framed responses; assert connect→init→get-accounts→get-positions ordering, error short-circuiting, and that `GetPositionList` is sent with the Real `trdEnv`, selected `accID`, `filterConditions.codeList`, and `refreshCache` value. No live socket.
7. **Persistence/privacy tests:** snapshot round-trips and legacy snapshots cover `TradingStateWire`; persisted snapshots must not contain raw `accID` values, raw OpenD payloads, or unsanitized error fragments.
8. **Live OpenD smoke test:** `#[ignore]` integration test, run manually against the user's running OpenD during the spike, capturing real JSON payloads. Captures must be sanitized into synthetic fixtures before commit.

## Dependencies

- Add `sha1` (body checksum) — small, widely used; check/add the workspace pin.
- `tokio` net (`TcpStream`) — already present.
- **No `prost`, no `tonic`, no `build.rs`, no vendored `.proto` files.**

## Risks & first step (connectivity spike)

The **first implementation step is a connectivity spike** against the user's running OpenD, before building out the full client:

1. **JSON mode** — confirm OpenD accepts `nProtoFmtType = 1` for 1001/2001/2102 and capture the exact JSON field casing, `uint64`/`int64` representation, and the success `retType`. (The format flag exists precisely for JSON; if it were unexpectedly unsupported, pause implementation for an explicit fallback decision rather than silently adding protobuf/codegen.)
2. **`packetEncAlgo` / no-encryption handshake** — capture the exact `InitConnect` C2S that OpenD accepts on a plaintext localhost connection.
3. **`userID` for `GetAccList`** — confirm whether `0` works or the `loginUserID` from `InitConnect` is required.
4. **Encryption assumption** — if the user's OpenD has an encryption key set, `InitConnect` will not complete plaintext; v1 reports `Unavailable` and documents disabling OpenD encryption.

The spike's captured payloads become the basis for tests 2–3 only after sanitization. Replace account IDs, symbols, quantities, costs, market values, P/L, and raw error strings with synthetic values before committing fixtures.

## Decisions / judgment calls (settled)

1. **Lazy fetch in `FundManagerTask`**, not the cycle-start enrichment fan-out — surgical, single consumer, no Preflight preserve-list change.
2. **Real-account-only, hardcoded** (`TrdEnv_Real = 1`) — no `trd_env` config; the feature reads positions only and never unlocks trading.
3. **Strictly read-only** — only position queries; no `Trd_UnlockTrade`, no order protocols, no trade password.
4. **Purpose-built `AccountPositionsState` enum** over `EnrichmentState` (may reuse the latter if it proves clean during implementation; representation-only, no behavior change).
5. **US market focus first**; HK/CN/futures are a market-mapping extension.
6. **Encryption assumed off** on OpenD (localhost default), with `127.0.0.1:11111` hardcoded for v1.

## Open questions

- **Empty vs. Unavailable for "reachable but nothing found":** when OpenD is reachable but there is no real account for the symbol's market (or the matched account holds zero positions), should that be `Available(empty snapshot)` (prompt: "no holdings") or `Unavailable("no real account for US")`? Leaning `Available(empty)` when an account exists but holds nothing, and `Unavailable` when no matching account exists at all. To finalize during implementation once the spike shows real `GetAccList` output.
- **Schema version bump:** confirm `THESIS_MEMORY_SCHEMA_VERSION` does not need bumping given the additive `#[serde(default)]` field (likely no bump).
