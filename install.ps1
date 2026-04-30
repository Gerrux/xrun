# xrun installer for Windows — https://github.com/gerrux/xrun
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/gerrux/xrun/main/install.ps1 | iex
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/main/install.ps1'))) -Version v0.4.0
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/main/install.ps1'))) -WithSkill
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/main/install.ps1'))) -SkillOnly

param(
    [string]$Version    = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\xrun\bin",
    [switch]$WithSkill,
    [switch]$SkillOnly
)

$ErrorActionPreference = "Stop"
$Repo     = "gerrux/xrun"
$Target   = "x86_64-pc-windows-msvc"
$RawBase  = "https://raw.githubusercontent.com/$Repo/main"

function Install-Skill {
    $SkillDir = "$env:USERPROFILE\.claude\skills\xrun"
    New-Item -ItemType Directory -Force -Path $SkillDir | Out-Null
    $SkillUrl = "$RawBase/claude/skill.md"
    Invoke-WebRequest -Uri $SkillUrl -OutFile "$SkillDir\SKILL.md" -UseBasicParsing
    Write-Host "Claude Code skill installed -> $SkillDir\SKILL.md"
}

if ($SkillOnly) {
    Install-Skill
    exit 0
}

# ── resolve version ───────────────────────────────────────────────────────────
if (-not $Version) {
    try {
        $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $release.tag_name
    } catch {
        Write-Error "Could not determine latest version. Pass -Version v0.4.0 explicitly."
        exit 1
    }
}

# ── download & install binary ─────────────────────────────────────────────────
$Archive = "xrun-$Version-$Target.zip"
$Url     = "https://github.com/$Repo/releases/download/$Version/$Archive"
$Tmp     = [System.IO.Path]::GetTempPath()
$ZipPath = Join-Path $Tmp $Archive

Write-Host "Installing xrun $Version ($Target) -> $InstallDir"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

Write-Host "Downloading $Url ..."
Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

Expand-Archive -Path $ZipPath -DestinationPath $Tmp -Force
Remove-Item $ZipPath -Force

$Src = Join-Path $Tmp "xrun.exe"
$Dst = Join-Path $InstallDir "xrun.exe"
Move-Item -Path $Src -Destination $Dst -Force

# ── add to PATH ───────────────────────────────────────────────────────────────
$UserPath = [System.Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [System.Environment]::SetEnvironmentVariable(
        "PATH", "$InstallDir;$UserPath", "User"
    )
    Write-Host "Added $InstallDir to user PATH (restart your terminal)"
}

Write-Host ""
Write-Host "xrun $Version installed to $Dst"

# ── optional: Claude Code skill ───────────────────────────────────────────────
if ($WithSkill) {
    Write-Host ""
    Install-Skill
}

Write-Host ""
Write-Host "Run 'xrun doctor' to verify your setup."
