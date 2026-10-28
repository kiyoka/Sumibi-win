param(
    [switch]$Provision,
    [switch]$RequirePresent
)

$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess) { throw 'Use 64-bit PowerShell.' }
. (Join-Path $PSScriptRoot 'SystemRegistrationDevAcl.ps1')

$installRoot = [IO.Path]::GetFullPath('D:\Program Files\Sumibi').TrimEnd('\')
$state = Join-Path $installRoot 'registration-state'
$parent = Split-Path -Path $installRoot -Parent
$drive = [IO.Path]::GetPathRoot($parent)

function Assert-Plain {
    param([string]$Path, [bool]$Directory)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -ne $Directory -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Unexpected reparse point or item kind: $Path"
    }
    return $item
}

$null = Assert-Plain -Path $drive -Directory $true
Assert-SumibiAncestorAcl -Path $drive -Kind DriveRoot
$null = Assert-Plain -Path $parent -Directory $true
Assert-SumibiAncestorAcl -Path $parent -Kind InstallParent
$null = Assert-Plain -Path $installRoot -Directory $true
Assert-SumibiProtectedAcl -Path $installRoot -Directory $true

if (-not (Test-Path -LiteralPath $state)) {
    if (-not $Provision) {
        if ($RequirePresent) { throw 'Protected registration-state is not provisioned.' }
        Write-Output 'Protected registration-state is not provisioned; no changes made.'
        return
    }
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Provisioning registration-state requires an elevated Administrator PowerShell.'
    }
    # The protected parent can only be changed by administrators. A failed ACL
    # operation leaves the new directory for explicit administrator review.
    $null = New-Item -ItemType Directory -Path $state -ErrorAction Stop
    Set-SumibiProtectedAcl -Path $state -Directory $true
}

$null = Assert-Plain -Path $state -Directory $true
Assert-SumibiProtectedAcl -Path $state -Directory $true
$files = @(Get-ChildItem -LiteralPath $state -Force -ErrorAction Stop)
$archiveCount = 0
foreach ($file in $files) {
    $null = Assert-Plain -Path $file.FullName -Directory $false
    if ($file.Name -cmatch '^removed-[0-9a-f]{32}\.bin$') {
        $archiveCount++
    } elseif ($file.Name -cnotin @('registration.lock', 'current.bin')) {
        throw "Unknown registration-state entry: $($file.Name)"
    }
    Assert-SumibiJournalFileAcl -Path $file.FullName
    $maxSize = if ($file.Name -ceq 'registration.lock') { 0 } else { 655616 }
    if ($file.Length -gt $maxSize) { throw "Oversized registration-state entry: $($file.Name)" }
}
if ($archiveCount -gt 64) { throw 'Too many removed registration journals.' }
Write-Output "Protected registration-state verified: $state (archives=$archiveCount; current=$([bool]($files.Name -contains 'current.bin')))"
Write-Output 'No COM/TSF registration, certificate trust or default IME changes made.'
