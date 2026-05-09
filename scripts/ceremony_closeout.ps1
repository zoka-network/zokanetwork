param(
    [Parameter(Mandatory = $true)]
    [string]$Transcript,
    [Parameter(Mandatory = $true)]
    [string]$FinalDir,
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,
    [string]$ReleaseEnvOut = "",
    [int]$MinUniqueContributors = 5,
    [switch]$ForceBundle,
    [switch]$BuildRelease,
    [string]$ReleaseBin = "zoka"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Transcript)) {
    throw "Transcript not found: $Transcript"
}
if (-not (Test-Path -LiteralPath $FinalDir)) {
    throw "Final setup directory not found: $FinalDir"
}

Write-Host "==> Step 1/6: verify transcript against final setup"
cargo run --features ops-tools --bin zoka-ceremony-contribute -- `
    verify `
    --transcript $Transcript `
    --final-dir $FinalDir

$readinessPath = Join-Path $BundleDir "readiness.json"
Write-Host ""
Write-Host "==> Step 2/6: readiness gate"
cargo run --features ops-tools --bin zoka-ceremony-contribute -- `
    readiness `
    --transcript $Transcript `
    --final-dir $FinalDir `
    --min-unique-contributors $MinUniqueContributors `
    --out-json $readinessPath

Write-Host ""
Write-Host "==> Step 3/6: publish bundle"
$publishArgs = @(
    "run", "--features", "ops-tools", "--bin", "zoka-ceremony-contribute", "--",
    "publish-bundle",
    "--transcript", $Transcript,
    "--final-dir", $FinalDir,
    "--out-dir", $BundleDir,
    "--min-unique-contributors", "$MinUniqueContributors"
)
if ($ForceBundle) {
    $publishArgs += "--force"
}
cargo @publishArgs

Write-Host ""
Write-Host "==> Step 4/6: verify bundle"
cargo run --features ops-tools --bin zoka-ceremony-contribute -- `
    verify-bundle `
    --bundle-dir $BundleDir

if ([string]::IsNullOrWhiteSpace($ReleaseEnvOut)) {
    $ReleaseEnvOut = Join-Path $BundleDir "release.env"
}

Write-Host ""
Write-Host "==> Step 5/6: generate release pin env"
cargo run --features ops-tools --bin zoka-ceremony-contribute -- `
    release-pin `
    --setup-dir $FinalDir `
    --transcript $Transcript `
    --out-env $ReleaseEnvOut

if ($BuildRelease) {
    Write-Host ""
    Write-Host "==> Step 6/6: build release with pinned digest"
    powershell -ExecutionPolicy Bypass -File scripts/build_release_with_setup_digest.ps1 `
        -SetupDir $FinalDir `
        -Bin $ReleaseBin
} else {
    Write-Host ""
    Write-Host "==> Step 6/6 skipped (use -BuildRelease to enable release build)"
}

Write-Host ""
Write-Host "Ceremony closeout flow completed."
Write-Host "Bundle: $BundleDir"
Write-Host "Release env: $ReleaseEnvOut"
