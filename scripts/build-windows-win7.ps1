param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$WebView2Installer,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$WebView2Sha256,
    [switch]$NoSccache
)

$ErrorActionPreference = 'Stop'
$requiredWebViewVersion = [version]'109.0.1518.78'
$projectRoot = Split-Path -Parent $PSScriptRoot
$installer = (Resolve-Path -LiteralPath $WebView2Installer).Path

if ([Environment]::Is64BitOperatingSystem -eq $false) { throw 'Win7 package builds require an x64 build host.' }
if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash -ne $WebView2Sha256.ToUpperInvariant()) {
    throw 'WebView2 installer SHA-256 mismatch.'
}
$signature = Get-AuthenticodeSignature -LiteralPath $installer
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(^|, )O=Microsoft Corporation(,|$)') {
    throw "WebView2 installer must have a valid Microsoft Authenticode signature; got '$($signature.Status)' / '$($signature.SignerCertificate.Subject)'."
}
$fileVersion = [version]([System.Diagnostics.FileVersionInfo]::GetVersionInfo($installer).FileVersion -replace '[^0-9.].*$','')
if ($fileVersion -ne $requiredWebViewVersion) {
    throw "WebView2 installer version must be exactly $requiredWebViewVersion; got $fileVersion."
}
$stream = [System.IO.File]::OpenRead($installer)
try {
    $reader = [System.IO.BinaryReader]::new($stream)
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'WebView2 installer is not a PE file.' }
    $stream.Position = 0x3c
    $peOffset = $reader.ReadUInt32()
    $stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw 'WebView2 installer is not an x64 PE file.'
    }
} finally { $stream.Dispose() }

$generatedDir = Join-Path $projectRoot 'src-tauri\.generated'
New-Item -ItemType Directory -Path $generatedDir -Force | Out-Null
$template = Get-Content -LiteralPath (Join-Path $projectRoot 'src-tauri\installer-v1.nsi') -Raw
$escapedInstaller = $installer.Replace('$', '$$').Replace('"', '$\"')
$template = $template.Replace('!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"', '!define INSTALLWEBVIEW2MODE "offlineInstaller"')
$template = $template.Replace('!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"', "!define WEBVIEW2INSTALLERPATH `"$escapedInstaller`"")
$template = $template.Replace('!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"', '!define MINIMUMWEBVIEW2VERSION "109.0.1518.78"')
[System.IO.File]::WriteAllText((Join-Path $generatedDir 'installer-win7.nsi'), $template, [System.Text.UTF8Encoding]::new($false))

$env:RUSTUP_TOOLCHAIN = '1.77.2'
$env:P2PREMOTE_RELEASE_TARGET = 'windows-win7-x64'
& (Join-Path $PSScriptRoot 'build-windows.ps1') -Version $Version -NoSccache:$NoSccache -TauriConfig 'src-tauri/tauri.win7.conf.json' -PackageTarget 'windows-win7-x64'
if ($LASTEXITCODE -ne 0) { throw "Win7 build failed with exit code $LASTEXITCODE" }
Write-Host 'Win7 package is build-ready; compatibility remains pending real Win7 SP1 x64 validation.'
