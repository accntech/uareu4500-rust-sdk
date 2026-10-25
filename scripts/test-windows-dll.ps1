# SPDX-License-Identifier: GPL-3.0-or-later
param([switch]$HardwareCancellation)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $workspace
try {
    $rustVersion = (& rustc -vV) -join "`n"
    if ($rustVersion -notmatch 'host: x86_64-pc-windows-msvc') {
        throw 'This verification script requires an x64 Windows MSVC Rust host, x64 MSVC tools, and the .NET 10 SDK.'
    }
    & cargo build --locked --release --lib
    if ($LASTEXITCODE -ne 0) { throw 'DLL build failed' }
    & dotnet build examples/csharp/UareuExample.csproj -c Release --nologo
    if ($LASTEXITCODE -ne 0) { throw 'C# build failed' }
    & dotnet restore examples/engine/UareuEngine.csproj --locked-mode --nologo
    if ($LASTEXITCODE -ne 0) { throw 'Recognition helper locked restore failed' }
    & dotnet build examples/engine/UareuEngine.csproj -c Release --no-restore --nologo
    if ($LASTEXITCODE -ne 0) { throw 'Recognition helper build failed' }
    if (Test-Path -LiteralPath examples/engine/bin/Release/net10.0/SixLabors.ImageSharp.dll) { throw 'Raw-only helper must not include the image decoder' }
    Copy-Item -LiteralPath examples/engine/README.md, examples/engine/THIRD_PARTY_NOTICES.md -Destination examples/engine/bin/Release/net10.0
    & examples/engine/bin/Release/net10.0/UareuEngine.exe --self-test
    if ($LASTEXITCODE -ne 0) { throw 'Recognition helper malformed-input verification failed' }
    $vsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vsInstall = & $vsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vsInstall) { throw 'Visual Studio x64 C++ build tools were not found' }
    Import-Module (Join-Path $vsInstall 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $vsInstall -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
    & cl /nologo /W4 /WX /std:c11 /I include tests/c/abi_smoke.c /Fo:target/release/uareu-abi-smoke.obj /Fe:target/release/uareu-abi-smoke.exe /link /LIBPATH:target/release uareu.dll.lib
    if ($LASTEXITCODE -ne 0) { throw 'C header/import-library compilation failed' }
    $fixturePath = Join-Path $workspace ('target\ffi-smoke-' + [Guid]::NewGuid().ToString('N') + '.png')
    & .\target\release\uareu-abi-smoke.exe $fixturePath
    if ($LASTEXITCODE -ne 0) { throw 'C ABI verification failed' }
    $previousDllPath = $env:UAREU_DLL_PATH
    try {
        $env:UAREU_DLL_PATH = Join-Path $workspace 'target\release\uareu.dll'
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --self-test
        if ($LASTEXITCODE -ne 0) { throw 'Managed snapshot/error verification failed' }
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --advanced-test
        if ($LASTEXITCODE -ne 0) { throw 'Managed image/report verification failed' }
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --recognition-test (Join-Path $workspace 'examples/engine/bin/Release/net10.0/UareuEngine.exe')
        if ($LASTEXITCODE -ne 0) { throw 'Managed recognition/interchange verification failed' }
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --inspect $fixturePath
        if ($LASTEXITCODE -ne 0) { throw 'Managed image/metadata/diagnostics verification failed' }
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --list
        if ($LASTEXITCODE -ne 0) { throw 'Managed discovery verification failed' }
        if ($HardwareCancellation) {
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --session-test
            if ($LASTEXITCODE -ne 0) { throw 'Persistent session/recovery verification failed' }
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --async-test
            if ($LASTEXITCODE -ne 0) { throw 'Native asynchronous capture verification failed' }
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --status
            if ($LASTEXITCODE -ne 0) { throw 'Hardware status/capabilities verification failed' }
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --timeout-test
            if ($LASTEXITCODE -ne 0) { throw 'Structured timeout verification failed' }
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --phase-test
            if ($LASTEXITCODE -ne 0) { throw 'Capture phase/status verification failed' }
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --cancel-test
            if ($LASTEXITCODE -ne 0) { throw 'Hardware cancellation verification failed' }
        }
    }
    finally { $env:UAREU_DLL_PATH = $previousDllPath }
    Write-Output 'Windows DLL verification passed.'
}
finally { Pop-Location }
