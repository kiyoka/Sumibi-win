<#
.SYNOPSIS
    Sumibi IME and Host Service Installer for Windows x64.
.DESCRIPTION
    Installs Sumibi TIP (TSF IME), Sumibi Host Service, and Sumibi Setup CLI
    into Program Files, registers COM/TSF at System scope, configures startup
    auto-launch, and creates Start Menu shortcuts.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = "$env:ProgramFiles\Sumibi",
    [switch]$NoStartup,
    [switch]$Silent
)

$ErrorActionPreference = 'Stop'

function Test-IsAdmin {
    $currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($currentIdentity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-IsAdmin)) {
    Write-Error "Administrator privileges are required to install Sumibi IME. Please run as Administrator."
    exit 1
}

$sourceDir = $PSScriptRoot
$dllSource = Join-Path $sourceDir "bin\sumibi_tip.dll"
$hostSource = Join-Path $sourceDir "bin\sumibi-host.exe"
$setupSource = Join-Path $sourceDir "bin\sumibi-setup.exe"

# If not in binary subfolder, check source root
if (-not (Test-Path $dllSource)) {
    $dllSource = Join-Path $sourceDir "sumibi_tip.dll"
}
if (-not (Test-Path $hostSource)) {
    $hostSource = Join-Path $sourceDir "sumibi-host.exe"
}
if (-not (Test-Path $setupSource)) {
    $setupSource = Join-Path $sourceDir "sumibi-setup.exe"
}

if (-not (Test-Path $dllSource) -or -not (Test-Path $hostSource) -or -not (Test-Path $setupSource)) {
    Write-Error "Installer binaries not found in $sourceDir. Required: sumibi_tip.dll, sumibi-host.exe, sumibi-setup.exe"
    exit 1
}

Write-Host "========================================================" -ForegroundColor Cyan
Write-Host "       Sumibi IME for Windows - Installation" -ForegroundColor Cyan
Write-Host "========================================================" -ForegroundColor Cyan
Write-Host "Target Directory: $InstallDir"

# 1. Terminate running sumibi-host if active
$runningHosts = Get-Process -Name "sumibi-host" -ErrorAction SilentlyContinue
if ($runningHosts) {
    Write-Host "==> Stopping running Sumibi Host process..." -ForegroundColor Yellow
    $runningHosts | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
}

# 2. Prepare target directories
$targetBin = Join-Path $InstallDir "bin"
$version = "0.1.0"
$targetVersions = Join-Path $InstallDir "versions\$version"

New-Item -ItemType Directory -Path $targetBin -Force | Out-Null
New-Item -ItemType Directory -Path $targetVersions -Force | Out-Null

# 3. Copy binaries
Write-Host "==> Copying binaries..." -ForegroundColor Cyan
Copy-Item -Path $hostSource -Destination (Join-Path $targetBin "sumibi-host.exe") -Force
Copy-Item -Path $setupSource -Destination (Join-Path $targetBin "sumibi-setup.exe") -Force
$targetDll = Join-Path $targetVersions "sumibi_tip.dll"
Copy-Item -Path $dllSource -Destination $targetDll -Force

# Also copy uninstaller scripts to install dir
if (Test-Path (Join-Path $sourceDir "Uninstall.cmd")) {
    Copy-Item -Path (Join-Path $sourceDir "Uninstall.cmd") -Destination (Join-Path $InstallDir "Uninstall.cmd") -Force
}
if (Test-Path (Join-Path $sourceDir "Uninstall.ps1")) {
    Copy-Item -Path (Join-Path $sourceDir "Uninstall.ps1") -Destination (Join-Path $InstallDir "Uninstall.ps1") -Force
}

# 4. Set secure and AppContainer ACLs
Write-Host "==> Applying security permissions and AppContainer ACLs..." -ForegroundColor Cyan
# Administrators / SYSTEM: FullControl, Users: ReadAndExecute
& icacls $InstallDir /inheritance:r /grant "BUILTIN\Administrators:(OI)(CI)F" "NT AUTHORITY\SYSTEM:(OI)(CI)F" "BUILTIN\Users:(OI)(CI)RX" /T /Q
# ALL APPLICATION PACKAGES (*S-1-15-2-1): ReadAndExecute (for Windows 11 Notepad, AppContainer apps)
& icacls $targetDll /grant "*S-1-15-2-1:(RX)" /Q

# 5. Register COM and TSF via sumibi-setup
Write-Host "==> Registering TSF IME and COM (System Scope)..." -ForegroundColor Cyan
$setupExe = Join-Path $targetBin "sumibi-setup.exe"
& $setupExe install --dll $targetDll --scope system
if ($LASTEXITCODE -ne 0) {
    Write-Error "Failed to register Sumibi IME with sumibi-setup ($LASTEXITCODE)"
    exit $LASTEXITCODE
}

# 6. Configure Startup Auto-Launch
if (-not $NoStartup) {
    Write-Host "==> Configuring Startup Auto-launch..." -ForegroundColor Cyan
    $runKey = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Run"
    $hostCmd = "`"$targetBin\sumibi-host.exe`" host-window --minimized"
    Set-ItemProperty -Path $runKey -Name "SumibiHost" -Value $hostCmd -Force
}

# 7. Create Start Menu Shortcuts
Write-Host "==> Creating Start Menu Shortcuts..." -ForegroundColor Cyan
$startMenuDir = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\Sumibi"
New-Item -ItemType Directory -Path $startMenuDir -Force | Out-Null

$wscript = New-Object -ComObject WScript.Shell

# Settings Window Shortcut
$s1 = $wscript.CreateShortcut((Join-Path $startMenuDir "Sumibi Settings.lnk"))
$s1.TargetPath = Join-Path $targetBin "sumibi-host.exe"
$s1.Arguments = "settings-window"
$s1.Description = "Configure Sumibi API Key and Model"
$s1.WorkingDirectory = $targetBin
$s1.Save()

# Host Tray Shortcut
$s2 = $wscript.CreateShortcut((Join-Path $startMenuDir "Sumibi Background Host.lnk"))
$s2.TargetPath = Join-Path $targetBin "sumibi-host.exe"
$s2.Arguments = "host-window --minimized"
$s2.Description = "Launch Sumibi Host in system tray"
$s2.WorkingDirectory = $targetBin
$s2.Save()

# Uninstall Shortcut
$uninstallerCmd = Join-Path $InstallDir "Uninstall.cmd"
if (Test-Path $uninstallerCmd) {
    $s3 = $wscript.CreateShortcut((Join-Path $startMenuDir "Uninstall Sumibi.lnk"))
    $s3.TargetPath = $uninstallerCmd
    $s3.Description = "Uninstall Sumibi IME and Host Service"
    $s3.WorkingDirectory = $InstallDir
    $s3.Save()
}

Write-Host ""
Write-Host "========================================================" -ForegroundColor Green
Write-Host "  Sumibi IME has been successfully installed!" -ForegroundColor Green
Write-Host "========================================================" -ForegroundColor Green
Write-Host "  Installed path: $InstallDir"
Write-Host "  Scope:          System (HKLM)"
Write-Host "  IME switch:     Press [Win + Space] to select 'Sumibi'"
Write-Host "  Conversion:     Type Romaji, then press [Ctrl + J]"
Write-Host ""
Write-Host "Start menu shortcuts created under 'Sumibi'."
Write-Host "Launch 'Sumibi Settings' to configure your OpenAI API Key."
Write-Host "========================================================" -ForegroundColor Green

if (-not $Silent) {
    # Prompt to launch settings now
    $choice = Read-Host "Would you like to open Sumibi Settings now to enter your API key? (Y/n)"
    if ($choice -ne 'n' -and $choice -ne 'N') {
        Start-Process -FilePath (Join-Path $targetBin "sumibi-host.exe") -ArgumentList "settings-window"
    }
}
