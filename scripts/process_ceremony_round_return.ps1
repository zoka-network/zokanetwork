param(
    [Parameter(Mandatory = $true)]
    [string]$CeremonyDir,
    [Parameter(Mandatory = $true)]
    [int]$RoundNumber,
    [Parameter(Mandatory = $true)]
    [string]$ContributorId,
    [Parameter(Mandatory = $true)]
    [string]$ReturnPath,
    [Parameter(Mandatory = $true)]
    [string]$Label,
    [string]$Note = "",
    [string]$NextContributorId = "",
    [switch]$InitNextRound,
    [switch]$ExportNextRoundPackage
)

$ErrorActionPreference = "Stop"

if ($RoundNumber -lt 1) {
    throw "RoundNumber must be >= 1"
}
if (-not (Test-Path -LiteralPath $CeremonyDir)) {
    throw "Ceremony directory not found: $CeremonyDir"
}
if (-not (Test-Path -LiteralPath $ReturnPath)) {
    throw "Return path not found: $ReturnPath"
}

$roundDir = Join-Path $CeremonyDir ("round-R{0}" -f $RoundNumber)
$outputDir = Join-Path $roundDir "output_from_contributor"
if (-not (Test-Path -LiteralPath $roundDir)) {
    throw "Round directory not found: $roundDir"
}
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null

$requiredParams = @("balance.params", "merkle.params", "nullifier.params", "private_tx.params")

function Resolve-ReturnRoot {
    param([string]$PathValue)
    if ((Get-Item -LiteralPath $PathValue).PSIsContainer) {
        return (Resolve-Path -LiteralPath $PathValue).Path
    }

    $ext = [IO.Path]::GetExtension($PathValue).ToLowerInvariant()
    if ($ext -ne ".zip") {
        throw "ReturnPath must be a directory or .zip archive: $PathValue"
    }

    $tmp = Join-Path $env:TEMP ("zoka_return_" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    Expand-Archive -LiteralPath $PathValue -DestinationPath $tmp -Force
    return $tmp
}

function Find-ParamFile {
    param(
        [string]$Root,
        [string]$Name
    )
    $matches = @(Get-ChildItem -Path $Root -Recurse -File -Filter $Name | Select-Object -ExpandProperty FullName)
    if ($matches.Length -eq 0) {
        return $null
    }
    if ($matches.Length -gt 1) {
        throw "Multiple files named $Name found in return package; keep only one"
    }
    return $matches[0]
}

$cleanupTmp = $null
$returnRoot = Resolve-ReturnRoot -PathValue $ReturnPath
if ($returnRoot.StartsWith($env:TEMP, [System.StringComparison]::OrdinalIgnoreCase)) {
    $cleanupTmp = $returnRoot
}

try {
    foreach ($name in $requiredParams) {
        $source = Find-ParamFile -Root $returnRoot -Name $name
        if ($null -eq $source) {
            throw "Missing required params file in return package: $name"
        }
        $dest = Join-Path $outputDir $name
        Copy-Item -LiteralPath $source -Destination $dest -Force
    }

    $attestationCandidates = Get-ChildItem -Path $returnRoot -Recurse -File -Filter "*ATTESTATION*.md"
    if ($attestationCandidates.Count -gt 0) {
        $attDest = Join-Path $roundDir ("ATTESTATION_contrib-{0}_returned.md" -f $ContributorId)
        Copy-Item -LiteralPath $attestationCandidates[0].FullName -Destination $attDest -Force
        Write-Host "Returned attestation captured: $attDest"
    }

    $finalizeArgs = @(
        "-ExecutionPolicy", "Bypass",
        "-File", "scripts/finalize_ceremony_round.ps1",
        "-CeremonyDir", $CeremonyDir,
        "-RoundNumber", "$RoundNumber",
        "-ContributorId", $ContributorId,
        "-Label", $Label
    )
    if (-not [string]::IsNullOrWhiteSpace($Note)) {
        $finalizeArgs += @("-Note", $Note)
    }
    if ($InitNextRound) {
        if ([string]::IsNullOrWhiteSpace($NextContributorId)) {
            throw "NextContributorId is required with -InitNextRound"
        }
        $finalizeArgs += "-InitNextRound"
        $finalizeArgs += @("-NextContributorId", $NextContributorId)
    }

    Write-Host "==> Finalizing ceremony round with imported return package"
    powershell @finalizeArgs

    if ($InitNextRound -and $ExportNextRoundPackage) {
        $nextRound = $RoundNumber + 1
        $nextRoundDir = Join-Path $CeremonyDir ("round-R{0}" -f $nextRound)
        $outDir = Join-Path $CeremonyDir "out"
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
        $zipPath = Join-Path $outDir ("zoka-round-R{0}-{1}.zip" -f $nextRound, $NextContributorId)
        powershell -ExecutionPolicy Bypass -File scripts/export_ceremony_round_package.ps1 `
          -RoundDir $nextRoundDir `
          -OutZip $zipPath
    }
}
finally {
    if ($cleanupTmp -and (Test-Path -LiteralPath $cleanupTmp)) {
        Remove-Item -LiteralPath $cleanupTmp -Recurse -Force
    }
}

Write-Host ""
Write-Host "Round return processed successfully."
