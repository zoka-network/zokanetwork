[CmdletBinding()]
param(
    [string]$ManifestPath = "",
    [switch]$RequireCleanTree,
    [switch]$RequireReviewedStatus
)

$ErrorActionPreference = "Stop"

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    return [System.IO.Path]::GetFullPath($Path)
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

function Get-CanonicalSourceSnapshotDigest {
    param(
        [Parameter(Mandatory = $true)][string]$RepositoryRoot,
        [Parameter(Mandatory = $true)][object[]]$Files
    )
    $payload = New-Object System.IO.MemoryStream
    try {
        foreach ($entry in $Files) {
            $categoryBytes = [System.Text.Encoding]::UTF8.GetBytes([string]$entry.category)
            $pathBytes = [System.Text.Encoding]::UTF8.GetBytes([string]$entry.path)
            $payload.Write($categoryBytes, 0, $categoryBytes.Length)
            $payload.WriteByte(0x1f)
            $payload.Write($pathBytes, 0, $pathBytes.Length)
            $payload.WriteByte(0x1f)
            $fileHash = Get-FileSha256Bytes (Join-Path $RepositoryRoot $entry.path)
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
if (-not $ManifestPath) {
    $ManifestPath = Join-Path $repoRoot "docs\artifacts\zkapriv-v2-i1-circuit-freeze.candidate.json"
}
$resolvedManifest = Resolve-FullPath $ManifestPath
if (-not (Test-Path -LiteralPath $resolvedManifest -PathType Leaf)) {
    throw "Missing circuit-freeze manifest: $resolvedManifest"
}

$expectedFiles = @(
    [ordered]@{ category = "circuit"; path = "src/zk/groth16_backend.rs" },
    [ordered]@{ category = "circuit"; path = "src/crypto/private_tree.rs" },
    [ordered]@{ category = "circuit"; path = "src/crypto/mod.rs" },
    [ordered]@{ category = "transaction-composition"; path = "src/private_wallet/mod.rs" },
    [ordered]@{ category = "transaction-composition"; path = "src/private_wallet/clsag.rs" },
    [ordered]@{ category = "transaction-composition"; path = "src/zk/range_proofs.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/state/mod.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/state/sync.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/zk/prover.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/zk/verifier.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/zk/setup_io.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/economics.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/mobile.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/network/message.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/network/p2p.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/mining/block_validator.rs" },
    [ordered]@{ category = "protocol-integration"; path = "src/types/mod.rs" },
    [ordered]@{ category = "dependency-lock"; path = "Cargo.toml" },
    [ordered]@{ category = "dependency-lock"; path = "Cargo.lock" }
)

$manifest = Get-Content -LiteralPath $resolvedManifest -Raw | ConvertFrom-Json
if ($manifest.schema_version -ne 1) {
    throw "Unsupported circuit-freeze schema_version: $($manifest.schema_version)"
}
if ($manifest.profile -ne "private_tx_v2_i1") {
    throw "Circuit-freeze profile mismatch: $($manifest.profile)"
}
if ($manifest.status -notin @("review-candidate", "reviewed")) {
    throw "Unsupported circuit-freeze status: $($manifest.status)"
}
if ($RequireReviewedStatus -and $manifest.status -ne "reviewed") {
    throw "Circuit freeze has not reached reviewed status."
}
if ($manifest.status -eq "reviewed") {
    if ($manifest.source_tree_dirty) {
        throw "Reviewed circuit freeze records a dirty source tree."
    }
    if (-not $manifest.review.reviewer) {
        throw "Reviewed circuit freeze is missing reviewer."
    }
    if ($manifest.review.review_record_sha256 -notmatch "^[0-9a-f]{64}$" -or
        $manifest.review.review_record_sha256 -eq ("0" * 64)) {
        throw "Reviewed circuit freeze is missing a valid external review record digest."
    }
}

$expectedShape = [ordered]@{
    private_proof_version = 2
    max_inputs = 1
    merkle_proven_inputs = 1
    private_tree_depth = 32
    private_tree_scalar_words = 4
    private_input_words = 173
    public_input_words = 12
    proof_bytes = 192
}
foreach ($property in $expectedShape.Keys) {
    if ([long]$manifest.shape.$property -ne [long]$expectedShape[$property]) {
        throw "Circuit shape mismatch for ${property}: expected=$($expectedShape[$property]) actual=$($manifest.shape.$property)"
    }
}

if (@($manifest.files).Count -ne $expectedFiles.Count) {
    throw "Circuit-freeze file count mismatch."
}
for ($index = 0; $index -lt $expectedFiles.Count; $index++) {
    $expected = $expectedFiles[$index]
    $recorded = @($manifest.files)[$index]
    $path = Join-Path $repoRoot $expected.path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing circuit-freeze source file: $path"
    }
    if ($recorded.category -ne $expected.category -or $recorded.path -ne $expected.path) {
        throw "Circuit-freeze source order mismatch at index $index."
    }
    if ([long]$recorded.bytes -ne (Get-Item -LiteralPath $path).Length) {
        throw "Circuit-freeze byte count mismatch for $($expected.path)."
    }
    if ($recorded.sha256 -ne (Get-FileSha256Hex $path)) {
        throw "Circuit-freeze SHA-256 mismatch for $($expected.path)."
    }
}

$actualSnapshotDigest = Get-CanonicalSourceSnapshotDigest $repoRoot $expectedFiles
if ($manifest.source_snapshot_digest -ne $actualSnapshotDigest) {
    throw "Circuit-freeze source snapshot digest mismatch."
}

Push-Location $repoRoot
try {
    $currentRevision = (& git rev-parse HEAD).Trim().ToLowerInvariant()
    if ($LASTEXITCODE -ne 0 -or $currentRevision -notmatch "^[0-9a-f]{40,64}$") {
        throw "Unable to resolve the source git revision."
    }
    $sourcePaths = @($expectedFiles | ForEach-Object { $_.path })
    $dirtyPaths = @(& git status --porcelain=v1 -- @sourcePaths)
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to inspect the circuit source tree."
    }
} finally {
    Pop-Location
}
if ($manifest.base_git_revision -ne $currentRevision) {
    throw "Circuit-freeze base_git_revision does not match the current checkout."
}
$recordedDirtyPaths = @($manifest.source_tree_dirty_paths)
if ([bool]$manifest.source_tree_dirty -ne ($dirtyPaths.Count -ne 0) -or
    $recordedDirtyPaths.Count -ne $dirtyPaths.Count) {
    throw "Circuit-freeze dirty source metadata mismatch."
}
for ($index = 0; $index -lt $dirtyPaths.Count; $index++) {
    if ($recordedDirtyPaths[$index] -ne $dirtyPaths[$index]) {
        throw "Circuit-freeze dirty source path mismatch at index $index."
    }
}
if ($RequireCleanTree -and $dirtyPaths.Count -ne 0) {
    throw "Circuit source tree is dirty; refusing clean-tree verification."
}

Write-Host "Verified ZokaPriv v2 i1 circuit freeze: $resolvedManifest"
Write-Host "Status: $($manifest.status)"
Write-Host "Base git revision: $($manifest.base_git_revision)"
Write-Host "Current source tree dirty: $($dirtyPaths.Count -ne 0)"
Write-Host "Source snapshot digest: $actualSnapshotDigest"
