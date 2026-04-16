# Install Script Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship release-published `install.sh` and `install.ps1` plus matching signed release assets so users can install `scorpio` safely from the latest GitHub release on Linux, macOS, and Windows.

**Architecture:** Keep the Rust build output as `scorpio-analyst(.exe)` in `target/.../release/`, then rename it to `scorpio(.exe)` only inside release archives so bootstrap installers and `scorpio upgrade` consume the same public asset contract. The release workflow is split into per-target packaging/signing jobs plus one non-matrix publish/verify stage that uploads shared installer assets and verifies overall release completeness. Both installers fetch the latest tag, construct deterministic release URLs, require the archive plus signed checksum sidecars, verify the signature with a checked-in RSA public key, and only then extract/install the binary.

**Tech Stack:** Bash (`#!/usr/bin/env bash`), PowerShell 5.1+, OpenSSL CLI, GitHub Actions YAML, GitHub Releases API, RSA PKCS#1 v1.5 with SHA-256.

---

## File Map

| Action | Path                            | Responsibility |
|--------|---------------------------------|----------------|
| Modify | `.github/workflows/release.yml` | Package versionless `scorpio-{target}` archives, generate/sign checksum sidecars, upload installers, verify release completeness |
| Create | `install.sh`                    | Bash installer for Linux/macOS using deterministic release URLs and signed checksum verification |
| Create | `install.ps1`                   | PowerShell installer for Windows x86_64 using deterministic release URLs and signed checksum verification |
| Create | `packaging/install-signing-public.pem` | Canonical RSA public key used by installer verification and PowerShell XML generation |
| Modify | `README.md`                     | Publish the final install commands only after the signed release flow exists |

---

### Task 1: Update the release workflow to publish shared installer assets and signed sidecars

**Files:**
- Modify: `.github/workflows/release.yml`
- Create: `packaging/install-signing-public.pem`

- [ ] **Step 1: Provision the install-signing keypair before touching the workflow**

Generate an RSA keypair once, commit the public key, and store the private key in GitHub Actions secrets.

```bash
openssl genrsa -out install-signing-private.pem 4096
openssl rsa -in install-signing-private.pem -pubout -out packaging/install-signing-public.pem
```

Then add the private key PEM contents to the repository secret named `SCORPIO_INSTALL_SIGNING_KEY_PEM` before testing a release.

- [ ] **Step 2: Write the failing release-contract test note in the plan execution log**

Before editing YAML, record the exact target asset contract to keep the implementation honest:

```text
Expected per-target release assets:
- scorpio-x86_64-unknown-linux-gnu.tar.gz
- scorpio-aarch64-unknown-linux-gnu.tar.gz
- scorpio-aarch64-apple-darwin.tar.gz
- scorpio-x86_64-apple-darwin.tar.gz
- scorpio-x86_64-pc-windows-msvc.zip

Expected sidecars for each asset:
- <asset>.sha256
- <asset>.sha256.sig

Expected shared assets:
- install.sh
- install.ps1
```

- [ ] **Step 3: Add macOS targets if missing and keep the macOS Protobuf step**

Ensure the build matrix contains:

```yaml
          - target: aarch64-apple-darwin
            os: macos-latest
            archive_ext: tar.gz
          - target: x86_64-apple-darwin
            os: macos-13
            archive_ext: tar.gz
```

And the build steps contain:

```yaml
      - name: Install Protobuf compiler (macOS)
        if: runner.os == 'macOS'
        run: brew install protobuf
```

- [ ] **Step 4: Add signing prerequisites per platform**

In the build matrix job, add the minimum signing tool support:

```yaml
      - name: Install OpenSSL (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: choco install openssl --no-progress
```

Linux and macOS runners already have `openssl` available.

- [ ] **Step 5: Package staged `scorpio` binaries instead of raw `scorpio-analyst` binaries**

Replace direct packaging with a staging step that renames the built output before archiving.

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

- [ ] **Step 6: Change archive names to the shared self-update contract**

For Linux/macOS:

```yaml
      - name: Package binary (Linux / macOS)
        if: runner.os != 'Windows'
        run: |
          ARCHIVE="scorpio-${{ matrix.target }}.tar.gz"
          tar -czf "$ARCHIVE" -C release-stage scorpio
          echo "ARCHIVE=$ARCHIVE" >> "$GITHUB_ENV"
```

For Windows:

```yaml
      - name: Package binary (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $archive = "scorpio-${{ matrix.target }}.zip"
          Compress-Archive -Path "release-stage\scorpio.exe" -DestinationPath $archive
          "ARCHIVE=$archive" | Out-File -FilePath $env:GITHUB_ENV -Append
```

- [ ] **Step 7: Generate the canonical checksum file**

The checksum file format must be exactly `<64 lowercase hex chars>\n`.

For Linux/macOS:

```yaml
      - name: Generate checksum (Linux / macOS)
        if: runner.os != 'Windows'
        run: |
          hash=$(shasum -a 256 "$ARCHIVE" | cut -d ' ' -f1 | tr '[:upper:]' '[:lower:]')
          printf '%s\n' "$hash" > "$ARCHIVE.sha256"
```

For Windows:

```yaml
      - name: Generate checksum (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $hash = (Get-FileHash -Algorithm SHA256 $env:ARCHIVE).Hash.ToLowerInvariant()
          $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
          [System.IO.File]::WriteAllText("$env:ARCHIVE.sha256", "$hash`n", $utf8NoBom)
```

- [ ] **Step 8: Sign the checksum file from CI secret material**

Use the private PEM key provided in `SCORPIO_INSTALL_SIGNING_KEY_PEM`.

For Linux/macOS:

```yaml
      - name: Sign checksum (Linux / macOS)
        if: runner.os != 'Windows'
        env:
          SCORPIO_INSTALL_SIGNING_KEY_PEM: ${{ secrets.SCORPIO_INSTALL_SIGNING_KEY_PEM }}
        run: |
          printf '%s' "$SCORPIO_INSTALL_SIGNING_KEY_PEM" > install-signing-key.pem
          chmod 600 install-signing-key.pem
          openssl dgst -sha256 -sign install-signing-key.pem -out "$ARCHIVE.sha256.sig" "$ARCHIVE.sha256"
```

For Windows:

```yaml
      - name: Sign checksum (Windows)
        if: runner.os == 'Windows'
        shell: pwsh
        env:
          SCORPIO_INSTALL_SIGNING_KEY_PEM: ${{ secrets.SCORPIO_INSTALL_SIGNING_KEY_PEM }}
        run: |
          Set-Content -Path install-signing-key.pem -Value $env:SCORPIO_INSTALL_SIGNING_KEY_PEM -NoNewline
          & openssl dgst -sha256 -sign install-signing-key.pem -out "$env:ARCHIVE.sha256.sig" "$env:ARCHIVE.sha256"
```

- [ ] **Step 9: Upload archive plus both sidecars from each matrix job**

Replace the single-file upload with:

```yaml
      - name: Upload signed release assets
        uses: softprops/action-gh-release@v2
        with:
          files: |
            ${{ env.ARCHIVE }}
            ${{ env.ARCHIVE }}.sha256
            ${{ env.ARCHIVE }}.sha256.sig
```

- [ ] **Step 10: Add one non-matrix publish/verify job for shared installer assets**

Add a single `publish_and_verify_release_assets` job that depends on the matrix build job, uploads `install.sh` and `install.ps1` exactly once, and then verifies the full release asset set before succeeding.

```yaml
  publish_and_verify_release_assets:
    name: Publish installer scripts and verify release assets
    needs: build
    runs-on: ubuntu-latest
    steps:
      - name: Check out repository
        uses: actions/checkout@v4

      - name: Upload installer scripts
        uses: softprops/action-gh-release@v2
        with:
          files: |
            install.sh
            install.ps1

      - name: Verify all expected release assets exist
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release view "${{ github.ref_name }}" --repo BigtoC/scorpio-analyst --json assets --jq '.assets[].name' > assets.txt
          cat <<'EOF' > expected.txt
          install.sh
          install.ps1
          scorpio-x86_64-unknown-linux-gnu.tar.gz
          scorpio-x86_64-unknown-linux-gnu.tar.gz.sha256
          scorpio-x86_64-unknown-linux-gnu.tar.gz.sha256.sig
          scorpio-aarch64-unknown-linux-gnu.tar.gz
          scorpio-aarch64-unknown-linux-gnu.tar.gz.sha256
          scorpio-aarch64-unknown-linux-gnu.tar.gz.sha256.sig
          scorpio-aarch64-apple-darwin.tar.gz
          scorpio-aarch64-apple-darwin.tar.gz.sha256
          scorpio-aarch64-apple-darwin.tar.gz.sha256.sig
          scorpio-x86_64-apple-darwin.tar.gz
          scorpio-x86_64-apple-darwin.tar.gz.sha256
          scorpio-x86_64-apple-darwin.tar.gz.sha256.sig
          scorpio-x86_64-pc-windows-msvc.zip
          scorpio-x86_64-pc-windows-msvc.zip.sha256
          scorpio-x86_64-pc-windows-msvc.zip.sha256.sig
          EOF
          missing=0
          while IFS= read -r asset; do
            if ! grep -Fxq "$asset" assets.txt; then
              echo "::error::Missing release asset: $asset"
              missing=1
            fi
          done < expected.txt
          test "$missing" -eq 0
```

- [ ] **Step 11: Validate the YAML syntax**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "YAML valid"
```

Expected: `YAML valid`

- [ ] **Step 12: Commit**

```bash
git add .github/workflows/release.yml packaging/install-signing-public.pem
git commit -m "feat(release): publish signed install assets"
```

---

### Task 2: Write `install.sh` for Bash-only, signed, deterministic installs

**Files:**
- Create: `install.sh`

- [ ] **Step 1: Write the failing Unix asset-resolution test as a shell transcript note**

The script behavior to lock in:

```text
Given tag v0.3.0 and target aarch64-apple-darwin,
the script must require:
- scorpio-aarch64-apple-darwin.tar.gz
- scorpio-aarch64-apple-darwin.tar.gz.sha256
- scorpio-aarch64-apple-darwin.tar.gz.sha256.sig
```

- [ ] **Step 2: Create `install.sh` with Bash-only runtime and deterministic asset names**

Start from this shape:

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO="BigtoC/scorpio-analyst"
INSTALL_DIR="${SCORPIO_INSTALL_DIR:-$HOME/.local/bin}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
```

Platform detection must map to:

```bash
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
aarch64-apple-darwin
x86_64-apple-darwin
```

Do not print the Windows installer command from the Unix script. Instead, print a targeted unsupported-platform error and exit 1.

- [ ] **Step 3: Run a focused shell syntax check and verify the expected red state**

```bash
bash -n install.sh
```

Expected: no output.

Then add one deliberate `echo "TODO" >&2; exit 1` after target detection and run:

```bash
bash install.sh
```

Expected: exits non-zero before install logic, proving the script runs under Bash.

Remove the deliberate failure before continuing.

- [ ] **Step 4: Implement shared curl settings with bounded retries and timeouts**

Use one array for all network requests:

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

- [ ] **Step 5: Fetch the latest tag and construct deterministic URLs**

Use the GitHub API only for the tag:

```bash
VERSION=$(curl "${CURL_OPTS[@]}" "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
test -n "$VERSION" || { echo "Failed to resolve latest release tag." >&2; exit 1; }
ARCHIVE="scorpio-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
ARCHIVE_URL="$BASE_URL/$ARCHIVE"
CHECKSUM_URL="$ARCHIVE_URL.sha256"
SIG_URL="$ARCHIVE_URL.sha256.sig"
```

- [ ] **Step 6: Preflight required tools and sidecar availability**

Before download, require `curl`, `tar`, `openssl`, `shasum`, and `bash`.

Then probe the three deterministic URLs:

```bash
curl "${CURL_OPTS[@]}" --head "$ARCHIVE_URL" >/dev/null
curl "${CURL_OPTS[@]}" --head "$CHECKSUM_URL" >/dev/null
curl "${CURL_OPTS[@]}" --head "$SIG_URL" >/dev/null
```

If any probe fails, print:

```text
Latest release does not include <target> yet.
```

- [ ] **Step 7: Download archive and sidecars**

```bash
curl "${CURL_OPTS[@]}" "$ARCHIVE_URL" -o "$TMP/$ARCHIVE"
curl "${CURL_OPTS[@]}" "$CHECKSUM_URL" -o "$TMP/$ARCHIVE.sha256"
curl "${CURL_OPTS[@]}" "$SIG_URL" -o "$TMP/$ARCHIVE.sha256.sig"
```

- [ ] **Step 8: Embed the RSA public key and verify the signed checksum**

Use `packaging/install-signing-public.pem` as the canonical checked-in authoring source. At source-control time, stamp its PEM contents directly into `install.sh` so the published installer remains standalone. Generate the PowerShell XML representation from that same PEM during implementation so both installers stay in sync.

In Bash, embed the PEM block directly in the script, write it to a temp file at runtime, then verify the detached signature:

```bash
cat > "$TMP/install-signing-pubkey.pem" <<'EOF'
<embedded from packaging/install-signing-public.pem>
EOF
openssl dgst -sha256 -verify "$TMP/install-signing-pubkey.pem" -signature "$TMP/$ARCHIVE.sha256.sig" "$TMP/$ARCHIVE.sha256"
```

- [ ] **Step 9: Enforce the canonical checksum format and verify the archive hash**

Require the raw file bytes to be exactly `<64 lowercase hex chars>\n`, then extract the hash from that validated content:

```bash
CHECKSUM_BYTES=$(wc -c < "$TMP/$ARCHIVE.sha256" | tr -d ' ')
test "$CHECKSUM_BYTES" -eq 65 || { echo "Invalid checksum file format." >&2; exit 1; }
EXPECTED_HASH=$(head -c 64 "$TMP/$ARCHIVE.sha256")
LAST_BYTE=$(tail -c 1 "$TMP/$ARCHIVE.sha256")
printf '%s' "$EXPECTED_HASH" | grep -Eq '^[0-9a-f]{64}$' || { echo "Invalid checksum file format." >&2; exit 1; }
test ${#EXPECTED_HASH} -eq 64 || { echo "Invalid checksum file format." >&2; exit 1; }
test -z "$LAST_BYTE" || { echo "Invalid checksum file format." >&2; exit 1; }

ACTUAL_HASH=$(shasum -a 256 "$TMP/$ARCHIVE" | cut -d ' ' -f1 | tr '[:upper:]' '[:lower:]')
test "$EXPECTED_HASH" = "$ACTUAL_HASH"
```

- [ ] **Step 10: Extract and install the staged `scorpio` binary unchanged**

```bash
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
test -f "$TMP/scorpio"
mkdir -p "$INSTALL_DIR"
mv "$TMP/scorpio" "$INSTALL_DIR/scorpio"
chmod +x "$INSTALL_DIR/scorpio"
```

- [ ] **Step 11: Keep the PATH hint, but respect `SCORPIO_INSTALL_DIR`**

If `INSTALL_DIR` is not in `PATH`, print:

```bash
export PATH="$INSTALL_DIR:$PATH"
```

- [ ] **Step 12: Lint with shellcheck**

```bash
shellcheck install.sh
```

Expected: no warnings.

- [ ] **Step 13: Run a local Bash smoke test against a real signed release**

```bash
bash install.sh
```

Expected on a supported platform after the first signed release exists:

```text
Installing scorpio <version> for <target>...
Downloading https://github.com/BigtoC/scorpio-analyst/releases/download/<version>/scorpio-<target>.tar.gz...
Installed: <install-dir>/scorpio
```

Then verify:

```bash
"${SCORPIO_INSTALL_DIR:-$HOME/.local/bin}/scorpio" --version
```

- [ ] **Step 14: Commit**

```bash
git add install.sh
git commit -m "feat: add signed bash installer"
```

---

### Task 3: Write `install.ps1` for signed Windows installs

**Files:**
- Create: `install.ps1`

- [ ] **Step 1: Write the failing Windows runtime contract note**

Lock in the supported runtime assumptions before coding:

```text
install.ps1 must work when invoked by `powershell` (Windows PowerShell 5.1),
must reject non-AMD64 hosts,
and must verify:
- scorpio-x86_64-pc-windows-msvc.zip
- scorpio-x86_64-pc-windows-msvc.zip.sha256
- scorpio-x86_64-pc-windows-msvc.zip.sha256.sig
```

- [ ] **Step 2: Create `install.ps1` with a `try/finally` structure**

Start with:

```powershell
$ErrorActionPreference = "Stop"

$Repo = "BigtoC/scorpio-analyst"
$InstallDir = if ($env:SCORPIO_INSTALL_DIR) { $env:SCORPIO_INSTALL_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())

try {
    New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
    # implementation
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
```

- [ ] **Step 3: Run the red-state script parse check**

```powershell
powershell -NoLogo -NoProfile -NonInteractive -File .\install.ps1
```

Expected initially: fails with a controlled error before install logic is complete.

- [ ] **Step 4: Add bounded timeout/retry helpers for HTTP calls**

Use one helper that retries transient failures for `Invoke-RestMethod` / `Invoke-WebRequest`.

```powershell
function Invoke-WithRetry {
    param(
        [scriptblock]$Action,
        [int]$MaxAttempts = 4,
        [int]$SleepSeconds = 2
    )

    for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
        try {
            return & $Action
        } catch {
            if ($attempt -eq $MaxAttempts) { throw }
            Start-Sleep -Seconds $SleepSeconds
        }
    }
}
```

- [ ] **Step 5: Fetch the latest tag, detect AMD64, and build deterministic URLs**

```powershell
$Release = Invoke-WithRetry { Invoke-RestMethod -TimeoutSec 30 "https://api.github.com/repos/$Repo/releases/latest" }
$Version = $Release.tag_name
if (-not $Version) { throw "Failed to resolve latest release tag." }
$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
if ($Arch -ne "X64" -and $Arch -ne "AMD64") { throw "Unsupported Windows architecture: $Arch" }

$Archive = "scorpio-x86_64-pc-windows-msvc.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"
$ArchiveUrl = "$BaseUrl/$Archive"
$ChecksumUrl = "$ArchiveUrl.sha256"
$SigUrl = "$ArchiveUrl.sha256.sig"
```

- [ ] **Step 6: Probe all three required assets before download**

Use `Invoke-WebRequest -Method Head` under retry logic for the archive and both sidecars. If any probe fails, raise:

```text
Latest release does not include x86_64-pc-windows-msvc yet.
```

- [ ] **Step 7: Download the ZIP plus both sidecars**

```powershell
Invoke-WithRetry { Invoke-WebRequest -TimeoutSec 60 -Uri $ArchiveUrl -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing }
Invoke-WithRetry { Invoke-WebRequest -TimeoutSec 60 -Uri $ChecksumUrl -OutFile (Join-Path $Tmp "$Archive.sha256") -UseBasicParsing }
Invoke-WithRetry { Invoke-WebRequest -TimeoutSec 60 -Uri $SigUrl -OutFile (Join-Path $Tmp "$Archive.sha256.sig") -UseBasicParsing }
```

- [ ] **Step 8: Embed the RSA public key and verify the detached signature in a PowerShell 5.1-compatible way**

Use `packaging/install-signing-public.pem` as the canonical checked-in authoring source, then generate a Windows PowerShell 5.1-compatible `RSAKeyValue` XML representation from that same key during implementation and embed that XML directly into `install.ps1`. Use XML RSA key import rather than PEM parsing APIs that require newer runtimes.

```powershell
$PublicKeyXml = @'
<embedded XML generated from packaging/install-signing-public.pem>
'@

$Rsa = New-Object System.Security.Cryptography.RSACryptoServiceProvider
$Rsa.FromXmlString($PublicKeyXml)
$Sha256 = [System.Security.Cryptography.SHA256]::Create()
$ChecksumBytes = [System.IO.File]::ReadAllBytes((Join-Path $Tmp "$Archive.sha256"))
$SignatureBytes = [System.IO.File]::ReadAllBytes((Join-Path $Tmp "$Archive.sha256.sig"))
if (-not $Rsa.VerifyData($ChecksumBytes, $Sha256, $SignatureBytes)) {
    throw "Checksum signature verification failed."
}
```

- [ ] **Step 9: Verify checksum file format and archive hash**

```powershell
$ChecksumBytes = [System.IO.File]::ReadAllBytes((Join-Path $Tmp "$Archive.sha256"))
$ChecksumText = [System.Text.Encoding]::UTF8.GetString($ChecksumBytes)
if ($ChecksumBytes.Length -ne 65) { throw "Invalid checksum file format." }
if ($ChecksumBytes[64] -ne 10) { throw "Invalid checksum file format." }
$ExpectedHash = [System.Text.Encoding]::UTF8.GetString($ChecksumBytes, 0, 64)
if ($ExpectedHash -notmatch '^[0-9a-f]{64}$') { throw "Invalid checksum file format." }
$ActualHash = (Get-FileHash -Algorithm SHA256 (Join-Path $Tmp $Archive)).Hash.ToLowerInvariant()
if ($ExpectedHash -ne $ActualHash) { throw "Checksum mismatch." }
```

- [ ] **Step 10: Extract and install `scorpio.exe` unchanged**

```powershell
Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Move-Item -Force (Join-Path $Tmp "scorpio.exe") (Join-Path $InstallDir "scorpio.exe")
```

- [ ] **Step 11: Update PATH only if needed**

Normalize the current user PATH into components, append `$InstallDir` only if absent, and then write it back with `[Environment]::SetEnvironmentVariable("Path", $NewPath, "User")`.

- [ ] **Step 12: Lint with PSScriptAnalyzer**

```powershell
Invoke-ScriptAnalyzer -Path install.ps1
```

Expected: no warnings/errors.

- [ ] **Step 13: Run a Windows smoke test against a real signed release**

```powershell
powershell -NoLogo -NoProfile -NonInteractive -File .\install.ps1
```

Expected:

```text
Installing scorpio <version> for x86_64-pc-windows-msvc...
Installed: <install-dir>\scorpio.exe
```

Then verify:

```powershell
& (Join-Path ${env:SCORPIO_INSTALL_DIR} "scorpio.exe") --version
```

Use a temporary `SCORPIO_INSTALL_DIR` when testing locally to avoid clobbering an existing install. If `SCORPIO_INSTALL_DIR` is unset, verify against `%USERPROFILE%\.local\bin\scorpio.exe` instead.

- [ ] **Step 14: Commit**

```bash
git add install.ps1
git commit -m "feat: add signed powershell installer"
```

---

### Task 4: Publish the final installer commands in the README after the first signed release exists

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Write the failing docs expectation**

README must show the exact release-published installer commands, not `raw.githubusercontent.com/.../main/...` commands.

- [ ] **Step 2: Add an Install section with the final commands only after the first signed release is cut**

Add a short `## Install` section with:

```sh
curl -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.sh | bash
```

and:

```powershell
curl.exe -fsSL https://github.com/BigtoC/scorpio-analyst/releases/latest/download/install.ps1 | powershell -NoLogo -NoProfile -NonInteractive -Command -
```

- [ ] **Step 3: Mention rollout order**

Add one note stating these commands should only be advertised after the first release containing installer assets and signed sidecars has been cut.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: publish signed installer commands"
```

---

## Self-Review

**Spec coverage:**
- [x] Release-published `install.sh` / `install.ps1` assets → Task 1 Step 10
- [x] Signed checksum sidecars for each release archive → Task 1 Steps 6-8
- [x] Versionless `scorpio-{target}` archive names shared with `scorpio upgrade` → Task 1 Steps 5-6
- [x] Per-target matrix packaging + non-matrix publish/verify job topology → Task 1 Steps 9-10
- [x] Bash-only Unix bootstrap command → Task 2 Steps 2-3
- [x] Deterministic release URLs with asset probing → Task 2 Steps 5-7 and Task 3 Steps 5-7
- [x] RSA signature verification before checksum verification → Task 2 Step 8 and Task 3 Step 8
- [x] Bounded timeout/retry behavior for API and downloads → Task 2 Step 4 and Task 3 Step 4
- [x] Windows x86_64-only detection → Task 3 Step 5
- [x] Archive contains `scorpio` / `scorpio.exe` directly after packaging rename → Task 1 Step 5 and Tasks 2-3 install steps
- [x] README publishes the final installer commands only after rollout is ready → Task 4

**No placeholders detected.**

**Type/name consistency:** build output remains `scorpio-analyst(.exe)` in `target/.../release/`, but every public release archive and installed binary is `scorpio(.exe)`.
