param(
    [string]$OutputDirectory = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repoRoot "lab-trusted-setup-v2-i1"
}
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
$paramsPath = Join-Path $resolvedOutput "private_tx_v2_i1.params"

if ((Test-Path -LiteralPath $paramsPath) -and -not $Force) {
    throw "Temporary params already exist at $paramsPath. Use -Force to replace them."
}

New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null
if ($Force -and (Test-Path -LiteralPath $paramsPath)) {
    Remove-Item -LiteralPath $paramsPath
}

$previousLabDir = $env:ZOKA_PRIVATE_TX_V2_I1_LAB_DIR
$env:ZOKA_PRIVATE_TX_V2_I1_LAB_DIR = $resolvedOutput
try {
    Push-Location $repoRoot
    try {
        & cargo test --release --no-default-features `
            "zk::groth16_backend::private_tx_v2_i1_lab_tests::generate_v2_i1_lab_params" `
            -- --ignored --exact --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw "Temporary private_tx_v2_i1 parameter generation failed."
        }
    } finally {
        Pop-Location
    }
} finally {
    $env:ZOKA_PRIVATE_TX_V2_I1_LAB_DIR = $previousLabDir
}

Write-Host "Generated temporary benchmark-only params: $paramsPath"
