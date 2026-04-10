# CLI Design

**Date:** 2026-04-10
**Status:** Approved

---

## Overview

Add a proper CLI to Scorpio Analyst using `clap` + `inquire`. Two user-facing subcommands:
`analyze` to run the analysis pipeline and `setup` to configure `~/.scorpio-analyst/config.toml`
interactively. The project-level `config.toml` is deprecated in favour of the user-level config
file.

---

## Command Surface

```
scorpio analyze <SYMBOL>    # Run full 5-phase analysis pipeline for a ticker symbol
scorpio setup               # Interactive config wizard — create or update ~/.scorpio-analyst/config.toml
scorpio help                # Show all subcommands and their descriptions (alias for --help)
```

### `scorpio analyze <SYMBOL>`

- `SYMBOL` is a required positional argument (e.g. `AAPL`, `NVDA`, `BTC-USD`).
- On startup, calls `Config::load()`. If `~/.scorpio-analyst/config.toml` is missing or any
  required field is absent, prints to stderr and exits with code 1:
  ```
  ✗ Config not found or incomplete. Run `scorpio setup` to configure your API keys and providers.
  ```
- If config is valid, proceeds with the existing pipeline exactly as today. The symbol is
  passed directly to `TradingState::new()` from the CLI argument.
  - `TradingConfig.asset_symbol` is **removed** from `config.rs` — it is superseded by the CLI
    argument and must not appear in `~/.scorpio-analyst/config.toml`. Note: `TradingState.asset_symbol`
    is runtime state and is unchanged.
  - Symbol validation (currently in `Config::validate()`) moves to the `analyze` subcommand
    handler in `cli/analyze.rs`.
- Target date defaults to today (current behaviour unchanged).
- No additional flags in scope.

### `scorpio setup`

Interactive wizard that writes `~/.scorpio-analyst/config.toml`. Re-runnable: existing values
are pre-filled so pressing Enter keeps them; typing replaces them. Creates
`~/.scorpio-analyst/` if it does not exist.

### `scorpio help`

Delegates to `clap`'s built-in long-help output, printing all subcommands with their
one-line descriptions. `clap` also provides `--help` / `-h` on every subcommand automatically.

---

## Setup Wizard — Step-by-Step

### Step 1 — Finnhub API key

```
Finnhub provides fundamental data, earnings, and company news.
Get your free key at: https://finnhub.io/dashboard

Finnhub API key:
```

- Uses `inquire::Password` (input masked).
- On re-run: existing value shown as `[already set — press Enter to keep]`; empty input
  keeps the existing value.
- Validator: rejects empty string on first-time setup (when no existing value).

### Step 2 — FRED API key

```
FRED provides macro indicators (CPI, inflation, interest rates).
Get your free key at: https://fredaccount.stlouisfed.org/apikeys

FRED API key:
```

- Same pattern as Step 1.

### Step 3 — LLM provider API keys

```
Which LLM providers do you want to configure?
  [x] OpenAI
  [ ] Anthropic
  [ ] Gemini
  [ ] OpenRouter
```

- Uses `inquire::MultiSelect`. Providers that already have a saved key are pre-checked.
- For each selected provider, prompts for its API key via `inquire::Password`.
- Providers that are deselected are left untouched in config (their keys are not deleted).
- Guard: if no providers end up with a saved key after this step, prints
  `"At least one LLM provider is required."` and re-runs Step 3.

### Step 4 — Provider and model routing

```
Quick-thinking provider (used by analyst agents):
> OpenAI

Quick-thinking model: gpt-4o-mini

Deep-thinking provider (used by researcher, trader, and risk agents):
> OpenAI

Deep-thinking model: o3
```

- Uses `inquire::Select` filtered to providers that have a saved key — prevents selecting a
  provider with no key.
- Model name uses `inquire::Text` with the current saved value pre-filled.
- Validators: model name must not be empty.

### Step 5 — LLM health check

```
Sending "Hello" to deep-thinking provider (openai / o3)...
✓ Health check passed.
```

- Sends a single `"Hello"` prompt through the existing `create_completion_model` +
  `prompt_with_retry` path using the configured deep-thinking provider and model.
- On failure:
  ```
  ✗ Health check failed: <error message>
  Save config anyway? (y/N)
  ```
  Default is No. Choosing Yes saves and exits. Choosing No discards changes and exits.

---

## Source Layout

```
src/
├── main.rs                      # clap dispatch: parse Cli, route to analyze or setup
├── cli/
│   ├── mod.rs                   # pub mod analyze; pub mod setup;
│   ├── analyze.rs               # analyze subcommand: config validation + pipeline invocation
│   └── setup/
│       ├── mod.rs               # run_setup() top-level orchestrator
│       ├── steps.rs             # one fn per wizard step; pure fns over PartialConfig
│       └── config_file.rs       # load_user_config() + save_user_config()
├── config.rs                    # Config::load() search path updated (no other changes)
```

No other source files are modified beyond `main.rs` and `config.rs`.

---

## Config Loading

### New search order in `Config::load()`

1. `~/.scorpio-analyst/config.toml` — user config (primary source)
2. Environment variables (`SCORPIO__*` prefix) — CI/CD and shell overrides
3. Compiled-in defaults — debate rounds, timeouts, etc.

Project-level `config.toml` is **removed from the search path**. It continues to exist on disk
for now but is no longer consulted. It will be deleted in a future cleanup.

### `PartialConfig` struct

A new `PartialConfig` struct in `cli/setup/config_file.rs` holds only the values the wizard
manages:

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PartialConfig {
    pub finnhub_api_key: Option<SecretString>,
    pub fred_api_key: Option<SecretString>,
    pub openai_api_key: Option<SecretString>,
    pub anthropic_api_key: Option<SecretString>,
    pub gemini_api_key: Option<SecretString>,
    pub openrouter_api_key: Option<SecretString>,
    pub quick_thinking_provider: Option<String>,
    pub quick_thinking_model: Option<String>,
    pub deep_thinking_provider: Option<String>,
    pub deep_thinking_model: Option<String>,
}
```

`Config::load()` reads `PartialConfig` from `~/.scorpio-analyst/config.toml`, then merges it
into the full `Config` struct alongside compiled-in defaults and env var overrides.

### Atomic writes

`save_user_config` writes to `~/.scorpio-analyst/config.toml.tmp` first, then renames to
`config.toml`. If rename fails, the original file is untouched and the error is surfaced to
the user.

---

## Error Handling

| Scenario | Behaviour |
|---|---|
| Config missing / incomplete on `analyze` | Print message → exit 1 |
| User hits Ctrl-C / ESC in `setup` | Print `"Setup cancelled."` → exit 0 |
| Atomic write rename fails | Surface error, original config untouched |
| Health check fails | Prompt to save anyway (default No) |
| No LLM providers configured in Step 3 | Re-run Step 3 with error message |
| Step 4 provider not in saved-key list | Provider not shown in Select options |

---

## New Dependencies

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
inquire = "0.7"
```

`secrecy` is already present but must have the `serde` feature enabled for `SecretString` to
derive `Serialize`/`Deserialize`:

```toml
secrecy = { version = "0.10", features = ["serde"] }
```

`toml` is already present with no changes required.

---

## Testing

- **`config_file.rs`** — unit tests using `tempfile` (already a dev dep): write a `PartialConfig`,
  reload it, assert round-trip fidelity. Test atomic write behaviour.
- **`analyze.rs`** — unit test that `Config::load()` returns a descriptive error when
  `~/.scorpio-analyst/config.toml` is absent and no env vars provide required fields.
- **`setup/steps.rs`** — each step function is a pure transformation over `PartialConfig`.
  Unit-testable without interactive I/O by calling step fns directly with fixture inputs.
- Interactive `inquire` prompts are not tested in CI (not automatable); covered by the pure
  step functions above.

---

## Out of Scope

- `--date` flag on `scorpio analyze` (deferred)
- `scorpio backtest` subcommand (separate capability)
- Per-agent provider overrides (listed in `docs/future-enhancements.md`)
- Hidden stdio MCP entrypoint in `src/cli/mod.rs` (Copilot tool-calling, deferred)
- TUI and GUI phases (Phase 2 / Phase 3)
