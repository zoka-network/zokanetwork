# Builds and packages ZOKA release artifacts for one or more targets.
#
# Example Windows release:
#   powershell -ExecutionPolicy Bypass -File scripts/package_release.ps1 `
#     -Version v1.0.0-rc1 `
#     -Targets x86_64-pc-windows-msvc `
#     -Features randomx
#
# Example local smoke package from existing debug binaries:
#   powershell -ExecutionPolicy Bypass -File scripts/package_release.ps1 `
#     -Version local-smoke `
#     -Targets native `
#     -Profile debug `
#     -SkipBuild `
#     -OutDir .zoka/test/release-smoke

param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string[]]$Targets = @("native"),
    [string]$OutDir = "releases",
    [ValidateSet("debug", "release")][string]$Profile = "release",
    [string]$Features = "randomx",
    [switch]$SkipBuild,
    [switch]$NoDocs
)

$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param([string]$Command, [string[]]$Args)
    Write-Host "==> $Command $($Args -join ' ')"
    & $Command @Args
    if ($LASTEXITCODE -ne 0) {
        throw "command failed: $Command $($Args -join ' ')"
    }
}

function Get-BinaryName {
    param([string]$Bin, [string]$Target)
    $nativeWindows = $env:OS -eq "Windows_NT"
    if ($Target -eq "native" -and $nativeWindows) {
        return "$Bin.exe"
    }
    if ($Target -like "*windows*") {
        return "$Bin.exe"
    }
    return $Bin
}

function Get-BinaryPath {
    param([string]$Bin, [string]$Target, [string]$Profile)
    $name = Get-BinaryName -Bin $Bin -Target $Target
    if ($Target -eq "native") {
        return Join-Path "target" (Join-Path $Profile $name)
    }
    return Join-Path "target" (Join-Path $Target (Join-Path $Profile $name))
}

$releaseRoot = Join-Path $OutDir $Version
New-Item -ItemType Directory -Force -Path $releaseRoot | Out-Null

if ($Features -split "," -contains "randomx") {
    powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict
    if ($LASTEXITCODE -ne 0) {
        throw "RandomX toolchain preflight failed; refusing to package a mainnet release."
    }
}

$gitCommit = (& git rev-parse HEAD 2>$null)
if ($LASTEXITCODE -ne 0) { $gitCommit = "unknown" }
$rustcCmd = Get-Command rustc -ErrorAction SilentlyContinue
$cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
$rustcVersion = if ($null -ne $rustcCmd) { (& $rustcCmd.Source --version 2>$null) } else { "unknown" }
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($rustcVersion)) { $rustcVersion = "unknown" }
$cargoVersion = if ($null -ne $cargoCmd) { (& $cargoCmd.Source --version 2>$null) } else { "unknown" }
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($cargoVersion)) { $cargoVersion = "unknown" }

foreach ($target in $Targets) {
    $cargoArgs = @("build", "--bin", "zoka", "--bin", "zokahd")
    if ($Profile -eq "release") { $cargoArgs += "--release" }
    if (-not [string]::IsNullOrWhiteSpace($Features)) {
        $cargoArgs += @("--features", $Features)
    }
    if ($target -ne "native") {
        $cargoArgs += @("--target", $target)
    }

    if (-not $SkipBuild) {
        Invoke-Checked -Command "cargo" -Args $cargoArgs
    }

    $artifactName = "zoka-$Version-$target"
    $stage = Join-Path $releaseRoot $artifactName
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    foreach ($bin in @("zoka", "zokahd")) {
        $source = Get-BinaryPath -Bin $bin -Target $target -Profile $Profile
        if (-not (Test-Path $source -PathType Leaf)) {
            throw "expected binary not found: $source"
        }
        Copy-Item -Path $source -Destination $stage
    }

    if (-not $NoDocs) {
        $docDir = Join-Path $stage "docs"
        New-Item -ItemType Directory -Force -Path $docDir | Out-Null
        foreach ($doc in @(
            "docs/release-verification.md",
            "docs/reproducible-builds.md",
            "docs/user-cli-reference.en.md",
            "docs/genesis.mainnet.v1.spec.md",
            "docs/genesis.mainnet.v1.json",
            "docs/mainnet-network.v1.json",
            "docs/mainnet-network-ops.md",
            "docs/infrastructure-logs-policy.md"
        )) {
            if (Test-Path $doc -PathType Leaf) {
                Copy-Item -Path $doc -Destination $docDir
            }
        }
    }

    $manifest = @(
        "ZOKA release artifact",
        "version=$Version",
        "target=$target",
        "profile=$Profile",
        "features=$Features",
        "git_commit=$gitCommit",
        "rustc=$rustcVersion",
        "cargo=$cargoVersion",
        "build_command=cargo $($cargoArgs -join ' ')",
        "protocol_freeze_id=/zoka/1.0.0"
    )
    $manifest | Out-File -Encoding ascii -FilePath (Join-Path $stage "RELEASE-MANIFEST.txt")

    $archiveBase = Join-Path $releaseRoot $artifactName
    $nativeWindows = $env:OS -eq "Windows_NT"
    if ($target -like "*windows*" -or ($target -eq "native" -and $nativeWindows)) {
        $archive = "$archiveBase.zip"
        if (Test-Path $archive) { Remove-Item -Force $archive }
        Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive
    } else {
        $archive = "$archiveBase.tar.gz"
        if (Test-Path $archive) { Remove-Item -Force $archive }
        Invoke-Checked -Command "tar" -Args @("-czf", $archive, "-C", $releaseRoot, $artifactName)
    }
    Remove-Item -Recurse -Force $stage
    Write-Host "Packaged $archive"
}

$checksums = Join-Path $releaseRoot "SHA256SUMS.txt"
powershell -ExecutionPolicy Bypass -File scripts/release_checksums.ps1 `
    -ReleaseDir $releaseRoot `
    -OutFile $checksums

Write-Host ""
Write-Host "Release package staged at: $releaseRoot"
Write-Host "Sign checksums with:"
Write-Host "  gpg --detach-sign --armor $checksums"
