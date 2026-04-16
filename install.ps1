$ErrorActionPreference = "Stop"
# Enable TLS 1.2 for powershell 5.1 compatibility with GitHub.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "BigtoC/scorpio-analyst"
$InstallDir = Join-Path $env:USERPROFILE ".local\bin"
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())

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
            if ($attempt -eq $MaxAttempts) {
                throw
            }
            Start-Sleep -Seconds $SleepSeconds
        }
    }
}

function Get-StatusCode {
    param([Parameter(Mandatory = $true)] [System.Management.Automation.ErrorRecord]$ErrorRecord)

    if ($ErrorRecord.Exception -and $ErrorRecord.Exception.Response) {
        try {
            return [int]$ErrorRecord.Exception.Response.StatusCode
        } catch {
            return $null
        }
    }

    return $null
}

try {
    New-Item -ItemType Directory -Force -Path $Tmp | Out-Null

    $Release = Invoke-WithRetry { Invoke-RestMethod -TimeoutSec 30 "https://api.github.com/repos/$Repo/releases/latest" }
    $Version = $Release.tag_name
    if (-not $Version) {
        throw "Failed to resolve latest release tag."
    }

    $Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    if ($Arch -ne "X64" -and $Arch -ne "AMD64") {
        throw "Unsupported Windows architecture: $Arch"
    }

    $Archive = "scorpio-x86_64-pc-windows-msvc.zip"
    $BaseUrl = "https://github.com/$Repo/releases/download/$Version"
    $ArchiveUrl = "$BaseUrl/$Archive"

    try {
        Invoke-WithRetry { Invoke-WebRequest -TimeoutSec 30 -Method Head -Uri $ArchiveUrl -UseBasicParsing } | Out-Null
    } catch {
        $StatusCode = Get-StatusCode $_
        if ($StatusCode -eq 404) {
            throw "Latest release does not include x86_64-pc-windows-msvc yet."
        }

        throw "Failed to access release archive: $ArchiveUrl`n$($_.Exception.Message)"
    }

    Write-Host "Installing scorpio $Version for x86_64-pc-windows-msvc..."

    Invoke-WithRetry { Invoke-WebRequest -TimeoutSec 60 -Uri $ArchiveUrl -OutFile (Join-Path $Tmp $Archive) -UseBasicParsing } | Out-Null

    Expand-Archive -Path (Join-Path $Tmp $Archive) -DestinationPath $Tmp -Force
    if (-not (Test-Path (Join-Path $Tmp "scorpio.exe"))) {
        throw "Expected scorpio.exe missing from archive."
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Move-Item -Force (Join-Path $Tmp "scorpio.exe") (Join-Path $InstallDir "scorpio.exe")

    Write-Host "Installed: $InstallDir\scorpio.exe"

    $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathParts = @()
    $NormalizedInstallDir = $InstallDir.TrimEnd('\\')
    if ($CurrentPath) {
        $PathParts = $CurrentPath -split ';' |
            Where-Object { $_ -ne '' } |
            ForEach-Object { $_.TrimEnd('\\') }
    }
    if ($NormalizedInstallDir -notin $PathParts) {
        $NewPath = ($PathParts + $InstallDir) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "NOTE: Added $InstallDir to your PATH. Restart your terminal for the change to take effect."
    }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
