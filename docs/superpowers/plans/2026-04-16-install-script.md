# Install Script Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add macOS release targets and ship `install.sh` + `install.ps1` so users can install the `scorpio` CLI with a single curl command.

**Architecture:** Three independent changes: (1) extend the release matrix in CI to build macOS binaries, (2) write a POSIX-compatible `install.sh` that detects OS/arch and installs for Linux and macOS, (3) write `install.ps1` for Windows. Scripts fetch the latest GitHub release tag via the API, download the right archive, and drop the renamed binary into `~/.local/bin` (or `%USERPROFILE%\.local\bin` on Windows).

**Tech Stack:** Bash (`#!/usr/bin/env bash`), PowerShell 5+, GitHub Actions YAML, GitHub Releases API.

---

## File Map

| Action | Path                            | Responsibility                                 |
|--------|---------------------------------|------------------------------------------------|
| Modify | `.github/workflows/release.yml` | Add macOS matrix entries + macOS Protobuf step |
| Create | `install.sh`                    | Unix installer (Linux + macOS)                 |
| Create | `install.ps1`                   | Windows installer                              |

---

### Task 1: Add macOS targets to the release workflow

**Files:**
- Modify: `.github/workflows/release.yml:51-62` (matrix) and `:68-74` (Protobuf steps)

- [ ] **Step 1: Add macOS matrix entries**

In `.github/workflows/release.yml`, extend the `matrix.include` list (after the Windows entry, before `steps:`):

```yaml
          - target: aarch64-apple-darwin
            os: macos-latest
            archive_ext: tar.gz
          - target: x86_64-apple-darwin
            os: macos-13
            archive_ext: tar.gz
```

The full matrix block after the change:

```yaml
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
            archive_ext: tar.gz
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-24.04-arm
            archive_ext: tar.gz
          - target: x86_64-pc-windows-msvc
            os: windows-latest
            archive_ext: zip
          - target: aarch64-apple-darwin
            os: macos-latest
            archive_ext: tar.gz
          - target: x86_64-apple-darwin
            os: macos-13
            archive_ext: tar.gz
```

- [ ] **Step 2: Add macOS Protobuf install step**

After the existing `Install Protobuf compiler (Windows)` step, add:

```yaml
      - name: Install Protobuf compiler (macOS)
        if: runner.os == 'macOS'
        run: brew install protobuf
```

The three Protobuf steps together should now look like:

```yaml
      - name: Install Protobuf compiler (Linux)
        if: runner.os == 'Linux'
        run: sudo apt-get update && sudo apt-get install -y protobuf-compiler

      - name: Install Protobuf compiler (Windows)
        if: runner.os == 'Windows'
        run: choco install protoc --no-progress

      - name: Install Protobuf compiler (macOS)
        if: runner.os == 'macOS'
        run: brew install protobuf
```

- [ ] **Step 3: Validate the YAML**

```bash
python3 -c "import yaml, sys; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "YAML valid"
```

Expected: `YAML valid`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "feat(release): add macOS aarch64 and x86_64 build targets"
```

---

### Task 2: Write `install.sh`

**Files:**
- Create: `install.sh`

The script must:
- Use `#!/usr/bin/env bash` with `set -euo pipefail` for safety
- Detect OS via `uname -s`, arch via `uname -m`
- Map to the correct release target string
- Fetch the latest release tag from the GitHub API (no `jq` dependency — use `grep` + `sed`)
- Download the `.tar.gz` archive to a temp dir (cleaned up on exit via `trap`)
- Extract, rename the binary from `scorpio-analyst` → `scorpio`, place in `~/.local/bin/`
- Warn if `~/.local/bin` is not in `$PATH`
- Print clear error messages to stderr for unsupported OS/arch

- [ ] **Step 1: Create `install.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO="BigtoC/scorpio-analyst"
BINARY_NAME="scorpio"
INSTALL_DIR="$HOME/.local/bin"

# --- Detect platform ---
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)
        echo "Unsupported Linux architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64)  TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *)
        echo "Unsupported macOS architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS" >&2
    echo "For Windows, run:" >&2
    echo "  curl.exe -fsSL https://raw.githubusercontent.com/$REPO/main/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -" >&2
    exit 1
    ;;
esac

# --- Fetch latest release tag ---
VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' \
  | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$VERSION" ]; then
  echo "Failed to determine latest release version." >&2
  exit 1
fi

echo "Installing $BINARY_NAME $VERSION for $TARGET..."

# --- Download ---
ARCHIVE="scorpio-analyst-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $URL..."
curl -fsSL "$URL" -o "$TMP/$ARCHIVE"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"

# --- Install ---
mkdir -p "$INSTALL_DIR"
mv "$TMP/scorpio-analyst" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

echo ""
echo "Installed: $INSTALL_DIR/$BINARY_NAME"
echo "Version:   $VERSION"

# --- PATH hint ---
case ":${PATH}:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo ""
    echo "NOTE: $INSTALL_DIR is not in your PATH."
    echo "Add the following line to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    ;;
esac
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x install.sh
```

- [ ] **Step 3: Lint with shellcheck**

Install shellcheck if needed: `brew install shellcheck` (macOS) or `apt-get install shellcheck` (Ubuntu).

```bash
shellcheck install.sh
```

Expected: no output (zero warnings).

- [ ] **Step 4: Dry-run smoke test (Linux or macOS only)**

This test hits the real GitHub API to verify the detection and download logic. It requires a Linux or macOS release to already exist in the repo.

```bash
bash install.sh
```

Expected output (version may differ):
```
Installing scorpio v0.2.0 for aarch64-apple-darwin...
Downloading https://github.com/BigtoC/scorpio-analyst/releases/download/v0.2.0/scorpio-analyst-v0.2.0-aarch64-apple-darwin.tar.gz...

Installed: /Users/<you>/.local/bin/scorpio
Version:   v0.2.0
```

Then verify:
```bash
~/.local/bin/scorpio --version
```

Expected: prints version string.

> **Note:** macOS assets will not exist until the first release after Task 1 is merged. Run this test on Linux to validate with an existing release, or cut a new release first.

- [ ] **Step 5: Commit**

```bash
git add install.sh
git commit -m "feat: add install.sh for Linux and macOS"
```

---

### Task 3: Write `install.ps1`

**Files:**
- Create: `install.ps1`

The script must:
- Set `$ErrorActionPreference = "Stop"` so any failure exits
- Fetch the latest release tag via `Invoke-RestMethod` (GitHub API)
- Download the `.zip` archive to a temp dir (cleaned up in a `finally` block)
- Extract, rename `scorpio-analyst.exe` → `scorpio.exe`, place in `%USERPROFILE%\.local\bin\`
- Permanently add that directory to the user `PATH` env var if not already present
- Print clear status messages throughout

- [ ] **Step 1: Create `install.ps1`**

```powershell
$ErrorActionPreference = "Stop"

$Repo       = "BigtoC/scorpio-analyst"
$BinaryName = "scorpio.exe"
$Target     = "x86_64-pc-windows-msvc"
$InstallDir = Join-Path $env:USERPROFILE ".local\bin"

# --- Fetch latest release tag ---
$Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
$Version = $Release.tag_name

Write-Host "Installing scorpio $Version for $Target..."

$Archive = "scorpio-analyst-$Version-$Target.zip"
$Url     = "https://github.com/$Repo/releases/download/$Version/$Archive"
$Tmp     = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $Tmp | Out-Null

try {
    # --- Download ---
    Write-Host "Downloading $Url..."
    Invoke-WebRequest -Uri $Url -OutFile (Join-Path $Tmp $Archive)
    Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force

    # --- Install ---
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Move-Item -Path (Join-Path $Tmp "scorpio-analyst.exe") `
              -Destination (Join-Path $InstallDir $BinaryName) `
              -Force

    Write-Host ""
    Write-Host "Installed: $InstallDir\$BinaryName"
    Write-Host "Version:   $Version"

    # --- PATH ---
    $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($CurrentPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$CurrentPath;$InstallDir", "User")
        Write-Host ""
        Write-Host "NOTE: Added $InstallDir to your PATH."
        Write-Host "Restart your terminal for the change to take effect."
    }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
```

- [ ] **Step 2: Lint with PSScriptAnalyzer**

Install if needed (run once in PowerShell):
```powershell
Install-Module -Name PSScriptAnalyzer -Scope CurrentUser -Force
```

Then lint:
```powershell
Invoke-ScriptAnalyzer -Path install.ps1
```

Expected: no output (zero warnings/errors).

- [ ] **Step 3: Commit**

```bash
git add install.ps1
git commit -m "feat: add install.ps1 for Windows"
```

---

## Self-Review

**Spec coverage:**
- [x] Add macOS aarch64 + x86_64 release targets → Task 1
- [x] Add macOS Protobuf step → Task 1 Step 2
- [x] `install.sh` detects OS + arch → Task 2 Step 1
- [x] `install.sh` fetches latest release tag → Task 2 Step 1
- [x] `install.sh` installs to `~/.local/bin/scorpio` → Task 2 Step 1
- [x] `install.sh` warns if `~/.local/bin` not in PATH → Task 2 Step 1
- [x] `install.ps1` fetches latest release tag → Task 3 Step 1
- [x] `install.ps1` installs to `%USERPROFILE%\.local\bin\scorpio.exe` → Task 3 Step 1
- [x] `install.ps1` adds dir to user PATH → Task 3 Step 1
- [x] Binary renamed from `scorpio-analyst` → `scorpio` → both scripts

**No placeholders detected.**

**Type/name consistency:** `scorpio-analyst` (archive binary name) and `scorpio` (installed name) used consistently across all tasks.
