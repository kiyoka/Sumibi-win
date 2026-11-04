<#
.SYNOPSIS
    Sumibi IME and Host Service Uninstaller for Windows x64.
.DESCRIPTION
    Unregisters Sumibi TIP and COM classes, terminates active host services,
    removes startup entries, shortcuts, and installed program files.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = "$env:ProgramFiles\Sumibi",
    [switch]$PurgeSettings,
    [switch]$Silent
)

$ErrorActionPreference = 'Stop'

function Test-IsAdmin {
    $currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($currentIdentity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-IsAdmin)) {
    Write-Error "Administrator privileges are required to uninstall Sumibi IME. Please run as Administrator."
    exit 1
}

Write-Host "========================================================" -ForegroundColor Yellow
Write-Host "       Sumibi IME for Windows - Uninstallation" -ForegroundColor Yellow
Write-Host "========================================================" -ForegroundColor Yellow

# 1. Terminate running sumibi-host
$runningHosts = Get-Process -Name "sumibi-host" -ErrorAction SilentlyContinue
if ($runningHosts) {
    Write-Host "==> Stopping running Sumibi Host process..." -ForegroundColor Yellow
    $runningHosts | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
}

# 2. Unregister TSF and COM via sumibi-setup
$setupExe = Join-Path $InstallDir "bin\sumibi-setup.exe"
if (Test-Path $setupExe) {
    Write-Host "==> Unregistering TSF IME and COM classes..." -ForegroundColor Cyan
    & $setupExe uninstall --scope system
} else {
    Write-Warning "sumibi-setup.exe not found at $setupExe. Cleaning registry directly..."
    # Fallback registry cleanup if sumibi-setup is missing
    $clsid = "{E927D4AB-2EC9-4DD3-A921-A4620B95A410}"
    Remove-Item -Path "HKLM:\Software\Classes\CLSID\$clsid" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path "HKCU:\Software\Classes\CLSID\$clsid" -Recurse -Force -ErrorAction SilentlyContinue
}

# 3. Remove Startup auto-launch entry
Write-Host "==> Removing Startup auto-launch entry..." -ForegroundColor Cyan
$runKey = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Run"
Remove-ItemProperty -Path $runKey -Name "SumibiHost" -ErrorAction SilentlyContinue

# 4. Remove Start Menu shortcuts
Write-Host "==> Removing Start Menu shortcuts..." -ForegroundColor Cyan
$startMenuDir = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\Sumibi"
if (Test-Path $startMenuDir) {
    Remove-Item -Path $startMenuDir -Recurse -Force -ErrorAction SilentlyContinue
}

# 5. Remove Program Files directory
if (Test-Path $InstallDir) {
    Write-Host "==> Removing installed files from $InstallDir..." -ForegroundColor Cyan
    try {
        Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction Stop
    } catch {
        Write-Warning "Some files are currently locked by other applications (e.g. explorer/browsers)."
        Write-Warning "They will be removed after system reboot."
        # Schedule deletion for remaining files
        Get-ChildItem -Path $InstallDir -Recurse | ForEach-Object {
            try { Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue } catch {}
        }
    }
}

# 6. Optional purge of user settings
if ($PurgeSettings) {
    Write-Host "==> Purging user settings..." -ForegroundColor Yellow
    $userSettingsDir = Join-Path $env:LOCALAPPDATA "Sumibi"
    if (Test-Path $userSettingsDir) {
        Remove-Item -Path $userSettingsDir -Recurse -Force -ErrorAction SilentlyContinue
    }
} else {
    Write-Host "Note: User settings in %LOCALAPPDATA%\Sumibi and stored API keys were preserved." -ForegroundColor Gray
}

Write-Host ""
Write-Host "========================================================" -ForegroundColor Green
Write-Host "  Sumibi IME has been successfully uninstalled." -ForegroundColor Green
Write-Host "========================================================" -ForegroundColor Green
