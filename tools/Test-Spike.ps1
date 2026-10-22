param(
    [string]$CargoPath = '',
    [switch]$SkipDesktop
)

$ErrorActionPreference = 'Stop'
$workspacePath = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (!$CargoPath) {
    $cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargoCommand) { $CargoPath = $cargoCommand.Source }
    else { $CargoPath = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
}
if (!(Test-Path -LiteralPath $CargoPath)) { throw 'Cargo was not found.' }

function Invoke-Cargo {
    param([string[]]$Arguments)
    & $CargoPath @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo check failed: $($Arguments -join ' ')" }
}
function Invoke-Spike {
    param([string]$Name)
    & (Join-Path $workspacePath 'target/debug/sumibi-setup.exe') $Name --dll (Join-Path $workspacePath 'target/debug/sumibi_tip.dll')
    if ($LASTEXITCODE -ne 0) { throw "Native spike check failed: $Name" }
}

$previousIncremental = $env:CARGO_INCREMENTAL
Push-Location -LiteralPath $workspacePath
try {
    $env:CARGO_INCREMENTAL = '0'
    Invoke-Cargo -Arguments @('fmt', '--all', '--check')
    Invoke-Cargo -Arguments @('test', '--workspace', '--locked', '--offline')
    Invoke-Cargo -Arguments @('clippy', '--workspace', '--all-targets', '--locked', '--offline', '--', '-D', 'warnings')
    Invoke-Cargo -Arguments @('clippy', '-p', 'sumibi-tip', '--no-default-features', '--all-targets', '--locked', '--offline', '--', '-D', 'warnings')
    Invoke-Cargo -Arguments @('clippy', '-p', 'sumibi-host', '--all-features', '--all-targets', '--locked', '--offline', '--', '-D', 'warnings')
    Invoke-Cargo -Arguments @('build', '--workspace', '--locked', '--offline')
    Invoke-Cargo -Arguments @('run', '-p', 'sumibi-tip', '--features', 'spike', '--example', 'load_smoke', '--locked', '--offline', '--', 'target/debug/sumibi_tip.dll')
    foreach ($name in @('self-test', 'edit-test', 'key-test')) { Invoke-Spike -Name $name }
    if (!$SkipDesktop) {
        Invoke-Cargo -Arguments @('build', '-p', 'sumibi-host', '--features', 'spike', '--locked', '--offline')
        Invoke-Spike -Name 'broker-test'
        Invoke-Spike -Name 'broker-key-test'
        Invoke-Spike -Name 'broker-input-test'
        Invoke-Spike -Name 'protection-test'
        Invoke-Spike -Name 'composition-test'
        Invoke-Spike -Name 'owned-composition-test'
        Invoke-Spike -Name 'tracking-input-test'
        Invoke-Spike -Name 'composing-input-test'
        Invoke-Spike -Name 'input-notify-test'
        Invoke-Spike -Name 'transaction-input-test'
        foreach ($name in @('window-test', 'input-test', 'close-test', 'delayed-test')) { Invoke-Spike -Name $name }
        & (Join-Path $workspacePath 'target/debug/sumibi-setup.exe') ipc-test
        if ($LASTEXITCODE -ne 0) { throw 'Desktop pipe check failed; use a normal user session.' }
        & (Join-Path $workspacePath 'target/debug/sumibi-setup.exe') ipc-peer-test
        if ($LASTEXITCODE -ne 0) { throw 'Two-process pipe check failed; use a normal user session.' }
    }
    Write-Output 'All selected spike checks passed.'
    if ($SkipDesktop) { Write-Output 'Desktop window/input/close/delayed/broker/protection/composition/owned-composition/tracking/composing/input-notify/transaction/pipe/peer checks were explicitly skipped.' }
}
finally {
    $env:CARGO_INCREMENTAL = $previousIncremental
    Pop-Location
}
