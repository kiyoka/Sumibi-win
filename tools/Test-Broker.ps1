param([string]$CargoPath = '')

$ErrorActionPreference = 'Stop'
$workspacePath = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (!$CargoPath) {
    $cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargoCommand) { $CargoPath = $cargoCommand.Source }
    else { $CargoPath = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
}
$CargoPath = [IO.Path]::GetFullPath($CargoPath)
if (!(Test-Path -LiteralPath $CargoPath)) { throw 'Cargo was not found.' }

function Invoke-Cargo {
    param([string[]]$Arguments)
    & $CargoPath @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed: $($Arguments -join ' ')" }
}
function Invoke-Broker {
    param([string]$Name)
    & (Join-Path $workspacePath 'target/debug/sumibi-setup.exe') $Name
    if ($LASTEXITCODE -ne 0) { throw "Broker check failed: $Name" }
}

$previousIncremental = $env:CARGO_INCREMENTAL
$restoreDefaultHost = $false
Push-Location -LiteralPath $workspacePath
try {
    $env:CARGO_INCREMENTAL = '0'
    # The default Host must be built first: it intentionally denies the
    # diagnostic converter. The second build replaces only that Host EXE.
    Invoke-Cargo -Arguments @('build', '--workspace', '--locked', '--offline')
    Invoke-Broker -Name 'broker-channel-denied-test'
    Invoke-Broker -Name 'broker-fanout-denied-test'
    Invoke-Broker -Name 'broker-policy-runtime-denied-test'
    $restoreDefaultHost = $true
    Invoke-Cargo -Arguments @('build', '-p', 'sumibi-host', '--features', 'spike', '--locked', '--offline')
    Invoke-Broker -Name 'broker-channel-test'
    Invoke-Broker -Name 'broker-fanout-test'
    Invoke-Broker -Name 'broker-policy-runtime-test'
    Invoke-Broker -Name 'broker-worker-test'
}
finally {
    try {
        if ($restoreDefaultHost) {
            Invoke-Cargo -Arguments @('build', '-p', 'sumibi-host', '--locked', '--offline')
        }
    }
    finally {
        $env:CARGO_INCREMENTAL = $previousIncremental
        Pop-Location
    }
}
Write-Output 'Default denial, Broker Channel, Broker Fanout, Host policy runtime, and Broker Worker checks passed; default Host restored.'
