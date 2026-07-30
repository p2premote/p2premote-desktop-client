param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [string]$Distribution = 'Debian',

    [ValidateSet('musl', 'gnu')]
    [string]$Target = 'musl'
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw 'version must be in semver format like 1.2.3'
}

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
    throw 'wsl.exe was not found; install WSL and the Debian distribution first'
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$targetArgument = if ($Target -eq 'gnu') { '--gnu' } else { '--static-musl' }
$linuxCommand = "exec bash ./scripts/build-linux-headless.sh -v $Version $targetArgument"

& wsl.exe -d $Distribution -- true
if ($LASTEXITCODE -ne 0) {
    throw "WSL distribution '$Distribution' is not available"
}

$wslArguments = @(
    '-d', $Distribution,
    '--cd', $projectRoot,
    '--',
    'bash', '-lc', $linuxCommand
)

Write-Host "Building Linux headless package in WSL distribution '$Distribution'..."
Write-Host "Project: $projectRoot"
Write-Host "Target: $Target"

& wsl.exe @wslArguments
if ($LASTEXITCODE -ne 0) {
    throw "Linux build failed with exit code $LASTEXITCODE"
}

$artifactDirectory = Join-Path $projectRoot 'build\linux\dist\headless'
Write-Host "Linux build completed: $artifactDirectory"
