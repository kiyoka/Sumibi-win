<#
.SYNOPSIS
    Installs and registers Sumibi TIP as a Windows Text Services Framework (TSF) IME.
.DESCRIPTION
    Requests elevation (UAC prompt) to register the COM class and TSF Input Processor Profile
    under HKLM/HKCU. Ensures the passive release DLL is built and preflight verified before registration.
#>
[CmdletBinding()]
param(
    [string]$DllPath
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

try {
    # Check for elevation
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

    if (-not $isAdmin) {
        Write-Host "Elevated administrator privileges required to register TSF IME profile." -ForegroundColor Yellow
        Write-Host "Requesting elevation via UAC..." -ForegroundColor Cyan
        $scriptPath = $MyInvocation.MyCommand.Definition
        $argList = @("-ExecutionPolicy", "Bypass", "-NoExit", "-File", "`"$scriptPath`"")
        if ($DllPath) {
            $argList += @("-DllPath", "`"$DllPath`"")
        }
        Start-Process powershell -Verb RunAs -ArgumentList $argList
        return
    }

    if (-not $DllPath) {
        $DllPath = Join-Path $repository "target\release\sumibi_tip.dll"
    }

    if (-not (Test-Path -LiteralPath $DllPath)) {
        Write-Host "==> Building passive release DLL..." -ForegroundColor Cyan
        & "$PSScriptRoot\Build-PassiveRelease.ps1"
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to build passive release DLL"
        }
    }

    $setupExe = Join-Path $repository "target\debug\sumibi-setup.exe"
    if (-not (Test-Path -LiteralPath $setupExe)) {
        & cargo build -p sumibi-setup
    }

    Write-Host "==> Registering Sumibi TIP (TSF IME)..." -ForegroundColor Cyan
    & $setupExe install --dll $DllPath
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to register Sumibi TIP"
    }

    Write-Host "`n==> Current Registration Status:" -ForegroundColor Cyan
    & $setupExe status

    Write-Host "`n========================================================" -ForegroundColor Green
    Write-Host "  Sumibi IME has been registered successfully!" -ForegroundColor Green
    Write-Host "========================================================" -ForegroundColor Green
    Write-Host "How to test on Windows:" -ForegroundColor Cyan
    Write-Host "  1. Start the backend host (in another console):"
    Write-Host "       cargo run -p sumibi-host -- run"
    Write-Host "  2. Open Notepad (notepad.exe) or any app."
    Write-Host "  3. Press [Win + Space] to switch input method to 'Sumibi'."
    Write-Host "  4. Type characters and verify composition / input."
    Write-Host "  5. When finished, run tools\Uninstall-SumibiIme.ps1 to unregister."
    Write-Host "========================================================`n"
}
finally {
    Pop-Location
}
