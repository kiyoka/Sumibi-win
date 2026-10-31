[CmdletBinding()]
param()

# Explicit Internet check. Test destinations and synthetic data are fixed in
# the Rust tests: Postman Echo and expired/self-signed badssl test services.
# No stored settings/keys, certificate-store changes, TSF or machine registration.
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    & cargo test -p sumibi-winhttp -p sumibi-host --lib --locked --offline public_https -- --ignored --nocapture --test-threads=1
    if ($LASTEXITCODE -ne 0) {
        throw 'Native HTTPS check failed; do not relax certificate validation.'
    }
    Write-Output 'Native HTTPS / Host policy / invalid certificate rejection passed; synthetic fixtures only.'
}
finally {
    Pop-Location
}
