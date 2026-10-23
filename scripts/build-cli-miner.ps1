# Arma el minero CLI de Zoka Quantic para Windows:
# target\cli-miner\<versión>\zoka-cli-miner-<versión>-windows-x64.zip
#
# Cada pieza se verifica antes de entrar, como en prepare-release.ps1 de ZSilent:
# zoka.exe tiene que ser la versión de Cargo.toml, traer Zoka Quantic y sus datos
# de versión de Windows; tor.exe, el del Tor Expert Bundle verificado. Adentro
# va un SHA256SUMS.txt de todos los archivos, y al lado del zip, el hash del zip.
# No publica nada.
#
# Uso: .\scripts\build-cli-miner.ps1 [-TorBundleDir .\third_party\tor\windows-x86_64-0.4.9.14]

[CmdletBinding()]
param(
    [string]$TorBundleDir = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($TorBundleDir)) {
    $TorBundleDir = Join-Path $repo "third_party\tor\windows-x86_64-0.4.9.14"
}
$TorBundleDir = (Resolve-Path -LiteralPath $TorBundleDir).Path
$source = Join-Path $repo "packaging\cli-miner"

$version = [regex]::Match((Get-Content -LiteralPath (Join-Path $repo "Cargo.toml") -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw "Could not read the version from Cargo.toml" }

# El nodo.
$zoka = Join-Path $repo "target\release\zoka.exe"
if (-not (Test-Path -LiteralPath $zoka)) { throw "Missing $zoka. Build it with: cargo build --release --features randomx,quantic --bins" }
$reported = (& $zoka --version 2>&1 | Out-String).Trim()
if ($reported -ne "zoka $version") { throw "zoka.exe says '$reported', the source is $version. Rebuild it." }
$info = (Get-Item -LiteralPath $zoka).VersionInfo
if ($info.ProductName -ne "Zoka" -or $info.ProductVersion -ne $version) {
    throw "zoka.exe has no Windows version data (product '$($info.ProductName)', version '$($info.ProductVersion)'). Rebuild it with the Windows SDK (rc.exe)."
}
$probe = Join-Path ([System.IO.Path]::GetTempPath()) ("zoka-cli-probe-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $probe | Out-Null
$previousHome = $env:ZOKA_HOME
$env:ZOKA_HOME = $probe
try {
    $list = (& $zoka quantic-wallet list --network quantic-testnet --json 2>&1 | Out-String)
    $listExit = $LASTEXITCODE
} finally {
    $env:ZOKA_HOME = $previousHome
}
if ($listExit -ne 0 -or $list -notmatch '"wallets"') { throw "zoka.exe was built without Zoka Quantic. Rebuild with: cargo build --release --features randomx,quantic --bins" }

# Tor, el del Expert Bundle verificado (ver prepare-tor-expert-bundle-windows.ps1 de ZSilent).
$torMeta = Get-Content -LiteralPath (Join-Path $TorBundleDir ".zsilent-bundle.json") -Raw | ConvertFrom-Json
$torExe = Join-Path $TorBundleDir "tor.exe"
$torHash = (Get-FileHash -LiteralPath $torExe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($torHash -ne $torMeta.tor_exe_sha256) { throw "tor.exe does not match the verified Tor Expert Bundle ($torHash)" }

# Armado.
$name = "zoka-cli-miner-$version-windows-x64"
$outRoot = Join-Path $repo "target\cli-miner\$version"
$pkg = Join-Path $outRoot $name
if (Test-Path -LiteralPath $pkg) { throw "$pkg already exists. Move it away first; this script does not delete anything." }
New-Item -ItemType Directory -Force -Path (Join-Path $pkg "tor") | Out-Null
Copy-Item -LiteralPath $zoka -Destination $pkg
Copy-Item -LiteralPath (Join-Path $source "windows\zoka-miner.ps1") -Destination $pkg
Get-ChildItem -LiteralPath (Join-Path $source "windows") -Filter "*.cmd" | Copy-Item -Destination $pkg
Copy-Item -LiteralPath (Join-Path $source "README.md") -Destination $pkg
Copy-Item -LiteralPath (Join-Path $source "LEEME.md") -Destination $pkg
foreach ($f in @("tor.exe", "geoip", "geoip6")) { Copy-Item -LiteralPath (Join-Path $TorBundleDir $f) -Destination (Join-Path $pkg "tor") }
Copy-Item -LiteralPath (Join-Path $source "windows\TOR-LICENSE.txt") -Destination (Join-Path $pkg "tor\LICENSE.txt")

# Los hashes de adentro, con rutas relativas y barras normales (como sha256sum).
$lines = Get-ChildItem -LiteralPath $pkg -Recurse -File | Sort-Object FullName | ForEach-Object {
    $rel = $_.FullName.Substring($pkg.Length + 1) -replace '\\', '/'
    "{0}  {1}" -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $rel
}
[System.IO.File]::WriteAllLines((Join-Path $pkg "SHA256SUMS.txt"), [string[]]$lines)

$zip = Join-Path $outRoot "$name.zip"
if (Test-Path -LiteralPath $zip) { throw "$zip already exists." }
# A mano y no con Compress-Archive: el de Windows PowerShell 5.1 guarda las rutas
# con barras invertidas, fuera del formato zip, y otros programas lo extraen mal.
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::Open($zip, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    Get-ChildItem -LiteralPath $pkg -Recurse -File | Sort-Object FullName | ForEach-Object {
        $entry = "$name/" + ($_.FullName.Substring($pkg.Length + 1) -replace '\\', '/')
        [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $entry, [System.IO.Compression.CompressionLevel]::Optimal)
    }
} finally {
    $archive.Dispose()
}
$zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
Add-Content -LiteralPath (Join-Path $outRoot "SHA256SUMS.txt") -Value ("{0}  {1}.zip" -f $zipHash, $name)

Write-Host "Prepared (not published): $zip"
Write-Host "SHA-256: $zipHash"
