# xrun installer for Windows - https://github.com/gerrux/xrun
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1 | iex
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -Version v0.7.2
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -NoTui
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -InstallPip
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -WithSkill
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -SkillOnly

param(
    [string]$Version    = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\xrun\bin",
    [switch]$WithSkill,
    [switch]$SkillOnly,
    [switch]$WithTui,
    [switch]$NoTui,
    [switch]$TuiOnly,
    [switch]$InstallPip
)

$ErrorActionPreference = "Stop"
$Repo     = "gerrux/xrun"
$Target   = "x86_64-pc-windows-msvc"
$RawBase  = "https://raw.githubusercontent.com/$Repo/master"

function Install-Skill {
    $SkillDir = "$env:USERPROFILE\.claude\skills\xrun"
    New-Item -ItemType Directory -Force -Path $SkillDir | Out-Null
    $SkillUrl = "$RawBase/claude/skill.md"
    Invoke-WebRequest -Uri $SkillUrl -OutFile "$SkillDir\SKILL.md" -UseBasicParsing
    Write-Host "Claude Code skill installed -> $SkillDir\SKILL.md"
}

# Every release after v0.9.0 ships SHA256SUMS, so a missing one there means a
# broken or tampered download, not an old release. An unparsable version
# counts as new (fail closed).
function Test-SumsRequired {
    param([Parameter(Mandatory = $true)][string]$ReleaseVersion)
    $core = $ReleaseVersion.TrimStart('v') -replace '-.*$', ''
    $parsed = $null
    if (-not [version]::TryParse($core, [ref]$parsed)) { return $true }
    return $parsed -gt [version]'0.9.0'
}

function Test-ArchiveChecksum {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$SumsUrl,
        [Parameter(Mandatory = $true)][string]$ReleaseVersion
    )

    $sumsText = $null
    try {
        $sumsText = (Invoke-WebRequest -Uri $SumsUrl -UseBasicParsing).Content
        if ($sumsText -is [byte[]]) { $sumsText = [System.Text.Encoding]::UTF8.GetString($sumsText) }
    } catch {
        $status = $null
        if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
        if ($status -eq 404) {
            if (Test-SumsRequired $ReleaseVersion) {
                throw "Release $ReleaseVersion has no SHA256SUMS; refusing to install unverified."
            }
            Write-Warning "release has no SHA256SUMS, skipping verification"
            return
        }
        throw "Could not download SHA256SUMS from ${SumsUrl}: $($_.Exception.Message)"
    }

    $expected = $null
    foreach ($line in ($sumsText -split "`r?`n")) {
        if ($line -match '^\s*([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $Name) {
            $expected = $Matches[1]
            break
        }
    }
    if (-not $expected) {
        throw "SHA256SUMS has no entry for $Name; refusing to install."
    }

    $actual = (Get-FileHash -Algorithm SHA256 -Path $Path).Hash
    if ($actual -ne $expected) {
        throw "Checksum mismatch for ${Name}:`n  expected: $($expected.ToLower())`n  actual:   $($actual.ToLower())`nRefusing to install."
    }
    Write-Host "Checksum OK ($Name)"
}

function Test-Python311 {
    param([string]$Exe, [string[]]$PrefixArgs = @())
    try {
        $args = @($PrefixArgs + @("-c", "import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)"))
        & $Exe @args | Out-Null
        return $LASTEXITCODE -eq 0
    } catch {
        return $false
    }
}

function Find-Python {
    $candidates = @(
        @{ Exe = "py"; Args = @("-3.11") },
        @{ Exe = "python"; Args = @() },
        @{ Exe = "python3"; Args = @() }
    )

    foreach ($candidate in $candidates) {
        $cmd = Get-Command $candidate.Exe -ErrorAction SilentlyContinue
        if ($cmd -and (Test-Python311 $candidate.Exe $candidate.Args)) {
            return [pscustomobject]@{
                Exe  = $candidate.Exe
                Args = $candidate.Args
            }
        }
    }

    return $null
}

function Invoke-Python {
    param(
        [Parameter(Mandatory = $true)]$Python,
        [Parameter(Mandatory = $true)][string[]]$Args
    )
    $allArgs = @($Python.Args + $Args)
    & $Python.Exe @allArgs
}

function Ensure-Pip {
    param([Parameter(Mandatory = $true)]$Python)

    Invoke-Python $Python @("-m", "pip", "--version") | Out-Null
    if ($LASTEXITCODE -eq 0) {
        return
    }

    if ($InstallPip) {
        Write-Host "pip not found; trying ensurepip..."
        Invoke-Python $Python @("-m", "ensurepip", "--upgrade")
        if ($LASTEXITCODE -ne 0) {
            throw "ensurepip failed. Install pip manually and re-run this installer."
        }
        Invoke-Python $Python @("-m", "pip", "--version") | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "ensurepip finished, but pip is still not available. Install pip manually and re-run this installer."
        }
        return
    }

    throw "pip not found. Re-run with -InstallPip to try 'python -m ensurepip --upgrade', or install pip manually."
}

function Install-Tui {
    $python = Find-Python
    if (-not $python) {
        throw "Python >= 3.11 is required for xrun-tui. Install Python 3.11+ and re-run, or pass -NoTui for CLI-only install."
    }

    Ensure-Pip $python

    $tuiRef = if ($Version) { $Version } else { "master" }
    $tuiUrl = "git+https://github.com/$Repo.git@$tuiRef#subdirectory=python/xrun_tui"
    Write-Host "Installing xrun-tui from $tuiRef..."
    Invoke-Python $python @("-m", "pip", "install", "--user", $tuiUrl)
    if ($LASTEXITCODE -ne 0) {
        throw "xrun-tui installation failed"
    }
    Write-Host "xrun-tui installed"
}

function Resolve-Version {
    if (-not $Version) {
        try {
            $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
            $script:Version = $release.tag_name
        } catch {
            Write-Error "Could not determine latest version. Pass -Version v0.7.2 explicitly."
            exit 1
        }
    }
}

if ($SkillOnly) {
    Install-Skill
    exit 0
}

Resolve-Version

if ($TuiOnly) {
    Install-Tui
    Write-Host ""
    Write-Host "Run 'xrun-tui' to start the TUI."
    exit 0
}

$Archive = "xrun-$Version-$Target.zip"
$Url     = "https://github.com/$Repo/releases/download/$Version/$Archive"
$Tmp     = [System.IO.Path]::GetTempPath()
$ZipPath = Join-Path $Tmp $Archive

Write-Host "Installing xrun $Version ($Target) -> $InstallDir"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

Write-Host "Downloading $Url ..."
Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

try {
    Test-ArchiveChecksum -Path $ZipPath -Name $Archive `
        -SumsUrl "https://github.com/$Repo/releases/download/$Version/SHA256SUMS" `
        -ReleaseVersion $Version
} catch {
    Remove-Item $ZipPath -Force -ErrorAction SilentlyContinue
    throw
}

Expand-Archive -Path $ZipPath -DestinationPath $Tmp -Force
Remove-Item $ZipPath -Force

$Src = Join-Path $Tmp "xrun.exe"
$Dst = Join-Path $InstallDir "xrun.exe"
if (Test-Path $Dst) {
    # A running xrun (poll daemon, open TUI) locks xrun.exe against overwrite,
    # but Windows still lets it be renamed; the old process keeps its image.
    $Old = Join-Path $InstallDir "xrun.old.exe"
    Remove-Item $Old -Force -ErrorAction SilentlyContinue
    if (Test-Path $Old) {
        # The previous old copy is itself still running.
        $Old = Join-Path $InstallDir ("xrun.old-{0}.exe" -f (Get-Date -Format "yyyyMMddHHmmss"))
    }
    Move-Item -Path $Dst -Destination $Old -Force
}
Move-Item -Path $Src -Destination $Dst -Force

$UserPath = [System.Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [System.Environment]::SetEnvironmentVariable(
        "PATH", "$InstallDir;$UserPath", "User"
    )
    Write-Host "Added $InstallDir to user PATH (restart your terminal)"
}

Write-Host ""
Write-Host "xrun $Version installed to $Dst"

if ($WithSkill) {
    Write-Host ""
    Install-Skill
}

if (-not $NoTui) {
    Write-Host ""
    Install-Tui
}

Write-Host ""
Write-Host "Run 'xrun doctor' to verify your setup."
