<#
.SYNOPSIS
    Unregisters Sumibi TIP from Windows Text Services Framework (TSF) and COM.
.DESCRIPTION
    Requests elevation (UAC prompt) to unregister the TSF profile and remove COM registrations.
    Preserves all user settings and credentials in %LOCALAPPDATA%\Sumibi.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

try {
    # Check for elevation
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

    if (-not $isAdmin) {
        Write-Host "Elevated administrator privileges required to unregister TSF IME profile." -ForegroundColor Yellow
        Write-Host "Requesting elevation via UAC..." -ForegroundColor Cyan
        $scriptPath = $MyInvocation.MyCommand.Definition
        $argList = @("-ExecutionPolicy", "Bypass", "-NoExit", "-File", "`"$scriptPath`"")
        Start-Process powershell -Verb RunAs -ArgumentList $argList
        return
    }

    $setupExe = Join-Path $repository "target\debug\sumibi-setup.exe"
    if (-not (Test-Path -LiteralPath $setupExe)) {
        & cargo build -p sumibi-setup
    }

    Write-Host "==> Unregistering Sumibi TIP (TSF IME)..." -ForegroundColor Cyan
    & $setupExe uninstall
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to unregister Sumibi TIP"
    }

    Write-Host "`n==> Current Registration Status:" -ForegroundColor Cyan
    & $setupExe status

    Write-Host "`n========================================================" -ForegroundColor Green
    Write-Host "  Sumibi IME has been unregistered successfully." -ForegroundColor Green
    Write-Host "  (User settings in %LOCALAPPDATA%\Sumibi remain preserved)" -ForegroundColor Green
    Write-Host "========================================================`n"
}
finally {
    Pop-Location
}
