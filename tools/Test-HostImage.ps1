[CmdletBinding()]
param()

# Read PE metadata from synthetic fixtures and the freshly built Host only.
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    if (-not [Environment]::Is64BitOperatingSystem -or $env:OS -ne 'Windows_NT') {
        throw 'This check requires Windows x64.'
    }
    & cargo test -p sumibi-ipc --lib --locked --offline host_image -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Host image parser checks failed.' }
    & cargo test -p sumibi-host --test host_image --locked --offline -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Built Host image check failed.' }
    Write-Output 'Host image metadata checks passed; no Host launched or dependency trust asserted.'
}
finally {
    Pop-Location
}
