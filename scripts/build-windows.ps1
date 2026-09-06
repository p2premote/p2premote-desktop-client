param(
    [Parameter(Mandatory = $true)]
    [Alias('v')]
    [string]$Version,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "version must be in semver format like 1.2.3"
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$gitCommitOutput = & git -C $projectRoot rev-parse --short=6 HEAD
if ($LASTEXITCODE -ne 0) {
    throw "failed to resolve the current git commit id"
}
$gitCommit = $gitCommitOutput.Trim()
if ($gitCommit -notmatch '^[0-9a-f]{6}$') {
    throw "failed to resolve the current git commit id"
}
$buildVersion = "$Version-$gitCommit"
$packageJsonPath = Join-Path $projectRoot 'package.json'
$tauriConfigPath = Join-Path $projectRoot 'src-tauri\tauri.conf.json'
$cargoTomlPath = Join-Path $projectRoot 'src-tauri\Cargo.toml'

function Write-Utf8NoBomFile {
    param(
        [string]$Path,
        [string]$Content
    )

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $utf8NoBom)
}

function Update-RegexReplace {
    param(
        [string]$Path,
        [string]$Pattern,
        [string]$Replacement
    )

    $content = Get-Content -Path $Path -Raw
    $regex = [System.Text.RegularExpressions.Regex]::new($Pattern)
    $matches = $regex.Matches($content)
    if ($matches.Count -ne 1) {
        throw "expected exactly one version field in $Path, found $($matches.Count)"
    }

    $updated = $regex.Replace($content, $Replacement)
    Write-Utf8NoBomFile -Path $Path -Content $updated
}

Update-RegexReplace -Path $packageJsonPath -Pattern '(?m)^  "version": ".*",$' -Replacement ('  "version": "{0}",' -f $buildVersion)
Update-RegexReplace -Path $tauriConfigPath -Pattern '(?m)^  "version": ".*",$' -Replacement ('  "version": "{0}",' -f $buildVersion)
Update-RegexReplace -Path $cargoTomlPath -Pattern '(?m)^version = ".*"$' -Replacement ('version = "{0}"' -f $buildVersion)

Write-Host "Updated client version to $buildVersion"

if ($SkipBuild) {
    Write-Host "SkipBuild enabled, tauri build was not executed."
    exit 0
}

$punchSource = Join-Path $projectRoot '..\p2premote-punch'
if (-not (Test-Path -LiteralPath $punchSource -PathType Container)) {
    throw "p2premote-punch source directory not found: $punchSource"
}
$engineRoot = Join-Path $projectRoot '..\remoteDesk\remote-desktop-engine'
$engineBuildScript = Join-Path $engineRoot 'scripts\build-windows.ps1'
$engineArtifact = Join-Path $engineRoot 'artifacts\windows-x86_64\p2premote-desktop-engine.exe'
if (-not (Test-Path -LiteralPath $engineBuildScript -PathType Leaf)) {
    throw "Windows Engine build script not found: $engineBuildScript"
}
& $engineBuildScript
if ($LASTEXITCODE -ne 0) {
    throw "Windows Engine build script failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $engineArtifact -PathType Leaf)) {
    throw "Windows Engine artifact is missing after build: $engineArtifact"
}
$engineResource = Join-Path $projectRoot 'src-tauri\resources\p2premote-desktop-engine.exe'
Copy-Item -LiteralPath $engineArtifact -Destination $engineResource -Force
Write-Host "Copied Windows Engine resource: $engineResource"
$previousClientVersion = $env:P2PREMOTE_CLIENT_VERSION
$previousPunchDir = $env:P2PREMOTE_PUNCH_DIR
$env:P2PREMOTE_CLIENT_VERSION = $buildVersion
$env:P2PREMOTE_PUNCH_DIR = (Resolve-Path -LiteralPath $punchSource).Path

Push-Location $projectRoot
try {
    # node_modules 可能被 WSL/容器内的 Linux 构建重装为 Linux 版本，每次构建前先恢复 Windows 版本
    & npm ci
    if ($LASTEXITCODE -ne 0) {
        throw "npm ci failed with exit code $LASTEXITCODE"
    }
    & npx tauri build
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
    $env:P2PREMOTE_CLIENT_VERSION = $previousClientVersion
    $env:P2PREMOTE_PUNCH_DIR = $previousPunchDir
}

# Tauri normally includes x64 in the NSIS filename, but normalize it here so
# every Windows package produced by this script is accepted by package
# management and cannot be mistaken for another architecture.
$nsisDirectory = Join-Path $projectRoot 'target\release\bundle\nsis'
$expectedInstallerName = "p2pRemote_${buildVersion}_x64-setup.exe"
$expectedInstallerPath = Join-Path $nsisDirectory $expectedInstallerName
if (-not (Test-Path -LiteralPath $expectedInstallerPath -PathType Leaf)) {
    $candidates = @(Get-ChildItem -LiteralPath $nsisDirectory -Filter "p2pRemote_${buildVersion}*.exe" -File)
    if ($candidates.Count -ne 1) {
        throw "expected exactly one Windows x64 NSIS installer under $nsisDirectory, found $($candidates.Count)"
    }
    Move-Item -LiteralPath $candidates[0].FullName -Destination $expectedInstallerPath
}
Write-Host "Generated Windows x64 installer: $expectedInstallerPath"
