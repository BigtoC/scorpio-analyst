# Install Script Design

**Date:** 2026-04-16
**Branch:** feature/cli-install-and-upgrade

## Goal

Allow users to install the `scorpio` CLI from a single platform-appropriate curl-based command against the latest GitHub release, on all supported platforms.

## Scope

- Add macOS targets to the release workflow
- Publish `install.sh` and `install.ps1` as release assets
- Publish signed checksum sidecars for each release archive
- Write `install.sh` for Linux + macOS
- Write `install.ps1` for Windows
- Unify release archive names with the `scorpio upgrade` self-update contract

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

In the same workflow, also publish these additional release assets:

- `install.sh`
- `install.ps1`
- `scorpio-{TARGET}.<ext>`
- `scorpio-{TARGET}.<ext>.sha256`
- `scorpio-{TARGET}.<ext>.sha256.sig`

The checksum file contains the archive SHA-256. The signature file is a detached RSA-SHA256 signature over that checksum file.

This archive naming is shared with the approved `scorpio upgrade` design so both bootstrap installers and in-product self-update consume the same release assets.

The release workflow must:

1. Generate the checksum after packaging each archive
2. Sign the checksum file with a PEM private key provided in CI via `SCORPIO_INSTALL_SIGNING_KEY_PEM`
3. Package the built `scorpio-analyst` / `scorpio-analyst.exe` binary under the public archive name `scorpio` / `scorpio.exe`
4. Upload the archive, checksum, signature, and installer script assets to the release
4. Fail the job if any required sidecar or installer asset is missing

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
4. Construct deterministic URLs for the archive, checksum sidecar, and signature sidecar from that tag
5. Probe all three URLs; if any are missing, print a clear `latest release does not include <target> yet` message and exit 1
6. Create `~/.local/bin` if it does not exist
7. Download `scorpio-{TARGET}.tar.gz`, `scorpio-{TARGET}.tar.gz.sha256`, and `scorpio-{TARGET}.tar.gz.sha256.sig` to a temp dir with bounded timeouts and retries
8. Verify the checksum signature with an embedded RSA public key using `openssl dgst -sha256 -verify`
9. Verify the downloaded archive SHA-256 against the trusted checksum file before extraction
10. Extract, move binary to `~/.local/bin/scorpio`, `chmod +x`
11. Warn if `~/.local/bin` is not in `$PATH`
12. Clean up temp dir on exit (trap)

**Platform mapping:**

| OS    | Arch (`uname -m`) | Target                      |
|-------|-------------------|-----------------------------|
| Linux | `x86_64`          | `x86_64-unknown-linux-gnu`  |
| Linux | `aarch64`         | `aarch64-unknown-linux-gnu` |
| macOS | `arm64`           | `aarch64-apple-darwin`      |
| macOS | `x86_64`          | `x86_64-apple-darwin`       |

**Error handling:**
- Unsupported OS or arch → print message and exit 1
- Missing release archive, checksum, or signature sidecar for the detected target → print a targeted message and exit 1
- Download failure or GitHub API timeout → bounded retries, then exit non-zero
- Signature verification failure → print message and exit 1 before extraction
- Checksum mismatch → print message and exit 1 before extraction
- All errors surface via `set -euo pipefail`

## `install.ps1` (Windows)

**File:** `install.ps1` (repo root)

**User command:**
```powershell
curl.exe -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -
```

Use `curl.exe` rather than `curl` so the command works consistently in both `cmd.exe` and PowerShell without hitting the `curl` alias.

**Logic:**
1. Fetch the latest release tag from GitHub API with bounded timeouts and retries
2. Detect the Windows architecture and fail fast with a targeted message unless it is `x86_64` / `AMD64`
3. Construct deterministic URLs for the ZIP, checksum sidecar, and signature sidecar from that tag
4. Probe all three URLs; if any are missing, print a clear `latest release does not include x86_64-pc-windows-msvc yet` message and exit 1
5. Create `%USERPROFILE%\.local\bin\` if it does not exist
6. Download `scorpio-x86_64-pc-windows-msvc.zip`, `scorpio-x86_64-pc-windows-msvc.zip.sha256`, and `scorpio-x86_64-pc-windows-msvc.zip.sha256.sig` to a temp dir with bounded timeouts and retries
7. Verify the checksum signature with the embedded RSA public key using .NET cryptography APIs in PowerShell
8. Verify the downloaded archive SHA-256 against the trusted checksum file before extraction
9. Extract `scorpio.exe`
10. Move `scorpio.exe` to `%USERPROFILE%\.local\bin\`
11. Permanently add that dir to the user `PATH` env var if not already present
12. Clean up temp dir

**Error handling:**
- `$ErrorActionPreference = "Stop"` — any failure exits immediately
- Missing release archive, checksum, or signature sidecar for Windows x86_64 → print a targeted message and exit 1
- HTTP timeout / transient download failure → bounded retries, then exit non-zero
- Unsupported Windows architecture → print message and exit 1 before download
- Signature verification failure → print message and exit 1 before extraction
- Checksum mismatch → print message and exit 1 before extraction
- Only x86_64 Windows is supported (ARM Windows not in release matrix)

## Integrity Contract

The release workflow must publish two sidecars for every installer archive:

- `scorpio-{TARGET}.<ext>.sha256`
- `scorpio-{TARGET}.<ext>.sha256.sig`

Archive names are versionless and match the `self_update` contract:

- `scorpio-x86_64-unknown-linux-gnu.tar.gz`
- `scorpio-aarch64-unknown-linux-gnu.tar.gz`
- `scorpio-aarch64-apple-darwin.tar.gz`
- `scorpio-x86_64-apple-darwin.tar.gz`
- `scorpio-x86_64-pc-windows-msvc.zip`

The `.sha256` file format is exact and canonical: UTF-8 text containing exactly `<64 lowercase hex chars>\n` and nothing else. The `.sha256.sig` file is a detached signature over those exact bytes.

The signing scheme is RSA PKCS#1 v1.5 with SHA-256:

- CI signs each `.sha256` file with the private PEM key from `SCORPIO_INSTALL_SIGNING_KEY_PEM`
- `install.sh` embeds the matching RSA public key in PEM format
- `install.ps1` embeds the matching RSA public key in a PowerShell-friendly representation of the same key

Both installers must:

1. Download the archive, checksum, and signature
2. Verify the checksum signature first
3. Verify the archive hash against the trusted checksum file
4. Abort before extraction or install on any verification failure

## Rollout Order

To avoid a broken transition window:

1. Merge the release workflow changes that publish installer assets and signed checksum sidecars
2. Cut the first release that includes `install.sh`, `install.ps1`, `.sha256`, and `.sha256.sig`
3. Only then document and advertise the one-line installer commands

## Workflow Topology

The release workflow should use two stages:

1. Per-target matrix jobs:
   - build `scorpio-analyst` / `scorpio-analyst.exe` for one target
   - rename the built `scorpio-analyst` / `scorpio-analyst.exe` artifact to `scorpio` / `scorpio.exe` inside a staging directory
   - package `scorpio-{TARGET}.<ext>` from that staged binary
   - generate `scorpio-{TARGET}.<ext>.sha256`
   - sign it to produce `scorpio-{TARGET}.<ext>.sha256.sig`
   - upload those three per-target assets to the release
2. One non-matrix publish/verify job:
   - upload `install.sh` and `install.ps1` once
   - verify that every expected archive, checksum, and signature exists on the release
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
