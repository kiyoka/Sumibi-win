param(
    [Parameter(Mandatory = $true)][string]$InstalledDll
)

$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess) { throw 'Run from a 64-bit PowerShell process.' }
$setup = Join-Path $PSScriptRoot '..\target\debug\sumibi-setup.exe'
$verify = Join-Path $PSScriptRoot 'Verify-SystemRegistrationDev.ps1'
$classPath = 'Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}'
$sentinelValue = 'SumibiOwnershipTestValue'
$sentinelChild = 'SumibiOwnershipTestChild'
$sentinel = 'sumibi-ownership-test-v1'
$marker = 'SumibiDevelopmentRegistration'
$base = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
    [Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryView]::Registry64
)

function Get-TestSnapshot {
    param([Microsoft.Win32.RegistryKey]$Key, [int]$Depth = 0)
    $names = @($Key.GetValueNames() | Sort-Object)
    $children = @($Key.GetSubKeyNames() | Sort-Object)
    if ($Depth -gt 3 -or $names.Count -gt 16 -or $children.Count -gt 16) {
        throw 'Unexpectedly large test registration; preserved for diagnosis.'
    }
    $values = @($names | ForEach-Object {
        [ordered]@{
            name = $_
            kind = [string]$Key.GetValueKind($_)
            value = $Key.GetValue($_, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        }
    })
    $subkeys = @($children | ForEach-Object {
        $child = $Key.OpenSubKey($_)
        try { [ordered]@{ name = $_; content = Get-TestSnapshot -Key $child -Depth ($Depth + 1) } }
        finally { $child.Dispose() }
    })
    return [ordered]@{ values = $values; children = $subkeys }
}

function Invoke-SetupExpected {
    param([string[]]$Arguments, [bool]$Success)
    & $setup @Arguments
    if (($LASTEXITCODE -eq 0) -ne $Success) {
        throw "Unexpected result from $($Arguments[0]): exit $LASTEXITCODE"
    }
}

function Assert-RegistrationAbsent {
    $key = $base.OpenSubKey($classPath)
    if ($null -ne $key) { $key.Dispose(); throw 'Development COM registration remains.' }
}

try {
    & $verify -InstalledDll $InstalledDll
    $cases = @('class-value', 'server-value', 'class-child', 'server-child',
        'marker', 'threading', 'dll-path', 'dll-value-kind')
    foreach ($case in $cases) {
        $registered = $false
        $injected = $false
        $class = $null
        $server = $null
        try {
            Invoke-SetupExpected -Arguments @('register-com-dev', '--dll', $InstalledDll) -Success $true
            $registered = $true
            $class = $base.OpenSubKey($classPath, $true)
            $server = $class.OpenSubKey('InprocServer32', $true)
            $originalMarker = $class.GetValue($marker)
            $originalPath = $server.GetValue('')
            if ($originalMarker -ne ('sumibi-win-com-dev-v2|' + $originalPath)) {
                throw 'The new registration does not contain a path-bound v2 owner marker.'
            }
            $target = if ($case.StartsWith('class-') -or $case -eq 'marker') { $class } else { $server }
            switch ($case) {
                { $_ -in @('class-value', 'server-value') } {
                    if ($target.GetValueNames() -contains $sentinelValue) { throw 'Test sentinel already exists.' }
                    $target.SetValue($sentinelValue, $sentinel, [Microsoft.Win32.RegistryValueKind]::String)
                }
                { $_ -in @('class-child', 'server-child') } {
                    if ($target.GetSubKeyNames() -contains $sentinelChild) { throw 'Test sentinel child already exists.' }
                    $child = $target.CreateSubKey($sentinelChild)
                    $child.Dispose()
                }
                'marker' { $class.SetValue($marker, $sentinel, [Microsoft.Win32.RegistryValueKind]::String) }
                'threading' { $server.SetValue('ThreadingModel', 'Both', [Microsoft.Win32.RegistryValueKind]::String) }
                'dll-path' { $server.SetValue('', 'C:\sumibi-ownership-test-missing.dll', [Microsoft.Win32.RegistryValueKind]::String) }
                'dll-value-kind' { $server.SetValue('', [byte[]](1, 2, 3, 4), [Microsoft.Win32.RegistryValueKind]::Binary) }
            }
            $injected = $true
            $target.Flush()
            $before = Get-TestSnapshot -Key $class | ConvertTo-Json -Depth 16 -Compress
            Invoke-SetupExpected -Arguments @('verify-com-dev') -Success $false
            Invoke-SetupExpected -Arguments @('unregister-com-dev') -Success $false
            $after = Get-TestSnapshot -Key $class | ConvertTo-Json -Depth 16 -Compress
            if ($before -cne $after) { throw "COM registration changed despite ownership conflict: $case" }
        } finally {
            try {
                if ($injected) {
                    # Restore only exact sentinels created by this test. Unknown
                    # changes remain in place and cause cleanup to fail closed.
                    switch ($case) {
                        { $_ -in @('class-value', 'server-value') } {
                            if ($target.GetValue($sentinelValue) -ne $sentinel -or
                                $target.GetValueKind($sentinelValue) -ne 'String') { throw 'Test value was changed; preserved.' }
                            $target.DeleteValue($sentinelValue, $true)
                        }
                        { $_ -in @('class-child', 'server-child') } {
                            $child = $target.OpenSubKey($sentinelChild)
                            if ($null -eq $child) { throw 'Test child disappeared.' }
                            try {
                                if ($child.ValueCount -ne 0 -or $child.SubKeyCount -ne 0) { throw 'Test child changed; preserved.' }
                            } finally { $child.Dispose() }
                            $target.DeleteSubKey($sentinelChild, $true)
                        }
                        'marker' {
                            if ($class.GetValue($marker) -ne $sentinel) { throw 'Test marker changed; preserved.' }
                            $class.SetValue($marker, $originalMarker, [Microsoft.Win32.RegistryValueKind]::String)
                        }
                        'threading' {
                            if ($server.GetValue('ThreadingModel') -ne 'Both') { throw 'Test threading value changed; preserved.' }
                            $server.SetValue('ThreadingModel', 'Apartment', [Microsoft.Win32.RegistryValueKind]::String)
                        }
                        'dll-path' {
                            if ($server.GetValue('') -ne 'C:\sumibi-ownership-test-missing.dll') { throw 'Test path changed; preserved.' }
                            $server.SetValue('', $originalPath, [Microsoft.Win32.RegistryValueKind]::String)
                        }
                        'dll-value-kind' {
                            if ($server.GetValueKind('') -ne 'Binary' -or
                                [Convert]::ToBase64String($server.GetValue('')) -ne 'AQIDBA==') { throw 'Test binary changed; preserved.' }
                            $server.SetValue('', $originalPath, [Microsoft.Win32.RegistryValueKind]::String)
                        }
                    }
                    $target.Flush()
                }
            } finally {
                if ($null -ne $server) { $server.Dispose() }
                if ($null -ne $class) { $class.Dispose() }
            }
            if ($registered) {
                Invoke-SetupExpected -Arguments @('unregister-com-dev') -Success $true
                Assert-RegistrationAbsent
            }
        }
        Write-Output "COM exact ownership / $case`: OK"
    }

    # A structurally owned registration whose COM activation fails must roll
    # back its fresh keys. The inert text fixture is never a loadable DLL.
    $fixtureDir = Join-Path $PSScriptRoot ('..\target\com-ownership-' + [guid]::NewGuid().ToString('N'))
    $null = New-Item -Path $fixtureDir -ItemType Directory
    $badDll = Join-Path $fixtureDir 'not-a-server.dll'
    try {
        [IO.File]::WriteAllBytes($badDll, [Text.Encoding]::ASCII.GetBytes('Sumibi inert COM failure fixture'))
        Invoke-SetupExpected -Arguments @('register-com-dev', '--dll', $badDll) -Success $false
        Assert-RegistrationAbsent
    } finally {
        # Nonrecursive deletion of only the two freshly created test items.
        [IO.File]::Delete($badDll)
        [IO.Directory]::Delete($fixtureDir, $false)
    }
    & $verify -InstalledDll $InstalledDll
    Write-Output 'COM exact ownership: 9 cases passed; no development COM registration remains.'
} finally {
    $base.Dispose()
}
