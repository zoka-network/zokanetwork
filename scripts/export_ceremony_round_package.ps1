param(
    [Parameter(Mandatory = $true)]
    [string]$RoundDir,
    [Parameter(Mandatory = $true)]
    [string]$OutZip
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $RoundDir)) {
    throw "Round directory not found: $RoundDir"
}

$required = @(
    "input",
    "CHECKLIST.md",
    "ROUND_SUMMARY.txt",
    "INSTRUCTIONS.txt",
    "manifest.json"
)

foreach ($entry in $required) {
    $path = Join-Path $RoundDir $entry
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Missing required round artifact: $path"
    }
}

if (Test-Path -LiteralPath $OutZip) {
    Remove-Item -LiteralPath $OutZip -Force
}

$tmp = Join-Path ([IO.Path]::GetDirectoryName($OutZip)) ("tmp_round_pkg_" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Copy-Item -LiteralPath (Join-Path $RoundDir "input") -Destination (Join-Path $tmp "input") -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $RoundDir "CHECKLIST.md") -Destination (Join-Path $tmp "CHECKLIST.md") -Force
    Copy-Item -LiteralPath (Join-Path $RoundDir "ROUND_SUMMARY.txt") -Destination (Join-Path $tmp "ROUND_SUMMARY.txt") -Force
    Copy-Item -LiteralPath (Join-Path $RoundDir "INSTRUCTIONS.txt") -Destination (Join-Path $tmp "INSTRUCTIONS.txt") -Force
    Copy-Item -LiteralPath (Join-Path $RoundDir "manifest.json") -Destination (Join-Path $tmp "manifest.json") -Force

    Get-ChildItem -LiteralPath $RoundDir -Filter "ATTESTATION_*.md" | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $tmp $_.Name) -Force
    }
    Get-ChildItem -LiteralPath $RoundDir -Filter "INVITATION_*.md" | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $tmp $_.Name) -Force
    }

    Compress-Archive -Path (Join-Path $tmp "*") -DestinationPath $OutZip -CompressionLevel Optimal -Force
} finally {
    if (Test-Path -LiteralPath $tmp) {
        Remove-Item -LiteralPath $tmp -Recurse -Force
    }
}

$hash = (Get-FileHash -LiteralPath $OutZip -Algorithm SHA256).Hash.ToLowerInvariant()
$hashPath = "$OutZip.sha256"
Set-Content -LiteralPath $hashPath -Value "$hash  $([IO.Path]::GetFileName($OutZip))" -Encoding ASCII

Write-Host "Round package exported:"
Write-Host "  ZIP : $OutZip"
Write-Host "  SHA : $hash"
Write-Host "  Hash file: $hashPath"
