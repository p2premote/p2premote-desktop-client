param(
    [Parameter(Mandatory = $true)]
    [Alias('v')]
    [string]$Version,
    [switch]$SkipBuild,
    [switch]$SkipNpmCi,
    [switch]$NoSccache, # Retained for compatibility; builds always use rustc directly.
    [string]$TauriConfig,
    [string]$PackageTarget = 'windows-x64',
    [string]$RustDeskTinyInstaller = $env:RUSTDESK_TINY_INSTALLER,
    # 不能命名为 Debug：高级脚本的公共参数已占用 -Debug
    [switch]$DebugBuild
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

function Set-ClientVersion {
    # \r? tolerates CRLF checkouts: GitHub Windows runners configure
    # core.autocrlf=true, so a plain "$" anchor never matches there and the
    # build dies with "expected exactly one version field ... found 0".
    Update-RegexReplace -Path $packageJsonPath -Pattern '(?m)^(\s*)"version":\s*".*"(,?)\r?$' -Replacement ('${1}"version": "' + $buildVersion + '"${2}')
    Update-RegexReplace -Path $tauriConfigPath -Pattern '(?m)^(\s*)"version":\s*".*"(,?)\r?$' -Replacement ('${1}"version": "' + $buildVersion + '"${2}')
    Update-RegexReplace -Path $cargoTomlPath -Pattern '(?m)^version = ".*"\r?$' -Replacement ('version = "{0}"' -f $buildVersion)

    Write-Host "Updated client version to $buildVersion"
}

# Snapshot which version files are clean so the build can restore them afterwards;
# files that already carry local edits are left untouched.
$versionTrackedFiles = @('package.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.toml', 'Cargo.lock')
$versionFilesToRestore = @(
    foreach ($trackedFile in $versionTrackedFiles) {
        & git -C $projectRoot diff --quiet -- $trackedFile
        if ($LASTEXITCODE -eq 0) { $trackedFile }
    }
)

function Restore-VersionFiles {
    if ($versionFilesToRestore.Count -eq 0) { return }
    & git -C $projectRoot restore --worktree -- $versionFilesToRestore
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "failed to restore version files: $($versionFilesToRestore -join ', ')"
    }
}

if ($SkipBuild) {
    # -SkipBuild deliberately leaves the stamped files dirty for a manual build.
    Set-ClientVersion
    Write-Host "SkipBuild enabled, tauri build was not executed."
    exit 0
}

$cargoMetadata = & cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $projectRoot 'Cargo.toml') | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($cargoMetadata.target_directory)) {
    throw "Unable to resolve Cargo target directory"
}
$cargoTargetDirectory = [System.IO.Path]::GetFullPath($cargoMetadata.target_directory)
$distDirectory = Join-Path $projectRoot ("artifacts\{0}" -f $PackageTarget)
$buildProfile = if ($DebugBuild) { 'debug' } else { 'release' }
$nsisDirectory = Join-Path $cargoTargetDirectory ($buildProfile + '\bundle\nsis')
$expectedInstallerName = "p2pRemote_${buildVersion}_x64-setup.exe"
$expectedInstallerPath = Join-Path $nsisDirectory $expectedInstallerName
$publishedInstallerName = if ($PackageTarget -eq 'windows-x64') {
    $expectedInstallerName
} else {
    "p2pRemote_${buildVersion}_${PackageTarget}-setup.exe"
}
$publishedInstallerPath = Join-Path $distDirectory $publishedInstallerName

$env:RUSTC_WRAPPER = ''
Write-Host "Rust will compile directly; target cache: $cargoTargetDirectory"

$rustPunchSource = Join-Path $projectRoot '..\p2premote-punch-rs'
if (-not (Test-Path -LiteralPath $rustPunchSource -PathType Container)) {
    throw "p2premote-punch-rs source directory not found: $rustPunchSource"
}
$desktopResource = Join-Path $projectRoot 'src-tauri\resources\RustDeskTiny-install.exe'
$desktopInstaller = $desktopResource
if (-not [string]::IsNullOrWhiteSpace($RustDeskTinyInstaller)) {
    $desktopInstaller = [System.IO.Path]::GetFullPath($RustDeskTinyInstaller)
    if (-not (Test-Path -LiteralPath $desktopInstaller -PathType Leaf)) {
        throw "RustDeskTiny installer is missing: $desktopInstaller"
    }
} elseif (-not (Test-Path -LiteralPath $desktopResource -PathType Leaf)) {
    # 产物不进 git：本地缺失时（Jenkins / GitHub Actions 等干净环境）按钉版自动下载
    & (Join-Path $PSScriptRoot 'fetch-rustdesktiny-artifact.ps1') -Kind 'windows-x64'
    if (-not (Test-Path -LiteralPath $desktopResource -PathType Leaf)) {
        throw "RustDeskTiny installer is still missing after download. Pass -RustDeskTinyInstaller or set RUSTDESK_TINY_INSTALLER."
    }
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
$env:P2PREMOTE_CLIENT_VERSION = $buildVersion
$wgFfiSource = Join-Path $projectRoot '..\p2premote-wg-ffi'
if (-not (Test-Path -LiteralPath $wgFfiSource -PathType Container)) { throw "p2premote-wg-ffi source directory not found: $wgFfiSource" }
$env:P2PREMOTE_WG_FFI_DIR = (Resolve-Path -LiteralPath $wgFfiSource).Path

# core 的 build.rs 在链接期要求 resources\p2premote-wg.dll 已存在,而产出它的
# src-tauri/build.rs 因 cargo 依赖序(core 先于 src-tauri 编译)必然晚于该检查。
# 全新环境(CI)没有历史产物可复用,必须在此预编译;src-tauri 侧随后会幂等重编。
$wgDllPath = Join-Path $projectRoot 'src-tauri\resources\p2premote-wg.dll'
if (-not (Test-Path -LiteralPath $wgDllPath -PathType Leaf)) {
    Push-Location $wgFfiSource
    try {
        # 与 src-tauri/build.rs 的 windows 参数完全一致(GOTOOLCHAIN 钉版保 Win7 兼容)
        $env:GOTOOLCHAIN = 'go1.20.14'
        & go build -tags=wgonly -mod=mod -buildmode=c-shared -ldflags '-s -w' -o $wgDllPath ./punchffi
        if ($LASTEXITCODE -ne 0) { throw "userspace WG DLL prebuild failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
}

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
    Set-ClientVersion
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
    if ($DebugBuild) { $tauriArgs += '--debug' }
    if ($TauriConfig) { $tauriArgs += @('--config', $TauriConfig) }
    & node @tauriArgs
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }
    # 防回归：主程序必须报告本次构建版本。注意版本串可能被优化器拆进
    # 指令立即数（曾实测 -O 下 13 字节版本串无连续形态），对二进制做字节
    # 扫描不可靠，改为运行 `--version` 取真实值（该分支在任何初始化之前）
    $mainExeCandidates = @(
        (Join-Path $cargoTargetDirectory ($buildProfile + '\p2pRemote.exe')),
        (Join-Path $cargoTargetDirectory ($buildProfile + '\p2premote.exe'))
    )
    $mainExe = $mainExeCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if (-not $mainExe) {
        throw "built GUI executable not found under $(Join-Path $cargoTargetDirectory $buildProfile)"
    }
    # GUI 子系统进程无控制台，但父进程重定向句柄后 stdout 有效
    $mainExeOutput = (& $mainExe --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $mainExeOutput.Contains($buildVersion)) {
        throw "built GUI executable reported version '$mainExeOutput', expected it to contain ${buildVersion}: $mainExe"
    }
}
finally {
    Pop-Location
    $env:P2PREMOTE_CLIENT_VERSION = $previousClientVersion
    $env:P2PREMOTE_WG_FFI_DIR = $previousWgFfiDir
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
    Restore-VersionFiles
}
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
