# 确保 src-tauri/resources 下存在指定的 RustDeskTiny 发布产物（Windows 侧）。
# 本地已存在时直接复用（本地构建语义，不校验哈希）；缺失时按
# scripts/rustdesktiny-artifacts.env 钉版从 GitHub Release 下载并校验 SHA-256。
#
# 本脚本设计为被 build-windows.ps1 以 & 调用（同进程），因此复用 return /
# throw 传递结果，不使用 exit。
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('windows-x64', 'windows-win7-x64')]
    [string]$Kind
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$pinFile = Join-Path $PSScriptRoot 'rustdesktiny-artifacts.env'
if (-not (Test-Path -LiteralPath $pinFile -PathType Leaf)) {
    throw "missing RustDeskTiny pin file: $pinFile"
}
$pin = @{}
foreach ($line in Get-Content -LiteralPath $pinFile) {
    if ($line -match '^\s*([A-Z0-9_]+)=(.*)$') {
        $pin[$Matches[1]] = $Matches[2].Trim()
    }
}

if ($Kind -eq 'windows-win7-x64') {
    # RustDeskTinyLegacy 是独立仓库，版本线与主仓分开
    $asset = 'RustDeskTinyLegacy-install.exe'
    $version = $pin['RUSTDESK_TINY_LEGACY_VERSION']
    $baseUrl = $pin['RUSTDESK_TINY_LEGACY_BASE_URL']
    $sha256 = $pin['RUSTDESK_TINY_WINDOWS_WIN7_X64_SHA256']
}
else {
    $asset = 'RustDeskTiny-install.exe'
    $version = $pin['RUSTDESK_TINY_VERSION']
    $baseUrl = $pin['RUSTDESK_TINY_BASE_URL']
    $sha256 = $pin['RUSTDESK_TINY_WINDOWS_X64_SHA256']
}
$target = Join-Path $repoRoot "src-tauri\resources\$asset"
if (Test-Path -LiteralPath $target -PathType Leaf) {
    Write-Host "Reusing local RustDeskTiny artifact: $target"
    return
}

# 只有确实需要下载时才要求钉版信息齐全（本地复用不校验）
if ([string]::IsNullOrWhiteSpace($version) -or
    [string]::IsNullOrWhiteSpace($baseUrl) -or
    [string]::IsNullOrWhiteSpace($sha256)) {
    throw "RustDeskTiny pin file is missing version, base URL, or the $Kind SHA-256"
}

$curl = Get-Command curl.exe -ErrorAction SilentlyContinue
if (-not $curl) {
    throw "curl.exe is required to download $asset"
}

$url = "$baseUrl/$version/$asset"
Write-Host "Downloading $url"
$resourcesDir = Split-Path -Parent $target
New-Item -ItemType Directory -Path $resourcesDir -Force | Out-Null
$partial = "$target.partial"
try {
    $curlArgs = @('-fL', '--retry', '3', '--connect-timeout', '15', '-o', $partial)
    # 标准代理环境变量 curl 自行识别；仓库约定的 P2PREMOTE_BUILD_PROXY 显式传入
    if (-not [string]::IsNullOrWhiteSpace($env:P2PREMOTE_BUILD_PROXY)) {
        $curlArgs += @('-x', $env:P2PREMOTE_BUILD_PROXY)
    }
    & $curl.Source @curlArgs $url
    if ($LASTEXITCODE -ne 0) {
        throw "download failed: $url"
    }

    $actual = (Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $sha256) {
        throw "SHA-256 mismatch for ${asset}: expected $sha256, got $actual"
    }
    Move-Item -LiteralPath $partial -Destination $target -Force
}
finally {
    if (Test-Path -LiteralPath $partial -PathType Leaf) {
        Remove-Item -LiteralPath $partial -Force
    }
}
Write-Host "Verified RustDeskTiny $Kind artifact: $target"
