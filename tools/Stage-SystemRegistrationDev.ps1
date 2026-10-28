param(
    [Parameter(Mandatory = $true)][string]$SignedDll,
    [string]$InstallRoot = 'D:\Program Files\Sumibi'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'SystemRegistrationDevAcl.ps1')

$principal = [System.Security.Principal.WindowsPrincipal]::new(
    [System.Security.Principal.WindowsIdentity]::GetCurrent()
)
if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this staging script from an elevated Administrator PowerShell session.'
}

$expectedRoot = [System.IO.Path]::GetFullPath('D:\Program Files\Sumibi').TrimEnd('\')
$resolvedRoot = [System.IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
if (-not [string]::Equals($resolvedRoot, $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'This development staging script is limited to D:\Program Files\Sumibi.'
}
$parent = Split-Path -Path $resolvedRoot -Parent
$driveRoot = [System.IO.Path]::GetPathRoot($parent)
$driveItem = Get-Item -LiteralPath $driveRoot -ErrorAction Stop
if (-not $driveItem.PSIsContainer -or
    ($driveItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'The drive root is missing or is a reparse point.'
}
Assert-SumibiAncestorAcl -Path $driveRoot -Kind DriveRoot
$parentItem = Get-Item -LiteralPath $parent -ErrorAction Stop
if (-not $parentItem.PSIsContainer -or
    ($parentItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'The protected parent directory is missing or is a reparse point.'
}
Assert-SumibiAncestorAcl -Path $parent -Kind InstallParent
$source = (Resolve-Path -LiteralPath $SignedDll).ProviderPath
$sourceItem = Get-Item -LiteralPath $source -ErrorAction Stop
if ($sourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
    throw 'The signed source DLL must not be a reparse point.'
}
$sourceManifestPath = Join-Path $sourceItem.DirectoryName 'manifest.json'
$sourceManifest = Get-Content -LiteralPath $sourceManifestPath -Raw | ConvertFrom-Json
$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$signature = Get-AuthenticodeSignature -LiteralPath $source
if ($sourceHash -ne $sourceManifest.signedSha256 -or
    -not $signature.SignerCertificate -or
    $signature.SignerCertificate.Thumbprint -ne $sourceManifest.signerThumbprint -or
    $signature.SignatureType -ne 'Authenticode' -or
    $sourceManifest.machineRegistrationPerformed -ne $false) {
    throw 'The signed DLL does not match its development staging manifest.'
}
$chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
$null = $chain.Build($signature.SignerCertificate)
$chainCodes = @($chain.ChainStatus | ForEach-Object { [string]$_.Status })
if ($signature.Status -ne 'UnknownError' -or
    $chainCodes.Count -ne 1 -or $chainCodes[0] -ne 'UntrustedRoot') {
    throw 'Expected an untrusted self-signed development certificate.'
}

$version = 'dev-' + $sourceHash.Substring(0, 16).ToLowerInvariant()
$versionPath = Join-Path $resolvedRoot $version
if (Test-Path -LiteralPath $versionPath) {
    throw 'This immutable development version is already staged.'
}
if (-not (Test-Path -LiteralPath $resolvedRoot)) {
    $null = New-Item -ItemType Directory -Path $resolvedRoot -ErrorAction Stop
    Set-SumibiProtectedAcl -Path $resolvedRoot -Directory $true
} else {
    $rootItem = Get-Item -LiteralPath $resolvedRoot
    if (-not $rootItem.PSIsContainer -or
        ($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        throw 'The install root is not a plain directory.'
    }
    Assert-SumibiProtectedAcl -Path $resolvedRoot -Directory $true
}

$null = New-Item -ItemType Directory -Path $versionPath -ErrorAction Stop
Set-SumibiProtectedAcl -Path $versionPath -Directory $true
$installedDll = Join-Path $versionPath 'sumibi_tip.dll'
Copy-Item -LiteralPath $source -Destination $installedDll -ErrorAction Stop
Set-SumibiProtectedAcl -Path $installedDll -Directory $false
if ((Get-FileHash -LiteralPath $installedDll -Algorithm SHA256).Hash -ne $sourceHash) {
    throw 'The protected copy differs from the signed source.'
}

$receiptPath = Join-Path $versionPath 'staging.json'
[ordered]@{
    purpose = 'Development staging only; not registered or trusted'
    sha256 = $sourceHash
    signerThumbprint = $signature.SignerCertificate.Thumbprint
    installedDll = $installedDll
    machineRegistrationPerformed = $false
} | ConvertTo-Json | Set-Content -LiteralPath $receiptPath -Encoding UTF8
Set-SumibiProtectedAcl -Path $receiptPath -Directory $false

# Re-read the protected copy through the same gate used before a future
# registration trial. Failure leaves the version for administrator diagnosis.
& (Join-Path $PSScriptRoot 'Verify-SystemRegistrationDev.ps1') -InstalledDll $installedDll

Write-Output "Protected development DLL: $installedDll"
Write-Output 'The self-signed certificate remains untrusted; no HKLM or TSF registration was performed.'
