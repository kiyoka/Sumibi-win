$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'SystemRegistrationDevAcl.ps1')

$script:cases = 0
$admin = 'S-1-5-32-544'
$system = 'S-1-5-18'
$users = 'S-1-5-32-545'
$individual = 'S-1-5-21-100-200-300-1001'
$customGroup = 'S-1-5-21-100-200-300-1100'
$trustedInstaller = 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464'

function Add-TestRule {
    param(
        [System.Security.AccessControl.FileSystemSecurity]$Security,
        [string]$Sid,
        $Rights,
        [System.Security.AccessControl.InheritanceFlags]$Inheritance = 'None',
        [System.Security.AccessControl.PropagationFlags]$Propagation = 'None',
        [System.Security.AccessControl.AccessControlType]$Type = 'Allow'
    )
    if ($Rights -is [int]) {
        # Generic access bits are present in real Windows ACLs but the typed
        # FileSystemAccessRule constructor rejects bits outside its enum.
        $flags = ''
        if ($Inheritance -band [System.Security.AccessControl.InheritanceFlags]::ContainerInherit) { $flags += 'CI' }
        if ($Inheritance -band [System.Security.AccessControl.InheritanceFlags]::ObjectInherit) { $flags += 'OI' }
        if ($Propagation -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) { $flags += 'IO' }
        if ($Propagation -band [System.Security.AccessControl.PropagationFlags]::NoPropagateInherit) { $flags += 'NP' }
        $aceType = if ($Type -eq 'Allow') { 'A' } else { 'D' }
        $mask = ([int]$Rights).ToString('x8')
        $sddl = $Security.GetSecurityDescriptorSddlForm([System.Security.AccessControl.AccessControlSections]::All)
        $combined = $sddl + ('({0};{1};0x{2};;;{3})' -f $aceType, $flags, $mask, $Sid)
        try { $Security.SetSecurityDescriptorSddlForm($combined) }
        catch { throw "Invalid test SDDL: $combined" }
        return
    }
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
        [System.Security.Principal.SecurityIdentifier]::new($Sid),
        $Rights, $Inheritance, $Propagation, $Type
    )
    $Security.AddAccessRule($rule)
}

function New-AncestorSecurity {
    $acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner([System.Security.Principal.SecurityIdentifier]::new($system))
    $acl.SetAccessRuleProtection($true, $false)
    Add-TestRule -Security $acl -Sid $admin -Rights FullControl
    Add-TestRule -Security $acl -Sid $system -Rights FullControl
    return $acl
}

function New-ProtectedSecurity {
    param([bool]$Directory)
    return New-SumibiProtectedSecurity -Directory $Directory
}

function Invoke-AclCase {
    param([string]$Name, [scriptblock]$Check, [bool]$Reject = $false)
    $failed = $false
    try { & $Check } catch { $failed = $true }
    if ($failed -ne $Reject) { throw "ACL case $Name`: expected rejection=$Reject, actual=$failed" }
    $script:cases++
}

foreach ($kind in @('DriveRoot', 'InstallParent')) {
    $acl = New-AncestorSecurity
    Add-TestRule -Security $acl -Sid $users -Rights ReadAndExecute
    Invoke-AclCase "$kind/read-only" { Assert-SumibiAncestorSecurity -Security $acl -Kind $kind -Label 'memory fixture' }
    foreach ($owner in @($admin, $system, $trustedInstaller)) {
        $acl = New-AncestorSecurity
        $acl.SetOwner([System.Security.Principal.SecurityIdentifier]::new($owner))
        Invoke-AclCase "$kind/trusted-owner" { Assert-SumibiAncestorSecurity -Security $acl -Kind $kind -Label 'memory fixture' }
    }
    $acl = New-AncestorSecurity
    $acl.SetOwner([System.Security.Principal.SecurityIdentifier]::new($individual))
    Invoke-AclCase "$kind/individual-owner" { Assert-SumibiAncestorSecurity -Security $acl -Kind $kind -Label 'memory fixture' } -Reject $true

    $rights = @('DeleteSubdirectoriesAndFiles', 'ChangePermissions', 'TakeOwnership', 'FullControl', 0x10000000)
    if ($kind -eq 'InstallParent') { $rights += @('Write', 'Delete', 0x40000000) }
    foreach ($sid in @('S-1-1-0', 'S-1-5-11', $users, $individual, $customGroup, 'S-1-15-2-1')) {
        foreach ($right in $rights) {
            $acl = New-AncestorSecurity
            Add-TestRule -Security $acl -Sid $sid -Rights $right
            Invoke-AclCase "$kind/$sid/$right" { Assert-SumibiAncestorSecurity -Security $acl -Kind $kind -Label 'memory fixture' } -Reject $true
        }
    }
    $acl = New-AncestorSecurity
    Add-TestRule -Security $acl -Sid $individual -Rights FullControl -Type Deny
    Add-TestRule -Security $acl -Sid $individual -Rights FullControl
    Invoke-AclCase "$kind/deny-does-not-hide-allow" { Assert-SumibiAncestorSecurity -Security $acl -Kind $kind -Label 'memory fixture' } -Reject $true

    $acl = New-AncestorSecurity
    Add-TestRule -Security $acl -Sid 'S-1-3-0' -Rights 0x10000000 -Inheritance 'ContainerInherit, ObjectInherit' -Propagation InheritOnly
    Invoke-AclCase "$kind/inherit-only-creator-owner" { Assert-SumibiAncestorSecurity -Security $acl -Kind $kind -Label 'memory fixture' }
}
$acl = New-AncestorSecurity
$acl.SetAccessRuleProtection($false, $true)
Invoke-AclCase 'parent/inheritance-enabled' { Assert-SumibiAncestorSecurity -Security $acl -Kind InstallParent -Label 'memory fixture' } -Reject $true
$acl = New-AncestorSecurity
Add-TestRule -Security $acl -Sid $individual -Rights Modify
Invoke-AclCase 'drive/modify-without-delete-child' { Assert-SumibiAncestorSecurity -Security $acl -Kind DriveRoot -Label 'memory fixture' }

foreach ($directory in @($true, $false)) {
    $acl = New-ProtectedSecurity -Directory $directory
    Invoke-AclCase "protected/$directory/exact" { Assert-SumibiProtectedSecurity -Security $acl -Directory $directory -Label 'memory fixture' }
    Add-TestRule -Security $acl -Sid $individual -Rights ReadAndExecute
    Invoke-AclCase "protected/$directory/extra-identity" { Assert-SumibiProtectedSecurity -Security $acl -Directory $directory -Label 'memory fixture' } -Reject $true

    $acl = New-ProtectedSecurity -Directory $directory
    $acl.SetOwner([System.Security.Principal.SecurityIdentifier]::new($individual))
    Invoke-AclCase "protected/$directory/owner" { Assert-SumibiProtectedSecurity -Security $acl -Directory $directory -Label 'memory fixture' } -Reject $true

    $acl = New-ProtectedSecurity -Directory $directory
    $acl.SetAccessRuleProtection($false, $true)
    Invoke-AclCase "protected/$directory/inheritance" { Assert-SumibiProtectedSecurity -Security $acl -Directory $directory -Label 'memory fixture' } -Reject $true

    $acl = New-ProtectedSecurity -Directory $directory
    Add-TestRule -Security $acl -Sid $users -Rights Write
    Invoke-AclCase "protected/$directory/users-write" { Assert-SumibiProtectedSecurity -Security $acl -Directory $directory -Label 'memory fixture' } -Reject $true

    if ($directory) {
        $acl = New-ProtectedSecurity -Directory $true
        Invoke-AclCase 'protected/wrong-inheritance-form' { Assert-SumibiProtectedSecurity -Security $acl -Directory $false -Label 'memory fixture' } -Reject $true
    }
}
foreach ($inherited in @($false, $true)) {
    $acl = New-SumibiProtectedSecurity -Directory $false
    if ($inherited) {
        # Model the ACL between create_new and an optional explicit ACL write.
        $acl = [System.Security.AccessControl.FileSecurity]::new()
        $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:AI(A;ID;0x001f01ff;;;BA)(A;ID;0x001f01ff;;;SY)(A;ID;0x001200a9;;;BU)')
        if (-not @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]) | Where-Object IsInherited).Count) {
            throw 'The inherited journal ACL fixture did not contain inherited ACEs.'
        }
    }
    Invoke-AclCase "journal/inherited-$inherited/exact" { Assert-SumibiJournalFileSecurity -Security $acl -Label 'memory fixture' }
}
$acl = New-SumibiProtectedSecurity -Directory $false
$acl.SetOwner([System.Security.Principal.SecurityIdentifier]::new($individual))
Invoke-AclCase 'journal/foreign-owner' { Assert-SumibiJournalFileSecurity -Security $acl -Label 'memory fixture' } -Reject $true
foreach ($sid in @($individual, $customGroup, $users)) {
    foreach ($rights in @('Write', 'Delete', 'FullControl')) {
        $acl = New-SumibiProtectedSecurity -Directory $false
        Add-TestRule -Security $acl -Sid $sid -Rights $rights
        Invoke-AclCase "journal/$sid/$rights" { Assert-SumibiJournalFileSecurity -Security $acl -Label 'memory fixture' } -Reject $true
    }
}
Write-Output "System registration ACL policy: $script:cases cases passed (in-memory ACLs; no filesystem changes)"
