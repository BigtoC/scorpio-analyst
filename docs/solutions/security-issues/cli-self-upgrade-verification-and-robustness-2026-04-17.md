---
title: "CLI Self-Upgrade Verification and Robustness"
date: 2026-04-17
category: docs/solutions/security-issues
module: cli
problem_type: security_issue
component: tooling
symptoms:
  - "`scorpio upgrade` could verify one archive and install different bytes from a second download path"
  - "Release asset selection could match the wrong artifact class, including `.sha256` sidecars"
  - "Update checks and upgrade fetches needed bounded timeout behavior under hanging network conditions"
  - "Malformed `SCORPIO_NO_UPDATE_CHECK` values could hard-fail CLI parsing"
  - "Update notices always rendered terminal styling even when stderr was not a TTY"
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - src/cli/update.rs
  - src/cli/mod.rs
  - src/main.rs
tags:
  - cli
  - self-upgrade
  - update-check
  - release-verification
  - sha256
  - reqwest
  - stderr
  - timeouts
---

# CLI Self-Upgrade Verification and Robustness

## Problem

The first implementation of the new background release check and `scorpio upgrade` command did not preserve a basic updater integrity invariant. The core flaw was that the CLI could verify one downloaded archive, then install bytes fetched again through a separate path, which broke the guarantee that the installed bytes were the verified bytes.

## Symptoms

- `scorpio upgrade` could report a verified checksum without guaranteeing that the installed binary came from the verified archive bytes.
- Asset matching in `src/cli/update.rs` was broad enough to risk selecting files like `scorpio-<target>.tar.gz.sha256` instead of the actual archive.
- Background update checks and foreground upgrade requests needed explicit bounded timeout behavior so they could not hang indefinitely under slow or stalled network conditions.
- `SCORPIO_NO_UPDATE_CHECK` could turn malformed env values into a hard clap parse error instead of safely suppressing the background check.
- Update notices always emitted box-drawing characters and ANSI color styling, even when stderr was redirected or otherwise non-interactive.

## What Didn't Work

- Verifying one archive and then delegating installation to a second `self_update` download path. That meant the checksum covered one set of bytes while the install step could consume another.
- Using loose target-based asset matching rather than exact archive names. Once `.sha256` sidecars were present next to release archives, similarity-based selection was no longer safe enough.
- Treating the timeout regression as a real-time stopwatch guarantee. Under a saturated runner, the code still enforced bounded behavior, but the wall-clock assertion was strict enough to fail spuriously.

## Solution

The fix was to make `src/cli/update.rs` own a single verified-archive pipeline from metadata fetch to final replacement, then harden the surrounding CLI behavior for automation and malformed runtime input.

### 1. Install only the bytes that were verified

`run_upgrade_with()` now fetches release metadata, selects the exact archive, downloads the sidecar and archive, verifies the archive bytes, and installs directly from those verified bytes:

```rust
let asset = select_release_asset(&release, &target)?;

let sidecar = updater.fetch_sidecar(&asset)?;
let archive = updater.fetch_archive(&asset)?;
verify_checksum(&archive, &sidecar)
    .with_context(|| format!("integrity check failed for {}", asset.name))?;

updater
    .install_archive(&asset.name, &archive, bin_name_in_archive(&asset.name))
    .with_context(|| format!("failed to install verified archive {}", asset.name))?;
```

`install_verified_archive()` now stages the verified archive locally, extracts the expected binary from that archive, and replaces the installed executable without triggering a second download.

### 2. Require exact archive names and bound network behavior

`select_release_asset()` now requires the exact archive filename for the current target, which means only `scorpio-{target}.tar.gz` or `scorpio-{target}.zip` can match. Because `.sha256` files do not end with those installable suffixes, they are no longer eligible install targets.

`GithubUpdater::build_http_client()` now applies both connect and overall request timeouts, and both background update checks and upgrade requests use that client path.

### 3. Make the surrounding CLI behavior fail safe

`src/cli/mod.rs` now uses clap's `FalseyValueParser`:

```rust
#[arg(
    long,
    global = true,
    env = "SCORPIO_NO_UPDATE_CHECK",
    value_parser = FalseyValueParser::new(),
    default_value_t = false
)]
pub no_update_check: bool,
```

False-ish values keep update checks enabled, but arbitrary malformed values no longer crash the CLI.

`try_show_update_notice()` now branches on `stderr.is_terminal()`. TTY users still get the boxed colored notice, while redirected stderr gets plain text.

The timeout regression test still checks that hanging release metadata requests fail in bounded time, but it no longer assumes a precise wall-clock cutoff from blocking HTTP timeout behavior.

## Why This Works

The central bug was a trust-boundary failure inside the updater flow. Verification and installation were separate operations over potentially different artifacts, so the updater could no longer claim that the installed bytes were the verified bytes. Moving to a staged verified-archive install path restores that invariant.

Exact filename matching closes the ambiguity introduced by archive sidecars. The updater no longer picks something that "looks close" to the target; it requires the exact installable filename for the platform.

Explicit timeout handling keeps the background and interactive flows bounded without changing their best-effort behavior. The env-var parser change and non-TTY notice fallback close the remaining robustness gaps for real CLI use: malformed suppression config no longer crashes commands, and automation no longer receives terminal-only output formatting.

This improves archive consistency and checksum enforcement inside the release pipeline, but it does not turn the updater into a fully authenticated supply-chain system by itself. The `.sha256` sidecar still comes from the same release channel; stronger authenticity guarantees would require signed metadata or signed assets.

## Prevention

- Treat release verification as an end-to-end invariant: never verify one artifact and install another.
- Keep exact asset-name tests for both Unix and Windows archives, plus negative coverage proving `.sha256` sidecars are not eligible install targets.
- Preserve explicit timeout coverage on every network entry point in `src/cli/update.rs`, but write timeout tests around bounded behavior rather than strict stopwatch precision unless the code truly guarantees real-time limits.
- For CLI suppression env vars, prefer fail-safe parsing over hard-fail parsing when malformed values should not block normal command execution.
- Keep TTY/non-TTY output tests for any user-facing CLI notice so interactive styling does not leak into redirected stderr or automation.
- Re-run the full verification sequence after cross-cutting CLI/runtime changes. This fix passed:
  - `cargo fmt -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo nextest run --all-features --locked --no-fail-fast`

## Related Issues

- Related doc: `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`
- Primary code areas: `src/cli/update.rs`, `src/cli/mod.rs`, `src/main.rs`
