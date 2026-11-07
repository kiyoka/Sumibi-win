#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$InstallDir = "$env:ProgramFiles\Sumibi",
    [switch]$Silent
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Installer.Common.ps1')
try {
    Assert-SumibiUnpackagedInstaller
    if (-not (Test-SumibiAdministrator)) {
        $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $PSCommandPath), '-InstallDir', ('"{0}"' -f (Get-SumibiInstallRoot $InstallDir)))
        if ($Silent) { $arguments += '-Silent' }
        $child = Start-Process powershell.exe -Verb RunAs -ArgumentList $arguments -Wait -PassThru
        exit $child.ExitCode
    }
    Invoke-SumibiInstallerLock { Invoke-SumibiUninstall -InstallDir $InstallDir }
    Write-Host 'Settings and stored API keys were preserved. Restart apps before retrying removal of locked files.'
} catch { Write-Error $_; exit 1 }
