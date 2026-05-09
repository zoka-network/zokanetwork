param(
    [string]$RepoRoot = ".",
    [string]$OutDir = "",
    [string]$ZokaExe = "",
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Push-Location $RepoRoot
try {
    $repo = (Resolve-Path ".").Path
    if ([string]::IsNullOrWhiteSpace($OutDir)) {
        $OutDir = Join-Path $env:USERPROFILE "zoka-secrets"
    }
    if ([string]::IsNullOrWhiteSpace($ZokaExe)) {
        $exeName = if ($env:OS -eq "Windows_NT") { "zoka.exe" } else { "zoka" }
        $ZokaExe = Join-Path $repo (Join-Path "target\release" $exeName)
    }
    if (!(Test-Path -LiteralPath $ZokaExe -PathType Leaf)) {
        throw "Missing release zoka binary: $ZokaExe. Run cargo build --release --features randomx --bin zoka --bin zokahd"
    }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

    $labels = @(
        "validator-na-1-consensus",
        "validator-na-1-operator",
        "validator-eu-1-consensus",
        "validator-eu-1-operator",
        "validator-sa-1-consensus",
        "validator-sa-1-operator",
        "snapshot-signer-1"
    )

    $publicKeys = @{}
    foreach ($label in $labels) {
        $outPath = Join-Path $OutDir "$label.ed25519.json"
        $args = @("release-keygen", "--label", $label, "--out", $outPath, "--json")
        if ($Force) {
            $args += "--force"
        }
        $raw = & $ZokaExe @args 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) {
            throw "release-keygen failed for $label`: $raw"
        }
        $report = $raw | ConvertFrom-Json
        $publicKeys[$label] = $report.public_key_hex
    }

    $templatePath = Join-Path $repo "docs\mainnet-launch-material.template.json"
    $material = Get-Content -LiteralPath $templatePath -Raw | ConvertFrom-Json

    $material.genesis.validators[0].consensus_pubkey_hex = $publicKeys["validator-na-1-consensus"]
    $material.genesis.validators[0].operator_pubkey_hex = $publicKeys["validator-na-1-operator"]
    $material.genesis.validators[1].consensus_pubkey_hex = $publicKeys["validator-eu-1-consensus"]
    $material.genesis.validators[1].operator_pubkey_hex = $publicKeys["validator-eu-1-operator"]
    $material.genesis.validators[2].consensus_pubkey_hex = $publicKeys["validator-sa-1-consensus"]
    $material.genesis.validators[2].operator_pubkey_hex = $publicKeys["validator-sa-1-operator"]
    $material.network.snapshot_signers[0] = $publicKeys["snapshot-signer-1"]

    $materialOut = Join-Path $OutDir "mainnet-launch-material.json"
    $material | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $materialOut -Encoding UTF8

    Write-Host "Generated launch key material outside the repo."
    Write-Host "Secrets directory: $OutDir"
    Write-Host "Material file:      $materialOut"
    Write-Host ""
    Write-Host "Still required before applying:"
    Write-Host "  - replace network seed/RPC/onion/I2P placeholders in the material file"
    Write-Host "  - install/configure GPG and replace release.gpg_fingerprint"
    Write-Host ""
    Write-Host "After editing real endpoints/fingerprint:"
    Write-Host "  PowerShell -ExecutionPolicy Bypass -File scripts\apply_mainnet_launch_material.ps1 -MaterialPath `"$materialOut`" -InPlace"
}
finally {
    Pop-Location
}
