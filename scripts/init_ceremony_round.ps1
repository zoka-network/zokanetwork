param(
    [Parameter(Mandatory = $true)]
    [string]$CeremonyDir,
    [Parameter(Mandatory = $true)]
    [string]$CurrentDir,
    [Parameter(Mandatory = $true)]
    [int]$RoundNumber,
    [Parameter(Mandatory = $true)]
    [string]$ContributorId,
    [string]$Label = "zoka-mainnet-v1",
    [string]$CoordinatorId = "zoka-core-coordinator",
    [string]$CoordinatorChannel = "secure-channel",
    [string]$ExpectedReturnUtc = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

if ($RoundNumber -lt 1) {
    throw "RoundNumber must be >= 1"
}
if (-not (Test-Path -LiteralPath $CeremonyDir)) {
    throw "Ceremony directory not found: $CeremonyDir"
}
if (-not (Test-Path -LiteralPath $CurrentDir)) {
    throw "Current setup directory not found: $CurrentDir"
}

$roundName = "round-R$RoundNumber"
$roundDir = Join-Path $CeremonyDir $roundName
$summaryPath = Join-Path $roundDir "ROUND_SUMMARY.txt"
$checklistPath = Join-Path $roundDir "CHECKLIST.md"
$attestationPath = Join-Path $roundDir ("ATTESTATION_contrib-{0}.md" -f $ContributorId)
$invitationPath = Join-Path $roundDir ("INVITATION_contrib-{0}.md" -f $ContributorId)
$outputDir = Join-Path $roundDir "output_from_contributor"

$prepareArgs = @(
    "run", "--bin", "zoka-ceremony-contribute", "--",
    "prepare-adapter",
    "--setup-dir", $CurrentDir,
    "--adapter-dir", $roundDir,
    "--label", ("{0}-R{1}" -f $Label, $RoundNumber)
)
if ($Force) {
    $prepareArgs += "--force"
}

Write-Host "==> Preparing adapter package for round R$RoundNumber"
cargo @prepareArgs

New-Item -ItemType Directory -Path $outputDir -Force | Out-Null

$checklistTemplate = Join-Path "docs" "templates/ceremony_round_checklist.md"
$attestationTemplate = Join-Path "docs" "templates/ceremony_attestation_template.md"
$invitationTemplate = Join-Path "docs" "templates/ceremony_contributor_invitation.md"

if (-not (Test-Path -LiteralPath $checklistTemplate)) {
    throw "Checklist template not found: $checklistTemplate"
}
if (-not (Test-Path -LiteralPath $attestationTemplate)) {
    throw "Attestation template not found: $attestationTemplate"
}
if (-not (Test-Path -LiteralPath $invitationTemplate)) {
    throw "Invitation template not found: $invitationTemplate"
}

$checklistContent = Get-Content -LiteralPath $checklistTemplate -Raw
$checklistContent = $checklistContent.Replace("<round-number>", "$RoundNumber")
$checklistContent = $checklistContent.Replace("<contributor-id>", $ContributorId)
$checklistContent = $checklistContent.Replace("<coordinator-id>", "coordinator")
Set-Content -LiteralPath $checklistPath -Value $checklistContent -Encoding UTF8

$attestationContent = Get-Content -LiteralPath $attestationTemplate -Raw
$attestationContent = $attestationContent.Replace("<contributor-id>", $ContributorId)
$attestationContent = $attestationContent.Replace("<round-number>", "$RoundNumber")
$attestationContent = $attestationContent.Replace("<label>", $Label)
$attestationContent = $attestationContent.Replace(
    "<YYYY-MM-DDTHH:MM:SSZ>",
    (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
)
Set-Content -LiteralPath $attestationPath -Value $attestationContent -Encoding UTF8

if ([string]::IsNullOrWhiteSpace($ExpectedReturnUtc)) {
    $ExpectedReturnUtc = (Get-Date).ToUniversalTime().AddDays(2).ToString("yyyy-MM-ddTHH:mm:ssZ")
}
$invitationContent = Get-Content -LiteralPath $invitationTemplate -Raw
$invitationContent = $invitationContent.Replace("<contributor-id>", $ContributorId)
$invitationContent = $invitationContent.Replace("<label>", $Label)
$invitationContent = $invitationContent.Replace("<round-number>", "$RoundNumber")
$invitationContent = $invitationContent.Replace("<date-time UTC>", $ExpectedReturnUtc)
$invitationContent = $invitationContent.Replace("<coordinator-id>", $CoordinatorId)
$invitationContent = $invitationContent.Replace("<email/chat>", $CoordinatorChannel)
Set-Content -LiteralPath $invitationPath -Value $invitationContent -Encoding UTF8

$summary = @(
    "ZOKA Ceremony Round Package",
    "",
    "Round: R$RoundNumber",
    "Contributor: $ContributorId",
    "Label: $Label",
    "Current setup dir: $CurrentDir",
    "Round dir: $roundDir",
    "Input params dir: $(Join-Path $roundDir 'input')",
    "Output params dir: $outputDir",
    "",
    "Next steps:",
    "1. Share input/*.params + checklist + attestation with contributor.",
    "2. Receive output/*.params in output_from_contributor/.",
    "3. Run finalize-adapter and update transcript."
)
Set-Content -LiteralPath $summaryPath -Value $summary -Encoding UTF8

Write-Host ""
Write-Host "Round package ready:"
Write-Host "  Round dir    : $roundDir"
Write-Host "  Checklist    : $checklistPath"
Write-Host "  Attestation  : $attestationPath"
Write-Host "  Invitation   : $invitationPath"
Write-Host "  Output target: $outputDir"
