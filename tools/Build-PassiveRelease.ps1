#Requires -Version 5.1
[CmdletBinding()]
param(
    [ValidateSet('Release', 'Debug')][string]$Configuration = 'Release',
    [string]$TargetDir = '',
    [string]$SetupExe = ''
)
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    $profile = $Configuration.ToLowerInvariant()
    $profileArgs = @()
    if ($Configuration -eq 'Release') { $profileArgs += '--release' }
    if (-not $TargetDir) { $TargetDir = Join-Path $repository 'target' }
    $TargetDir = [IO.Path]::GetFullPath($TargetDir)
    # Setup depends on the diagnostic TIP. Build it in another target directory
    # before the production DLL; validation itself must never trigger a build.
    if (-not $SetupExe) {
        $toolTarget = Join-Path $repository 'target\passive-tools'
        & cargo build -p sumibi-setup --locked --offline --target-dir $toolTarget @profileArgs
        if ($LASTEXITCODE -ne 0) { throw 'Failed to build passive inspection tool.' }
        $SetupExe = Join-Path $toolTarget "$profile\sumibi-setup.exe"
    }
    & cargo build -p sumibi-tip --no-default-features --locked --offline --target-dir $TargetDir @profileArgs
    if ($LASTEXITCODE -ne 0) { throw 'Failed to build production TIP.' }
    $dll = Join-Path $TargetDir "$profile\sumibi_tip.dll"
    & $SetupExe plan-system-dev --dll $dll
    if ($LASTEXITCODE -ne 0) { throw 'Production TIP preflight failed.' }
    Write-Host "Production TIP verified: $dll"
} finally { Pop-Location }
