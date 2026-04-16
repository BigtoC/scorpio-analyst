# Install Script Design

**Date:** 2026-04-16
**Branch:** feature/cli-install-and-upgrade

## Goal

Allow users to install the `scorpio` CLI from a single platform-appropriate curl-based command against the latest GitHub release, on all supported platforms.

## Scope

- Add macOS targets to the release workflow
- Publish `install.sh` and `install.ps1` as release assets
- Publish versionless `scorpio-{TARGET}` archives that match the `scorpio upgrade` contract
- Write `install.sh` for Linux + macOS
- Write `install.ps1` for Windows
- Remove checksum and detached-signature verification from the bootstrap installers and release asset contract

## Release Workflow Changes

File: `.github/workflows/release.yml`

Add two entries to the build matrix:

```yaml
- target: aarch64-apple-darwin
  os: macos-latest
  archive_ext: tar.gz
- target: x86_64-apple-darwin
  os: macos-13
  archive_ext: tar.gz
```

Add one macOS-specific prerequisite step before `cargo build`:

```yaml
- name: Install Protobuf compiler (macOS)
  if: runner.os == 'macOS'
  run: brew install protobuf
```

In the same workflow, publish these additional release assets:

- `install.sh`
- `install.ps1`
- `scorpio-{TARGET}.<ext>`

This archive naming is shared with the approved `scorpio upgrade` design so both bootstrap installers and in-product self-update consume the same release assets.

The release workflow must:

1. Build `scorpio-analyst` / `scorpio-analyst.exe`
2. Package the built binary under the public archive name `scorpio` / `scorpio.exe`
3. Upload the per-target archive assets to the release
4. Upload the shared installer scripts to the release
5. Fail the workflow if any required archive or installer asset is missing

## `install.sh` (Linux + macOS)

**File:** `install.sh` (repo root)

**User command:**
```sh
curl -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.sh | bash
```

The Unix installer explicitly requires Bash because it uses `set -euo pipefail`.

**Logic:**
1. Detect OS via `uname -s`, arch via `uname -m`
2. Map to release target string (see table below)
3. Fetch the latest release tag from GitHub API (`/releases/latest`) with bounded timeouts and retries
4. Construct a deterministic URL for the archive from that tag
5. Probe the archive URL; if it is missing, print a clear `latest release does not include <target> yet` message and exit 1
6. Create `~/.local/bin` if it does not exist
7. Download `scorpio-{TARGET}.tar.gz` to a temp dir with bounded timeouts and retries
8. Extract, move binary to `~/.local/bin/scorpio`, `chmod +x`
9. Warn if `~/.local/bin` is not in `$PATH`
10. Clean up temp dir on exit (trap)

**Platform mapping:**

| OS    | Arch (`uname -m`) | Target                      |
|-------|-------------------|-----------------------------|
| Linux | `x86_64`          | `x86_64-unknown-linux-gnu`  |
| Linux | `aarch64`         | `aarch64-unknown-linux-gnu` |
| macOS | `arm64`           | `aarch64-apple-darwin`      |
| macOS | `x86_64`          | `x86_64-apple-darwin`       |

**Error handling:**
- Unsupported OS or arch -> print message and exit 1
- Missing release archive for the detected target -> print a targeted message and exit 1
- Download failure or GitHub API timeout -> bounded retries, then exit non-zero
- Extraction failure or missing `scorpio` in the archive -> print message and exit 1
- All errors surface via `set -euo pipefail`

## `install.ps1` (Windows)

**File:** `install.ps1` (repo root)

**User command:**
```powershell
curl.exe -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -
```

Use `curl.exe` rather than `curl` so the command works consistently in both `cmd.exe` and PowerShell without hitting the `curl` alias.

**Logic:**
1. Enable TLS 1.2 for Windows PowerShell 5.1 compatibility with GitHub
2. Fetch the latest release tag from GitHub API with bounded timeouts and retries
3. Detect the Windows architecture and fail fast with a targeted message unless it is `x86_64` / `AMD64`
4. Construct a deterministic URL for the ZIP from that tag
5. Probe the archive URL; if it is missing, print a clear `latest release does not include x86_64-pc-windows-msvc yet` message and exit 1
6. Create `%USERPROFILE%\.local\bin\` if it does not exist
7. Download `scorpio-x86_64-pc-windows-msvc.zip` to a temp dir with bounded timeouts and retries
8. Extract `scorpio.exe`
9. Move `scorpio.exe` to `%USERPROFILE%\.local\bin\`
10. Permanently add that dir to the user `PATH` env var if not already present
11. Clean up temp dir

**Error handling:**
- `$ErrorActionPreference = "Stop"` -- any failure exits immediately
- Missing release archive for Windows x86_64 -> print a targeted message and exit 1
- HTTP timeout / transient download failure -> bounded retries, then exit non-zero
- Unsupported Windows architecture -> print message and exit 1 before download
- Extraction failure or missing `scorpio.exe` in the archive -> print message and exit 1
- Only x86_64 Windows is supported (ARM Windows not in release matrix)

## Release Asset Contract

Archive names are versionless and match the `self_update` contract:

- `scorpio-x86_64-unknown-linux-gnu.tar.gz`
- `scorpio-aarch64-unknown-linux-gnu.tar.gz`
- `scorpio-aarch64-apple-darwin.tar.gz`
- `scorpio-x86_64-apple-darwin.tar.gz`
- `scorpio-x86_64-pc-windows-msvc.zip`

The shared installer assets are:

- `install.sh`
- `install.ps1`

No checksum or detached-signature sidecars are part of the public release contract for bootstrap install.

## Trust Model

The bootstrap installers trust the GitHub release asset served from the latest release for `BigtoC/scorpio-analyst`.

That means:

- installers do not download or verify `.sha256` files
- installers do not verify detached signatures
- release verification checks only that the expected installer scripts and archives are present

This is intentionally simpler than the previous signed-checksum design. The trade-off is that bootstrap install integrity now depends on GitHub release delivery rather than an additional detached-verification layer.

## Rollout Order

To avoid a broken transition window:

1. Merge the release workflow changes that publish installer assets and versionless archives
2. Cut the first release that includes `install.sh`, `install.ps1`, and all expected archives
3. Only then document and advertise the one-line installer commands

## Workflow Topology

The release workflow should use two stages:

1. Per-target matrix jobs:
   - build `scorpio-analyst` / `scorpio-analyst.exe` for one target
   - rename the built `scorpio-analyst` / `scorpio-analyst.exe` artifact to `scorpio` / `scorpio.exe` inside a staging directory
   - package `scorpio-{TARGET}.<ext>` from that staged binary
   - upload that per-target archive to the release
2. One non-matrix publish/verify job:
   - upload `install.sh` and `install.ps1` once
   - verify that every expected archive and installer script exists on the release
   - fail the workflow if any shared or per-target asset is missing

## Install Location

| Platform      | Path                                   |
|---------------|----------------------------------------|
| Linux / macOS | `~/.local/bin/scorpio`                 |
| Windows       | `%USERPROFILE%\.local\bin\scorpio.exe` |

## Binary Naming

The Rust build still produces `scorpio-analyst` (or `scorpio-analyst.exe`) in `target/.../release/`.

The packaging step renames that built artifact to `scorpio` (or `scorpio.exe`) inside the release archive, so:

- release archives contain `scorpio` / `scorpio.exe` directly
- bootstrap installers install `scorpio` / `scorpio.exe` unchanged
- `scorpio upgrade` consumes the same archive layout and asset names
