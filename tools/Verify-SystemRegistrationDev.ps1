param(
    [Parameter(Mandatory = $true)][string]$InstalledDll
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'SystemRegistrationDevAcl.ps1')

function Assert-PlainItem {
    param([string]$Path, [bool]$Directory)
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.PSIsContainer -ne $Directory -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        throw "Expected a plain protected item: $Path"
    }
}

$expectedRoot = [System.IO.Path]::GetFullPath('D:\Program Files\Sumibi').TrimEnd('\')
$path = [System.IO.Path]::GetFullPath($InstalledDll)
$versionPath = Split-Path -Path $path -Parent
$root = Split-Path -Path $versionPath -Parent
if (-not [string]::Equals($root, $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
    -not [string]::Equals((Split-Path -Path $path -Leaf), 'sumibi_tip.dll', [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'The DLL must be a direct child of a version under D:\Program Files\Sumibi.'
}
$parent = Split-Path -Path $root -Parent
$driveRoot = [System.IO.Path]::GetPathRoot($parent)
$receiptPath = Join-Path $versionPath 'staging.json'
Assert-PlainItem -Path $driveRoot -Directory $true
Assert-SumibiAncestorAcl -Path $driveRoot -Kind DriveRoot
Assert-PlainItem -Path $parent -Directory $true
Assert-SumibiAncestorAcl -Path $parent -Kind InstallParent
foreach ($item in @($root, $versionPath)) {
    Assert-PlainItem -Path $item -Directory $true
    Assert-SumibiProtectedAcl -Path $item -Directory $true
}
foreach ($item in @($path, $receiptPath)) {
    Assert-PlainItem -Path $item -Directory $false
    Assert-SumibiProtectedAcl -Path $item -Directory $false
}
$contents = @(Get-ChildItem -LiteralPath $versionPath -Force)
if ($contents.Count -ne 2 -or
    @($contents | Where-Object { $_.Name -notin @('sumibi_tip.dll', 'staging.json') }).Count -ne 0) {
    throw 'The immutable version contains an unexpected item.'
}

$receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
$hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
$expectedVersion = 'dev-' + $hash.Substring(0, 16).ToLowerInvariant()
if ($receipt.sha256 -ne $hash -or
    $receipt.installedDll -ne $path -or
    $receipt.machineRegistrationPerformed -ne $false -or
    -not [string]::Equals((Split-Path -Path $versionPath -Leaf), $expectedVersion, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'The protected DLL does not match its staging receipt or version.'
}
$signature = Get-AuthenticodeSignature -LiteralPath $path
if (-not $signature.SignerCertificate -or
    $signature.SignerCertificate.Thumbprint -ne $receipt.signerThumbprint -or
    $signature.SignerCertificate.Subject -ne 'CN=Sumibi Development Code Signing' -or
    $signature.SignerCertificate.NotAfter -le (Get-Date) -or
    -not @($signature.SignerCertificate.EnhancedKeyUsageList | Where-Object { $_.ObjectId -eq '1.3.6.1.5.5.7.3.3' }).Count -or
    $signature.SignatureType -ne 'Authenticode') {
    throw 'The protected DLL has an unexpected development signature.'
}
$chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
$null = $chain.Build($signature.SignerCertificate)
$chainCodes = @($chain.ChainStatus | ForEach-Object { [string]$_.Status })
if ($signature.Status -ne 'UnknownError' -or
    $chainCodes.Count -ne 1 -or $chainCodes[0] -ne 'UntrustedRoot') {
    throw 'The development signature must remain untrusted until the trust policy is designed.'
}

$setup = Join-Path $PSScriptRoot '..\target\debug\sumibi-setup.exe'
Assert-PlainItem -Path $setup -Directory $false
& $setup plan-system-dev --dll $path
if ($LASTEXITCODE -ne 0) {
    throw 'The installed passive DLL or registry preflight failed.'
}
Write-Output "Protected development staging verified: $path"
Write-Output "SHA-256: $hash"
Write-Output 'Self-signed certificate remains untrusted; no registration changes made.'
