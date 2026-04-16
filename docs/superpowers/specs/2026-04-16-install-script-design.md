# Install Script Design

**Date:** 2026-04-16
**Branch:** feature/cli-install-and-upgrade

## Goal

Allow users to install the `scorpio` CLI from a single platform-appropriate curl-based command against the latest GitHub release, on all supported platforms.

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
curl -fsSL https://raw.githubusercontent.com/BigtoC/scorpio-analyst/main/install.sh | bash
```

The Unix installer explicitly requires Bash because it uses `set -euo pipefail`.

**Logic:**
1. Detect OS via `uname -s`, arch via `uname -m`
2. Map to release target string (see table below)
3. Fetch latest release metadata from GitHub API (`/releases/latest`) with bounded timeouts and retries
4. Locate the expected asset in the release `assets[]` list and read its `digest`
5. If the target asset is missing, print a clear `latest release does not include <target> yet` message and exit 1
6. Create `~/.local/bin` if it does not exist
7. Download `scorpio-analyst-{VERSION}-{TARGET}.tar.gz` to a temp dir with bounded timeouts and retries
8. Verify the downloaded archive SHA-256 against the release metadata `digest` before extraction
9. Extract, move binary to `~/.local/bin/scorpio`, `chmod +x`
10. Warn if `~/.local/bin` is not in `$PATH`
11. Clean up temp dir on exit (trap)

**Platform mapping:**

| OS    | Arch (`uname -m`) | Target                      |
|-------|-------------------|-----------------------------|
| Linux | `x86_64`          | `x86_64-unknown-linux-gnu`  |
| Linux | `aarch64`         | `aarch64-unknown-linux-gnu` |
| macOS | `arm64`           | `aarch64-apple-darwin`      |
| macOS | `x86_64`          | `x86_64-apple-darwin`       |

**Error handling:**
- Unsupported OS or arch → print message and exit 1
- Missing release asset for the detected target → print a targeted message and exit 1
- Download failure or GitHub API timeout → bounded retries, then exit non-zero
- Digest mismatch → print message and exit 1 before extraction
- All errors surface via `set -euo pipefail`

## `install.ps1` (Windows)

**File:** `install.ps1` (repo root)

**User command:**
```powershell
curl.exe -fsSL https://raw.githubusercontent.com/BigtoC/scorpio-analyst/main/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -
```

Use `curl.exe` rather than `curl` so the command works consistently in both `cmd.exe` and PowerShell without hitting the `curl` alias.

**Logic:**
1. Fetch latest release metadata from GitHub API with bounded timeouts and retries
2. Locate `scorpio-analyst-{VERSION}-x86_64-pc-windows-msvc.zip` in the release `assets[]` list and read its `digest`
3. If the asset is missing, print a clear `latest release does not include x86_64-pc-windows-msvc yet` message and exit 1
4. Create `%USERPROFILE%\.local\bin\` if it does not exist
5. Download the ZIP to a temp dir with bounded timeouts and retries
6. Verify the downloaded archive SHA-256 against the release metadata `digest` before extraction
7. Extract, rename `scorpio-analyst.exe` → `scorpio.exe`
8. Move to `%USERPROFILE%\.local\bin\`
9. Permanently add that dir to the user `PATH` env var if not already present
10. Clean up temp dir

**Error handling:**
- `$ErrorActionPreference = "Stop"` — any failure exits immediately
- Missing release asset for Windows x86_64 → print a targeted message and exit 1
- HTTP timeout / transient download failure → bounded retries, then exit non-zero
- Digest mismatch → print message and exit 1 before extraction
- Only x86_64 Windows is supported (ARM Windows not in release matrix)

## Install Location

| Platform      | Path                                   |
|---------------|----------------------------------------|
| Linux / macOS | `~/.local/bin/scorpio`                 |
| Windows       | `%USERPROFILE%\.local\bin\scorpio.exe` |

## Binary Naming

The archive contains `scorpio-analyst` (or `scorpio-analyst.exe`). The install scripts rename it to `scorpio` (or `scorpio.exe`) so users run `scorpio analyze AAPL`, matching the docs.
