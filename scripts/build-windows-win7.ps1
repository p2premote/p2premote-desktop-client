param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$NoSccache,
    [string]$RustDeskTinyLegacyInstaller
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$installer = [System.IO.Path]::GetFullPath((Join-Path $projectRoot '..\webview\MicrosoftEdgeWebView2RuntimeInstallerX64V109.0.1518.140.exe'))
# WebView2 v109 离线安装器不入 git（../webview/ 仅本地存在）；干净环境按钉版
# 从 rustdeskTiny 仓库 Release 自动下载并校验（微软官方已不提供该版本直链）
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    & (Join-Path $PSScriptRoot 'fetch-rustdesktiny-artifact.ps1') -Kind 'webview2-win7-x64'
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw "WebView2 v109 offline installer is still missing after download: $installer"
    }
}
$legacyInstallerProvided = -not [string]::IsNullOrWhiteSpace($RustDeskTinyLegacyInstaller)
if (-not $legacyInstallerProvided) {
    $RustDeskTinyLegacyInstaller = Join-Path $projectRoot 'src-tauri\resources\RustDeskTinyLegacy-install.exe'
}
$RustDeskTinyLegacyInstaller = [System.IO.Path]::GetFullPath($RustDeskTinyLegacyInstaller)
if (-not (Test-Path -LiteralPath $RustDeskTinyLegacyInstaller -PathType Leaf)) {
    if ($legacyInstallerProvided) {
        throw "RustDeskTinyLegacy installer is missing: $RustDeskTinyLegacyInstaller"
    }
    # 产物不进 git：默认路径缺失时（Jenkins / GitHub Actions 等干净环境）按钉版自动下载
    & (Join-Path $PSScriptRoot 'fetch-rustdesktiny-artifact.ps1') -Kind 'windows-win7-x64'
    if (-not (Test-Path -LiteralPath $RustDeskTinyLegacyInstaller -PathType Leaf)) {
        throw "RustDeskTinyLegacy installer is still missing after download. Pass -RustDeskTinyLegacyInstaller to use a local file."
    }
}

if ([Environment]::Is64BitOperatingSystem -eq $false) { throw 'Win7 package builds require an x64 build host.' }

$generatedDir = Join-Path $projectRoot 'src-tauri\.generated'
New-Item -ItemType Directory -Path $generatedDir -Force | Out-Null
$template = Get-Content -LiteralPath (Join-Path $projectRoot 'src-tauri\installer-v1.nsi') -Raw
$escapedInstaller = $installer.Replace('$', '$$').Replace('"', '$\"')
$template = $template.Replace('!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"', '!define INSTALLWEBVIEW2MODE "offlineInstaller"')
$template = $template.Replace('!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"', '!define WEBVIEW2INSTALLERARGS "/silent --msedgewebview --system-level --do-not-launch-msedge"')
$template = $template.Replace('!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"', "!define WEBVIEW2INSTALLERPATH `"$escapedInstaller`"")
$template = $template.Replace('!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"', '!define MINIMUMWEBVIEW2VERSION "109.0.1518.78"')
[System.IO.File]::WriteAllText((Join-Path $generatedDir 'installer-win7.nsi'), $template, [System.Text.UTF8Encoding]::new($false))

$env:RUSTUP_TOOLCHAIN = '1.77.2'
$env:P2PREMOTE_RELEASE_TARGET = 'windows-win7-x64'
& (Join-Path $PSScriptRoot 'build-windows.ps1') `
    -Version $Version `
    -NoSccache:$NoSccache `
    -TauriConfig 'src-tauri/tauri.win7.conf.json' `
    -PackageTarget 'windows-win7-x64' `
    -RustDeskTinyInstaller $RustDeskTinyLegacyInstaller
if ($LASTEXITCODE -ne 0) { throw "Win7 build failed with exit code $LASTEXITCODE" }
Write-Host 'Win7 package is build-ready; compatibility remains pending real Win7 SP1 x64 validation.'
