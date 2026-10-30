param(
    [Parameter(Mandatory = $true)][string]$InstalledDll
)

$ErrorActionPreference = 'Stop'
$verify = Join-Path $PSScriptRoot 'Verify-SystemRegistrationDev.ps1'
$roundTrip = Join-Path $PSScriptRoot 'Test-RegisteredTsf.ps1'

# Verify the exact protected version before it can be loaded by COM.
& $verify -InstalledDll $InstalledDll

try {
    # This test creates only a current-user COM registration. Its TSF profile
    # is process-local and Test-RegisteredTsf removes the COM key in finally.
    & $roundTrip -Dll $InstalledDll
} finally {
    # A failure here also makes an incomplete cleanup visible. Never treat a
    # successful activation alone as a completed registration round trip.
    & $verify -InstalledDll $InstalledDll
}

Write-Output 'Protected DLL current-user COM/TSF round trip and cleanup: OK'
