param(
    [string]$OutPath = "src-tauri/resources/p2premote-punch.dll",
    [string]$PunchSource = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
if ($PunchSource) {
    $sourceDir = Resolve-Path $PunchSource
} elseif ($env:P2PREMOTE_PUNCH_DIR) {
    $sourceDir = Resolve-Path $env:P2PREMOTE_PUNCH_DIR
} else {
    $sourceDir = Resolve-Path (Join-Path $repoRoot "..\p2premote-punch")
}
$resolvedOutPath = Join-Path $repoRoot $OutPath
$outDir = Split-Path -Parent $resolvedOutPath
$headerPath = [System.IO.Path]::ChangeExtension($resolvedOutPath, ".h")

New-Item -ItemType Directory -Force $outDir | Out-Null

$previousGoos = $env:GOOS
$previousGoarch = $env:GOARCH

try {
    $env:GOOS = "windows"
    $env:GOARCH = "amd64"

    Push-Location $sourceDir
    try {
        go build -buildmode=c-shared -ldflags "-s -w" -o $resolvedOutPath .\punchffi
    } finally {
        Pop-Location
    }
} finally {
    $env:GOOS = $previousGoos
    $env:GOARCH = $previousGoarch
}

if (Test-Path $headerPath) {
    Remove-Item $headerPath -Force
}

$objdump = Get-Command objdump -ErrorAction SilentlyContinue
if ($objdump) {
    $dllNames = & $objdump.Source -p $resolvedOutPath |
        Select-String "DLL Name:" |
        ForEach-Object { $_.Line.Split(":")[-1].Trim().ToLowerInvariant() }

    $unexpected = $dllNames | Where-Object {
        $_ -notin @("kernel32.dll", "msvcrt.dll")
    }
    if ($unexpected) {
        throw "unexpected DLL imports: $($unexpected -join ', ')"
    }
}

Get-Item $resolvedOutPath | Select-Object FullName, Length, LastWriteTime
