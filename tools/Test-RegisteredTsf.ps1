param(
    [string]$Dll = (Join-Path $PSScriptRoot '..\target\debug\sumibi_tip.dll')
)

$ErrorActionPreference = 'Stop'
$setup = Join-Path $PSScriptRoot '..\target\debug\sumibi-setup.exe'
if (-not (Test-Path -LiteralPath $setup -PathType Leaf)) {
    throw 'Build sumibi-setup before running this test.'
}
if (-not (Test-Path -LiteralPath $Dll -PathType Leaf)) {
    throw 'Build sumibi-tip before running this test.'
}

& $setup register-com-dev --dll $Dll
if ($LASTEXITCODE -ne 0) {
    throw "Current-user COM registration failed ($LASTEXITCODE)."
}

try {
    & $setup registered-tsf-test-dev
    if ($LASTEXITCODE -ne 0) {
        throw "Registry-loaded TSF probe failed ($LASTEXITCODE)."
    }
}
finally {
    & $setup unregister-com-dev
    if ($LASTEXITCODE -ne 0) {
        throw "Development COM registration cleanup failed ($LASTEXITCODE)."
    }
}
