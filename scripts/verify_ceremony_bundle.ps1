param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir
)

$ErrorActionPreference = "Stop"

Write-Host "==> verifying ceremony bundle"
Write-Host "    bundle dir: $BundleDir"

cargo run --bin zoka-ceremony-contribute -- verify-bundle --bundle-dir $BundleDir

Write-Host ""
Write-Host "Bundle verification passed."
