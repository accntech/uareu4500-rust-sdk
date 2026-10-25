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
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --inspect $fixturePath
        if ($LASTEXITCODE -ne 0) { throw 'Managed image/metadata/diagnostics verification failed' }
        & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --list
        if ($LASTEXITCODE -ne 0) { throw 'Managed discovery verification failed' }
        if ($HardwareCancellation) {
            & dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --cancel-test
            if ($LASTEXITCODE -ne 0) { throw 'Hardware cancellation verification failed' }
        }
    }
    finally { $env:UAREU_DLL_PATH = $previousDllPath }
    Write-Output 'Windows DLL verification passed.'
}
finally { Pop-Location }
