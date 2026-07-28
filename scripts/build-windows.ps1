param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "version must be in semver format like 1.2.3"
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$gitCommit = (git -C $projectRoot rev-parse --short=6 HEAD).Trim()
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
    if (-not $regex.IsMatch($content)) {
        throw "failed to update version in $Path"
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

Push-Location $projectRoot
try {
    npx tauri build
}
finally {
    Pop-Location
}
