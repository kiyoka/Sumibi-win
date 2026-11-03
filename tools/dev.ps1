<#
.SYNOPSIS
    Consolidated development and test runner for Sumibi-win.
.DESCRIPTION
    Provides single-command invocation for frequent build, test, and verification workflows,
    minimizing sandbox permission approval requests in agent environments.
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('session-test', 'native-edit', 'build', 'build-tip', 'test', 'test-all', 'check', 'fmt', 'verify', 'ci', 'register-dev', 'unregister-dev', 'verify-com', 'help')]
    [string]$Task = 'session-test',

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository

function Invoke-Step {
    param(
        [string]$Description,
        [scriptblock]$Action
    )
    Write-Host "==> $Description" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "Step failed ($Description) with exit code $LASTEXITCODE"
    }
}

try {
    switch ($Task) {
        'session-test' {
            Invoke-Step 'Build sumibi-tip with spike feature' {
                & cargo build --package sumibi-tip --features spike
            }
            Invoke-Step 'Run sumibi-setup session-key-test' {
                & cargo run --package sumibi-setup -- session-key-test --dll target/debug/sumibi_tip.dll @ExtraArgs
            }
        }
        'native-edit' {
            Invoke-Step 'Build sumibi-tip with spike feature' {
                & cargo build --package sumibi-tip --features spike
            }
            Invoke-Step 'Run sumibi-setup native-edit-test' {
                & cargo run --package sumibi-setup -- native-edit-test --dll target/debug/sumibi_tip.dll @ExtraArgs
            }
        }
        'build-tip' {
            Invoke-Step 'Build sumibi-tip with spike feature' {
                & cargo build --package sumibi-tip --features spike @ExtraArgs
            }
        }
        'build' {
            Invoke-Step 'Build workspace' {
                & cargo build --workspace @ExtraArgs
            }
        }
        'test' {
            Invoke-Step 'Run standard workspace tests' {
                & cargo test --workspace @ExtraArgs
            }
            Invoke-Step 'Run workspace tests with sumibi-host/spike' {
                & cargo test --workspace --features sumibi-host/spike @ExtraArgs
            }
        }
        'test-all' {
            Invoke-Step 'Run workspace tests' {
                & cargo test --workspace
            }
            Invoke-Step 'Run workspace tests with sumibi-host/spike' {
                & cargo test --workspace --features sumibi-host/spike
            }
            Invoke-Step 'Run Test-HostImage' {
                & "$PSScriptRoot/Test-HostImage.ps1"
            }
            Invoke-Step 'Run Test-SuspendedImageProbe' {
                & "$PSScriptRoot/Test-SuspendedImageProbe.ps1"
            }
            Invoke-Step 'Build sumibi-tip with spike' {
                & cargo build --package sumibi-tip --features spike
            }
            Invoke-Step 'Run session-key-test' {
                & cargo run --package sumibi-setup -- session-key-test --dll target/debug/sumibi_tip.dll
            }
            Invoke-Step 'Run native-edit-test' {
                & cargo run --package sumibi-setup -- native-edit-test --dll target/debug/sumibi_tip.dll
            }
        }
        'check' {
            Invoke-Step 'Check format' {
                & cargo fmt --all -- --check
            }
            Invoke-Step 'Clippy check' {
                & cargo clippy --workspace --all-targets -- -D warnings
            }
        }
        'fmt' {
            Invoke-Step 'Format workspace' {
                & cargo fmt --all
            }
        }
        'verify' {
            Invoke-Step 'Format workspace' {
                & cargo fmt --all
            }
            Invoke-Step 'Check format' {
                & cargo fmt --all -- --check
            }
            Invoke-Step 'Clippy check' {
                & cargo clippy --workspace --all-targets -- -D warnings
            }
            Invoke-Step 'Run full test suite' {
                & powershell -ExecutionPolicy Bypass -File "$PSScriptRoot/dev.ps1" test-all
            }
        }
        'ci' {
            Invoke-Step 'Run verify workflow' {
                & powershell -ExecutionPolicy Bypass -File "$PSScriptRoot/dev.ps1" verify
            }
        }
        'register-dev' {
            Invoke-Step 'Build sumibi-tip' {
                & cargo build --package sumibi-tip --features spike
            }
            Invoke-Step 'Build sumibi-setup' {
                & cargo build --package sumibi-setup
            }
            Invoke-Step 'Register development COM class in HKCU' {
                & cargo run --package sumibi-setup -- register-com-dev --dll target/debug/sumibi_tip.dll
            }
        }
        'unregister-dev' {
            Invoke-Step 'Unregister development COM class in HKCU' {
                & cargo run --package sumibi-setup -- unregister-com-dev
            }
        }
        'verify-com' {
            Invoke-Step 'Verify development COM class in HKCU' {
                & cargo run --package sumibi-setup -- verify-com-dev
            }
        }
        'help' {
            Write-Host "Usage: .\tools\dev.ps1 <task> [extra args...]" -ForegroundColor Yellow
            Write-Host "Available tasks:"
            Write-Host "  session-test   : Build sumibi-tip with spike and run session-key-test (default)"
            Write-Host "  native-edit    : Build sumibi-tip and run native-edit-test (RichEdit & EDIT rejection)"
            Write-Host "  build-tip      : Build sumibi-tip with spike feature"
            Write-Host "  build          : Build entire workspace"
            Write-Host "  test           : Run workspace unit/integration tests"
            Write-Host "  test-all       : Run workspace tests, image security checks, and session tests"
            Write-Host "  check          : Run cargo fmt check and clippy (-D warnings)"
            Write-Host "  fmt            : Run cargo fmt --all"
            Write-Host "  verify / ci    : Run fmt, check, and test-all end-to-end"
            Write-Host "  register-dev   : Register development COM class in HKCU for sumibi_tip.dll"
            Write-Host "  unregister-dev : Unregister development COM class from HKCU"
            Write-Host "  verify-com     : Verify development COM registration in HKCU"
            Write-Host "  help           : Show this message"
        }
    }
}
finally {
    Pop-Location
}
