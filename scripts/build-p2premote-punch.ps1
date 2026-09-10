param(
    [Parameter(Mandatory = $true)]
    [string]$OutPath,
    [Parameter(Mandatory = $true)]
    [string]$PunchSource,
    [switch]$WgOnly,
    [string]$GoModFile
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$sourceDir = Resolve-Path $PunchSource
$goCommand = Get-Command go -ErrorAction Stop
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
        $buildArgs = @("build")
        if ($GoModFile) {
            $buildArgs += "-modfile=$([System.IO.Path]::GetFullPath($GoModFile))"
        }
        if ($WgOnly) { $buildArgs += "-tags=wgonly" }
        $buildArgs += @("-buildmode=c-shared", "-ldflags", "-s -w", "-o", $resolvedOutPath, ".\punchffi")
        & $goCommand.Source @buildArgs
        if ($LASTEXITCODE -ne 0) {
            throw "punch DLL build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
} finally {
    $env:GOOS = $previousGoos
    $env:GOARCH = $previousGoarch
}

if (-not (Test-Path -LiteralPath $resolvedOutPath -PathType Leaf)) {
    throw "punch DLL was not generated: $resolvedOutPath"
}

if (Test-Path $headerPath) {
    Remove-Item $headerPath -Force
}

$objdump = Get-Command objdump -ErrorAction Stop
$dumpOutput = & $objdump.Source -p $resolvedOutPath
if ($LASTEXITCODE -ne 0) {
    throw "objdump failed with exit code $LASTEXITCODE"
}
if (-not $dumpOutput) {
    throw "objdump produced no output for $resolvedOutPath"
}
$dllNames = $dumpOutput |
    Select-String "DLL Name:" |
    ForEach-Object { $_.Line.Split(":")[-1].Trim().ToLowerInvariant() }

$unexpected = $dllNames | Where-Object {
    $_ -notin @("kernel32.dll", "msvcrt.dll")
}
if ($unexpected) {
    throw "unexpected DLL imports: $($unexpected -join ', ')"
}

Get-Item $resolvedOutPath | Select-Object FullName, Length, LastWriteTime
