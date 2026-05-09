param(
    [Parameter(Mandatory = $true)]
    [string]$Transcript,
    [Parameter(Mandatory = $true)]
    [string]$FinalDir,
    [Parameter(Mandatory = $true)]
    [string]$OutDir,
    [int]$MinUniqueContributors = 5,
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$args = @(
    "run",
    "--features", "ops-tools",
    "--bin", "zoka-ceremony-contribute",
    "--",
    "publish-bundle",
    "--transcript", $Transcript,
    "--final-dir", $FinalDir,
    "--out-dir", $OutDir,
    "--min-unique-contributors", $MinUniqueContributors
)

if ($Force) {
    $args += "--force"
}

Write-Host "==> building publishable ceremony bundle"
Write-Host "    transcript: $Transcript"
Write-Host "    final dir : $FinalDir"
Write-Host "    out dir   : $OutDir"
Write-Host "    min unique contributors: $MinUniqueContributors"

cargo @args

Write-Host ""
Write-Host "Bundle generated at: $OutDir"
Write-Host "Contains: transcript.json, readiness.json, RELEASE.env, setup/, manifest.json"
