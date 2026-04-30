param(
    [Parameter(Mandatory = $true)]
    [string]$CeremonyDir,
    [Parameter(Mandatory = $true)]
    [string]$PlanPath,
    [Parameter(Mandatory = $true)]
    [int]$RoundNumber,
    [Parameter(Mandatory = $true)]
    [string]$ReturnPath,
    [switch]$ExportNextRoundPackage
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $PlanPath)) {
    throw "Plan file not found: $PlanPath"
}

$plan = Get-Content -LiteralPath $PlanPath -Raw | ConvertFrom-Json
if ($null -eq $plan.rounds -or $plan.rounds.Count -eq 0) {
    throw "Invalid plan file: missing rounds"
}

$current = $plan.rounds | Where-Object { $_.round -eq $RoundNumber } | Select-Object -First 1
if ($null -eq $current) {
    throw "Round $RoundNumber not found in plan"
}

$currentContributor = [string]$current.contributor_id
if ([string]::IsNullOrWhiteSpace($currentContributor)) {
    throw "Round $RoundNumber has empty contributor_id in plan"
}

$next = $plan.rounds | Where-Object { $_.round -eq ($RoundNumber + 1) } | Select-Object -First 1
$hasNext = $null -ne $next

$args = @(
    "-ExecutionPolicy", "Bypass",
    "-File", "scripts/process_ceremony_round_return.ps1",
    "-CeremonyDir", $CeremonyDir,
    "-RoundNumber", "$RoundNumber",
    "-ContributorId", $currentContributor,
    "-ReturnPath", $ReturnPath,
    "-Label", [string]$plan.label
)

if ($hasNext) {
    $args += "-InitNextRound"
    $args += @("-NextContributorId", [string]$next.contributor_id)
    if ($ExportNextRoundPackage) {
        $args += "-ExportNextRoundPackage"
    }
}

Write-Host "==> Processing round return using plan"
Write-Host "  Round: $RoundNumber"
Write-Host "  Contributor: $currentContributor"
if ($hasNext) {
    Write-Host "  Next contributor: $($next.contributor_id)"
}
powershell @args

$current.status = "finalized"
$current.finalized = $true
$current.return_artifact = $ReturnPath
$current.notes = "processed at $((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))"

if ($hasNext) {
    $next.status = "prepared"
    if ($ExportNextRoundPackage) {
        $next.package = (Join-Path $CeremonyDir ("out/zoka-round-R{0}-{1}.zip" -f $next.round, $next.contributor_id))
    }
}

$updatedJson = $plan | ConvertTo-Json -Depth 10
Set-Content -LiteralPath $PlanPath -Value $updatedJson -Encoding UTF8

Write-Host ""
Write-Host "Plan updated: $PlanPath"
