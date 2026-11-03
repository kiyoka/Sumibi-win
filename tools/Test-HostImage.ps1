[CmdletBinding()]
param()

# Read PE metadata and bounded forwarders using only explicit held OS fixtures.
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    if (-not [Environment]::Is64BitOperatingSystem -or $env:OS -ne 'Windows_NT') {
        throw 'This check requires Windows x64.'
    }
    & cargo test -p sumibi-ipc --lib --locked --offline host_image -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Host image parser checks failed.' }
    & cargo test -p sumibi-ipc --lib --locked --offline dll_exports -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'DLL export parser checks failed.' }
    & cargo test -p sumibi-ipc --lib --locked --offline system_module -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'System module lease checks failed.' }
    & cargo test -p sumibi-host --test host_image --test dependency_exports --locked --offline -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Built Host / Kernel32 metadata check failed.' }
    Write-Output 'Host / DLL metadata checks passed; no Host launched or dependency trust asserted.'
}
finally {
    Pop-Location
}
