[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$I1ParamsPath,

    [string]$BaseSetupDirectory = "",
    [string]$CircuitFreezeManifestPath = "",
    [string]$OutputDirectory = "",

    [Parameter(Mandatory = $true)]
    [ValidatePattern("^[0-9a-fA-F]{40,64}$")]
    [string]$CircuitRevision,

    [Parameter(Mandatory = $true)]
    [ValidatePattern("^[0-9a-fA-F]{64}$")]
    [string]$TranscriptSha256,

    [switch]$AllowUnsafeLabArtifact,
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    return [System.IO.Path]::GetFullPath($Path)
}

function Test-PathWithin {
    param(
        [Parameter(Mandatory = $true)][string]$Child,
        [Parameter(Mandatory = $true)][string]$Parent
    )
    $resolvedChild = Resolve-FullPath $Child
    $resolvedParent = (Resolve-FullPath $Parent).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    ) + [System.IO.Path]::DirectorySeparatorChar
    return $resolvedChild.StartsWith(
        $resolvedParent,
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function ConvertTo-LowerHex {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    return [System.BitConverter]::ToString($Bytes).Replace("-", "").ToLowerInvariant()
}

function Get-FileSha256Bytes {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            return $sha256.ComputeHash($stream)
        } finally {
            $sha256.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Get-FileSha256Hex {
    param([Parameter(Mandatory = $true)][string]$Path)
    return ConvertTo-LowerHex (Get-FileSha256Bytes $Path)
}

function Get-CanonicalBundleDigest {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][object[]]$Circuits
    )
    $payload = New-Object System.IO.MemoryStream
    try {
        foreach ($entry in $Circuits) {
            $path = Join-Path $Directory $entry.file
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
                throw "Missing trusted-setup file: $path"
            }
            $nameBytes = [System.Text.Encoding]::UTF8.GetBytes([string]$entry.circuit)
            $payload.Write($nameBytes, 0, $nameBytes.Length)
            $payload.WriteByte(0x1f)
            $fileHash = Get-FileSha256Bytes $path
            $payload.Write($fileHash, 0, $fileHash.Length)
            $payload.WriteByte(0x1e)
        }
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            return ConvertTo-LowerHex ($sha256.ComputeHash($payload.ToArray()))
        } finally {
            $sha256.Dispose()
        }
    } finally {
        $payload.Dispose()
    }
}

$repoRoot = Resolve-FullPath (Join-Path $PSScriptRoot "..")
if (-not $BaseSetupDirectory) {
    $BaseSetupDirectory = Join-Path $repoRoot "trusted-setup"
}
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repoRoot "ceremony-output\zkapriv-v2-i1-release-candidate"
}
if (-not $CircuitFreezeManifestPath) {
    $CircuitFreezeManifestPath =
        Join-Path $repoRoot "docs\artifacts\zkapriv-v2-i1-circuit-freeze.candidate.json"
}

$resolvedBase = Resolve-FullPath $BaseSetupDirectory
$resolvedI1 = Resolve-FullPath $I1ParamsPath
$resolvedCircuitFreeze = Resolve-FullPath $CircuitFreezeManifestPath
$resolvedOutput = Resolve-FullPath $OutputDirectory
$resolvedLab = Resolve-FullPath (Join-Path $repoRoot "lab-trusted-setup-v2-i1")
$zeroTranscript = "0" * 64

if (-not (Test-Path -LiteralPath $resolvedI1 -PathType Leaf)) {
    throw "Missing finalized private_tx_v2_i1.params artifact: $resolvedI1"
}
if (-not (Test-Path -LiteralPath $resolvedCircuitFreeze -PathType Leaf)) {
    throw "Missing circuit-freeze manifest: $resolvedCircuitFreeze"
}
if ((Test-PathWithin $resolvedI1 $resolvedLab) -and -not $AllowUnsafeLabArtifact) {
    throw "Refusing benchmark-only lab params at $resolvedI1. Supply an independently finalized ceremony artifact, or pass -AllowUnsafeLabArtifact for a rehearsal only."
}
if (($TranscriptSha256.ToLowerInvariant() -eq $zeroTranscript) -and -not $AllowUnsafeLabArtifact) {
    throw "TranscriptSha256 cannot be all zeros for a release candidate."
}
if (Test-Path -LiteralPath (Join-Path $resolvedBase "private_tx_v2_i1.params")) {
    throw "BaseSetupDirectory must be the clean v1 bundle without private_tx_v2_i1.params."
}

$circuitFreeze = Get-Content -LiteralPath $resolvedCircuitFreeze -Raw | ConvertFrom-Json
if ($circuitFreeze.schema_version -ne 1 -or $circuitFreeze.profile -ne "private_tx_v2_i1") {
    throw "Circuit-freeze manifest schema/profile mismatch."
}
if ($circuitFreeze.status -notin @("review-candidate", "reviewed")) {
    throw "Unsupported circuit-freeze status: $($circuitFreeze.status)"
}
if ($CircuitRevision.ToLowerInvariant() -ne $circuitFreeze.base_git_revision) {
    throw "CircuitRevision does not match circuit-freeze base_git_revision."
}
if ($circuitFreeze.source_snapshot_digest -notmatch "^[0-9a-f]{64}$") {
    throw "Circuit-freeze source_snapshot_digest is invalid."
}
if (-not $AllowUnsafeLabArtifact) {
    if ($circuitFreeze.status -ne "reviewed") {
        throw "Release candidate requires a reviewed circuit-freeze manifest."
    }
    if ($circuitFreeze.source_tree_dirty) {
        throw "Release candidate refuses a reviewed circuit freeze with a dirty source tree."
    }
    if (-not $circuitFreeze.review.reviewer) {
        throw "Release candidate requires a reviewer in the circuit freeze."
    }
    if ($circuitFreeze.review.review_record_sha256 -notmatch "^[0-9a-f]{64}$" -or
        $circuitFreeze.review.review_record_sha256 -eq ("0" * 64)) {
        throw "Release candidate requires a valid external review record digest."
    }
}
$circuitFreezeManifestSha256 = Get-FileSha256Hex $resolvedCircuitFreeze

$baseCircuits = @(
    [ordered]@{ circuit = "balance_circuit"; file = "balance.params" },
    [ordered]@{ circuit = "merkle_circuit"; file = "merkle.params" },
    [ordered]@{ circuit = "nullifier_circuit"; file = "nullifier.params" },
    [ordered]@{ circuit = "private_tx"; file = "private_tx.params" }
)
$candidateCircuits = @(
    $baseCircuits[0],
    $baseCircuits[1],
    $baseCircuits[2],
    $baseCircuits[3],
    [ordered]@{ circuit = "private_tx_v2_i1"; file = "private_tx_v2_i1.params" }
)

$baseDigestPath = Join-Path $resolvedBase "DIGEST"
if (-not (Test-Path -LiteralPath $baseDigestPath -PathType Leaf)) {
    throw "Missing base DIGEST file: $baseDigestPath"
}
$storedBaseDigest = (Get-Content -LiteralPath $baseDigestPath -Raw).Trim().ToLowerInvariant()
$actualBaseDigest = Get-CanonicalBundleDigest $resolvedBase $baseCircuits
if ($storedBaseDigest -ne $actualBaseDigest) {
    throw "Base trusted-setup DIGEST mismatch: stored=$storedBaseDigest actual=$actualBaseDigest"
}

if (Test-Path -LiteralPath $resolvedOutput) {
    $existing = @(Get-ChildItem -LiteralPath $resolvedOutput -Force)
    if (($existing.Count -gt 0) -and -not $Force) {
        throw "Output directory is not empty: $resolvedOutput. Use -Force to replace this explicit target."
    }
    if ($Force) {
        foreach ($item in $existing) {
            if (-not (Test-PathWithin $item.FullName $resolvedOutput)) {
                throw "Refusing to remove path outside output directory: $($item.FullName)"
            }
            Remove-Item -LiteralPath $item.FullName -Recurse -Force
        }
    }
} else {
    New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null
}

foreach ($entry in $baseCircuits) {
    Copy-Item -LiteralPath (Join-Path $resolvedBase $entry.file) `
        -Destination (Join-Path $resolvedOutput $entry.file)
}
Copy-Item -LiteralPath $resolvedI1 `
    -Destination (Join-Path $resolvedOutput "private_tx_v2_i1.params")
Copy-Item -LiteralPath $resolvedCircuitFreeze `
    -Destination (Join-Path $resolvedOutput "CIRCUIT-FREEZE.json")

$candidateDigest = Get-CanonicalBundleDigest $resolvedOutput $candidateCircuits
Set-Content -LiteralPath (Join-Path $resolvedOutput "DIGEST") `
    -Value $candidateDigest -Encoding Ascii

$unsafeRehearsal = $AllowUnsafeLabArtifact.IsPresent
$files = @()
foreach ($entry in $candidateCircuits) {
    $path = Join-Path $resolvedOutput $entry.file
    $files += [ordered]@{
        circuit = $entry.circuit
        file = $entry.file
        bytes = (Get-Item -LiteralPath $path).Length
        sha256 = Get-FileSha256Hex $path
    }
}

$manifest = [ordered]@{
    schema_version = 1
    network = "zoka-mainnet"
    profile = "private_tx_v2_i1"
    status = $(if ($unsafeRehearsal) { "unsafe-rehearsal" } else { "release-candidate" })
    created_utc = [System.DateTime]::UtcNow.ToString("o")
    circuit_revision = $CircuitRevision.ToLowerInvariant()
    circuit_freeze_status = $circuitFreeze.status
    circuit_freeze_manifest_sha256 = $circuitFreezeManifestSha256
    circuit_freeze_source_snapshot_digest = $circuitFreeze.source_snapshot_digest
    ceremony_transcript_sha256 = $TranscriptSha256.ToLowerInvariant()
    base_v1_digest = $actualBaseDigest
    canonical_bundle_digest = $candidateDigest
    files = $files
}
$manifestPath = Join-Path $resolvedOutput "MANIFEST.json"
$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding Ascii
$manifestHash = Get-FileSha256Hex $manifestPath
Set-Content -LiteralPath (Join-Path $resolvedOutput "MANIFEST.sha256") `
    -Value $manifestHash -Encoding Ascii

Write-Host "Prepared ZokaPriv v2 i1 bundle: $resolvedOutput"
Write-Host "Status: $($manifest.status)"
Write-Host "Canonical DIGEST: $candidateDigest"
Write-Host "Manifest SHA-256: $manifestHash"
if ($unsafeRehearsal) {
    Write-Warning "This package contains an explicitly allowed unsafe rehearsal artifact. Never deploy it."
} else {
    Write-Host "Next: obtain an offline detached RSA signature for MANIFEST.json and verify with -RequireSignature before staging."
}
