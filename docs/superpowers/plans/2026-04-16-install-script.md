# Install Script Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship release-published `install.sh` and `install.ps1` plus matching versionless release archives so users can install `scorpio` from the latest GitHub release on Linux, macOS, and Windows.

**Architecture:** Keep the Rust build output as `scorpio-analyst(.exe)` in `target/.../release/`, then rename it to `scorpio(.exe)` only inside release archives so bootstrap installers and `scorpio upgrade` consume the same public asset contract. The release workflow is split into per-target packaging jobs plus one non-matrix publish/verify stage that uploads shared installer assets and verifies overall release completeness. Both installers fetch the latest tag, construct deterministic release URLs, verify the target archive exists, and then extract/install the binary without checksum or detached-signature verification.

**Tech Stack:** Bash (`#!/usr/bin/env bash`), PowerShell 5.1+, GitHub Actions YAML, GitHub Releases API.

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `.github/workflows/release.yml` | Package versionless `scorpio-{target}` archives, upload installer scripts, verify release completeness |
| Modify | `install.sh` | Bash installer for Linux/macOS using deterministic release URLs and archive-only install flow |
| Modify | `install.ps1` | PowerShell installer for Windows x86_64 using deterministic release URLs and archive-only install flow |
| Modify | `tests/install_release_contract.rs` | Lock the simplified release/install asset contract |
| Delete | `packaging/install-signing-public.pem` | Remove obsolete signing-era public key artifact |
| Modify | `README.md` | Keep rollout note aligned with the simplified installer release until the first installer-capable release exists |

---

## Chunk 1: Simplify Release Workflow Contract

### Task 1: Remove checksum/signature assets from the release workflow

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `tests/install_release_contract.rs`

- [ ] **Step 1: Write the failing contract test update first**

Update `tests/install_release_contract.rs` so the release workflow contract expects only:

```text
Expected per-target release assets:
- scorpio-x86_64-unknown-linux-gnu.tar.gz
- scorpio-aarch64-unknown-linux-gnu.tar.gz
- scorpio-aarch64-apple-darwin.tar.gz
- scorpio-x86_64-apple-darwin.tar.gz
- scorpio-x86_64-pc-windows-msvc.zip

Expected shared assets:
- install.sh
- install.ps1
```

Remove assertions for:

```text
.sha256
.sha256.sig
BEGIN PUBLIC KEY
RSACryptoServiceProvider
<RSAKeyValue>
```

- [ ] **Step 2: Run the focused test to verify the red state**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast --test install_release_contract
```

Expected: FAIL because the workflow and installer files still reference checksum/signature behavior.

- [ ] **Step 3: Remove the obsolete signing public key artifact from the repo contract**

Delete:

```text
packaging/install-signing-public.pem
```

and remove the obsolete `signing_public_key_is_checked_in` test from `tests/install_release_contract.rs`.

- [ ] **Step 4: Add or confirm macOS targets and prerequisite steps**

Ensure the workflow includes:

```yaml
          - target: aarch64-apple-darwin
            os: macos-latest
            archive_ext: tar.gz
          - target: x86_64-apple-darwin
            os: macos-13
            archive_ext: tar.gz
```

and:

```yaml
      - name: Install Protobuf compiler (macOS)
        if: runner.os == 'macOS'
        run: brew install protobuf
```

- [ ] **Step 5: Add or confirm staged binary rename before packaging**

Ensure the workflow copies the built binaries into `release-stage/` under the public names before packaging:

For Linux/macOS:

```yaml
      - name: Stage binary (Linux / macOS)
        if: runner.os != 'Windows'
        run: |
          mkdir -p release-stage
          cp "target/${{ matrix.target }}/release/scorpio-analyst" "release-stage/scorpio"
```

For Windows:

```yaml
      - name: Stage binary (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force -Path release-stage | Out-Null
          Copy-Item "target\${{ matrix.target }}\release\scorpio-analyst.exe" "release-stage\scorpio.exe"
```

- [ ] **Step 6: Remove checksum/signature generation and upload steps**

Delete the workflow steps that:

```text
- install OpenSSL for signing purposes
- generate .sha256 files
- sign .sha256 files
- upload .sha256 and .sha256.sig assets
```

Keep only archive packaging and upload.

- [ ] **Step 7: Keep staged archive packaging with versionless names**

Ensure Linux/macOS packaging still uses:

```yaml
      - name: Package binary (Linux / macOS)
        if: runner.os != 'Windows'
        run: |
          ARCHIVE="scorpio-${{ matrix.target }}.tar.gz"
          tar -czf "$ARCHIVE" -C release-stage scorpio
          echo "ARCHIVE=$ARCHIVE" >> "$GITHUB_ENV"
```

and Windows packaging still uses:

```yaml
      - name: Package binary (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $archive = "scorpio-${{ matrix.target }}.zip"
          Compress-Archive -Path "release-stage\scorpio.exe" -DestinationPath $archive
          "ARCHIVE=$archive" | Out-File -FilePath $env:GITHUB_ENV -Append
```

- [ ] **Step 8: Upload only the archive from each matrix job**

The per-target release upload should become:

```yaml
      - name: Upload release archive
        uses: softprops/action-gh-release@v2
        with:
          files: |
            ${{ env.ARCHIVE }}
```

- [ ] **Step 9: Simplify final release completeness verification**

The non-matrix `publish_and_verify_release_assets` job must first upload the shared installer scripts once:

```yaml
      - name: Upload installer scripts
        uses: softprops/action-gh-release@v2
        with:
          files: |
            install.sh
            install.ps1
```

Then it should verify only:

```text
install.sh
install.ps1
scorpio-x86_64-unknown-linux-gnu.tar.gz
scorpio-aarch64-unknown-linux-gnu.tar.gz
scorpio-aarch64-apple-darwin.tar.gz
scorpio-x86_64-apple-darwin.tar.gz
scorpio-x86_64-pc-windows-msvc.zip
```

- [ ] **Step 10: Validate the workflow YAML**

Use a concrete validator already available in the working environment. If `python3` with `yaml` is available, run:

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('YAML valid')"
```

Expected: `YAML valid`

If PyYAML is unavailable locally, skip local YAML parsing and rely on GitHub Actions workflow parsing plus the contract test for validation. Record that PyYAML was unavailable.

- [ ] **Step 11: Re-run the focused contract test**

Run:

```bash
cargo nextest run --all-features --locked --no-fail-fast --test install_release_contract
```

Expected: PASS for the updated contract test.

- [ ] **Step 12: Commit the workflow contract simplification**

```bash
git add .github/workflows/release.yml tests/install_release_contract.rs packaging/install-signing-public.pem
git commit -m "feat(release): simplify installer asset contract"
```

---

## Chunk 2: Simplify `install.sh`

### Task 2: Remove checksum/signature logic from the Unix installer

**Files:**
- Modify: `install.sh`
- Modify: `tests/install_release_contract.rs`

- [ ] **Step 1: Update the install-script contract expectation first**

Lock in this simplified behavior in `tests/install_release_contract.rs`:

```text
Given tag v0.3.0 and target aarch64-apple-darwin,
the script must require:
- scorpio-aarch64-apple-darwin.tar.gz
```

and must no longer mention:

```text
.sha256
.sha256.sig
openssl dgst -sha256 -verify
BEGIN PUBLIC KEY
```

- [ ] **Step 2: Keep Bash-only runtime and deterministic target mapping**

Preserve:

```bash
#!/usr/bin/env bash
set -euo pipefail
```

and target mappings for:

```bash
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
aarch64-apple-darwin
x86_64-apple-darwin
```

- [ ] **Step 3: Keep shared curl retry settings**

Retain one shared array for network requests:

```bash
CURL_OPTS=(
  --fail
  --silent
  --show-error
  --location
  --connect-timeout 10
  --max-time 60
  --retry 3
  --retry-delay 2
  --retry-all-errors
)
```

- [ ] **Step 4: Simplify latest-release URL construction to the archive only**

The Unix installer should build only:

```bash
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
VERSION=$(curl "${CURL_OPTS[@]}" "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
[ -n "$VERSION" ] || { echo "Failed to resolve latest release tag." >&2; exit 1; }
ARCHIVE="scorpio-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
ARCHIVE_URL="$BASE_URL/$ARCHIVE"
```

- [ ] **Step 5: Require only archive/install tools**

Before download, require only the tools still needed for the simplified flow, such as:

```text
bash
curl
tar
sed
```

Do not require `openssl`, `shasum`, `sha256sum`, `wc`, `head`, `tail`, or `grep` for checksum-related work. Keep `sed` only because the final tag-resolution snippet still uses it.

- [ ] **Step 6: Probe only the archive URL before download**

Probe the archive URL and distinguish missing assets from transport/API failures.

- `404` / not found -> print:

```text
Latest release does not include <target> yet.
```

- any other failure -> surface a transport/API error and exit non-zero after retries.

One acceptable Bash shape is:

```bash
probe_status=$(curl --silent --show-error --location --connect-timeout 10 --max-time 60 --retry 3 --retry-delay 2 --retry-all-errors --head --write-out '%{http_code}' --output /dev/null "$ARCHIVE_URL") || probe_status="curl_error"
if [ "$probe_status" = "404" ]; then
  echo "Latest release does not include ${TARGET} yet." >&2
  exit 1
fi
[ "$probe_status" = "200" ] || { echo "Failed to access release archive: $ARCHIVE_URL" >&2; exit 1; }
```

- [ ] **Step 7: Download only the archive**

Use:

```bash
curl "${CURL_OPTS[@]}" "$ARCHIVE_URL" -o "$TMP/$ARCHIVE"
```

- [ ] **Step 8: Delete all embedded-key and checksum-verification code**

Remove the code that:

```text
- embeds a PEM public key
- writes install-signing-pubkey.pem
- runs openssl verification
- reads checksum bytes
- validates checksum file format
- compares archive hash to expected hash
```

- [ ] **Step 9: Keep extraction, install, and PATH hint behavior**

Preserve the archive extraction/install flow:

```bash
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
test -f "$TMP/scorpio"
mkdir -p "$INSTALL_DIR"
mv "$TMP/scorpio" "$INSTALL_DIR/scorpio"
chmod +x "$INSTALL_DIR/scorpio"
```

and the PATH hint when `INSTALL_DIR` is absent from `PATH`.

- [ ] **Step 10: Run shell validation**

Run:

```bash
bash -n install.sh
shellcheck install.sh
```

Expected: no output from `bash -n` and no warnings from `shellcheck`.

- [ ] **Step 11: Commit the Unix installer simplification**

```bash
git add install.sh tests/install_release_contract.rs
git commit -m "feat: simplify bash installer"
```

---

## Chunk 3: Simplify `install.ps1`

### Task 3: Remove checksum/signature logic from the Windows installer

**Files:**
- Modify: `install.ps1`
- Modify: `tests/install_release_contract.rs`

- [ ] **Step 1: Update the Windows contract expectation first**

Lock in this simplified behavior in `tests/install_release_contract.rs`:

```text
install.ps1 must work when invoked by `powershell` (Windows PowerShell 5.1),
must reject non-AMD64 hosts,
and must download/install:
- scorpio-x86_64-pc-windows-msvc.zip
```

and must no longer mention:

```text
.sha256
.sha256.sig
RSACryptoServiceProvider
<RSAKeyValue>
```

- [ ] **Step 2: Keep the `try/finally` structure and add TLS 1.2 setup**

Preserve the script structure and add TLS 1.2 before the first network request:

```powershell
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
```

- [ ] **Step 3: Keep retry helper and AMD64-only detection**

Retain:

```powershell
function Invoke-WithRetry { ... }
```

and:

```powershell
$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
if ($Arch -ne "X64" -and $Arch -ne "AMD64") { throw "Unsupported Windows architecture: $Arch" }
```

- [ ] **Step 4: Simplify latest-release URL construction to the archive only**

The Windows installer should resolve the latest tag and then build only the archive URL:

```powershell
$Release = Invoke-WithRetry { Invoke-RestMethod -TimeoutSec 30 "https://api.github.com/repos/$Repo/releases/latest" }
$Version = $Release.tag_name
if (-not $Version) { throw "Failed to resolve latest release tag." }
$Archive = "scorpio-x86_64-pc-windows-msvc.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"
$ArchiveUrl = "$BaseUrl/$Archive"
```

Do not build checksum or signature URLs.

- [ ] **Step 5: Probe only the archive URL before download**

Use `Invoke-WebRequest -Method Head` under retry logic for only the ZIP asset.

- `404` / not found -> raise:

```text
Latest release does not include x86_64-pc-windows-msvc yet.
```

- any other failure -> surface a transport/API error and exit non-zero after retries.

- [ ] **Step 6: Download only the ZIP archive**

Use:

```powershell
Invoke-WithRetry { Invoke-WebRequest -TimeoutSec 60 -Uri $ArchiveUrl -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing }
```

- [ ] **Step 7: Delete all embedded-key and checksum-verification code**

Remove the code that:

```text
- embeds RSA XML key material
- creates RSACryptoServiceProvider
- reads .sha256 or .sha256.sig files
- verifies signatures
- validates checksum file format
- compares archive hash to expected hash
```

- [ ] **Step 8: Keep extraction, install, and PATH update behavior**

Preserve:

```powershell
Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force
if (-not (Test-Path (Join-Path $Tmp "scorpio.exe"))) { throw "Expected scorpio.exe missing from archive." }
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Move-Item -Force (Join-Path $Tmp "scorpio.exe") (Join-Path $InstallDir "scorpio.exe")
```

and keep the user PATH update only if `$InstallDir` is absent.

- [ ] **Step 9: Run a PowerShell parse check with the best available interpreter**

Prefer Windows PowerShell (`powershell`) when available; otherwise use `pwsh` if that is the available parser in the environment.

If `powershell` is available locally, run:

```bash
powershell -NoLogo -NoProfile -NonInteractive -Command '$errors=$null; [System.Management.Automation.Language.Parser]::ParseFile("install.ps1", [ref]$null, [ref]$errors) | Out-Null; if ($errors) { $errors | ForEach-Object { $_.ToString() }; exit 1 }'
```

If only `pwsh` is available, run:

```bash
pwsh -NoLogo -NoProfile -NonInteractive -Command '$errors=$null; [System.Management.Automation.Language.Parser]::ParseFile("install.ps1", [ref]$null, [ref]$errors) | Out-Null; if ($errors) { $errors | ForEach-Object { $_.ToString() }; exit 1 }'
```

Expected: no output.

If neither `powershell` nor `pwsh` is available, record that the parse check could not run in this environment.

- [ ] **Step 10: Commit the Windows installer simplification**

```bash
git add install.ps1 tests/install_release_contract.rs
git commit -m "feat: simplify powershell installer"
```

---

## Chunk 4: Update README and Final Verification

### Task 4: Align docs and run full verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README rollout wording to match the simplified design**

If the README already includes the final one-line install commands:

```sh
curl -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.sh | bash
```

and:

```powershell
curl.exe -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -
```

remove those commands for now and keep only rollout-safe wording stating that installer commands should be documented after the first release containing installer scripts and archives exists. Do not mention signed checksum sidecars.

- [ ] **Step 2: Audit touched installer-facing files for stale checksum/signature language**

Re-read these files and confirm no stale wording about signed sidecars or verification logic remains:

```text
.github/workflows/release.yml
install.sh
install.ps1
tests/install_release_contract.rs
README.md
```

This is an audit step only. Any required code/document edits should already have been made in earlier tasks.

- [ ] **Step 3: Run the repo-required verification commands in order**

Run:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run --all-features --locked --no-fail-fast
```

Expected: all three commands pass.

- [ ] **Step 4: Commit the doc alignment and final cleanup**

```bash
git add README.md
git commit -m "docs: align installer rollout guidance"
```

---

## Self-Review

**Spec coverage:**
- [x] Release-published `install.sh` / `install.ps1` assets -> Chunk 1 Task 1 Step 9
- [x] Versionless `scorpio-{target}` archive names shared with `scorpio upgrade` -> Chunk 1 Task 1 Steps 5-8
- [x] Per-target matrix packaging + non-matrix publish/verify job topology -> Chunk 1 Task 1 Steps 5-9
- [x] Bash-only Unix bootstrap command -> Chunk 2 Task 2 Steps 2-3
- [x] Deterministic release URLs with archive probing -> Chunk 2 Task 2 Steps 4-7 and Chunk 3 Task 3 Steps 4-6
- [x] Windows x86_64-only detection -> Chunk 3 Task 3 Step 3
- [x] Windows PowerShell 5.1 TLS 1.2 compatibility -> Chunk 3 Task 3 Step 2
- [x] Archive contains `scorpio` / `scorpio.exe` directly after packaging rename -> Chunk 1 Task 1 Step 5 and installer extraction steps
- [x] README rollout guidance stays aligned until the first installer-capable release exists -> Chunk 4 Task 4 Step 1
- [x] No checksum or detached-signature verification in bootstrap install path -> Chunks 1-3
- [x] Obsolete signing-era public key artifact removed -> Chunk 1 Task 1 Step 3

**No placeholders detected.**

**Type/name consistency:** build output remains `scorpio-analyst(.exe)` in `target/.../release/`, but every public release archive and installed binary is `scorpio(.exe)`.
