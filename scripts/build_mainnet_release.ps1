param(
    [string]$Features = "randomx"
)

$ErrorActionPreference = "Stop"

function Find-VsDevCmd {
    $candidates = @(
        "C:\Program Files\Microsoft Visual Studio\18\Insiders\Common7\Tools\VsDevCmd.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\Professional\Common7\Tools\VsDevCmd.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\Common7\Tools\VsDevCmd.bat"
    )
    foreach ($path in $candidates) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            return $path
        }
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
        $install = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if (-not [string]::IsNullOrWhiteSpace($install)) {
            $path = Join-Path $install.Trim() "Common7\Tools\VsDevCmd.bat"
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                return $path
            }
        }
    }

    throw "Visual Studio C++ environment not found. Install the Desktop development with C++ workload."
}

function Find-LibclangPath {
    if (-not [string]::IsNullOrWhiteSpace($env:LIBCLANG_PATH)) {
        if ((Test-Path -LiteralPath (Join-Path $env:LIBCLANG_PATH "libclang.dll") -PathType Leaf) -or
            (Test-Path -LiteralPath (Join-Path $env:LIBCLANG_PATH "clang.dll") -PathType Leaf)) {
            return $env:LIBCLANG_PATH
        }
    }

    $roots = @(
        (Join-Path $env:APPDATA "Python"),
        (Join-Path $env:LOCALAPPDATA "Programs\Python"),
        "C:\Program Files\LLVM\bin"
    )

    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) {
            continue
        }
        $hit = Get-ChildItem -LiteralPath $root -Recurse -Filter "libclang.dll" -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like "*\clang\native\libclang.dll" } |
            Select-Object -First 1
        if ($null -ne $hit) {
            return $hit.DirectoryName
        }
    }

    throw "libclang.dll not found. Install Python package 'clang' or set LIBCLANG_PATH to its native directory."
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$vsDevCmd = Find-VsDevCmd
$libclangPath = Find-LibclangPath

Write-Host "ZOKA mainnet release build"
Write-Host "VS DevCmd:     $vsDevCmd"
Write-Host "LIBCLANG_PATH: $libclangPath"
Write-Host "Features:      $Features"
Write-Host ""

$cargoHome = Join-Path $env:USERPROFILE ".cargo"
$rustupHome = Join-Path $repoRoot ".rustup-zoka"
$cargoBin = Join-Path $cargoHome "bin"
$command = @(
    "`"$vsDevCmd`" -arch=x64",
    "set `"PATH=$cargoBin;%PATH%`"",
    "set `"CARGO_HOME=$cargoHome`"",
    "set `"RUSTUP_HOME=$rustupHome`"",
    "set `"LIBCLANG_PATH=$libclangPath`"",
    "cargo build --release --features $Features --bin zoka --bin zokahd"
) -join " && "

Push-Location $repoRoot
try {
    & cmd.exe /c $command
    if ($LASTEXITCODE -ne 0) {
        throw "mainnet release build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

Write-Host ""
Write-Host "Built:"
Write-Host "  target\release\zoka.exe"
Write-Host "  target\release\zokahd.exe"
