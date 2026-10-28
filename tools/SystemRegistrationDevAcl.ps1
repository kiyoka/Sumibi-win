# Shared ACL policy for development staging and its independent verifier.
# Dot-sourcing defines validators only; no files, ACLs or registry values change.

function Assert-SumibiAncestorSecurity {
    param(
        [System.Security.AccessControl.FileSystemSecurity]$Security,
        [ValidateSet('DriveRoot', 'InstallParent')][string]$Kind,
        [string]$Label
    )
    $trusted = @(
        'S-1-5-18', # SYSTEM
        'S-1-5-32-544', # Administrators
        'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464' # TrustedInstaller
    )
    if ($Security.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -notin $trusted) {
        throw "An untrusted owner can change the ancestor ACL: $Label"
    }
    if ($Kind -eq 'InstallParent' -and -not $Security.AreAccessRulesProtected) {
        throw "The install parent must block inherited access rules: $Label"
    }
    $dangerous = [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
        [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
        [System.Security.AccessControl.FileSystemRights]::TakeOwnership -bor
        0x10000000 # GENERIC_ALL
    if ($Kind -eq 'InstallParent') {
        $dangerous = $dangerous -bor [System.Security.AccessControl.FileSystemRights]::Write -bor
            [System.Security.AccessControl.FileSystemRights]::Delete -bor 0x40000000 # GENERIC_WRITE
    }
    foreach ($rule in $Security.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        # The next directory in the protected chain has inheritance disabled.
        # InheritOnly entries (including CREATOR OWNER) do not grant access to
        # this directory and cannot propagate into that protected child.
        $inheritOnly = ($rule.PropagationFlags -band
            [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0
        if ($rule.AccessControlType -eq 'Allow' -and -not $inheritOnly -and
            $rule.IdentityReference.Value -notin $trusted -and
            ($rule.FileSystemRights -band $dangerous) -ne 0) {
            throw "An untrusted principal can modify the protected ancestor ($Kind): $Label"
        }
    }
}

function Assert-SumibiAncestorAcl {
    param([string]$Path, [ValidateSet('DriveRoot', 'InstallParent')][string]$Kind)
    Assert-SumibiAncestorSecurity -Security (Get-Acl -LiteralPath $Path) -Kind $Kind -Label $Path
}

function Assert-SumibiProtectedSecurity {
    param(
        [System.Security.AccessControl.FileSystemSecurity]$Security,
        [bool]$Directory,
        [string]$Label
    )
    $admin = 'S-1-5-32-544'
    $system = 'S-1-5-18'
    $users = 'S-1-5-32-545'
    if (-not $Security.AreAccessRulesProtected -or
        $Security.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $admin) {
        throw "Unexpected owner or inherited ACL: $Label"
    }
    $rules = @($Security.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
    if ($rules.Count -ne 3) { throw "Expected exactly three explicit access rules: $Label" }
    $expectedInheritance = if ($Directory) {
        [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    } else {
        [System.Security.AccessControl.InheritanceFlags]::None
    }
    foreach ($sid in @($admin, $system, $users)) {
        $matching = @($rules | Where-Object { $_.IdentityReference.Value -eq $sid })
        if ($matching.Count -ne 1) { throw "Missing or duplicate access rule for $sid`: $Label" }
        $rule = $matching[0]
        $expectedRights = if ($sid -eq $users) {
            [System.Security.AccessControl.FileSystemRights]::ReadAndExecute -bor
                [System.Security.AccessControl.FileSystemRights]::Synchronize
        } else {
            [System.Security.AccessControl.FileSystemRights]::FullControl
        }
        if ($rule.AccessControlType -ne 'Allow' -or $rule.IsInherited -or
            $rule.InheritanceFlags -ne $expectedInheritance -or
            $rule.PropagationFlags -ne [System.Security.AccessControl.PropagationFlags]::None -or
            $rule.FileSystemRights -ne $expectedRights) {
            throw "Unexpected access rule for $sid`: $Label"
        }
    }
}

function Assert-SumibiProtectedAcl {
    param([string]$Path, [bool]$Directory)
    Assert-SumibiProtectedSecurity -Security (Get-Acl -LiteralPath $Path) -Directory $Directory -Label $Path
}
