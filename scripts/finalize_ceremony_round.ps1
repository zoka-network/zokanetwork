param(
    [Parameter(Mandatory = $true)]
    [string]$CeremonyDir,
    [Parameter(Mandatory = $true)]
    [int]$RoundNumber,
    [Parameter(Mandatory = $true)]
    [string]$ContributorId,
    [Parameter(Mandatory = $true)]
    [string]$Label,
    [string]$Note = "",
    [string]$NextContributorId = "",
    [switch]$InitNextRound
)

$ErrorActionPreference = "Stop"

if ($RoundNumber -lt 1) {
    throw "RoundNumber must be >= 1"
}

$transcript = Join-Path $CeremonyDir "transcript.json"
$roundDir = Join-Path $CeremonyDir ("round-R{0}" -f $RoundNumber)
$outputDir = Join-Path $roundDir "output_from_contributor"
$nextCurrent = Join-Path $CeremonyDir ("current-R{0}" -f $RoundNumber)

if (-not (Test-Path -LiteralPath $transcript)) {
    throw "Transcript not found: $transcript"
}
if (-not (Test-Path -LiteralPath $roundDir)) {
    throw "Round dir not found: $roundDir"
}
if (-not (Test-Path -LiteralPath $outputDir)) {
    throw "Contributor output dir not found: $outputDir"
}

if ([string]::IsNullOrWhiteSpace($Note)) {
    $Note = "round R$RoundNumber"
}

Write-Host "==> Finalizing R$RoundNumber"
cargo run --bin zoka-ceremony-contribute -- `
  finalize-adapter `
  --adapter-dir $roundDir `
  --phase2-out-dir $outputDir `
  --out-setup-dir $nextCurrent `
  --transcript $transcript `
  --contributor $ContributorId `
  --note $Note

Write-Host ""
Write-Host "==> Verifying transcript after R$RoundNumber"
cargo run --bin zoka-ceremony-contribute -- `
  verify `
  --transcript $transcript `
  --final-dir $nextCurrent

if ($InitNextRound) {
    if ([string]::IsNullOrWhiteSpace($NextContributorId)) {
        throw "NextContributorId is required when -InitNextRound is used"
    }
    $nextRound = $RoundNumber + 1
    Write-Host ""
    Write-Host "==> Initializing R$nextRound for $NextContributorId"
    powershell -ExecutionPolicy Bypass -File scripts/init_ceremony_round.ps1 `
      -CeremonyDir $CeremonyDir `
      -CurrentDir $nextCurrent `
      -RoundNumber $nextRound `
      -ContributorId $NextContributorId `
      -Label $Label `
      -Force
}

Write-Host ""
Write-Host "Round R$RoundNumber finalized."
Write-Host "Current setup dir: $nextCurrent"
