<#
.SYNOPSIS
    Verifies that the distribution package structure, checksums, and files are valid.
#>
[CmdletBinding()]
param(
    [string]$DistDir = "dist",
    [string]$Version = "0.1.0"
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

try {
    $targetDist = Join-Path $repository $DistDir
    $packageName = "Sumibi-v$Version-windows-x64"
    $stageDir = Join-Path $targetDist $packageName
    $zipPath = Join-Path $targetDist "$packageName.zip"
    $checksumFile = Join-Path $targetDist "SHA256SUMS.txt"

    Write-Host "==> Verifying distribution package: $packageName..." -ForegroundColor Cyan

    if (-not (Test-Path $zipPath)) {
        throw "ZIP archive not found at: $zipPath"
    }
    if (-not (Test-Path $checksumFile)) {
        throw "Checksum file not found at: $checksumFile"
    }

    # Verify SHA-256 match
    $expectedHash = (Get-Content $checksumFile).Split(' ')[0].Trim()
    $actualHash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expectedHash -ne $actualHash) {
        throw "SHA-256 checksum mismatch! Expected: $expectedHash, Actual: $actualHash"
    }
    Write-Host "  [OK] SHA-256 checksum verified: $actualHash" -ForegroundColor Green

    # Verify required files in package folder
    $requiredFiles = @(
        "Install.cmd",
        "Install.ps1",
        "Uninstall.cmd",
        "Uninstall.ps1",
        "README.txt",
        "bin\sumibi-host.exe",
        "bin\sumibi-setup.exe",
        "bin\sumibi_tip.dll"
    )

    foreach ($file in $requiredFiles) {
        $filePath = Join-Path $stageDir $file
        if (-not (Test-Path $filePath)) {
            throw "Missing required package file: $file"
        }
    }
    Write-Host "  [OK] All $( $requiredFiles.Count ) required package files present." -ForegroundColor Green

    # Verify passive fail-closed properties of packaged DLL
    $packagedDll = Join-Path $stageDir "bin\sumibi_tip.dll"
    & cargo run -p sumibi-setup --target-dir target\dist-build -- plan-system-dev --dll $packagedDll
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged DLL failed passive fail-closed preflight inspection!"
    }
    Write-Host "  [OK] Packaged DLL verified as passive production build." -ForegroundColor Green

    Write-Host "==> Distribution package validation SUCCESS!" -ForegroundColor Green
}
finally {
    Pop-Location
}
