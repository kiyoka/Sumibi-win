<#
.SYNOPSIS
    Builds the passive production release DLL for Sumibi TIP and verifies fail-closed passive properties.
.DESCRIPTION
    Builds `sumibi_tip.dll` under release profile without default features (excluding developer spike APIs).
    Verifies that development spike exports (e.g. SumibiSpikeBodyReadCount) are absent and standard COM
    exports (DllGetClassObject, DllCanUnloadNow, DllRegisterServer, DllUnregisterServer) are intact.
#>
[CmdletBinding()]
param(
    [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

try {
    Write-Host "==> Building sumibi-tip ($Configuration, no default features / passive)..." -ForegroundColor Cyan
    $buildArgs = @('build', '-p', 'sumibi-tip', '--no-default-features')
    if ($Configuration -eq 'Release') {
        $buildArgs += '--release'
    }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to build sumibi-tip DLL ($LASTEXITCODE)"
    }

    $targetSubdir = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
    $dllPath = Join-Path $repository "target\$targetSubdir\sumibi_tip.dll"
    if (-not (Test-Path -LiteralPath $dllPath)) {
        throw "Built DLL not found at: $dllPath"
    }

    Write-Host "==> Verifying passive fail-closed properties with sumibi-setup plan-system-dev..." -ForegroundColor Cyan
    & cargo run -p sumibi-setup -- plan-system-dev --dll $dllPath
    if ($LASTEXITCODE -ne 0) {
        throw "Passive DLL preflight failed! Dll may contain development spike APIs or invalid exports."
    }

    Write-Host "==> Passive release DLL verified successfully: $dllPath" -ForegroundColor Green
}
finally {
    Pop-Location
}
