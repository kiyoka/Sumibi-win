#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$InstallDir = "$env:ProgramFiles\Sumibi",
    [switch]$NoStartup,
    [switch]$Silent
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Installer.Common.ps1')
try {
    if (-not (Test-SumibiAdministrator)) {
        $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $PSCommandPath), '-InstallDir', ('"{0}"' -f (Get-SumibiInstallRoot $InstallDir)))
        if ($NoStartup) { $arguments += '-NoStartup' }
        if ($Silent) { $arguments += '-Silent' }
        $child = Start-Process powershell.exe -Verb RunAs -ArgumentList $arguments -Wait -PassThru
        exit $child.ExitCode
    }
    Invoke-SumibiInstallerLock { Invoke-SumibiInstall -SourceDir $PSScriptRoot -InstallDir $InstallDir -NoStartup:$NoStartup }
    Write-Host 'Installed. Restart apps that used Sumibi before this update.'
    Write-Host 'Open Sumibi from the Start menu to configure the destination, model and API key.'
    Write-Host 'Then open the Host, explicitly allow sending, and select Sumibi with Win+Space.'
    Write-Host 'Settings and API keys belong to the signed-in user. Sending is disabled at each Host start.'
    if (-not $Silent) { Write-Host 'See README.txt for setup and troubleshooting.' }
} catch { Write-Error $_; exit 1 }
