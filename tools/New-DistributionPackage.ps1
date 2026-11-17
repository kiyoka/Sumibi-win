#Requires -Version 5.1
[CmdletBinding()]
param([string]$Version = '', [string]$OutputDir = '')
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
. (Join-Path $repository 'installer\Installer.Common.ps1')
Push-Location -LiteralPath $repository
try {
    if (-not $Version) {
        $toml = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') -Raw
        if ($toml -notmatch '(?s)\[workspace\.package\]\s*version\s*=\s*"([^"]+)"') { throw 'Workspace package version not found.' }
        $Version = $matches[1]
    }
    Assert-SumibiVersion $Version
    if (-not $OutputDir) { $OutputDir = Join-Path $repository 'dist' }
    $OutputDir = [IO.Path]::GetFullPath($OutputDir)
    Assert-SumibiPlainPath -Path $OutputDir -StopAt $OutputDir
    $null = New-Item -ItemType Directory -Path $OutputDir -Force
    $packageName = "Sumibi-v$Version-windows-x64"
    $zipPath = Get-SumibiChildPath $OutputDir "$packageName.zip"
    if (Test-Path -LiteralPath $zipPath) { throw 'Package ZIP already exists. Use a new output directory or remove that ZIP explicitly.' }
    $stage = Get-SumibiChildPath $OutputDir ($packageName + '-' + [guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path (Join-Path $stage 'bin') -Force
    $toolsTarget = Join-Path $repository 'target\dist-tools'
    $tipTarget = Join-Path $repository 'target\dist-tip'
    & cargo build --release -p sumibi-host -p sumibi-setup --locked --offline --target-dir $toolsTarget
    if ($LASTEXITCODE -ne 0) { throw 'Release Host/Setup build failed.' }
    $setup = Join-Path $toolsTarget 'release\sumibi-setup.exe'
    & (Join-Path $PSScriptRoot 'Build-PassiveRelease.ps1') -TargetDir $tipTarget -SetupExe $setup
    Copy-Item -LiteralPath (Join-Path $tipTarget 'release\sumibi_tip.dll') -Destination (Join-Path $stage 'bin')
    foreach ($name in @('sumibi-host.exe', 'sumibi-setup.exe')) {
        Copy-Item -LiteralPath (Join-Path $toolsTarget "release\$name") -Destination (Join-Path $stage 'bin')
    }
    foreach ($name in @('Install.cmd', 'Install.ps1', 'Uninstall.cmd', 'Uninstall.ps1', 'Installer.Common.ps1', 'README.txt')) {
        Copy-Item -LiteralPath (Join-Path $repository "installer\$name") -Destination $stage
    }
    # Keep the package layout compatible with earlier installers.
    $dictionaryRoot = Join-Path $repository 'crates/sumibi-host/dictionaries'
    $license = [IO.File]::ReadAllText((Join-Path $repository 'LICENSE'))
    foreach ($relative in @('README.md', 'ThirdParty/Mozc/ATTRIBUTION.txt', 'ThirdParty/Mozc/LICENSE',
        'ThirdParty/Mozc/README.md', 'ThirdParty/SudachiDict/LEGAL',
        'ThirdParty/SudachiDict/LICENSE-2.0.txt', 'ThirdParty/SudachiDict/README.md')) {
        $notice = [IO.File]::ReadAllText((Join-Path $dictionaryRoot $relative))
        $license += "`r`n`r`n--- Dictionary notices: $relative ---`r`n`r`n$notice"
    }
    [IO.File]::WriteAllText((Join-Path $stage 'LICENSE'), $license)
    $files = [ordered]@{}
    foreach ($relative in @(Get-SumibiPackageFiles)) {
        $files[$relative] = (Get-FileHash -LiteralPath (Get-SumibiChildPath $stage $relative) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    [ordered]@{ schema = 1; product = 'Sumibi'; version = $Version; files = $files } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stage 'package.json') -Encoding UTF8
    $null = Read-SumibiPackage $stage
    Invoke-SumibiSetup $setup @('plan-system-dev', '--dll', (Join-Path $stage 'bin\sumibi_tip.dll'))
    # Use a literal directory wildcard constructed from a checked path; a source
    # directory with [] must not be interpreted as a wildcard by Compress-Archive.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($stage, $zipPath)
    $checksum = Get-SumibiChildPath $OutputDir 'SHA256SUMS.txt'
    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $entries = @(if (Test-Path -LiteralPath $checksum) { Get-Content -LiteralPath $checksum })
    @($entries | Where-Object { $_ -notmatch ('\s' + [regex]::Escape("$packageName.zip") + '$') }) + @("$hash  $packageName.zip") |
        Set-Content -LiteralPath $checksum -Encoding UTF8
    & (Join-Path $PSScriptRoot 'Test-DistributionPackage.ps1') -DistDir $OutputDir -Version $Version
    Write-Host "Package created and verified: $zipPath"
} finally { Pop-Location }
