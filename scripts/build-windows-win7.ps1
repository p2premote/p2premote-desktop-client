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
# 显式 UTF-8：模板内含中文注释，PS 5.1 对无 BOM 文件按 ANSI 解码时，UTF-8
# 注释的尾字节会与换行符组成双字节字符而吞掉换行，把下一行并进注释。
$template = Get-Content -LiteralPath (Join-Path $projectRoot 'src-tauri\installer-v1.nsi') -Raw -Encoding UTF8
$escapedInstaller = $installer.Replace('$', '$$').Replace('"', '$\"')
$template = $template.Replace('!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"', '!define INSTALLWEBVIEW2MODE "offlineInstaller"')
# 载荷为浏览器/WebView2 共享 mini_installer，产品由参数定向。此参数集是
# 2026-09-10 本机实装成功、2026-10-01 复验 exit 0 的原样命令（msedge_installer.log
# 可查）。三个硬约束（均实机验证）：
#   1) 必须 --msedgewebview，否则默认装出 Edge 浏览器；
#   2) 不能带 /silent、/install——"unsupported switch for WebView" 退出码 53；
#   3) --system-level 必须显式给（清单原词省略它是因为 EdgeUpdate 会动态补）。
# 注意 installer-v1.nsi 的 offlineInstaller 分支不会追加 /install（其他模式会）。
$template = $template.Replace('!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"', '!define WEBVIEW2INSTALLERARGS "--msedgewebview --system-level --verbose-logging --do-not-launch-msedge"')
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
