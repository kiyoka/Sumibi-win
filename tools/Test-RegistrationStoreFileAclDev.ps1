# Run directly from an elevated 64-bit PowerShell. This tests the ACL inherited
# by a newly created journal file; it does not register COM or TSF resources.
$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess) { throw 'Use 64-bit PowerShell.' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this probe directly in an elevated Administrator PowerShell.'
}
. (Join-Path $PSScriptRoot 'SystemRegistrationDevAcl.ps1')

$state = 'D:\Program Files\Sumibi\registration-state'
$current = Join-Path $state 'current.bin'
# Validate the protected chain and exact directory ACL before creating a file.
& (Join-Path $PSScriptRoot 'Prepare-RegistrationStoreDev.ps1') -RequirePresent
if (@(Get-ChildItem -LiteralPath $state -Force -ErrorAction Stop).Count -ne 0) {
    throw 'The registration store must be empty before this file ACL probe.'
}

$file = $null
$verified = $false
try {
    $file = [IO.File]::Open($current, [IO.FileMode]::CreateNew,
        [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $file.Flush($true)
    $file.Dispose()
    $file = $null
    $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        $item.Length -ne 0) {
        throw 'The journal file is not an empty plain file.'
    }
    Assert-SumibiJournalFileAcl -Path $current
    & (Join-Path $PSScriptRoot 'Prepare-RegistrationStoreDev.ps1') -RequirePresent
    $verified = $true
} finally {
    if ($file) { $file.Dispose() }
    # Preserve unexpected contents or ACLs for explicit administrator review.
    if ($verified) { Remove-Item -LiteralPath $current -ErrorAction Stop }
}
& (Join-Path $PSScriptRoot 'Prepare-RegistrationStoreDev.ps1') -RequirePresent
Write-Output 'Protected journal file ACL probe passed; current.bin removed.'
