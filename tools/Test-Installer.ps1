#Requires -Version 5.1
# Real file/lock fixtures; machine registration, process termination, ACL writes
# and Start menu changes are replaced by explicitly scoped test doubles.
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
. (Join-Path $repository 'installer\Installer.Common.ps1')
$script:cases = 0
function Assert-Case { param([bool]$Condition, [string]$Message) if (-not $Condition) { throw $Message }; $script:cases++ }
function Assert-Rejected {
    param([scriptblock]$Action, [string]$Message)
    $failed = $false
    try { & $Action } catch { $failed = $true }
    Assert-Case $failed $Message
}

Assert-Rejected { Get-SumibiInstallRoot 'C:\' } 'Drive root was admitted.'
Assert-Rejected { Get-SumibiInstallRoot $repository } 'Workspace was admitted as install root.'
foreach ($version in @('../other', '1.0', '1.0.0/child', '1.0.0"', '')) {
    Assert-Rejected { Assert-SumibiVersion $version } "Invalid version admitted: $version"
}
$testParent = Join-Path ([IO.Path]::GetTempPath()) 'SumibiInstallerTests'
$null = New-Item -ItemType Directory -Path $testParent -Force
$script:fixture = Get-SumibiChildPath $testParent ([guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $fixture
$source = Join-Path $fixture '配布 [package]'
$script:installRoot = Join-Path $fixture 'Program Files\Sumibi'
$null = New-Item -ItemType Directory -Path (Join-Path $source 'bin') -Force
try {
    Assert-Rejected { Get-SumibiChildPath $fixture '..\outside' } 'Traversal escaped fixture.'
    Assert-Rejected { Get-SumibiChildPath $fixture 'C:\outside' } 'Absolute child was admitted.'
    $junction = Join-Path $fixture 'redirect'
    $junctionTarget = Join-Path $fixture 'junction-target'
    $null = New-Item -ItemType Directory -Path $junctionTarget
    $null = New-Item -ItemType Junction -Path $junction -Target $junctionTarget
    try {
        Assert-Rejected { Get-SumibiChildPath $fixture 'redirect\bin\sumibi_tip.dll' } 'Junction redirected a managed path.'
    } finally { [IO.Directory]::Delete($junction) }
    foreach ($file in @(Get-SumibiPackageFiles)) {
        Set-Content -LiteralPath (Get-SumibiChildPath $source $file) -Value "fixture:$file" -Encoding UTF8
    }
    $hashes = [ordered]@{}
    foreach ($file in @(Get-SumibiPackageFiles)) {
        $hashes[$file] = (Get-FileHash -LiteralPath (Get-SumibiChildPath $source $file) -Algorithm SHA256).Hash
    }
    $manifest = [pscustomobject]@{ schema = 1; product = 'Sumibi'; version = '0.1.0'; files = $hashes }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $source 'package.json') -Encoding UTF8
    $null = Read-SumibiPackage $source
    Assert-Case $true 'Valid package rejected.'

    # Inspect ACL construction in memory, using SIDs rather than localized names.
    function Set-Acl { param($LiteralPath, $AclObject) $script:capturedAcl = $AclObject }
    Set-SumibiInstallAcl $fixture
    $rules = @($capturedAcl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    Assert-Case ($rules.Count -eq 4 -and $capturedAcl.AreAccessRulesProtected) 'Protected installer ACL is incomplete.'
    foreach ($sid in @('S-1-5-32-545', 'S-1-15-2-1')) {
        $rule = @($rules | Where-Object { $_.IdentityReference.Value -eq $sid })
        Assert-Case ($rule.Count -eq 1 -and -not ($rule[0].FileSystemRights -band [Security.AccessControl.FileSystemRights]::Write)) 'Reader received write access.'
    }

    function Test-SumibiAdministrator { return $true }
    function Get-SumibiInstallRoot {
        param([string]$Path)
        if ($Path -ne $script:installRoot) { throw 'Test target outside fixture.' }
        Assert-SumibiPlainPath $Path
        return $Path
    }
    function Set-SumibiInstallAcl { param([string]$Path) Assert-SumibiPlainPath $Path }
    $script:startup = $null
    $script:registered = @()
    $script:setupCalls = @()
    $script:fail = ''
    $script:shortcuts = $false
    $script:stops = 0
    # Exercise real process filtering without terminating any actual process.
    $ownedProcess = [pscustomobject]@{ Id = 101; Path = (Join-Path $installRoot 'versions\old\sumibi-host.exe') }
    $ownedProcess | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value { param($Timeout) return $true }
    $foreignProcess = [pscustomobject]@{ Id = 102; Path = ($installRoot + '-other\sumibi-host.exe') }
    function Get-Process { param($Name, $ErrorAction) return @($ownedProcess, $foreignProcess) }
    function Stop-Process { param($Id, [switch]$Force, $ErrorAction) $script:stoppedId = $Id }
    Stop-SumibiInstalledHosts $installRoot
    Assert-Case ($script:stoppedId -eq 101) 'Process filter stopped an unrelated Host.'
    function Get-SumibiStartupCommand { return $script:startup }
    function Get-SumibiRegisteredDlls { return $script:registered }
    function Stop-SumibiInstalledHosts { param([string]$Root) $script:stops++ }
    function Invoke-SumibiSetup {
        param([string]$Exe, [string[]]$Arguments)
        $script:setupCalls += $Arguments[0]
        if ($script:fail -eq $Arguments[0]) { throw 'Injected setup failure.' }
        if ($Arguments[0] -eq 'install') { $script:registered = @($Arguments[2], $Arguments[2]) }
        if ($Arguments[0] -eq 'uninstall' -and $script:fail -ne 'stale-registration') { $script:registered = @() }
    }
    function Assert-SumibiRegistration {
        param([string]$Dll, [switch]$Absent)
        if ($Absent -and $script:registered.Count) { throw 'Injected incomplete unregistration.' }
        if (-not $Absent -and @($script:registered | Where-Object { $_ -ne $Dll }).Count) { throw 'Wrong registered DLL.' }
    }
    function Set-SumibiStartup {
        param([string]$Root, [string]$HostExe, [switch]$Disabled)
        if ($Disabled) { $script:startup = $null } else { $script:startup = '"' + $HostExe + '" --minimized' }
    }
    function Set-SumibiShortcuts {
        param([string]$Root, [string]$HostExe, [switch]$Remove)
        $script:shortcuts = -not $Remove
    }

    $script:startup = '"C:\other\sumibi-host.exe" --minimized'
    Assert-Rejected { Invoke-SumibiInstall $source $installRoot } 'Foreign startup value was overwritten.'
    Assert-Case (-not (Test-Path -LiteralPath $installRoot)) 'Install mutated disk despite foreign startup.'
    $script:startup = $null
    $damaged = Join-Path $source 'bin\sumibi_tip.dll'
    $original = [IO.File]::ReadAllBytes($damaged)
    [IO.File]::WriteAllText($damaged, 'damaged')
    Assert-Rejected { Invoke-SumibiInstall $source $installRoot } 'Corrupt binary admitted.'
    Assert-Case (-not (Test-Path -LiteralPath $installRoot)) 'Corrupt package caused install mutation.'
    [IO.File]::WriteAllBytes($damaged, $original)

    Invoke-SumibiInstall $source $installRoot
    $first = Read-SumibiInstallState $installRoot
    Assert-Case ($first.versions.Count -eq 1 -and $first.status -eq 'Installed') 'Fresh installation receipt invalid.'
    Assert-Case ($shortcuts -and $startup.EndsWith('" --minimized')) 'Launchers missing.'
    $firstDll = Join-Path $installRoot "versions\$($first.current)\bin\sumibi_tip.dll"
    $lock = [IO.File]::Open($firstDll, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Invoke-SumibiInstall $source $installRoot -NoStartup
        $second = Read-SumibiInstallState $installRoot
        Assert-Case ($second.current -ne $first.current -and $second.versions.Count -eq 2) 'Reinstall reused old version path.'
        Assert-Case ((Get-FileHash -LiteralPath $firstDll -Algorithm SHA256).Hash -eq $hashes['bin/sumibi_tip.dll']) 'Loaded old DLL was modified.'
        Assert-Case ($null -eq $startup) 'NoStartup did not remove owned startup.'
    } finally { $lock.Dispose() }

    $script:fail = 'install'
    Assert-Rejected { Invoke-SumibiInstall $source $installRoot } 'Failed registration reported success.'
    $prepared = Read-SumibiInstallState $installRoot
    Assert-Case ($prepared.status -eq 'Prepared' -and $prepared.versions.Count -eq 3) 'Failure lost recovery receipt.'
    Assert-Case (Test-Path -LiteralPath $firstDll) 'Failed upgrade removed old DLL.'
    $script:fail = ''
    Invoke-SumibiInstall $source $installRoot
    $installed = Read-SumibiInstallState $installRoot
    Assert-Case ($installed.status -eq 'Installed' -and $installed.versions.Count -eq 4) 'Retry after failure did not recover.'
    $receiptPath = Join-Path $installRoot 'installation.json'
    $receiptBytes = [IO.File]::ReadAllBytes($receiptPath)
    $invalidReceipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
    $invalidReceipt.versions = @('..\other')
    $invalidReceipt | ConvertTo-Json | Set-Content -LiteralPath $receiptPath -Encoding UTF8
    Assert-Rejected { Invoke-SumibiUninstall $installRoot } 'Receipt traversal was admitted.'
    Assert-Case (Test-Path -LiteralPath $firstDll) 'Invalid receipt caused deletion.'
    [IO.File]::WriteAllBytes($receiptPath, $receiptBytes)

    $script:fail = 'uninstall'
    Assert-Rejected { Invoke-SumibiUninstall $installRoot } 'Failed unregistration reported success.'
    Assert-Case ((Test-Path -LiteralPath $firstDll) -and $shortcuts -and $null -ne $startup) 'Failed uninstall removed files or launchers.'
    $script:fail = 'stale-registration'
    Assert-Rejected { Invoke-SumibiUninstall $installRoot } 'Incomplete unregistration was trusted.'
    Assert-Case (Test-Path -LiteralPath $firstDll) 'Incomplete unregistration deleted files.'
    $script:fail = ''

    $savedRegistration = $script:registered
    $script:registered = @('C:\other\sumibi_tip.dll')
    Assert-Rejected { Invoke-SumibiUninstall $installRoot } 'Foreign COM registration was removed.'
    $script:registered = $savedRegistration
    $currentSetup = Join-Path $installRoot "versions\$($installed.current)\bin\sumibi-setup.exe"
    $setupBytes = [IO.File]::ReadAllBytes($currentSetup)
    Remove-Item -LiteralPath $currentSetup
    Assert-Rejected { Invoke-SumibiUninstall $installRoot } 'Missing setup tool triggered unsafe fallback.'
    Assert-Case (Test-Path -LiteralPath $firstDll) 'Missing setup caused file deletion.'
    [IO.File]::WriteAllBytes($currentSetup, $setupBytes)

    $unmanaged = Join-Path $installRoot 'keep-me.txt'
    Set-Content -LiteralPath $unmanaged -Value 'unrelated'
    $lock = [IO.File]::Open($firstDll, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Assert-Rejected { Invoke-SumibiUninstall $installRoot } 'Locked files were reported as a complete uninstall.'
        Assert-Case (Test-Path -LiteralPath $firstDll) 'Locked DLL was removed.'
        Assert-Case ((Test-Path -LiteralPath $currentSetup) -and (Test-Path -LiteralPath (Join-Path $installRoot 'installation.json'))) 'Partial removal lost retry tools.'
        Assert-Case ($script:registered.Count -eq 0 -and $null -eq $startup -and -not $shortcuts) 'Partial removal left registration active.'
    } finally { $lock.Dispose() }
    Invoke-SumibiUninstall $installRoot
    Assert-Case (-not (Test-Path -LiteralPath $firstDll)) 'Retry did not remove unlocked DLL.'
    Assert-Case (Test-Path -LiteralPath $unmanaged) 'Unrelated file was deleted.'
    Assert-Case (-not (Test-Path -LiteralPath (Join-Path $installRoot 'installation.json'))) 'Completed removal retained receipt.'
    Remove-Item -LiteralPath $unmanaged
    Remove-Item -LiteralPath $installRoot
    Invoke-SumibiUninstall $installRoot
    Assert-Case $true 'Absent installation was not idempotent.'
    Write-Host "Installer regression fixtures: $script:cases checks passed. No machine settings were changed."
} finally {
    # The resolved random fixture is verified against its dedicated parent.
    if ($fixture -and $fixture.StartsWith([IO.Path]::GetFullPath($testParent).TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }
}
