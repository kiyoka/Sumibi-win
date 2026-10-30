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
$previousIncremental = $env:CARGO_INCREMENTAL
Push-Location -LiteralPath $workspacePath
try {
    $env:CARGO_INCREMENTAL = '0'
    Invoke-Cargo -Arguments @('build', '--workspace', '--locked', '--offline')
    Invoke-Cargo -Arguments @('build', '-p', 'sumibi-tip', '--features', 'spike', '--locked', '--offline')
    & (Join-Path $workspacePath 'target/debug/sumibi-setup.exe') 'broker-http-key-test' '--dll' (Join-Path $workspacePath 'target/debug/sumibi_tip.dll')
    if ($LASTEXITCODE -ne 0) { throw 'Broker HTTP / TSF check failed.' }
}
finally {
    try { Invoke-Cargo -Arguments @('build', '-p', 'sumibi-tip', '--locked', '--offline') }
    finally {
        $env:CARGO_INCREMENTAL = $previousIncremental
        Pop-Location
    }
}
Write-Output 'Loopback HTTP / Broker / TSF check passed; passive TIP restored.'
