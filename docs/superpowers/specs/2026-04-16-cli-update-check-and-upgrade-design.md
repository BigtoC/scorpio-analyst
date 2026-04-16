# CLI Update Check & Self-Upgrade

**Date:** 2026-04-16
**Status:** Approved

## Goal

Enable the CLI to detect new versions from GitHub releases and notify users on every invocation, plus provide a `scorpio upgrade` subcommand for in-place self-update.

## Decisions

| Decision            | Choice                       | Rationale                                                      |
|---------------------|------------------------------|----------------------------------------------------------------|
| When to check       | Every CLI invocation         | User always knows when updates are available                   |
| Network failure     | Silent skip                  | Never block or annoy the user due to connectivity              |
| Blocking behavior   | Non-blocking background task | Zero added latency to normal CLI usage                         |
| Notice style        | Colored box (npm-style)      | Visually distinct, hard to miss                                |
| Upgrade mechanism   | `self_update` crate (v0.44+) | Actively maintained, batteries-included GitHub release updater |
| Self-update command | `scorpio upgrade`            | Users can update without leaving the CLI                       |

## Version Check (Background, Non-Blocking)

`main.rs` becomes `#[tokio::main] async fn main()`. Before dispatching any subcommand, a `tokio::spawn` fires a lightweight GitHub API call.

**Flow:**

1. Spawn a background task that calls `self_update::backends::github::Update::configure()` with `.repo_owner("BigtoC").repo_name("scorpio-analyst")`, then calls `.get_latest_release()` to fetch the latest tag.
2. Compare the fetched tag against `env!("CARGO_PKG_VERSION")` using `semver`.
3. Send the result through a `tokio::sync::oneshot` channel.
4. After the subcommand completes, check the oneshot receiver:
   - If a newer version exists → print the update notice.
   - If up-to-date, check failed, or still pending → do nothing.

**Failure handling:** Any error (network, JSON parse, timeout, GitHub rate-limit) is silently swallowed. The CLI proceeds as normal.

## Update Notice UX

Printed to **stderr** (so piped stdout is clean), using the existing `colored` crate:

```
╭──────────────────────────────────────────────────╮
│                                                  │
│   Update available: v0.2.0 → v0.3.0              │
│   Run `scorpio upgrade` to update                │
│                                                  │
╰──────────────────────────────────────────────────╯
```

**Placement:** After the subcommand finishes — the last thing the user sees.

**Suppression:** Two mechanisms:
- `--no-update-check` global flag on the `Cli` struct
- `SCORPIO_NO_UPDATE_CHECK=1` environment variable

Either one disables the background check entirely. Useful for CI/CD and scripting.

## `scorpio upgrade` Subcommand

New `Commands::Upgrade` variant. When invoked:

1. Print current version.
2. Configure `self_update`:
   ```rust
   self_update::backends::github::Update::configure()
       .repo_owner("BigtoC")
       .repo_name("scorpio-analyst")
       .bin_name("scorpio")
       .show_download_progress(true)
       .current_version(cargo_crate_version!())
       // target auto-detected by self_update
       .build()?
       .update()?
   ```
3. Print result:
   - "Already up to date (v0.2.0)" if no newer version.
   - "Updated successfully: v0.2.0 → v0.3.0" on success.

### Release Asset Naming Convention

`self_update` expects release assets named: `{bin_name}-{target}.{archive_ext}`

Examples:
- `scorpio-x86_64-unknown-linux-gnu.tar.gz`
- `scorpio-aarch64-unknown-linux-gnu.tar.gz`
- `scorpio-aarch64-apple-darwin.tar.gz`
- `scorpio-x86_64-apple-darwin.tar.gz`
- `scorpio-x86_64-pc-windows-msvc.zip`

The CI release workflow must produce archives matching this pattern. This is already aligned with the targets in `install.sh`.

### Required `self_update` Features

- `archive-tar` — for `.tar.gz` assets (Linux/macOS)
- `compression-flate2` — gzip decompression
- `archive-zip` — for `.zip` assets (Windows)
- `compression-zip-deflate` — ZIP decompression

Windows is part of the shared release asset contract now, so ZIP support is required rather than deferred.

## Source Layout

### New file: `src/cli/update.rs`

Contains all version-check and upgrade logic:

- `check_latest_version() -> Option<String>` — async fn that hits GitHub API, returns the newer version tag or `None` if up-to-date. Swallows all errors.
- `print_update_notice(current: &str, latest: &str)` — renders the colored box to stderr.
- `run_upgrade() -> anyhow::Result<()>` — the `scorpio upgrade` handler that calls `self_update` to perform the actual update.

### Modified files

| File             | Change                                                                                                                                            |
|------------------|---------------------------------------------------------------------------------------------------------------------------------------------------|
| `src/cli/mod.rs` | Add `pub mod update;`, add `Upgrade` variant to `Commands`, add `--no-update-check` global flag                                                   |
| `src/main.rs`    | Convert to `#[tokio::main] async`, spawn background version check via oneshot, await result after subcommand dispatch, print notice if applicable |
| `Cargo.toml`     | Add `self_update = { version = "0.44", default-features = false, features = ["archive-tar", "compression-flate2", "archive-zip", "compression-zip-deflate"] }` |

### Untouched files

No changes to `config.rs`, `constants.rs`, `analyze.rs`, `setup/`, agents, pipeline, data, indicators, providers, or workflow modules.

## Testing

- **Unit tests in `update.rs`:**
  - `semver` comparison logic (current < latest, current == latest, current > latest, malformed tags)
  - Notice formatting produces expected box layout
  - Suppression flag/env var prevents check from running
- **Integration:** Manual verification against actual GitHub releases. The `self_update` crate itself is well-tested for the download/replace flow.
