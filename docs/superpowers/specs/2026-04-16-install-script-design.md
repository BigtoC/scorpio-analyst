# Install Script Design

**Date:** 2026-04-16
**Branch:** feature/cli-install-and-upgrade

## Goal

Allow users to install the `scorpio` CLI from a single curl/iwr command against the latest GitHub release, on all supported platforms.

## Scope

- Add macOS targets to the release workflow
- Write `install.sh` for Linux + macOS
- Write `install.ps1` for Windows

## Release Workflow Changes

File: `.github/workflows/release.yml`

Add two entries to the build matrix:

```yaml
- target: aarch64-apple-darwin
  os: macos-latest      # Apple Silicon runner
  archive_ext: tar.gz
- target: x86_64-apple-darwin
  os: macos-13          # last Intel macOS runner
  archive_ext: tar.gz
```

Add one macOS-specific prerequisite step before `cargo build`:

```yaml
- name: Install Protobuf compiler (macOS)
  if: runner.os == 'macOS'
  run: brew install protobuf
```

The existing strip and package steps are already gated on `runner.os != 'Windows'` and work for macOS.

## `install.sh` (Linux + macOS)

**File:** `install.sh` (repo root)

**User command:**
```sh
curl -fsSL https://raw.githubusercontent.com/BigtoC/scorpio-analyst/main/install.sh | sh
```

**Logic:**
1. Detect OS via `uname -s`, arch via `uname -m`
2. Map to release target string (see table below)
3. Fetch latest release tag from GitHub API (`/releases/latest`)
4. Create `~/.local/bin` if it does not exist
5. Download `scorpio-analyst-{VERSION}-{TARGET}.tar.gz` to a temp dir
6. Extract, move binary to `~/.local/bin/scorpio`, `chmod +x`
7. Warn if `~/.local/bin` is not in `$PATH`
8. Clean up temp dir on exit (trap)

**Platform mapping:**

| OS    | Arch (`uname -m`) | Target                      |
|-------|-------------------|-----------------------------|
| Linux | `x86_64`          | `x86_64-unknown-linux-gnu`  |
| Linux | `aarch64`         | `aarch64-unknown-linux-gnu` |
| macOS | `arm64`           | `aarch64-apple-darwin`      |
| macOS | `x86_64`          | `x86_64-apple-darwin`       |

**Error handling:**
- Unsupported OS or arch → print message and exit 1
- Download failure → curl `-f` flag causes non-zero exit
- All errors surface via `set -euo pipefail`

## `install.ps1` (Windows)

**File:** `install.ps1` (repo root)

**User command:**
```powershell
iwr https://raw.githubusercontent.com/BigtoC/scorpio-analyst/main/install.ps1 | iex
```

**Logic:**
1. Fetch latest release tag from GitHub API
2. Create `%USERPROFILE%\.local\bin\` if it does not exist
3. Download `scorpio-analyst-{VERSION}-x86_64-pc-windows-msvc.zip` to a temp dir
4. Extract, rename `scorpio-analyst.exe` → `scorpio.exe`
5. Move to `%USERPROFILE%\.local\bin\`
6. Permanently add that dir to the user `PATH` env var if not already present
7. Clean up temp dir

**Error handling:**
- `$ErrorActionPreference = "Stop"` — any failure exits immediately
- Only x86_64 Windows is supported (ARM Windows not in release matrix)

## Install Location

| Platform      | Path                                   |
|---------------|----------------------------------------|
| Linux / macOS | `~/.local/bin/scorpio`                 |
| Windows       | `%USERPROFILE%\.local\bin\scorpio.exe` |

## Binary Naming

The archive contains `scorpio-analyst` (or `scorpio-analyst.exe`). The install scripts rename it to `scorpio` (or `scorpio.exe`) so users run `scorpio analyze AAPL`, matching the docs.
