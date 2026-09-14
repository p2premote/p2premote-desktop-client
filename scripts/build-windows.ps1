param(
    [Parameter(Mandatory = $true)]
    [Alias('v')]
    [string]$Version,
    [switch]$SkipBuild,
    [switch]$SkipNpmCi,
    [switch]$NoSccache,
    [string]$TauriConfig,
    [string]$PackageTarget = 'windows-x64',
    [string]$RustDeskTinyInstaller = $env:RUSTDESK_TINY_INSTALLER
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

$cargoTargetDirectory = Join-Path $projectRoot ("target\{0}" -f $PackageTarget)
$distDirectory = Join-Path $projectRoot ("artifacts\{0}" -f $PackageTarget)
$nsisDirectory = Join-Path $cargoTargetDirectory 'release\bundle\nsis'
$expectedInstallerName = "p2pRemote_${buildVersion}_x64-setup.exe"
$expectedInstallerPath = Join-Path $nsisDirectory $expectedInstallerName
$publishedInstallerName = if ($PackageTarget -eq 'windows-x64') {
    $expectedInstallerName
} else {
    "p2pRemote_${buildVersion}_${PackageTarget}-setup.exe"
}
$publishedInstallerPath = Join-Path $distDirectory $publishedInstallerName

if ($NoSccache) {
    $env:RUSTC_WRAPPER = ''
    Write-Host "sccache disabled; Rust will compile locally"
}
else {
    $null = Get-Command sccache -ErrorAction Stop
    $env:RUSTC_WRAPPER = "sccache"
    Write-Host "sccache enabled (use -NoSccache to disable)"
}

$rustPunchSource = Join-Path $projectRoot '..\p2premote-punch-rs-gonc'
if (-not (Test-Path -LiteralPath $rustPunchSource -PathType Container)) {
    throw "p2premote-punch-rs-gonc source directory not found: $rustPunchSource"
}
$desktopResource = Join-Path $projectRoot 'src-tauri\resources\RustDeskTiny-install.exe'
$desktopInstaller = $desktopResource
if (-not [string]::IsNullOrWhiteSpace($RustDeskTinyInstaller)) {
    $desktopInstaller = [System.IO.Path]::GetFullPath($RustDeskTinyInstaller)
    if (-not (Test-Path -LiteralPath $desktopInstaller -PathType Leaf)) {
        throw "RustDeskTiny installer is missing: $desktopInstaller"
    }
} elseif (-not (Test-Path -LiteralPath $desktopResource -PathType Leaf)) {
    throw "RustDeskTiny installer is missing. Pass -RustDeskTinyInstaller or set RUSTDESK_TINY_INSTALLER."
}
$desktopInstallerProduct = (Get-Item -LiteralPath $desktopInstaller).VersionInfo.ProductName
if ($PackageTarget -eq 'windows-win7-x64') {
    if ($desktopInstallerProduct -ne 'RustDeskTinyLegacy') {
        throw "windows-win7-x64 requires the RustDeskTinyLegacy installer; got product '$desktopInstallerProduct' from $desktopInstaller"
    }
} elseif ($desktopInstallerProduct -eq 'RustDeskTinyLegacy') {
    throw "$PackageTarget cannot package the Windows 7-only RustDeskTinyLegacy installer"
}
$replaceDesktopResource = -not [string]::Equals(
    [System.IO.Path]::GetFullPath($desktopInstaller),
    [System.IO.Path]::GetFullPath($desktopResource),
    [StringComparison]::OrdinalIgnoreCase)
$previousClientVersion = $env:P2PREMOTE_CLIENT_VERSION
$previousWgFfiDir = $env:P2PREMOTE_WG_FFI_DIR
$previousCargoTargetDir = $env:CARGO_TARGET_DIR
$env:P2PREMOTE_CLIENT_VERSION = $buildVersion
$env:CARGO_TARGET_DIR = $cargoTargetDirectory
$wgFfiSource = Join-Path $projectRoot '..\p2premote-wg-ffi'
if (-not (Test-Path -LiteralPath $wgFfiSource -PathType Container)) { throw "p2premote-wg-ffi source directory not found: $wgFfiSource" }
$env:P2PREMOTE_WG_FFI_DIR = (Resolve-Path -LiteralPath $wgFfiSource).Path

$desktopResourceBackup = $null
$desktopResourceOriginallyExisted = Test-Path -LiteralPath $desktopResource -PathType Leaf
if ($replaceDesktopResource) {
    if ($desktopResourceOriginallyExisted) {
        $desktopResourceBackup = [System.IO.Path]::GetTempFileName()
        Copy-Item -LiteralPath $desktopResource -Destination $desktopResourceBackup -Force
    }
    Copy-Item -LiteralPath $desktopInstaller -Destination $desktopResource -Force
    Write-Host "Staged $desktopInstallerProduct installer for $PackageTarget"
}
Push-Location $projectRoot
try {
    # node_modules 可能被 WSL/容器内的 Linux 构建重装为 Linux 版本，每次构建前先恢复 Windows 版本
    if (-not $SkipNpmCi) {
        & npm ci
        if ($LASTEXITCODE -ne 0) {
            throw "npm ci failed with exit code $LASTEXITCODE"
        }
    }
    $tauriCli = Join-Path $projectRoot 'node_modules\@tauri-apps\cli\tauri.js'
    if (-not (Test-Path -LiteralPath $tauriCli -PathType Leaf)) {
        throw "Tauri CLI is missing: $tauriCli"
    }
    $tauriArgs = @($tauriCli, 'build')
    if ($TauriConfig) { $tauriArgs += @('--config', $TauriConfig) }
    & node @tauriArgs
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
    $env:P2PREMOTE_CLIENT_VERSION = $previousClientVersion
    $env:P2PREMOTE_WG_FFI_DIR = $previousWgFfiDir
    $env:CARGO_TARGET_DIR = $previousCargoTargetDir
    if ($replaceDesktopResource) {
        if ($desktopResourceOriginallyExisted) {
            Copy-Item -LiteralPath $desktopResourceBackup -Destination $desktopResource -Force
        } elseif (Test-Path -LiteralPath $desktopResource -PathType Leaf) {
            Remove-Item -LiteralPath $desktopResource -Force
        }
        if ($desktopResourceBackup -and (Test-Path -LiteralPath $desktopResourceBackup -PathType Leaf)) {
            Remove-Item -LiteralPath $desktopResourceBackup -Force
        }
        Write-Host "Restored default RustDeskTiny installer resource"
    }
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
New-Item -ItemType Directory -Path $distDirectory -Force | Out-Null
Move-Item -LiteralPath $expectedInstallerPath -Destination $publishedInstallerPath -Force
$installerHash = (Get-FileHash -LiteralPath $publishedInstallerPath -Algorithm SHA256).Hash
Write-Host "Generated $PackageTarget installer: $publishedInstallerPath"
Write-Host "SHA-256: $installerHash"
