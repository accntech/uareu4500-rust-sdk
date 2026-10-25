# SPDX-License-Identifier: GPL-3.0-or-later
[CmdletBinding()]
param([switch]$SelfTest, [string]$OutputDirectory, [bool]$IncludeRecognitionHelper = $true, [string[]]$ApprovedDirtySource = @())
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
function Assert-Fresh([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'Supply an explicit new output directory.' }
    if (Test-Path -LiteralPath $Path) { throw "Destination already exists: $Path" }
}
function Source-Relative([string]$Root, [string]$Path) {
    $relative = [IO.Path]::GetRelativePath($Root, $Path).Replace('\', '/')
    if ($relative -eq '..' -or $relative.StartsWith('../') -or [IO.Path]::IsPathRooted($relative)) { throw 'Source escapes its snapshot root.' }
    return $relative
}
function Assert-Permitted([string]$Path) {
    if ($Path -match '["\r\n\t]') { throw 'Quoted/control-character source paths are unsupported; refusing ambiguous Git path parsing.' }
    if ([IO.Path]::IsPathRooted($Path) -or $Path -match '(^|/)\.\.(/|$)') { throw 'Source path escapes its root.' }
    if ($Path -match '(?i)(^|/)(bin|obj|captures|target|dist|\.git|\.agents)(/|$)') { throw "Generated/private directory forbidden in source inventory: $Path" }
    if ($Path -match '(?i)\.(png|pgm|jpg|jpeg|bmp|wsq|fmr|fir|dat|dll|exe|pdb|key|pfx|env)$') { throw "Binary/private input forbidden in source inventory: $Path" }
    if ($Path -match '(?i)(^|/)(digitalpersona|vendor-runtime)(/|$)') { throw 'Vendor runtime input forbidden.' }
    if ($Path -match '(?i)\.(wire|gray)$' -and $Path -notmatch '^fixtures/synthetic-(plain|scrambled)\.(wire|gray)$') { throw 'Only explicit project synthetic wire/gray fixtures may be included.' }
}
function Get-SourceInventory([string]$Root) {
    $files = [Collections.Generic.List[object]]::new()
    $rootFiles = @('Cargo.toml', 'Cargo.lock', 'README.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md', '.cargo/config.toml')
    foreach ($name in $rootFiles) {
        $path = Join-Path $Root $name
        if (Test-Path -LiteralPath $path -PathType Leaf) { $files.Add((Get-Item -LiteralPath $path)) }
        elseif ($name -ne 'THIRD_PARTY_NOTICES.md') { throw "Required source file missing: $name" }
    }
    $extensions = @{
        'src' = @('.rs'); 'include' = @('.h'); 'examples' = @('.rs', '.cs', '.csproj', '.json', '.md');
        'tests' = @('.rs', '.c'); 'scripts' = @('.ps1', '.py'); 'xtask' = @('.rs', '.toml');
        'fixtures' = @('.wire', '.gray', '.md', '.json'); 'licenses' = @('*');
        'udev' = @('.rules'); '.github/workflows' = @('.yml', '.yaml'); 'docs/validation' = @('.md', '.json')
    }
    foreach ($group in $extensions.Keys) {
        $directory = Join-Path $Root $group
        if (-not (Test-Path -LiteralPath $directory)) { continue }
        foreach ($item in Get-ChildItem -LiteralPath $directory -Recurse -Force) {
            $relative = Source-Relative $Root $item.FullName
            if ($relative -match '(?i)(^|/)(bin|obj|target|dist|captures|\.git|\.agents)(/|$)') { continue }
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Symlink/reparse point not allowed: $relative" }
            if ($item.PSIsContainer) { continue }
            if ($item.Extension -notin $extensions[$group] -and '*' -notin $extensions[$group]) { continue }
            Assert-Permitted $relative
            # JSON input is restricted to project lock/config and public validation manifests.
            if ($item.Extension -eq '.json' -and $relative -notmatch '(^examples/[^/]+/(packages\.lock|.*\.runtimeconfig)\.json$|^fixtures/reference/manifest\.json$|^docs/validation/)') { continue }
            $files.Add($item)
        }
    }
    $records = @($files | Sort-Object FullName -Unique | ForEach-Object {
        $relative = Source-Relative $Root $_.FullName
        Assert-Permitted $relative
        if (($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse source: $relative" }
        [pscustomobject]@{ path = $relative; bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    })
    foreach ($required in @('Cargo.toml', 'Cargo.lock', '.cargo/config.toml', 'include/uareu.h', 'src/lib.rs', 'xtask/Cargo.toml', 'examples/engine/packages.lock.json', 'examples/engine/UareuEngine.csproj', 'examples/csharp/UareuExample.csproj')) {
        if ($required -notin $records.path) { throw "Source inventory incomplete: $required" }
    }
    return $records
}
function Get-ChangedSource([object[]]$Inventory, [string[]]$TrackedPaths, [string[]]$DirtyPaths) {
    # Every filesystem input absent from Git's tracked set is untracked, even
    # when .gitignore hides it from ls-files --others --exclude-standard.
    return @($Inventory.path | Where-Object { $_ -notin $TrackedPaths -or $_ -in $DirtyPaths } | Sort-Object -Unique)
}
function Assert-SourceApproval([string[]]$Changed, [string[]]$Approved) {
    $unapproved = @($Changed | Where-Object { $_ -notin $Approved })
    if ($unapproved.Count -gt 0) { throw ('Review and explicitly name dirty/untracked source inputs with -ApprovedDirtySource: ' + ($unapproved -join ', ')) }
}
function Assert-Inventory([string]$Root, [object[]]$Records) {
    foreach ($record in $Records) {
        Assert-Permitted $record.path
        $path = Join-Path $Root $record.path
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Snapshot missing $($record.path)" }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $record.sha256) { throw "Snapshot hash differs: $($record.path)" }
    }
}
function Invoke-Checked([string]$Executable, [string[]]$Arguments) {
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Executable failed with exit $LASTEXITCODE" }
}
function Assert-X64PE([string]$Path) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) { throw "Not a PE image: $Path" }
    $offset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($offset -lt 0 -or $offset + 6 -gt $bytes.Length -or [BitConverter]::ToUInt32($bytes, $offset) -ne 0x4550 -or [BitConverter]::ToUInt16($bytes, $offset + 4) -ne 0x8664) { throw "Not an x64 PE image: $Path" }
}
Push-Location -LiteralPath $workspace
try {
    if ($SelfTest) {
        $temporary = Join-Path ([IO.Path]::GetTempPath()) ('uareu-package-selftest-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $temporary | Out-Null
        try {
            $rejections = 0
            foreach ($unsafe in @('captures/private.png', 'examples/engine/bin/Release/tool.dll', 'fixtures/real.gray', 'secret.pfx', '../escape.rs')) {
                try { Assert-Permitted $unsafe }
                catch { $rejections++ }
            }
            if ($rejections -ne 5) { throw 'Private/generated source input was accepted.' }
            $rejected = $false; try { Assert-Fresh $temporary } catch { $rejected = $true }
            if (-not $rejected) { throw 'Existing destination was accepted.' }
            Set-Content -LiteralPath (Join-Path $temporary 'input.rs') -Value 'original' -Encoding utf8
            $record = [pscustomobject]@{ path = 'input.rs'; sha256 = (Get-FileHash -LiteralPath (Join-Path $temporary 'input.rs')).Hash.ToLowerInvariant() }
            Assert-Inventory $temporary @($record)
            Set-Content -LiteralPath (Join-Path $temporary 'input.rs') -Value 'changed' -Encoding utf8
            $rejected = $false; try { Assert-Inventory $temporary @($record) } catch { $rejected = $true }
            if (-not $rejected) { throw 'Altered snapshot passed hash validation.' }
            $ignoredRoot = Join-Path $temporary 'ignored-repository'
            New-Item -ItemType Directory -Path $ignoredRoot | Out-Null
            & git -C $ignoredRoot init --quiet
            if ($LASTEXITCODE -ne 0) { throw 'Ignored-source test repository initialization failed.' }
            Set-Content -LiteralPath (Join-Path $ignoredRoot '.gitignore') -Value 'ignored.cs' -Encoding utf8
            Set-Content -LiteralPath (Join-Path $ignoredRoot 'ignored.cs') -Value '// unrelated ignored source' -Encoding utf8
            & git -C $ignoredRoot check-ignore --quiet ignored.cs
            if ($LASTEXITCODE -ne 0) { throw 'Ignored-source test fixture is not ignored.' }
            $tracked = @(& git -c core.quotePath=false -C $ignoredRoot ls-files)
            if ($LASTEXITCODE -ne 0) { throw 'Ignored-source tracked-set query failed.' }
            $visibleOthers = @(& git -c core.quotePath=false -C $ignoredRoot ls-files --others --exclude-standard)
            if ($LASTEXITCODE -ne 0) { throw 'Ignored-source visibility query failed.' }
            if ('ignored.cs' -in $visibleOthers) { throw 'Ignored input unexpectedly visible in old discovery path.' }
            $changed = @(Get-ChangedSource @([pscustomobject]@{ path = 'ignored.cs' }) $tracked @())
            $rejected = $false; try { Assert-SourceApproval $changed @() } catch { $rejected = $true }
            if (-not $rejected) { throw 'Ignored source input bypassed explicit approval.' }
            Assert-SourceApproval $changed @('ignored.cs')
            $inventory = @(Get-SourceInventory $workspace)
            if ((Test-Path -LiteralPath (Join-Path $workspace "scripts/test-macos-dylib.py")) -and "scripts/test-macos-dylib.py" -notin $inventory.path) { throw "Source inventory omitted optional native-loader verification script." }
            if ($inventory.Count -lt 10) { throw 'Project source inventory is implausibly small.' }
            Write-Output 'Source inventory, private-input rejection, destination and tampered-snapshot checks passed.'
        }
        finally { Remove-Item -LiteralPath $temporary -Recurse -Force }
        return
    }
    if (-not $IsWindows) { throw 'Packaging requires a Windows x64 MSVC build host.' }
    if (((& rustc -vV) -join "`n") -notmatch 'host: x86_64-pc-windows-msvc') { throw 'Use the x64 MSVC Rust host.' }
    Assert-Fresh $OutputDirectory
    $destination = [IO.Path]::GetFullPath($OutputDirectory)
    $workspaceFull = [IO.Path]::GetFullPath($workspace).TrimEnd([IO.Path]::DirectorySeparatorChar)
    if ($destination.Equals($workspaceFull, [StringComparison]::OrdinalIgnoreCase) -or $destination.StartsWith($workspaceFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Use an output directory outside the source checkout to prevent self-inclusion.' }
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('uareu-package-' + [Guid]::NewGuid().ToString('N'))
    $snapshot = Join-Path $temporary 'source'
    New-Item -ItemType Directory -Path $snapshot -Force | Out-Null
    try {
        $inventory = @(Get-SourceInventory $workspace)
        $trackedPaths = @(& git -c core.quotePath=false ls-files)
        if ($LASTEXITCODE -ne 0) { throw 'Cannot determine tracked source inputs.' }
        $dirtyPaths = @(& git -c core.quotePath=false diff HEAD --name-only)
        if ($LASTEXITCODE -ne 0) { throw 'Cannot determine modified tracked source inputs.' }
        if (@($trackedPaths + $dirtyPaths | Where-Object { $_ -match '["\r\n\t]' }).Count -gt 0) { throw 'Unsupported quoted/control-character Git pathname; refusing an ambiguous source approval set.' }
        $sourceCommit = & git rev-parse HEAD
        if ($LASTEXITCODE -ne 0) { throw 'Cannot determine source commit.' }
        $includedPending = @(Get-ChangedSource $inventory $trackedPaths $dirtyPaths)
        Assert-SourceApproval $includedPending $ApprovedDirtySource
        $approvedRecords = @($inventory | Where-Object { $_.path -in $includedPending })
        $approvedPatch = ''
        if ($includedPending.Count -gt 0) {
            $approvedPatch = (& git -c core.quotePath=false diff HEAD --binary -- @includedPending | Out-String)
            if ($LASTEXITCODE -ne 0) { throw 'Cannot capture approved tracked source changes.' }
        }
        foreach ($record in $inventory) {
            $to = Join-Path $snapshot $record.path
            New-Item -ItemType Directory -Path (Split-Path -Parent $to) -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $workspace $record.path) -Destination $to
        }
        Assert-Inventory $snapshot $inventory
        $inventory | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $temporary 'source-manifest.json') -Encoding utf8
        # All binary builds happen after freezing source. Never package old checkout binaries.
        Push-Location -LiteralPath $snapshot
        try {
            Invoke-Checked 'cargo' @('build', '--locked', '--release')
            Invoke-Checked 'cargo' @('xtask', 'notices')
            Invoke-Checked 'cargo' @('xtask', 'notices', '--check')
            Invoke-Checked 'dotnet' @('build', 'examples/csharp/UareuExample.csproj', '-c', 'Release', '--nologo')
            if ($IncludeRecognitionHelper) {
                Invoke-Checked 'dotnet' @('restore', 'examples/engine/UareuEngine.csproj', '--locked-mode', '--nologo')
                Invoke-Checked 'dotnet' @('build', 'examples/engine/UareuEngine.csproj', '-c', 'Release', '--no-restore', '--nologo')
                Invoke-Checked 'dotnet' @('examples/engine/bin/Release/net10.0/UareuEngine.dll', '--self-test')
            }
        }
        finally { Pop-Location }
        # Reject sources that changed during build or generated tooling.
        Assert-Inventory $snapshot $inventory
        New-Item -ItemType Directory -Path $destination | Out-Null
        $package = Join-Path $destination 'uareu-windows-x64'
        New-Item -ItemType Directory -Path $package | Out-Null
        $packageSource = Join-Path $package 'source'
        foreach ($record in $inventory) {
            $to = Join-Path $packageSource $record.path
            New-Item -ItemType Directory -Path (Split-Path -Parent $to) -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $snapshot $record.path) -Destination $to
        }
        New-Item -ItemType Directory -Path (Join-Path $package 'bin'), (Join-Path $package 'include'), (Join-Path $package 'csharp'), (Join-Path $package 'notices') | Out-Null
        foreach ($binary in @('uareu.exe', 'uareu.dll')) {
            $from = Join-Path $snapshot "target/release/$binary"
            Assert-X64PE $from
            Copy-Item -LiteralPath $from -Destination (Join-Path $package 'bin')
        }
        Copy-Item -LiteralPath (Join-Path $snapshot 'target/release/uareu.dll.lib') -Destination (Join-Path $package 'bin')
        Copy-Item -LiteralPath (Join-Path $snapshot 'include/uareu.h') -Destination (Join-Path $package 'include')
        Copy-Item -Path (Join-Path $snapshot 'examples/csharp/bin/Release/net10.0/*') -Destination (Join-Path $package 'csharp') -Recurse
        Copy-Item -Path (Join-Path $snapshot 'target/license-notices/*') -Destination (Join-Path $package 'notices') -Recurse
        if ($IncludeRecognitionHelper) {
            $helper = Join-Path $snapshot 'examples/engine/bin/Release/net10.0'
            if (Test-Path -LiteralPath (Join-Path $helper 'SixLabors.ImageSharp.dll')) { throw 'Image decoder must not be shipped.' }
            New-Item -ItemType Directory -Path (Join-Path $package 'engine') | Out-Null
            Copy-Item -Path (Join-Path $helper '*') -Destination (Join-Path $package 'engine') -Recurse
            Copy-Item -LiteralPath (Join-Path $snapshot 'examples/engine/README.md'), (Join-Path $snapshot 'examples/engine/THIRD_PARTY_NOTICES.md') -Destination (Join-Path $package 'engine')
        }
        Copy-Item -LiteralPath (Join-Path $temporary 'source-manifest.json') -Destination $package
        Copy-Item -LiteralPath (Join-Path $snapshot 'LICENSE'), (Join-Path $snapshot 'NOTICE') -Destination $package
        ConvertTo-Json -InputObject @($approvedRecords) -Depth 6 | Set-Content -LiteralPath (Join-Path $package 'approved-source-changes.json') -Encoding utf8
        $approvedPatch | Set-Content -LiteralPath (Join-Path $package 'approved-source.patch') -Encoding utf8
        [ordered]@{
            schema_version = 1; experimental = $true; utc = [DateTimeOffset]::UtcNow; source_commit = $sourceCommit;
            approved_changed_source = $includedPending; runtime = 'Windows x64; .NET 10 and VC++ runtime required; WinUSB manually provisioned';
            recognition_helper_included = $IncludeRecognitionHelper; source_restore_requires_network = $true;
            physical_acceptance = 'not_run'; scale = 'unknown'; independent_interchange = 'blocked_input'; external_nbis_nfiq = 'not_bundled'; biometric_accuracy = 'not_validated'
        } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $package 'feature-acceptance.json') -Encoding utf8
        $archive = Join-Path $destination 'uareu-windows-x64.zip'
        Compress-Archive -LiteralPath $package -DestinationPath $archive
        $verify = Join-Path $temporary 'extracted'
        Expand-Archive -LiteralPath $archive -DestinationPath $verify
        $rebuild = Join-Path $verify 'uareu-windows-x64/source'
        Assert-Inventory $rebuild $inventory
        Push-Location -LiteralPath $rebuild
        try {
            Invoke-Checked 'cargo' @('test', '--locked', '--workspace', '--all-targets')
            Invoke-Checked 'cargo' @('test', '--locked', '--workspace', '--doc')
            Invoke-Checked 'cargo' @('build', '--locked', '--release')
            Invoke-Checked 'dotnet' @('restore', 'examples/engine/UareuEngine.csproj', '--locked-mode', '--nologo')
            Invoke-Checked 'dotnet' @('build', 'examples/engine/UareuEngine.csproj', '-c', 'Release', '--no-restore', '--nologo')
            Invoke-Checked 'dotnet' @('examples/engine/bin/Release/net10.0/UareuEngine.dll', '--self-test')
        }
        finally { Pop-Location }
        $hashes = @(Get-ChildItem -LiteralPath $package -File -Recurse | Sort-Object FullName | ForEach-Object { [pscustomobject]@{ path = Source-Relative $package $_.FullName; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() } })
        [ordered]@{ schema_version = 1; extracted_source_rebuild = 'passed'; archive_sha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant(); files = $hashes } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $destination 'package-receipt.json') -Encoding utf8
        Write-Output "Experimental SDK packaged and extracted source rebuilt: $archive"
        Write-Output 'Clean-machine runtime/reader acceptance remains required; no signing or publication performed.'
    }
    catch {
        if (Test-Path -LiteralPath $destination) { [ordered]@{ status = 'failed'; error = $_.Exception.Message } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $destination 'package-failed.json') -Encoding utf8 }
        throw
    }
    finally { Remove-Item -LiteralPath $temporary -Recurse -Force }
}
finally { Pop-Location }
