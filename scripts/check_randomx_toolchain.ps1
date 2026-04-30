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
    Write-Host "  cargo build --release --features randomx --bin zoka --bin zokahd"
    if ($Strict) {
        exit 1
    }
}
else {
    Write-Host ""
    Write-Host "RandomX native toolchain preflight: OK"
}
