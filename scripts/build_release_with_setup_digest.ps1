param(
    [Parameter(Mandatory = $true)]
    [string]$SetupDir,
    [string]$Bin = "zoka",
    [switch]$NoCheck
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $SetupDir)) {
    throw "Setup directory not found: $SetupDir"
}

if (-not $NoCheck) {
    cargo run --quiet --features ops-tools --bin zoka-ceremony-contribute -- release-pin --setup-dir $SetupDir > $null
}

$digestPath = Join-Path $SetupDir "DIGEST"
if (-not (Test-Path -LiteralPath $digestPath)) {
    throw "DIGEST missing in setup directory: $digestPath"
}

$digest = (Get-Content -LiteralPath $digestPath | Select-Object -First 1).Trim()
if ($digest.Length -ne 64) {
    throw "Invalid digest length in ${digestPath}: expected 64 hex chars, got $($digest.Length)"
}

$env:ZOKA_EXPECTED_SETUP_DIGEST_COMPILED = $digest

Write-Host "Building release binary '$Bin' pinned to digest $digest"
cargo build --release --bin $Bin

Write-Host "Done."
Write-Host "Runtime setup dir should be: $SetupDir"
Write-Host "Pinned digest: $digest"
