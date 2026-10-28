param(
    [Parameter(Mandatory = $true)][string]$SignedDll,
    [string]$InstallRoot = 'D:\Program Files\Sumibi'
)

$ErrorActionPreference = 'Stop'
$adminSid = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
$systemSid = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18')
$usersSid = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545')
$broadSids = @('S-1-1-0', 'S-1-5-11', 'S-1-5-32-545')
$writeRights = [System.Security.AccessControl.FileSystemRights]::Write -bor
    [System.Security.AccessControl.FileSystemRights]::Delete -bor
    [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
    [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
    [System.Security.AccessControl.FileSystemRights]::TakeOwnership -bor
    0x50000000 # GENERIC_WRITE and GENERIC_ALL
$replaceParentRights = [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
    [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
    [System.Security.AccessControl.FileSystemRights]::TakeOwnership -bor
    0x10000000 # GENERIC_ALL

function Test-DriveRootAcl {
    param([string]$Path)
    $acl = Get-Acl -LiteralPath $Path
    foreach ($rule in $acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq 'Allow' -and
            $rule.IdentityReference.Value -in $broadSids -and
            ($rule.FileSystemRights -band $replaceParentRights) -ne 0) {
            throw "General users can replace the protected parent from the drive root: $Path"
        }
    }
}

function Test-ParentAcl {
    param([string]$Path)
    $acl = Get-Acl -LiteralPath $Path
    foreach ($rule in $acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq 'Allow' -and
            $rule.IdentityReference.Value -in $broadSids -and
            ($rule.FileSystemRights -band $writeRights) -ne 0) {
            throw "The parent directory allows general users to modify it: $Path"
        }
    }
}

function Test-ProtectedAcl {
    param([string]$Path)
    $acl = Get-Acl -LiteralPath $Path
    if (-not $acl.AreAccessRulesProtected -or
        $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $adminSid.Value) {
        throw "The staged path has an unprotected ACL or unexpected owner: $Path"
    }
    $rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
    $identities = @($rules | ForEach-Object { $_.IdentityReference.Value } | Sort-Object -Unique)
    if ($identities.Count -ne 3 -or
        $adminSid.Value -notin $identities -or
        $systemSid.Value -notin $identities -or
        $usersSid.Value -notin $identities) {
        throw "The staged path has an unexpected access rule: $Path"
    }
    foreach ($rule in $rules) {
        if ($rule.AccessControlType -ne 'Allow' -or $rule.IsInherited) {
            throw "The staged path has an unexpected inherited or deny rule: $Path"
        }
        if ($rule.IdentityReference.Value -eq $usersSid.Value -and
            ($rule.FileSystemRights -band $writeRights) -ne 0) {
            throw "Users have excessive access to the staged path: $Path"
        }
    }
}

function Set-ProtectedAcl {
    param([string]$Path, [bool]$Directory)
    if ($Directory) {
        $acl = [System.Security.AccessControl.DirectorySecurity]::new()
        $inheritance = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    } else {
        $acl = [System.Security.AccessControl.FileSecurity]::new()
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::None
    }
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner($adminSid)
    foreach ($entry in @(
        @($adminSid, [System.Security.AccessControl.FileSystemRights]::FullControl),
        @($systemSid, [System.Security.AccessControl.FileSystemRights]::FullControl),
        @($usersSid, [System.Security.AccessControl.FileSystemRights]::ReadAndExecute)
    )) {
        $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $entry[0], $entry[1], $inheritance,
            [System.Security.AccessControl.PropagationFlags]::None,
            [System.Security.AccessControl.AccessControlType]::Allow
        )
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
    Test-ProtectedAcl -Path $Path
}

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
Test-DriveRootAcl -Path $driveRoot
$parentItem = Get-Item -LiteralPath $parent -ErrorAction Stop
if (-not $parentItem.PSIsContainer -or
    ($parentItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'The protected parent directory is missing or is a reparse point.'
}
Test-ParentAcl -Path $parent
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
    Set-ProtectedAcl -Path $resolvedRoot -Directory $true
} else {
    $rootItem = Get-Item -LiteralPath $resolvedRoot
    if (-not $rootItem.PSIsContainer -or
        ($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        throw 'The install root is not a plain directory.'
    }
    Test-ProtectedAcl -Path $resolvedRoot
}

$null = New-Item -ItemType Directory -Path $versionPath -ErrorAction Stop
Set-ProtectedAcl -Path $versionPath -Directory $true
$installedDll = Join-Path $versionPath 'sumibi_tip.dll'
Copy-Item -LiteralPath $source -Destination $installedDll -ErrorAction Stop
Set-ProtectedAcl -Path $installedDll -Directory $false
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
Set-ProtectedAcl -Path $receiptPath -Directory $false

# Re-read the protected copy through the same gate used before a future
# registration trial. Failure leaves the version for administrator diagnosis.
& (Join-Path $PSScriptRoot 'Verify-SystemRegistrationDev.ps1') -InstalledDll $installedDll

Write-Output "Protected development DLL: $installedDll"
Write-Output 'The self-signed certificate remains untrusted; no HKLM or TSF registration was performed.'
