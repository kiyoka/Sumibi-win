param(
    [string]$Setup = (Join-Path $PSScriptRoot '..\target\debug\sumibi-setup.exe')
)
$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess) { throw 'Use 64-bit PowerShell.' }
$source = (Resolve-Path -LiteralPath $Setup).ProviderPath
$nativeSource = Join-Path (Split-Path -Path $source -Parent) 'sumibi-registration-worker.exe'
if (-not (Test-Path -LiteralPath $nativeSource -PathType Leaf)) { throw 'Build sumibi-setup including its dedicated registration worker first.' }
$sourceHash = (Get-FileHash -LiteralPath $nativeSource -Algorithm SHA256).Hash
$workerDir = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ('..\target\native-registration-' + [guid]::NewGuid().ToString('N'))))
$null = New-Item -Path $workerDir -ItemType Directory
# Label only this freshly created fixture directory and executable copy.
# Windows starts an executable at min(caller integrity, executable integrity).
& icacls.exe $workerDir /setintegritylevel '(OI)(CI)L' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not label the new worker directory Low.' }
$worker = Join-Path $workerDir 'sumibi-setup.exe'
Copy-Item -LiteralPath $nativeSource -Destination $worker
& icacls.exe $worker /setintegritylevel Low | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not label the worker copy Low.' }
if ((Get-FileHash -LiteralPath $worker -Algorithm SHA256).Hash -ne $sourceHash) { throw 'Worker copy changed.' }
[ordered]@{ sourceSha256 = $sourceHash; purpose = 'Low integrity, redirected native TSF API test only' } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $workerDir 'manifest.json') -Encoding UTF8
Write-Output "Native registration worker/evidence directory: $workerDir"

function Invoke-Worker {
    param(
        [string]$Action, [string]$Id, [string]$Stage = 'none', [int]$Index = 0,
        [int]$Expected = 0, [switch]$Normal
    )
    if ($Action -notmatch '^[a-z-]+$' -or $Id -notmatch '^[0-9a-f]{32}$' -or
        $Stage -notmatch '^[a-z-]+$' -or $Index -lt 0 -or $Index -gt 3) { throw 'Invalid fixture arguments.' }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = if ($Normal) { $source } else { $worker }
    $start.Arguments = "tsf-registry-fixture $Action $Id $Stage $Index"
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        $null = $process.Start()
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            $null = $process.WaitForExit(3000)
            throw "Native fixture timed out: $Action/$Stage/$Index, ID=$Id; evidence=$workerDir"
        }
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        if ($process.ExitCode -ne $Expected) {
            throw "Native fixture $Action/$Stage/$Index expected exit $Expected, got $($process.ExitCode). ID=$Id; $stdout $stderr; evidence=$workerDir"
        }
        return ($stdout + $stderr)
    } finally { $process.Dispose() }
}

$cases = @([pscustomobject]@{Stage='none'; Index=0; Kind='normal'})
foreach ($stage in @('create-before', 'create-after', 'create-fail', 'delete-before', 'delete-after-api', 'delete-during-prune', 'delete-after')) {
    foreach ($index in 0..3) {
        $kind = if ($stage.StartsWith('delete-')) { 'remove' } elseif ($stage -eq 'create-fail') { 'failure' } else { 'install' }
        $cases += [pscustomobject]@{Stage=$stage; Index=$index; Kind=$kind}
    }
}
foreach ($stage in @('com-empty', 'com-partial')) { $cases += [pscustomobject]@{Stage=$stage; Index=0; Kind='install'} }
$passed = 0
foreach ($case in $cases) {
    $id = [guid]::NewGuid().ToString('N')
    $null = Invoke-Worker -Action init -Id $id -Normal
    if ($case.Kind -eq 'normal') {
        $denied = Invoke-Worker -Action install -Id $id -Normal -Expected 1
        if (-not $denied.Contains('requires low integrity')) { throw 'Normal-integrity worker did not fail at its safety gate.' }
        $null = Invoke-Worker -Action init -Id $id -Normal -Expected 1
    }
    if ($case.Kind -in @('normal', 'remove')) {
        $null = Invoke-Worker -Action install -Id $id
        $null = Invoke-Worker -Action recover -Id $id
        $null = Invoke-Worker -Action assert-installed -Id $id
        $exit = if ($case.Kind -eq 'normal') { 0 } else { 77 }
        $null = Invoke-Worker -Action uninstall -Id $id -Stage $case.Stage -Index $case.Index -Expected $exit
    } elseif ($case.Kind -eq 'failure') {
        $failure = Invoke-Worker -Action install -Id $id -Stage $case.Stage -Index $case.Index -Expected 1
        if (-not $failure.Contains('rollback complete')) { throw "Native API failure did not complete rollback: $failure" }
    } else {
        $null = Invoke-Worker -Action install -Id $id -Stage $case.Stage -Index $case.Index -Expected 77
    }
    $null = Invoke-Worker -Action recover -Id $id
    $null = Invoke-Worker -Action assert-removed -Id $id
    $null = Invoke-Worker -Action uninstall -Id $id
    $null = Invoke-Worker -Action assert-real-absent -Id $id -Normal
    $null = Invoke-Worker -Action finish -Id $id -Normal
    $passed++
    Write-Output "Native COM/TSF registration / $($case.Stage)/$($case.Index): OK"
}
foreach ($kind in @('value', 'child', 'owner', 'enabled', 'second-profile', 'user', 'classes')) {
    $indices = if ($kind -in @('value', 'child')) { @(0, 1, 2, 3) } elseif ($kind -in @('enabled', 'second-profile')) { @(1) } else { @(0) }
    foreach ($index in $indices) {
        $id = [guid]::NewGuid().ToString('N')
        $null = Invoke-Worker -Action init -Id $id -Normal
        $null = Invoke-Worker -Action install -Id $id
        $null = Invoke-Worker -Action inject -Id $id -Stage $kind -Index $index
        $before = Invoke-Worker -Action inspect -Id $id -Normal
        $null = Invoke-Worker -Action recover -Id $id -Expected 1
        $null = Invoke-Worker -Action uninstall -Id $id -Expected 1
        $after = Invoke-Worker -Action inspect -Id $id -Normal
        if ($before -cne $after) { throw "Native conflict unexpectedly changed registration: $kind/$index ID=$id" }
        $null = Invoke-Worker -Action restore -Id $id -Stage $kind -Index $index
        $null = Invoke-Worker -Action assert-installed -Id $id
        $null = Invoke-Worker -Action uninstall -Id $id
        $null = Invoke-Worker -Action assert-removed -Id $id
        $null = Invoke-Worker -Action finish -Id $id -Normal
        $passed++
        Write-Output "Native COM/TSF registration / conflict-$kind/$index`: OK"
    }
}
foreach ($index in @(1, 2, 3)) {
    $id = [guid]::NewGuid().ToString('N')
    $null = Invoke-Worker -Action init -Id $id -Normal
    $null = Invoke-Worker -Action install -Id $id -Stage create-after -Index $index -Expected 77
    $mutation = if ($index -eq 1) { 'drop-enable' } else { 'drop-alias' }
    $null = Invoke-Worker -Action $mutation -Id $id -Index $index
    $null = Invoke-Worker -Action recover -Id $id
    $null = Invoke-Worker -Action assert-removed -Id $id
    $null = Invoke-Worker -Action finish -Id $id -Normal
    $passed++
    Write-Output "Native COM/TSF registration / partial-$mutation/$index`: OK"
}
Write-Output "Native COM/TSF registration backend: $passed cases passed; fixture roots removed."
Write-Output 'No production CLSID, default IME, certificate trust or real machine registration changed.'
# Preserve journals/manifest; remove only the known copied executable on success.
[IO.File]::Delete($worker)
