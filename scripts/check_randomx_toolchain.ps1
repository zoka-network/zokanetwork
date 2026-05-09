# Verifies the native toolchain required to build the RandomX mainnet backend.
#
# This script intentionally checks host tools before Cargo reaches the
# `randomx-rs` build script. Without this preflight, release builds fail later
# with a less actionable "cmake: program not found" panic from the dependency.

param(
    [switch]$Strict
)

$ErrorActionPreference = "Stop"
$missing = New-Object System.Collections.Generic.List[string]

function Test-Command {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$InstallHint
    )

    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        $missing.Add("$Name - $InstallHint")
        return
    }
    Write-Host "${Name}: $($cmd.Source)"
}

function Find-MsvcInstall {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere) {
        $install = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if (![string]::IsNullOrWhiteSpace($install)) {
            return $install.Trim()
        }
    }

    $roots = @(
        (Join-Path ${env:ProgramFiles} "Microsoft Visual Studio"),
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio")
    )
    foreach ($root in $roots) {
        if (!(Test-Path -LiteralPath $root)) {
            continue
        }
        $tools = Get-ChildItem -LiteralPath $root -Recurse -Filter "cl.exe" -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like "*\VC\Tools\MSVC\*\bin\Hostx64\x64\cl.exe" } |
            Select-Object -First 1
        if ($null -ne $tools) {
            return $tools.FullName
        }
    }

    return $null
}

function Find-LibclangPath {
    if (![string]::IsNullOrWhiteSpace($env:LIBCLANG_PATH)) {
        $dll = Join-Path $env:LIBCLANG_PATH "libclang.dll"
        $alt = Join-Path $env:LIBCLANG_PATH "clang.dll"
        if ((Test-Path -LiteralPath $dll -PathType Leaf) -or (Test-Path -LiteralPath $alt -PathType Leaf)) {
            return $env:LIBCLANG_PATH
        }
    }

    $roots = @(
        (Join-Path $env:APPDATA "Python"),
        (Join-Path $env:LOCALAPPDATA "Programs\Python")
    )
    foreach ($root in $roots) {
        if (!(Test-Path -LiteralPath $root)) {
            continue
        }
        $hit = Get-ChildItem -LiteralPath $root -Recurse -Filter "libclang.dll" -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like "*\clang\native\libclang.dll" } |
            Select-Object -First 1
        if ($null -ne $hit) {
            return $hit.DirectoryName
        }
    }

    $llvm = "C:\Program Files\LLVM\bin"
    if (Test-Path -LiteralPath (Join-Path $llvm "libclang.dll") -PathType Leaf) {
        return $llvm
    }

    return $null
}

Write-Host "ZOKA RandomX toolchain preflight"
Write-Host "--------------------------------"

Test-Command -Name "cmake" -InstallHint "Install CMake and add it to PATH."

if ($env:OS -eq "Windows_NT") {
    $cl = Get-Command "cl" -ErrorAction SilentlyContinue
    $msbuild = Get-Command "msbuild" -ErrorAction SilentlyContinue
    $msvcInstall = Find-MsvcInstall
    if ($null -eq $cl -and $null -eq $msbuild -and [string]::IsNullOrWhiteSpace($msvcInstall)) {
        $missing.Add("MSVC Build Tools - Install Visual Studio Build Tools 2019/2022 with C++ workload.")
    } else {
        if ($null -ne $cl) { Write-Host "cl: $($cl.Source)" }
        if ($null -ne $msbuild) { Write-Host "msbuild: $($msbuild.Source)" }
        if (![string]::IsNullOrWhiteSpace($msvcInstall)) { Write-Host "MSVC Build Tools: $msvcInstall" }
    }

    $libclangPath = Find-LibclangPath
    if ([string]::IsNullOrWhiteSpace($libclangPath)) {
        $missing.Add("libclang.dll - Install Python package 'clang' or set LIBCLANG_PATH to a valid x64 libclang directory.")
    } else {
        Write-Host "LIBCLANG_PATH candidate: $libclangPath"
        if ($libclangPath -like "C:\Program Files\LLVM\bin*") {
            Write-Host "Note: if this LLVM libclang is ARM64, set LIBCLANG_PATH to the Python clang native x64 directory instead."
        }
    }
} else {
    Test-Command -Name "cc" -InstallHint "Install gcc or clang."
    Test-Command -Name "c++" -InstallHint "Install g++ or clang++."
    Test-Command -Name "make" -InstallHint "Install make/build-essential."
}

if ($missing.Count -gt 0) {
    Write-Host ""
    Write-Host "Missing RandomX build prerequisites:"
    foreach ($item in $missing) {
        Write-Host "  - $item"
    }
    Write-Host ""
    Write-Host "Mainnet release binaries must compile with:"
    Write-Host "  powershell -ExecutionPolicy Bypass -File scripts/build_mainnet_release.ps1"
    if ($Strict) {
        exit 1
    }
}
else {
    Write-Host ""
    Write-Host "RandomX native toolchain preflight: OK"
}
