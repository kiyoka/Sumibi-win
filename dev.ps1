<#
.SYNOPSIS
    Convenience entrypoint forwarding to tools/dev.ps1.
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Task = 'session-test',

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

& powershell -ExecutionPolicy Bypass -File "$PSScriptRoot/tools/dev.ps1" -Task $Task @ExtraArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
