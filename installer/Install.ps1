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
    Assert-SumibiUnpackagedInstaller
    if (-not (Test-SumibiAdministrator)) {
        $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $PSCommandPath), '-InstallDir', ('"{0}"' -f (Get-SumibiInstallRoot $InstallDir)))
        if ($NoStartup) { $arguments += '-NoStartup' }
        if ($Silent) { $arguments += '-Silent' }
        $child = Start-Process powershell.exe -Verb RunAs -ArgumentList $arguments -Wait -PassThru
        if ($child.ExitCode -ne 0 -or $Silent) { exit $child.ExitCode }
        # The unelevated parent owns the completion UI and launches Sumibi as
        # the signed-in user, including when UAC used a different account.
        Add-Type -AssemblyName System.Windows.Forms
        $choice = [System.Windows.Forms.MessageBox]::Show(
            "Sumibi is installed. Open Sumibi now to set up conversion?`r`n`r`nYou can also open Sumibi from the Start menu. In your input app, press Win+Space and select Sumibi.",
            'Sumibi', 'YesNo', 'Information')
        if ($choice -eq 'Yes') {
            try {
                $root = Get-SumibiInstallRoot $InstallDir
                $state = Read-SumibiInstallState $root
                if ($null -eq $state -or $state.status -ne 'Installed') { throw 'Installation is not complete.' }
                $exe = Get-SumibiChildPath $root "versions\$($state.current)\bin\sumibi-host.exe"
                Start-Process -FilePath $exe -WindowStyle Normal
            } catch {
                $null = [System.Windows.Forms.MessageBox]::Show('Open Sumibi from the Start menu to continue.', 'Sumibi')
            }
        }
        exit 0
    }
    Invoke-SumibiInstallerLock { Invoke-SumibiInstall -SourceDir $PSScriptRoot -InstallDir $InstallDir -NoStartup:$NoStartup }
    Write-Host 'Installed. Restart apps that used Sumibi before this update.'
    Write-Host 'Open Sumibi from the Start menu to set up conversion and allow sending.'
    Write-Host 'Then select Sumibi with Win+Space in your input app.'
    Write-Host 'Settings and API keys belong to the signed-in user. Manual sending requires permission for the selected destination and model.'
    if (-not $Silent) { Write-Host 'See README.txt for setup and troubleshooting.' }
} catch { Write-Error $_; exit 1 }
