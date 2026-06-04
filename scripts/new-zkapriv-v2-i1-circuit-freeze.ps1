[CmdletBinding()]
param(
    [string]$OutputPath = "",

    [ValidateSet("review-candidate", "reviewed")]
    [string]$Status = "review-candidate",

    [string]$Reviewer = "",

    [ValidatePattern("^$|^[0-9a-fA-F]{64}$")]
    [string]$ReviewRecordSha256 = "",

    [switch]$Force
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
if (-not $OutputPath) {
    $OutputPath = Join-Path $repoRoot "docs\artifacts\zkapriv-v2-i1-circuit-freeze.candidate.json"
}
$resolvedOutput = Resolve-FullPath $OutputPath

$sourceFiles = @(
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

foreach ($entry in $sourceFiles) {
    $path = Join-Path $repoRoot $entry.path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing circuit-freeze source file: $path"
    }
}

Push-Location $repoRoot
try {
    $baseRevision = (& git rev-parse HEAD).Trim().ToLowerInvariant()
    if ($LASTEXITCODE -ne 0 -or $baseRevision -notmatch "^[0-9a-f]{40,64}$") {
        throw "Unable to resolve the source git revision."
    }
    $sourcePaths = @($sourceFiles | ForEach-Object { $_.path })
    $dirtyPaths = @(& git status --porcelain=v1 -- @sourcePaths)
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to inspect the circuit source tree."
    }
} finally {
    Pop-Location
}

if ($Status -eq "reviewed") {
    if ($dirtyPaths.Count -ne 0) {
        throw "A reviewed circuit freeze requires a clean source tree."
    }
    if (-not $Reviewer) {
        throw "A reviewed circuit freeze requires -Reviewer."
    }
    if (-not $ReviewRecordSha256 -or $ReviewRecordSha256 -eq ("0" * 64)) {
        throw "A reviewed circuit freeze requires a non-zero -ReviewRecordSha256."
    }
}

$records = @()
foreach ($entry in $sourceFiles) {
    $path = Join-Path $repoRoot $entry.path
    $records += [ordered]@{
        category = $entry.category
        path = $entry.path
        bytes = (Get-Item -LiteralPath $path).Length
        sha256 = Get-FileSha256Hex $path
    }
}

$manifest = [ordered]@{
    schema_version = 1
    profile = "private_tx_v2_i1"
    status = $Status
    generated_utc = [System.DateTime]::UtcNow.ToString("o")
    base_git_revision = $baseRevision
    source_tree_dirty = ($dirtyPaths.Count -ne 0)
    source_tree_dirty_paths = @($dirtyPaths)
    source_snapshot_digest = Get-CanonicalSourceSnapshotDigest $repoRoot $sourceFiles
    shape = [ordered]@{
        private_proof_version = 2
        max_inputs = 1
        merkle_proven_inputs = 1
        private_tree_depth = 32
        private_tree_scalar_words = 4
        private_input_words = 173
        public_input_words = 12
        proof_bytes = 192
    }
    review = [ordered]@{
        reviewer = $Reviewer
        review_record_sha256 = $ReviewRecordSha256.ToLowerInvariant()
    }
    files = $records
}

if ((Test-Path -LiteralPath $resolvedOutput) -and -not $Force) {
    throw "Circuit-freeze manifest already exists: $resolvedOutput. Pass -Force to replace this explicit target."
}
$outputDirectory = Split-Path -Parent $resolvedOutput
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
    New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
}
$manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resolvedOutput -Encoding Ascii

Write-Host "Prepared ZokaPriv v2 i1 circuit freeze: $resolvedOutput"
Write-Host "Status: $Status"
Write-Host "Base git revision: $baseRevision"
Write-Host "Source tree dirty: $($manifest.source_tree_dirty)"
Write-Host "Source snapshot digest: $($manifest.source_snapshot_digest)"
if ($Status -eq "review-candidate") {
    Write-Warning "This is a review candidate snapshot, not an audited or ceremony-ready freeze."
}
