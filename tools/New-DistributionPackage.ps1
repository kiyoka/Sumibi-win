<#
.SYNOPSIS
    Builds the complete production distribution package (ZIP) for Sumibi IME for Windows x64.
.DESCRIPTION
    1. Builds passive release DLL (spike-free, verified with plan-system-dev).
    2. Builds production release binaries for sumibi-host and sumibi-setup.
    3. Assembles files into dist\Sumibi-v<version>-windows-x64.
    4. Creates standalone release ZIP and SHA-256 checksums.
#>
[CmdletBinding()]
param(
    [string]$Version = "0.1.0",
    [string]$OutputDir = ""
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

try {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $cargoToml = Get-Content (Join-Path $repository "Cargo.toml") -Raw
        if ($cargoToml -match '\[workspace\.package\][\s\S]*?version\s*=\s*"([^"]+)"') {
            $Version = $matches[1]
        } else {
            $Version = "0.1.0"
        }
    }

    if ([string]::IsNullOrWhiteSpace($OutputDir)) {
        $OutputDir = Join-Path $repository "dist"
    }

    $packageName = "Sumibi-v$Version-windows-x64"
    $stageDir = Join-Path $OutputDir $packageName
    $zipPath = Join-Path $OutputDir "$packageName.zip"

    Write-Host "========================================================" -ForegroundColor Cyan
    Write-Host "   Creating Sumibi Distribution Package ($packageName)" -ForegroundColor Cyan
    Write-Host "========================================================" -ForegroundColor Cyan

    # 1. Clean previous dist staging
    if (Test-Path $stageDir) {
        Remove-Item -Path $stageDir -Recurse -Force
    }
    if (Test-Path $zipPath) {
        Remove-Item -Path $zipPath -Force
    }
    New-Item -ItemType Directory -Path $stageDir -Force | Out-Null

    $distTarget = Join-Path $repository "target\dist-build"

    # 1. Build Release Host and Setup binaries FIRST
    Write-Host "`n==> Step 1: Building release sumibi-host and sumibi-setup..." -ForegroundColor Cyan
    & cargo build --release -p sumibi-host -p sumibi-setup --target-dir $distTarget
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to build release binaries ($LASTEXITCODE)"
    }
    $releaseHost = Join-Path $distTarget "release\sumibi-host.exe"
    $releaseSetup = Join-Path $distTarget "release\sumibi-setup.exe"

    # 2. Build Passive Production DLL SECOND (so workspace dependency resolution cannot overwrite it)
    Write-Host "`n==> Step 2: Building and validating passive release DLL (spike-free)..." -ForegroundColor Cyan
    & "$PSScriptRoot\Build-PassiveRelease.ps1" -Configuration Release -TargetDir $distTarget
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to build passive release DLL"
    }
    $releaseDll = Join-Path $distTarget "release\sumibi_tip.dll"

    # 4. Assemble package layout
    Write-Host "`n==> Step 3: Assembling package files..." -ForegroundColor Cyan
    $binDir = Join-Path $stageDir "bin"
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null

    Copy-Item -Path $releaseDll -Destination $binDir -Force
    Copy-Item -Path $releaseHost -Destination $binDir -Force
    Copy-Item -Path $releaseSetup -Destination $binDir -Force

    # Double check staged DLL is passive
    $stagedDll = Join-Path $binDir "sumibi_tip.dll"
    & cargo run -p sumibi-setup --target-dir $distTarget -- plan-system-dev --dll $stagedDll
    if ($LASTEXITCODE -ne 0) {
        throw "Staged DLL in package failed passive fail-closed verification!"
    }

    # Copy installer scripts and docs to package root
    $installerSource = Join-Path $repository "installer"
    Copy-Item -Path (Join-Path $installerSource "Install.cmd") -Destination $stageDir -Force
    Copy-Item -Path (Join-Path $installerSource "Install.ps1") -Destination $stageDir -Force
    Copy-Item -Path (Join-Path $installerSource "Uninstall.cmd") -Destination $stageDir -Force
    Copy-Item -Path (Join-Path $installerSource "Uninstall.ps1") -Destination $stageDir -Force
    Copy-Item -Path (Join-Path $installerSource "README.txt") -Destination $stageDir -Force

    # 5. Create ZIP archive
    Write-Host "`n==> Step 4: Compressing into $zipPath..." -ForegroundColor Cyan
    Compress-Archive -Path "$stageDir\*" -DestinationPath $zipPath -Force

    # 6. Generate SHA-256 checksums
    Write-Host "`n==> Step 5: Generating SHA-256 checksums..." -ForegroundColor Cyan
    $hash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksumFile = Join-Path $OutputDir "SHA256SUMS.txt"
    $checksumEntry = "$hash  $packageName.zip"
    Set-Content -Path $checksumFile -Value $checksumEntry -Encoding UTF8

    Write-Host ""
    Write-Host "========================================================" -ForegroundColor Green
    Write-Host "  Distribution package successfully created!" -ForegroundColor Green
    Write-Host "========================================================" -ForegroundColor Green
    Write-Host "  Package Folder: $stageDir"
    Write-Host "  ZIP Archive:    $zipPath"
    Write-Host "  SHA-256 Hash:   $hash"
    Write-Host "  Checksum File:  $checksumFile"
    Write-Host "========================================================" -ForegroundColor Green
}
finally {
    Pop-Location
}
