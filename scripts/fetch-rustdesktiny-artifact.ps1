# 确保 src-tauri/resources 下存在指定的 RustDeskTiny 发布产物（Windows 侧）。
# 本地已存在时直接复用（本地构建语义，不校验哈希）；缺失时按
# scripts/rustdesktiny-artifacts.env 钉版从 GitHub Release 下载并校验 SHA-256。
#
# 本脚本设计为被 build-windows.ps1 以 & 调用（同进程），因此复用 return /
# throw 传递结果，不使用 exit。
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('windows-x64', 'windows-win7-x64', 'webview2-win7-x64')]
    [string]$Kind
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$pinFile = Join-Path $PSScriptRoot 'rustdesktiny-artifacts.env'
if (-not (Test-Path -LiteralPath $pinFile -PathType Leaf)) {
    throw "missing RustDeskTiny pin file: $pinFile"
}
$pin = @{}
# 显式 UTF-8：无 BOM 的中文注释在 Windows PowerShell 5.1 下会被按 ANSI
# 误解码，注释行末 UTF-8 尾字节可与换行符组成一个双字节字符而吞掉换行，
# 使紧随其后的键值行解析失败
foreach ($line in Get-Content -LiteralPath $pinFile -Encoding UTF8) {
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
    $target = Join-Path $repoRoot "src-tauri\resources\$asset"
}
elseif ($Kind -eq 'webview2-win7-x64') {
    # WebView2 Runtime v109（Win7/8 最后版本）离线安装器：本地布局在 ../webview/。
    # 134MB 超 GitHub 单文件 100MB 上限，无法直接入库，拆分片存放于仓库
    # packaging/webview2-v109/，缺失时按序拼接并校验 SHA-256。
    $version = $pin['WEBVIEW2_WIN7_X64_VERSION']
    $asset = "MicrosoftEdgeWebView2RuntimeInstallerX64V$version.exe"
    $sha256 = $pin['WEBVIEW2_WIN7_X64_SHA256']
    $target = Join-Path $repoRoot "..\webview\$asset"
    if (Test-Path -LiteralPath $target -PathType Leaf) {
        Write-Host "Reusing local webview2 artifact: $target"
        return
    }
    if ([string]::IsNullOrWhiteSpace($version) -or [string]::IsNullOrWhiteSpace($sha256)) {
        throw "pin file is missing WEBVIEW2_WIN7_X64_VERSION/SHA256 for $Kind"
    }
    $partsDir = Join-Path $repoRoot 'packaging\webview2-v109'
    $parts = @(Get-ChildItem -LiteralPath $partsDir -Filter 'webview2-v109.exe.part-*.bin' -ErrorAction SilentlyContinue | Sort-Object Name)
    if ($parts.Count -lt 2) {
        throw "webview2 v109 split parts not found in $partsDir (expected webview2-v109.exe.part-*.bin)"
    }
    $webviewDir = Split-Path -Parent $target
    New-Item -ItemType Directory -Path $webviewDir -Force | Out-Null
    $partial = "$target.partial"
    try {
        $out = [System.IO.File]::Open($partial, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
        try {
            foreach ($part in $parts) {
                $in = [System.IO.File]::OpenRead($part.FullName)
                try { $in.CopyTo($out) } finally { $in.Dispose() }
            }
        }
        finally { $out.Dispose() }
        $actual = (Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $sha256) {
            throw "SHA-256 mismatch for reassembled ${asset}: expected $sha256, got $actual"
        }
        Move-Item -LiteralPath $partial -Destination $target -Force
    }
    finally {
        if (Test-Path -LiteralPath $partial -PathType Leaf) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
    Write-Host "Verified webview2 $Kind artifact: $target"
    return
}
else {
    $asset = 'RustDeskTiny-install.exe'
    $version = $pin['RUSTDESK_TINY_VERSION']
    $baseUrl = $pin['RUSTDESK_TINY_BASE_URL']
    $sha256 = $pin['RUSTDESK_TINY_WINDOWS_X64_SHA256']
    $target = Join-Path $repoRoot "src-tauri\resources\$asset"
}
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
