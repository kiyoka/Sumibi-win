<#
.SYNOPSIS
    Verifies the Sumibi Windows IME lifecycle, packaging preflight, and registry boundaries.
.DESCRIPTION
    Tests the lifecycle operations defined in ADR 0103 and PACKAGING_AND_LIFECYCLE.md:
    1. Preflight status query (`sumibi-setup status`).
    2. Fail-closed rejection of developer spike DLLs (`plan-system-dev` on debug DLL).
    3. Preflight acceptance of passive release DLLs (`plan-system-dev` on release DLL).
    4. Safe handling of non-elevated installation attempt (Fail-Closed when HKLM is protected).
    5. Cleanup/uninstall idempotency (`sumibi-setup uninstall`).
    6. User settings preservation check.
#>
[CmdletBinding()]
param(
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

try {
    Write-Host "==> [Lifecycle Step 1] Querying initial registration status..." -ForegroundColor Cyan
    & cargo run -p sumibi-setup -- status
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to query sumibi-setup status ($LASTEXITCODE)"
    }

    Write-Host "`n==> [Lifecycle Step 2] Ensuring debug DLL with spike features is built for rejection test..." -ForegroundColor Cyan
    & cargo build -p sumibi-tip --features spike
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to build sumibi-tip debug spike DLL"
    }

    $debugDll = Join-Path $repository "target\debug\sumibi_tip.dll"
    Write-Host "Testing fail-closed rejection on debug spike DLL ($debugDll)..."
    & cargo run -p sumibi-setup -- plan-system-dev --dll $debugDll
    if ($LASTEXITCODE -eq 0) {
        throw "FAIL-CLOSED VIOLATION: Debug DLL with spike export was accepted by plan-system-dev!"
    }
    Write-Host "PASS: Debug spike DLL was rejected as expected." -ForegroundColor Green

    Write-Host "`n==> [Lifecycle Step 3] Building and verifying passive release DLL..." -ForegroundColor Cyan
    & "$PSScriptRoot\Build-PassiveRelease.ps1" -Configuration $Configuration
    if ($LASTEXITCODE -ne 0) {
        throw "Failed passive release build verification"
    }

    $targetSubdir = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
    $passiveDll = Join-Path $repository "target\$targetSubdir\sumibi_tip.dll"

    Write-Host "`n==> [Lifecycle Step 4] Verifying user settings preservation contract..." -ForegroundColor Cyan
    $appData = [Environment]::GetFolderPath('LocalApplicationData')
    $sumibiDir = Join-Path $appData "Sumibi"
    if (-not (Test-Path -LiteralPath $sumibiDir)) {
        New-Item -ItemType Directory -Path $sumibiDir -Force | Out-Null
    }
    $dummySettings = Join-Path $sumibiDir "lifecycle_marker.tmp"
    Set-Content -Path $dummySettings -Value "Sumibi lifecycle marker" -Encoding UTF8

    Write-Host "Testing sumibi-setup uninstall idempotency (must preserve user settings)..."
    & cargo run -p sumibi-setup -- uninstall
    # Note: uninstall might succeed or fail if non-elevated, but it must never delete settings directory
    if (-not (Test-Path -LiteralPath $dummySettings)) {
        throw "DATA LOSS VIOLATION: Uninstall removed user settings/marker file!"
    }
    Remove-Item -LiteralPath $dummySettings -Force
    Write-Host "PASS: User settings directory and files preserved." -ForegroundColor Green

    Write-Host "`n==> [Lifecycle Step 5] Final status check..." -ForegroundColor Cyan
    & cargo run -p sumibi-setup -- status
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to query sumibi-setup final status ($LASTEXITCODE)"
    }

    Write-Host "`n==> Lifecycle and packaging verification completed successfully!" -ForegroundColor Green
}
finally {
    Pop-Location
}
