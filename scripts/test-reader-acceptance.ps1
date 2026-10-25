# SPDX-License-Identifier: GPL-3.0-or-later
[CmdletBinding()]
param([switch]$SelfTest, [string]$OutputDirectory, [string]$TraceDirectory)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
function Assert-FreshDirectory([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'Supply a new explicit output directory.' }
    if (Test-Path -LiteralPath $Path) { throw "Output already exists: $Path" }
}
function Invoke-OwnedChild([string]$Executable, [string[]]$Arguments, [int]$TimeoutMilliseconds) {
    $start = [Diagnostics.ProcessStartInfo]::new($Executable)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($item in $Arguments) { $start.ArgumentList.Add($item) }
    $child = [Diagnostics.Process]::new()
    $child.StartInfo = $start
    try {
        if (-not $child.Start()) { throw 'Failed to start owned acceptance process.' }
        $stdout = $child.StandardOutput.ReadToEndAsync()
        $errors = [Text.StringBuilder]::new()
        $nextLine = $child.StandardError.ReadLineAsync()
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while (-not $child.HasExited -and $clock.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
            if ($nextLine.IsCompleted) {
                $line = $nextLine.GetAwaiter().GetResult()
                if ($null -ne $line) { [void]$errors.AppendLine($line); Write-Host $line; $nextLine = $child.StandardError.ReadLineAsync() }
            }
            [void]$child.WaitForExit(25)
        }
        $timedOut = -not $child.HasExited
        if ($timedOut) { $child.Kill($true); $child.WaitForExit() }
        while ($null -ne ($line = $nextLine.GetAwaiter().GetResult())) {
            [void]$errors.AppendLine($line); Write-Host $line; $nextLine = $child.StandardError.ReadLineAsync()
        }
        [pscustomobject]@{ stdout = $stdout.GetAwaiter().GetResult(); stderr = $errors.ToString(); timed_out = $timedOut; exit_code = $child.ExitCode }
    }
    finally { $child.Dispose() }
}
function Get-CaseOutcome([string]$Id, [object]$Result) {
    if ($Result.timed_out) { return [pscustomobject]@{ id = $Id; status = 'failed'; reason = 'Owned child exceeded watchdog budget.'; exit_code = $Result.exit_code } }
    if ($Result.exit_code -notin @(0, 3)) { return [pscustomobject]@{ id = $Id; status = 'failed'; reason = 'Child exited unsuccessfully.'; exit_code = $Result.exit_code } }
    try {
        $events = @($Result.stdout -split '\r?\n' | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json -ErrorAction Stop })
        $final = @($events | Where-Object kind -eq 'result')
        if ($final.Count -ne 1) { throw 'Missing or ambiguous final result.' }
        $status = $final[0].details.status
        if ($status -notin @('passed', 'inconclusive', 'failed')) { throw 'Invalid child result status.' }
        if (($Result.exit_code -eq 0 -and $status -ne 'passed') -or ($Result.exit_code -eq 3 -and $status -ne 'inconclusive')) { throw 'Child exit code and result disagree.' }
        if (@($events | Where-Object { $_.kind -in @('error', 'cleanup_error', 'reconnect_error') }).Count -gt 0) { throw 'Child emitted an operation failure.' }
        return [pscustomobject]@{ id = $Id; status = $status; exit_code = $Result.exit_code }
    }
    catch { return [pscustomobject]@{ id = $Id; status = 'failed'; reason = $_.Exception.Message; exit_code = $Result.exit_code } }
}
Push-Location -LiteralPath $workspace
try {
    if ($SelfTest) {
        $temporary = Join-Path ([IO.Path]::GetTempPath()) ('uareu-reader-selftest-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $temporary | Out-Null
        try {
            $rejected = $false
            try { Assert-FreshDirectory $temporary } catch { $rejected = $true }
            if (-not $rejected) { throw 'Fresh-directory guard accepted an existing output.' }
            $result = Invoke-OwnedChild (Get-Process -Id $PID).Path @('-NoProfile', '-Command', 'Start-Sleep -Seconds 5') 150
            if (-not $result.timed_out) { throw 'Owned-process watchdog did not terminate its test child.' }
            $testBin = Join-Path $temporary 'bin/'
            $testObj = Join-Path $temporary 'obj/'
            & dotnet build examples/csharp/UareuExample.csproj -c Release --nologo '-p:PlatformTarget=AnyCPU' "-p:OutputPath=$testBin" "-p:BaseIntermediateOutputPath=$testObj" '-p:DefaultItemExcludes=**/obj/**'
            if ($LASTEXITCODE -ne 0) { throw 'C# build failed.' }
            & dotnet (Join-Path $testBin "UareuExample.dll") --hardware-acceptance-self-test
            if ($LASTEXITCODE -ne 0) { throw 'Hardware policy self-tests failed.' }
            foreach ($sample in @(
                [pscustomobject]@{ stdout = ''; exit_code = 99; timed_out = $false },
                [pscustomobject]@{ stdout = '{bad json'; exit_code = 0; timed_out = $false },
                [pscustomobject]@{ stdout = '{"kind":"phase"}'; exit_code = 0; timed_out = $false },
                [pscustomobject]@{ stdout = '{"kind":"result","details":{"status":"passed"}}'; exit_code = 3; timed_out = $false }
            )) {
                if ((Get-CaseOutcome 'self-test' $sample).status -ne 'failed') { throw 'Abrupt, malformed or inconsistent child output was not classified as failed.' }
            }
            $valid = [pscustomobject]@{ stdout = '{"kind":"result","details":{"status":"inconclusive"}}'; exit_code = 3; timed_out = $false }
            if ((Get-CaseOutcome 'self-test' $valid).status -ne 'inconclusive') { throw 'Valid inconclusive child result was lost.' }
            Write-Output 'Fresh-directory, owned-child watchdog, child-result validation and no-native hardware policy checks passed.'
        }
        finally { Remove-Item -LiteralPath $temporary -Recurse -Force }
        return
    }
    if (-not $IsWindows) { throw 'Reader acceptance requires Windows x64 with a manually provisioned WinUSB reader.' }
    Assert-FreshDirectory $OutputDirectory
    $destination = [IO.Path]::GetFullPath($OutputDirectory)
    if ($TraceDirectory -and -not (Test-Path -LiteralPath $TraceDirectory -PathType Container)) { throw 'TraceDirectory does not exist.' }
    New-Item -ItemType Directory -Path $destination | Out-Null
    & dotnet build examples/csharp/UareuExample.csproj -c Release --nologo
    if ($LASTEXITCODE -ne 0) { throw 'C# build failed.' }
    $dll = Join-Path $workspace 'target/release/uareu.dll'
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) { throw 'Build the x64 release DLL before acceptance.' }
    $previousDll = $env:UAREU_DLL_PATH
    $env:UAREU_DLL_PATH = $dll
    $cases = @(
        [pscustomobject]@{ name = 'placements-default'; count = 50; timeout = 1860000 },
        [pscustomobject]@{ name = 'placements-retained'; count = 50; timeout = 1860000 },
        [pscustomobject]@{ name = 'continuous'; count = 1; timeout = 90000 },
        [pscustomobject]@{ name = 'prolonged'; count = 1; timeout = 1860000 }
    )
    foreach ($phase in @('capture', 'probe', 'recovery', 'async')) {
        1..5 | ForEach-Object { $cases += [pscustomobject]@{ name = "unplug-$phase"; count = 1; timeout = 90000 } }
    }
    1..25 | ForEach-Object { $cases += [pscustomobject]@{ name = 'race'; count = $_; timeout = 90000 } }
    $summary = [Collections.Generic.List[object]]::new()
    try {
        $run = [ordered]@{ schema_version = 1; utc = [DateTimeOffset]::UtcNow; commit = (& git rev-parse HEAD); dirty_diff_sha256 = ([Convert]::ToHexStringLower([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes((& git diff --binary | Out-String))))); os = [Environment]::OSVersion.ToString(); dll_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $dll).Hash.ToLowerInvariant(); dotnet = (& dotnet --version); limitation = 'Operator acknowledgements and call markers alone do not prove physical USB phase overlap.' }
        $run | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $destination 'run.json') -Encoding utf8
        $index = 0
        foreach ($case in $cases) {
            $index++
            $id = '{0:D3}-{1}' -f $index, $case.name
            $reply = Read-Host "Prepare case $id. Press Enter to run, or type stop to leave remaining cases not_run"
            if ($reply -eq 'stop') { $summary.Add([pscustomobject]@{ id = $id; status = 'not_run'; reason = 'Operator stopped.' }); break }
            $trace = '-'
            if ($TraceDirectory) { $trace = Join-Path ([IO.Path]::GetFullPath($TraceDirectory)) "$id.json" }
            $caseSummaryIndex = $summary.Count
            $summary.Add([pscustomobject]@{ id = $id; status = 'failed'; reason = 'Child startup, logging or result collection did not complete.' })
            $result = Invoke-OwnedChild 'dotnet' @((Join-Path $workspace 'examples/csharp/bin/Release/net10.0/UareuExample.dll'), '--hardware-acceptance', $case.name, [string]$case.count, $trace) $case.timeout
            $result.stdout | Set-Content -LiteralPath (Join-Path $destination "$id.jsonl") -Encoding utf8
            $result.stderr | Set-Content -LiteralPath (Join-Path $destination "$id.operator.txt") -Encoding utf8
            $outcome = Get-CaseOutcome $id $result
            $summary[$caseSummaryIndex] = $outcome
            if ($outcome.status -eq 'failed') { throw "Case $id failed; later cases are not_run. Inspect the retained child logs before another run." }
            # Re-enumeration is performed by each next owned child. No failed session is reused.
        }
    }
    finally {
        $env:UAREU_DLL_PATH = $previousDll
        [ordered]@{ schema_version = 1; acceptance = 'A1'; status = $(if ($summary.Count -eq $cases.Count -and @($summary | Where-Object status -ne 'passed').Count -eq 0) { 'passed' } elseif (@($summary | Where-Object status -eq 'failed').Count -gt 0) { 'failed' } else { 'inconclusive' }); expected_case_count = $cases.Count; completed = $summary } | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $destination 'summary.json') -Encoding utf8
    }
}
finally { Pop-Location }
