[CmdletBinding()]
param()

# Signed vswhere copies are created suspended and terminated without resume.
# No Host discovery, registration, signer trust installation or API traffic.
$ErrorActionPreference = 'Stop'
$previousRequirement = [Environment]::GetEnvironmentVariable('SUMIBI_REQUIRE_SIGNED_LAUNCH_PROBE', 'Process')
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    [Environment]::SetEnvironmentVariable('SUMIBI_REQUIRE_SIGNED_LAUNCH_PROBE', '1', 'Process')
    & cargo test -p sumibi-ipc --lib --locked --offline launch_probe -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Signed suspended-image probe failed.' }
    Write-Output 'Signed suspended-image observation, private-job termination and cleanup passed; no resume performed.'
}
finally {
    [Environment]::SetEnvironmentVariable('SUMIBI_REQUIRE_SIGNED_LAUNCH_PROBE', $previousRequirement, 'Process')
    Pop-Location
}
