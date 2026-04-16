$ErrorActionPreference = "Stop"

$Repo       = "BigtoC/scorpio-analyst"
$BinaryName = "scorpio.exe"
$Target     = "x86_64-pc-windows-msvc"
$InstallDir = Join-Path $env:USERPROFILE ".local\bin"

# --- Fetch latest release tag ---
$Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
$Version = $Release.tag_name
if (-not $Version) {
    Write-Error "Could not determine latest release version. Check https://github.com/$Repo/releases"
    exit 1
}

Write-Host "Installing scorpio $Version for $Target..."

$Archive = "scorpio-analyst-$Version-$Target.zip"
$Url     = "https://github.com/$Repo/releases/download/$Version/$Archive"
$Tmp     = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())

try {
    New-Item -ItemType Directory -Path $Tmp | Out-Null

    # --- Download ---
    Write-Host "Downloading $Url..."
    Invoke-WebRequest -Uri $Url -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing
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
    $PathParts = ($CurrentPath -split ';') | Where-Object { $_ -ne '' }
    if ($InstallDir -notin $PathParts) {
        $NewPath = ($PathParts + $InstallDir) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host ""
        Write-Host "NOTE: Added $InstallDir to your PATH."
        Write-Host "Restart your terminal for the change to take effect."
    }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
