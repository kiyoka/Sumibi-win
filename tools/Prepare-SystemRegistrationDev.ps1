param(
    [string]$Dll = (Join-Path $PSScriptRoot '..\target\debug\sumibi_tip.dll'),
    [switch]$CreateCertificate,
    [string]$CertificateThumbprint
)

$ErrorActionPreference = 'Stop'

if ($CreateCertificate -eq [bool]$CertificateThumbprint) {
    throw 'Specify exactly one of -CreateCertificate or -CertificateThumbprint.'
}

$setup = Join-Path $PSScriptRoot '..\target\debug\sumibi-setup.exe'
if (-not (Test-Path -LiteralPath $setup -PathType Leaf)) {
    throw 'Build sumibi-setup before preparing a machine registration trial.'
}
$source = (Resolve-Path -LiteralPath $Dll).ProviderPath
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw 'The passive DLL was not found.'
}

# This check loads only the caller-supplied development DLL and reads registry
# state. It rejects spike exports and an existing COM/TSF registration.
& $setup plan-system-dev --dll $source
if ($LASTEXITCODE -ne 0) {
    throw 'The passive DLL and registry preflight failed.'
}
if ((Get-AuthenticodeSignature -LiteralPath $source).Status -ne 'NotSigned') {
    throw 'The source DLL must be an unsigned passive build; keep signed copies separate.'
}

if ($CreateCertificate) {
    $certificate = New-SelfSignedCertificate -Type CodeSigningCert `
        -Subject 'CN=Sumibi Development Code Signing' `
        -FriendlyName 'Sumibi development signing only' `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -KeyExportPolicy NonExportable `
        -KeyUsage DigitalSignature `
        -HashAlgorithm SHA256 `
        -NotAfter (Get-Date).AddYears(1)
} else {
    $certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$CertificateThumbprint"
}
if (-not $certificate.HasPrivateKey -or
    $certificate.Subject -ne 'CN=Sumibi Development Code Signing' -or
    $certificate.NotAfter -le (Get-Date) -or
    -not @($certificate.EnhancedKeyUsageList | Where-Object { $_.ObjectId -eq '1.3.6.1.5.5.7.3.3' }).Count) {
    throw 'The selected certificate is not a valid Sumibi development code-signing certificate.'
}

$sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
$stageRoot = Join-Path $PSScriptRoot '..\target\system-dev'
$stageName = (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss-fffffff')
$stage = (New-Item -ItemType Directory -Path (Join-Path $stageRoot $stageName) -ErrorAction Stop).FullName
$signedDll = Join-Path $stage 'sumibi_tip.dll'
Copy-Item -LiteralPath $source -Destination $signedDll -ErrorAction Stop
if ((Get-FileHash -LiteralPath $signedDll -Algorithm SHA256).Hash -ne $sourceHash) {
    throw 'The source DLL changed while staging the development copy.'
}

$null = Set-AuthenticodeSignature -LiteralPath $signedDll `
    -Certificate $certificate -HashAlgorithm SHA256 -ErrorAction Stop
$signature = Get-AuthenticodeSignature -LiteralPath $signedDll
if (-not $signature.SignerCertificate -or
    $signature.SignerCertificate.Thumbprint -ne $certificate.Thumbprint -or
    $signature.SignatureType -ne 'Authenticode') {
    throw 'The staged DLL does not carry the expected development signature.'
}
$chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
$null = $chain.Build($signature.SignerCertificate)
$chainCodes = @($chain.ChainStatus | ForEach-Object { [string]$_.Status })
if ($signature.Status -eq 'Valid') {
    $trust = 'Trusted'
} elseif ($signature.Status -eq 'UnknownError' -and
          $chainCodes.Count -eq 1 -and $chainCodes[0] -eq 'UntrustedRoot') {
    $trust = 'UntrustedRoot'
} else {
    throw "Unexpected development signature result: $($signature.Status); chain=$($chainCodes -join ',')"
}

& $setup plan-system-dev --dll $signedDll
if ($LASTEXITCODE -ne 0) {
    throw 'The signed DLL failed the passive DLL and registry preflight.'
}

$publicCertificate = Join-Path $stage 'sumibi-dev-code-signing.cer'
$null = Export-Certificate -Cert $certificate -FilePath $publicCertificate -ErrorAction Stop

$manifest = [ordered]@{
    purpose = 'Development machine-registration preparation only'
    sourceSha256 = $sourceHash
    signedSha256 = (Get-FileHash -LiteralPath $signedDll -Algorithm SHA256).Hash
    signerThumbprint = $certificate.Thumbprint
    signatureStatus = [string]$signature.Status
    certificateTrust = $trust
    signedDll = $signedDll
    publicCertificate = $publicCertificate
    machineRegistrationPerformed = $false
    trustStoreChanged = $false
}
$manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'manifest.json') -Encoding UTF8

Write-Output "Development signed DLL: $signedDll"
Write-Output "Manifest: $(Join-Path $stage 'manifest.json')"
Write-Output "Signer thumbprint: $($certificate.Thumbprint)"
Write-Output "Signature status: $($signature.Status) ($trust)"
Write-Output 'The certificate is in CurrentUser\My; no trust-root or HKLM changes were made.'
