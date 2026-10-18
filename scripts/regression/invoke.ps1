param(
    [Parameter(Mandatory=$true)][string]$ConfigPath,
    [string]$Version = '1.14.0',
    [string]$OutputDirectory,
    [ValidateRange(1,100)][int]$Repeat = 1,
    [switch]$UseInstalledBuild
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$ConfigPath = [IO.Path]::GetFullPath($ConfigPath)
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root 'build/windows-regression' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
$config = Get-Content -LiteralPath $ConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
Push-Location $root
try {
    & python scripts/regression/run.py --config $ConfigPath --output $OutputDirectory --validate-only
    if ($LASTEXITCODE -ne 0) { throw 'Regression configuration or credential references are invalid' }
    & python -m unittest discover -s scripts/regression -p 'test_*.py'
    if ($LASTEXITCODE -ne 0) { throw 'Regression driver unit tests failed' }
    & cargo test -p p2premote-core -p p2premote-cli -p p2premote-notifier
    if ($LASTEXITCODE -ne 0) { throw 'Desktop unit tests failed' }
    $arguments = @('scripts/regression/run.py', '--output', $OutputDirectory, '--repeat', [string]$Repeat)
    if (-not $UseInstalledBuild) {
        & (Join-Path $root 'scripts/build-windows.ps1') -Version $Version
        if (-not $?) { throw 'Windows package build failed' }
        $commit = (& git rev-parse --short=6 HEAD).Trim()
        if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve build revision' }
        $buildVersion = $Version + '-' + $commit
        $installer = Join-Path $root ('artifacts/windows-x64/p2pRemote_' + $buildVersion + '_x64-setup.exe')
        if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) { throw 'Current build installer is missing' }
        $env:P2PREMOTE_REGRESSION_BUILT_INSTALLER = $installer
        $env:P2PREMOTE_REGRESSION_BUILT_SHA256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
        foreach ($name in @('a','b')) {
            $peer = $config.peers.$name
            $peer | Add-Member -Force NoteProperty expected_service_version $buildVersion
            $peer | Add-Member -Force NoteProperty installer_env 'P2PREMOTE_REGRESSION_BUILT_INSTALLER'
            $peer | Add-Member -Force NoteProperty installer_sha256_env 'P2PREMOTE_REGRESSION_BUILT_SHA256'
        }
        $arguments += '--deploy'
    }
    $manifest = @{ runner_revision=(& git rev-parse HEAD).Trim(); expected_versions=@{} }
    foreach ($dependency in @('p2premote-punch-rs', 'p2premote-wg-ffi')) {
        $dependencyPath = Join-Path (Split-Path -Parent $root) $dependency
        if (Test-Path -LiteralPath (Join-Path $dependencyPath '.git')) {
            $manifest[$dependency] = (& git -C $dependencyPath rev-parse HEAD).Trim()
            if ($LASTEXITCODE -ne 0) { throw 'Cannot record dependency revision' }
        }
    }
    foreach ($name in @('a','b')) { $manifest.expected_versions[$name] = $config.peers.$name.expected_service_version }
    if (-not $UseInstalledBuild) { $manifest.installer_sha256 = $env:P2PREMOTE_REGRESSION_BUILT_SHA256 }
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'build-manifest.json'),
        ($manifest | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding($false)))
    $effectiveConfig = Join-Path $OutputDirectory 'effective-config.json'
    [IO.File]::WriteAllText($effectiveConfig, ($config | ConvertTo-Json -Depth 30), (New-Object Text.UTF8Encoding($false)))
    $arguments += @('--config', $effectiveConfig)
    & python @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Windows VM regression failed; inspect results.json and junit.xml' }
} finally {
    Pop-Location
}
