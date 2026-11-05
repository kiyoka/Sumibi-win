#Requires -Version 5.1
[CmdletBinding()]
param([string]$DistDir = 'dist', [string]$Version = '')
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
. (Join-Path $repository 'installer\Installer.Common.ps1')
if (-not $Version) {
    $toml = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') -Raw
    if ($toml -notmatch '(?s)\[workspace\.package\]\s*version\s*=\s*"([^"]+)"') { throw 'Workspace version not found.' }
    $Version = $matches[1]
}
Assert-SumibiVersion $Version
if (-not [IO.Path]::IsPathRooted($DistDir)) { $DistDir = Join-Path $repository $DistDir }
$name = "Sumibi-v$Version-windows-x64.zip"
$zipPath = Get-SumibiChildPath $DistDir $name
$checksumPath = Get-SumibiChildPath $DistDir 'SHA256SUMS.txt'
$entry = @(Get-Content -LiteralPath $checksumPath | Where-Object { $_ -match ('^[0-9a-fA-F]{64}\s+' + [regex]::Escape($name) + '$') })
if ($entry.Count -ne 1) { throw 'Expected one checksum entry for this ZIP.' }
$hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash
if (($entry[0] -split '\s+')[0] -ne $hash) { throw 'ZIP checksum mismatch.' }
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead($zipPath)
try {
    $expected = @((Get-SumibiPackageFiles)) + @('package.json')
    $actual = @($archive.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    if ($actual.Count -ne $expected.Count -or @(Compare-Object ($actual | Sort-Object) ($expected | Sort-Object)).Count) {
        throw 'Unexpected, duplicate or missing entries in ZIP.'
    }
} finally { $archive.Dispose() }
# Inspect the delivered archive, not a separate staging directory.
$verifyRoot = Join-Path $repository 'target\package-verification'
$null = New-Item -ItemType Directory -Path $verifyRoot -Force
$extract = Get-SumibiChildPath $verifyRoot ([guid]::NewGuid().ToString('N'))
[IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $extract)
$manifest = Read-SumibiPackage $extract
if ($manifest.version -ne $Version) { throw 'Manifest version differs from package version.' }
Invoke-SumibiSetup (Join-Path $extract 'bin\sumibi-setup.exe') @('plan-system-dev', '--dll', (Join-Path $extract 'bin\sumibi_tip.dll'))
Write-Host "ZIP checksum, $($expected.Count) files, manifest and production DLL verified: $zipPath"
