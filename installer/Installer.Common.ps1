# Shared installer functions. Dot-sourcing performs no machine changes.
#Requires -Version 5.1
Set-StrictMode -Version Latest
$script:SumibiComServerKey = 'CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\InprocServer32'

function Get-SumibiPackageIdentityStatus {
    if (-not ('SumibiInstaller.PackageIdentity' -as [type])) {
        Add-Type -Namespace SumibiInstaller -Name PackageIdentity -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", ExactSpelling = true)]
public static extern int GetCurrentPackageFullName(ref uint length, System.IntPtr name);
'@
    }
    [uint32]$length = 0
    return [SumibiInstaller.PackageIdentity]::GetCurrentPackageFullName([ref]$length, [IntPtr]::Zero)
}

function Assert-SumibiUnpackagedInstaller {
    # APPMODEL_ERROR_NO_PACKAGE is the only result that proves this process
    # is unpackaged. ERROR_INSUFFICIENT_BUFFER indicates a package identity.
    $status = Get-SumibiPackageIdentityStatus
    if ($status -eq 122) {
        throw 'This installer is running inside a Windows app package. Open the installer from File Explorer or a standalone Windows PowerShell window, then retry.'
    }
    if ($status -ne 15700) { throw "Cannot verify the installer process package identity (Windows error $status)." }
    Assert-SumibiUserRegistryView
}

function Get-SumibiUserRegistration {
    param([switch]$Native)
    if ($Native) {
        $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $key = [Microsoft.Win32.Registry]::Users.OpenSubKey("${sid}_Classes\$script:SumibiComServerKey")
    } else {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\$script:SumibiComServerKey")
    }
    if ($null -eq $key) { return $null }
    try { return $key.GetValue('') } finally { $key.Dispose() }
}

function Assert-SumibiUserRegistryView {
    $current = Get-SumibiUserRegistration
    $native = Get-SumibiUserRegistration -Native
    if ($current -ne $native) {
        throw 'This process sees a different Sumibi user registration than the Windows desktop. Run the installer from File Explorer or a standalone Windows PowerShell window.'
    }
}

function Test-SumibiAdministrator {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Invoke-SumibiInstallerLock {
    param([scriptblock]$Action)
    $mutex = [Threading.Mutex]::new($false, 'Global\SumibiInstaller-E927D4AB-2EC9-4DD3-A921-A4620B95A410')
    $held = $false
    try {
        try { $held = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $held = $true }
        if (-not $held) { throw 'Another Sumibi installer is running.' }
        & $Action
    } finally {
        if ($held) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}

function Assert-SumibiPlainPath {
    param([string]$Path, [string]$StopAt = '')
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse points are unsupported: $cursor" }
        }
        if ($StopAt -and [string]::Equals($cursor, $StopAt, [StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = Split-Path -Parent $cursor
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
}

function Get-SumibiInstallRoot {
    param([string]$Path)
    if (-not [Environment]::Is64BitProcess) { throw 'Run the 64-bit Windows PowerShell installer.' }
    $expected = [IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Sumibi')).TrimEnd('\')
    $root = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    if (-not [string]::Equals($root, $expected, [StringComparison]::OrdinalIgnoreCase)) {
        throw "InstallDir must be $expected. Other directories are not managed by this installer."
    }
    Assert-SumibiPlainPath $root
    return $root
}

function Get-SumibiChildPath {
    param([string]$Root, [string]$Relative)
    if ([IO.Path]::IsPathRooted($Relative)) { throw 'Expected a relative managed path.' }
    $parent = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $path = [IO.Path]::GetFullPath((Join-Path $parent $Relative))
    if (-not $path.StartsWith($parent + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Managed path escapes its root.' }
    Assert-SumibiPlainPath -Path $path -StopAt $parent
    return $path
}

function Get-SumibiPackageFiles {
    return @('Install.cmd', 'Install.ps1', 'Uninstall.cmd', 'Uninstall.ps1', 'Installer.Common.ps1', 'README.txt', 'LICENSE',
        'bin/sumibi-host.exe', 'bin/sumibi-setup.exe', 'bin/sumibi_tip.dll')
}

function Assert-SumibiVersion {
    param([string]$Version)
    if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$') { throw 'Invalid package version.' }
}

function Read-SumibiPackage {
    param([string]$SourceDir)
    $manifest = Get-Content -LiteralPath (Get-SumibiChildPath $SourceDir 'package.json') -Raw | ConvertFrom-Json
    if ($manifest.schema -ne 1 -or $manifest.product -ne 'Sumibi') { throw 'Unrecognized package manifest.' }
    Assert-SumibiVersion $manifest.version
    $required = @(Get-SumibiPackageFiles)
    if (@($manifest.files.PSObject.Properties).Count -ne $required.Count) { throw 'Unexpected manifest file count.' }
    foreach ($relative in $required) {
        $path = Get-SumibiChildPath $SourceDir $relative
        $hash = [string]$manifest.files.$relative
        if ($hash -notmatch '^[0-9a-fA-F]{64}$' -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $hash) {
            throw "Package file checksum mismatch: $relative"
        }
    }
    return $manifest
}

function Set-SumibiInstallAcl {
    param([string]$Path)
    Assert-SumibiPlainPath $Path
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    foreach ($entry in @(@('S-1-5-32-544', 'FullControl'), @('S-1-5-18', 'FullControl'),
        @('S-1-5-32-545', 'ReadAndExecute'), @('S-1-15-2-1', 'ReadAndExecute'))) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($entry[0]),
            [Security.AccessControl.FileSystemRights]$entry[1],
            [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
            [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
        $null = $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

function Invoke-SumibiSetup {
    param([string]$Exe, [string[]]$Arguments)
    & $Exe @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "sumibi-setup failed ($LASTEXITCODE): $($Arguments[0])" }
}

function Get-SumibiRegisteredDlls {
    $key = "Software\Classes\$script:SumibiComServerKey"
    foreach ($hive in @('HKLM', 'HKCU')) {
        $path = "${hive}:\$key"
        if (Test-Path -LiteralPath $path) { [string](Get-Item -LiteralPath $path).GetValue('') }
    }
}

function Assert-SumibiRegistration {
    param([string]$Dll, [switch]$Absent)
    Assert-SumibiUserRegistryView
    $paths = @(Get-SumibiRegisteredDlls)
    $profile = 'HKLM:\Software\Microsoft\CTF\TIP\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}\LanguageProfile\0x00000411\{166150AD-12EF-4F52-A75B-CBB19FF97D27}'
    if ($Absent) {
        if ($paths.Count -or (Test-Path -LiteralPath $profile)) { throw 'Registration still exists. Files were retained; rerun after resolving the registration error.' }
    } else {
        if ($paths.Count -ne 2 -or -not (Test-Path -LiteralPath $profile)) { throw 'Registration is incomplete.' }
        foreach ($path in $paths) {
            if (($path -replace '^\\\\\?\\', '') -ne $Dll) { throw 'COM registration points to another DLL.' }
        }
    }
}

function Stop-SumibiInstalledHosts {
    param([string]$Root)
    foreach ($process in @(Get-Process -Name 'sumibi-host' -ErrorAction SilentlyContinue)) {
        $path = $process.Path
        if ($path -and $path.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) {
            Stop-Process -Id $process.Id -Force -ErrorAction Stop
            if (-not $process.WaitForExit(5000)) { throw 'An installed Host did not stop.' }
        }
    }
}

function Get-SumibiStartupCommand {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
    if ($null -eq $key) { return $null }
    try { return $key.GetValue('SumibiHost') } finally { $key.Dispose() }
}

function Assert-SumibiStartupOwner {
    param([string]$Root)
    $command = Get-SumibiStartupCommand
    if ($command -and -not $command.StartsWith('"' + $Root + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The SumibiHost startup value belongs to another installation.'
    }
}

function Set-SumibiStartup {
    param([string]$Root, [string]$HostExe, [switch]$Disabled)
    Assert-SumibiStartupOwner $Root
    $key = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run'
    if ($Disabled) {
        if (Get-SumibiStartupCommand) { Remove-ItemProperty -LiteralPath $key -Name 'SumibiHost' }
    } else {
        # Default GUI entry opens settings when unconfigured; host-window does not.
        Set-ItemProperty -LiteralPath $key -Name 'SumibiHost' -Value ('"{0}" --minimized' -f $HostExe)
    }
}

function Get-SumibiStartMenu {
    $folder = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Sumibi'
    Assert-SumibiPlainPath $folder
    return $folder
}

function Set-SumibiShortcuts {
    param([string]$Root, [string]$HostExe, [switch]$Remove)
    $folder = Get-SumibiStartMenu
    $shell = New-Object -ComObject WScript.Shell
    $legacy = 'Sumibi Background Host.lnk'
    $entries = @(@('Sumibi.lnk', $HostExe, ''),
        @('Sumibi Settings.lnk', $HostExe, 'settings-window'),
        @('Uninstall Sumibi.lnk', (Join-Path $Root 'Uninstall.cmd'), ''))
    foreach ($name in @($entries | ForEach-Object { $_[0] }) + @($legacy)) {
        $path = Get-SumibiChildPath $folder $name
        if (Test-Path -LiteralPath $path) {
            $target = $shell.CreateShortcut($path).TargetPath
            if (-not $target.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Shortcut belongs to another installation: $path" }
        }
    }
    $legacyPath = Get-SumibiChildPath $folder $legacy
    if (Test-Path -LiteralPath $legacyPath) { Remove-Item -LiteralPath $legacyPath -Force }
    if (-not $Remove) { $null = New-Item -ItemType Directory -Path $folder -Force }
    foreach ($entry in $entries) {
        $path = Get-SumibiChildPath $folder $entry[0]
        if ($Remove) {
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
        } else {
            $shortcut = $shell.CreateShortcut($path)
            $shortcut.TargetPath = $entry[1]
            $shortcut.Arguments = $entry[2]
            if ($entry[1] -eq $HostExe) { $shortcut.IconLocation = "$HostExe,0" }
            $shortcut.WorkingDirectory = Split-Path -Parent $entry[1]
            $shortcut.Save()
        }
    }
    if ($Remove -and (Test-Path -LiteralPath $folder) -and @(Get-ChildItem -LiteralPath $folder -Force).Count -eq 0) { Remove-Item -LiteralPath $folder }
}

function Read-SumibiInstallState {
    param([string]$Root)
    $file = Get-SumibiChildPath $Root 'installation.json'
    if (-not (Test-Path -LiteralPath $file)) { return $null }
    $state = Get-Content -LiteralPath $file -Raw | ConvertFrom-Json
    if ($state.schema -ne 1 -or $state.product -ne 'Sumibi' -or @($state.versions).Count -eq 0) { throw 'Invalid installation receipt.' }
    foreach ($version in @($state.versions)) {
        if ($version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?-[0-9a-f]{32}$') { throw 'Invalid version directory in installation receipt.' }
        $null = Get-SumibiChildPath $Root "versions\$version"
    }
    if ($state.current -notin @($state.versions)) { throw 'Current version is missing from installation receipt.' }
    return $state
}

function Write-SumibiInstallState {
    param([string]$Root, $State)
    $path = Get-SumibiChildPath $Root 'installation.json'
    $temporary = Get-SumibiChildPath $Root ('receipt-' + [guid]::NewGuid().ToString('N') + '.tmp')
    $State | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $temporary -Encoding UTF8
    if (Test-Path -LiteralPath $path) { [IO.File]::Replace($temporary, $path, [System.Management.Automation.Language.NullString]::Value) }
    else { [IO.File]::Move($temporary, $path) }
}

function Invoke-SumibiInstall {
    param([string]$SourceDir, [string]$InstallDir, [switch]$NoStartup)
    Assert-SumibiUnpackagedInstaller
    if (-not (Test-SumibiAdministrator)) { throw 'Administrator privileges required.' }
    $root = Get-SumibiInstallRoot $InstallDir
    $manifest = Read-SumibiPackage $SourceDir
    Assert-SumibiStartupOwner $root
    $previous = Read-SumibiInstallState $root
    $versions = @(if ($null -ne $previous) { $previous.versions })
    $leaf = $manifest.version + '-' + [guid]::NewGuid().ToString('N')
    $versionDir = Get-SumibiChildPath $root "versions\$leaf"
    $null = New-Item -ItemType Directory -Path $root -Force
    Set-SumibiInstallAcl $root
    $versionsDir = Get-SumibiChildPath $root 'versions'
    $null = New-Item -ItemType Directory -Path $versionsDir -Force
    Set-SumibiInstallAcl $versionsDir
    # Never overwrite a loaded EXE or DLL, even for a reinstall of the same version.
    $null = New-Item -ItemType Directory -Path $versionDir
    Set-SumibiInstallAcl $versionDir
    $null = New-Item -ItemType Directory -Path (Join-Path $versionDir 'bin')
    foreach ($relative in @((Get-SumibiPackageFiles)) + @('package.json')) {
        Copy-Item -LiteralPath (Get-SumibiChildPath $SourceDir $relative) -Destination (Get-SumibiChildPath $versionDir $relative)
    }
    $null = Read-SumibiPackage $versionDir
    $setup = Join-Path $versionDir 'bin\sumibi-setup.exe'
    $dll = Join-Path $versionDir 'bin\sumibi_tip.dll'
    Invoke-SumibiSetup $setup @('plan-system-dev', '--dll', $dll)
    $state = [pscustomobject]@{ schema = 1; product = 'Sumibi'; current = $leaf; versions = @($versions) + @($leaf); status = 'Prepared' }
    # Keep a recovery receipt and the uninstaller before any registration mutation.
    foreach ($name in @('Uninstall.cmd', 'Uninstall.ps1', 'Installer.Common.ps1')) {
        Copy-Item -LiteralPath (Join-Path $versionDir $name) -Destination (Get-SumibiChildPath $root $name) -Force
    }
    Write-SumibiInstallState $root $state
    Stop-SumibiInstalledHosts $root
    Invoke-SumibiSetup $setup @('install', '--dll', $dll, '--scope', 'system')
    Assert-SumibiRegistration -Dll $dll
    $hostExe = Join-Path $versionDir 'bin\sumibi-host.exe'
    Set-SumibiStartup -Root $root -HostExe $hostExe -Disabled:$NoStartup
    Set-SumibiShortcuts -Root $root -HostExe $hostExe
    $state.status = 'Installed'
    Write-SumibiInstallState $root $state
    Write-Host "Installed Sumibi $($manifest.version) in $versionDir"
}

function Invoke-SumibiUninstall {
    param([string]$InstallDir)
    Assert-SumibiUnpackagedInstaller
    if (-not (Test-SumibiAdministrator)) { throw 'Administrator privileges required.' }
    $root = Get-SumibiInstallRoot $InstallDir
    if (-not (Test-Path -LiteralPath $root)) { Write-Host 'Sumibi is not installed at the managed location.'; return }
    $state = Read-SumibiInstallState $root
    if ($null -eq $state) { throw 'No installation receipt. Use the original setup tool to remove a legacy installation.' }
    Assert-SumibiStartupOwner $root
    foreach ($registered in @(Get-SumibiRegisteredDlls)) {
        $path = $registered -replace '^\\\\\?\\', ''
        if (-not $path.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'COM registration belongs to another installation.' }
    }
    $versionDir = Get-SumibiChildPath $root ('versions\' + $state.current)
    $setup = Get-SumibiChildPath $versionDir 'bin\sumibi-setup.exe'
    if (-not (Test-Path -LiteralPath $setup -PathType Leaf)) { throw 'Setup tool is missing. Reinstall before uninstalling; no files were removed.' }
    foreach ($version in @($state.versions)) {
        $directory = Get-SumibiChildPath $root "versions\$version"
        foreach ($relative in @((Get-SumibiPackageFiles)) + @('package.json')) { $null = Get-SumibiChildPath $directory $relative }
    }
    Stop-SumibiInstalledHosts $root
    Invoke-SumibiSetup $setup @('uninstall', '--scope', 'system')
    Assert-SumibiRegistration -Absent
    Set-SumibiStartup -Root $root -Disabled
    Set-SumibiShortcuts -Root $root -Remove
    $remaining = $false
    # Only known files are removed. Unrelated files and loaded DLLs are retained.
    foreach ($version in @($state.versions)) {
        $directory = Get-SumibiChildPath $root "versions\$version"
        foreach ($relative in @((Get-SumibiPackageFiles)) + @('package.json')) {
            $file = Get-SumibiChildPath $directory $relative
            if (Test-Path -LiteralPath $file -PathType Leaf) {
                if ($relative -eq 'bin/sumibi-setup.exe') { continue }
                try { Remove-Item -LiteralPath $file -Force } catch { $remaining = $true; Write-Warning "Retained file: $file" }
            }
        }
    }
    if ($remaining) {
        throw 'Registration removed, but loaded files remain. Restart applications and rerun this uninstaller. No reboot deletion was scheduled.'
    }
    foreach ($version in @($state.versions)) {
        $directory = Get-SumibiChildPath $root "versions\$version"
        $setupFile = Get-SumibiChildPath $directory 'bin\sumibi-setup.exe'
        if (Test-Path -LiteralPath $setupFile) { Remove-Item -LiteralPath $setupFile -Force }
        foreach ($folder in @((Join-Path $directory 'bin'), $directory)) {
            if ((Test-Path -LiteralPath $folder) -and @(Get-ChildItem -LiteralPath $folder -Force).Count -eq 0) { Remove-Item -LiteralPath $folder }
        }
    }
    foreach ($name in @('Uninstall.cmd', 'Uninstall.ps1', 'Installer.Common.ps1', 'installation.json')) {
        Remove-Item -LiteralPath (Get-SumibiChildPath $root $name) -Force
    }
    foreach ($folder in @((Join-Path $root 'versions'), $root)) {
        if ((Test-Path -LiteralPath $folder) -and @(Get-ChildItem -LiteralPath $folder -Force).Count -eq 0) { Remove-Item -LiteralPath $folder }
    }
    if (Test-Path -LiteralPath $root) { Write-Warning "Unmanaged files were retained in $root" }
    Write-Host 'Sumibi registration and managed files removed.'
}
