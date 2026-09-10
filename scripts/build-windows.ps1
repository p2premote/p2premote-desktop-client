param(
    [Parameter(Mandatory = $true)]
    [Alias('v')]
    [string]$Version,
    [switch]$SkipBuild,
    [switch]$NoSccache,
    [string]$TauriConfig,
    [string]$PackageTarget = 'windows-x64',
    [string]$RustDeskTinyArtifactDir = $env:RUSTDESK_TINY_ARTIFACT_DIR
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

Update-RegexReplace -Path $packageJsonPath -Pattern '(?m)^(\s*)"version":\s*".*"(,?)$' -Replacement ('${1}"version": "' + $buildVersion + '"${2}')
Update-RegexReplace -Path $tauriConfigPath -Pattern '(?m)^(\s*)"version":\s*".*"(,?)$' -Replacement ('${1}"version": "' + $buildVersion + '"${2}')
Update-RegexReplace -Path $cargoTomlPath -Pattern '(?m)^version = ".*"$' -Replacement ('version = "{0}"' -f $buildVersion)

Write-Host "Updated client version to $buildVersion"

if ($SkipBuild) {
    Write-Host "SkipBuild enabled, tauri build was not executed."
    exit 0
}

$nsisDirectory = Join-Path $projectRoot 'target\release\bundle\nsis'
$expectedInstallerName = "p2pRemote_${buildVersion}_x64-setup.exe"
$expectedInstallerPath = Join-Path $nsisDirectory $expectedInstallerName
$modernInstallerBackup = "$expectedInstallerPath.windows-x64-backup"
if ($PackageTarget -ne 'windows-x64' -and (Test-Path -LiteralPath $expectedInstallerPath -PathType Leaf)) {
    Copy-Item -LiteralPath $expectedInstallerPath -Destination $modernInstallerBackup -Force
}

if ($NoSccache) {
    $env:RUSTC_WRAPPER = ''
    Write-Host "sccache disabled; Rust will compile locally"
}
else {
    $null = Get-Command sccache -ErrorAction Stop
    $env:RUSTC_WRAPPER = "sccache"
    Write-Host "sccache enabled (use -NoSccache to disable)"
}

$punchSource = Join-Path $projectRoot '..\p2premote-punch'
if (-not (Test-Path -LiteralPath $punchSource -PathType Container)) {
    throw "p2premote-punch source directory not found: $punchSource"
}
$desktopResource = Join-Path $projectRoot 'src-tauri\resources\RustDeskTiny'
if (-not [string]::IsNullOrWhiteSpace($RustDeskTinyArtifactDir)) {
    $desktopArtifact = [System.IO.Path]::GetFullPath($RustDeskTinyArtifactDir)
    $desktopExecutable = Join-Path $desktopArtifact 'RustDeskTiny.exe'
    if (-not (Test-Path -LiteralPath $desktopExecutable -PathType Leaf)) {
        throw "RustDeskTiny artifact is missing: $desktopExecutable"
    }
    if (Test-Path -LiteralPath $desktopResource) {
        Remove-Item -LiteralPath $desktopResource -Recurse -Force
    }
    New-Item -ItemType Directory -Path $desktopResource | Out-Null
    Copy-Item -Path (Join-Path $desktopArtifact '*') -Destination $desktopResource -Recurse -Force
    Write-Host "Copied RustDeskTiny release artifact: $desktopResource"
} elseif (-not (Test-Path -LiteralPath (Join-Path $desktopResource 'RustDeskTiny.exe') -PathType Leaf)) {
    throw "RustDeskTiny resource is missing. Pass -RustDeskTinyArtifactDir or set RUSTDESK_TINY_ARTIFACT_DIR."
}
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
    $tauriArgs = @('tauri', 'build')
    if ($TauriConfig) { $tauriArgs += @('--config', $TauriConfig) }
    & npx @tauriArgs
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
    $env:P2PREMOTE_CLIENT_VERSION = $previousClientVersion
    $env:P2PREMOTE_PUNCH_DIR = $previousPunchDir
}
if (-not $NoSccache) { & sccache --show-stats }

# Tauri normally includes x64 in the NSIS filename, but normalize it here so
# every Windows package produced by this script is accepted by package
# management and cannot be mistaken for another architecture.
if (-not (Test-Path -LiteralPath $expectedInstallerPath -PathType Leaf)) {
    $candidates = @(Get-ChildItem -LiteralPath $nsisDirectory -Filter "p2pRemote_${buildVersion}*.exe" -File)
    if ($candidates.Count -ne 1) {
        throw "expected exactly one Windows x64 NSIS installer under $nsisDirectory, found $($candidates.Count)"
    }
    Move-Item -LiteralPath $candidates[0].FullName -Destination $expectedInstallerPath
}
$targetInstallerPath = Join-Path $nsisDirectory "p2pRemote_${buildVersion}_${PackageTarget}-setup.exe"
if ($PackageTarget -ne 'windows-x64') {
    Move-Item -LiteralPath $expectedInstallerPath -Destination $targetInstallerPath -Force
    if (Test-Path -LiteralPath $modernInstallerBackup -PathType Leaf) {
        Move-Item -LiteralPath $modernInstallerBackup -Destination $expectedInstallerPath -Force
        Write-Host "Preserved existing windows-x64 installer: $expectedInstallerPath"
    }
} else {
    $targetInstallerPath = $expectedInstallerPath
}
$installerHash = (Get-FileHash -LiteralPath $targetInstallerPath -Algorithm SHA256).Hash
Write-Host "Generated $PackageTarget installer: $targetInstallerPath"
Write-Host "SHA-256: $installerHash"
