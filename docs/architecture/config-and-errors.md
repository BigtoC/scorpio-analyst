# Configuration and Error Handling

## Configuration Loading Order

**Precedence (highest wins):** env vars > user file > compiled defaults.

1. `~/.scorpio-analyst/config.toml` — written by `scorpio setup`; flat `PartialConfig` (API keys + routing). Created with `0o600` permissions.
2. `.env` via `dotenvy` — local env overrides (git-ignored), loaded before the config crate pipeline.
3. `SCORPIO__*` environment variables — CI/CD overrides (double-underscore separator, e.g. `SCORPIO__LLM__MAX_DEBATE_ROUNDS=5`). Wins over the user file on any overlapping field.
4. `SCORPIO_*_API_KEY` env vars — secret injection; always override the corresponding key from the user file (with a `tracing::warn!` on collision).

The project-level `config.toml` at the repo root is **not read at runtime** — it is inert and kept only to avoid disrupting existing workspaces. See the deprecation notice inside the file itself.

API keys use a flat `SCORPIO_` prefix (single underscore) — see `.env.example`. The asset symbol is a CLI argument to `scorpio analyze <SYMBOL>`, not a config key.

### Storage Paths

- `~/.scorpio-analyst/config.toml` — user config (written by `scorpio setup`, `0o600`).
- `~/.scorpio-analyst/transcript_cache.db` — Alpha Vantage transcript cache (separate from snapshot DB). Overridable via `SCORPIO__STORAGE__TRANSCRIPT_CACHE_DB_PATH`. See `design-decisions.md` → *Transcript Cache* for migration boundary rules.

## Error Handling Pattern

- `thiserror` for the `TradingError` enum (typed variants: `AnalystError`, `RateLimitExceeded`, `NetworkTimeout`, `SchemaViolation`, `Rig`, `Config`, `Storage`, `GraphFlow`)
- `anyhow` for flexible context propagation within tasks
- Retry: exponential backoff (max 3 retries, base 500ms) for LLM calls via `RetryPolicy`
- Graceful degradation: 1 analyst failure continues with partial data; 2+ failures abort the cycle
- Per-analyst timeout: configurable via `analyst_timeout_secs` (default 3000s) via `tokio::time::timeout`
