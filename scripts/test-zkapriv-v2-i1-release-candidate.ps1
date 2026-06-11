[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDirectory,

    [switch]$AllowUnsafeRehearsal,
    [switch]$RequireSignature,
    [string]$SignaturePath = "",
    [string]$CertificatePath = "",

    [ValidatePattern("^$|^[0-9a-fA-F]{64}$")]
    [string]$ExpectedCertificateSha256 = ""
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

function Assert-DetachedSignature {
    param(
        [Parameter(Mandatory = $true)][string]$ManifestPath,
        [Parameter(Mandatory = $true)][string]$SignaturePath,
        [Parameter(Mandatory = $true)][string]$CertificatePath,
        [Parameter(Mandatory = $true)][string]$ExpectedCertificateSha256
    )
    if (-not (Test-Path -LiteralPath $SignaturePath -PathType Leaf)) {
        throw "Missing detached manifest signature: $SignaturePath"
    }
    if (-not (Test-Path -LiteralPath $CertificatePath -PathType Leaf)) {
        throw "Missing RSA signer certificate: $CertificatePath"
    }
    $actualCertificateSha256 = Get-FileSha256Hex $CertificatePath
    if ($actualCertificateSha256 -ne $ExpectedCertificateSha256.ToLowerInvariant()) {
        throw "Signer certificate SHA-256 mismatch: expected=$ExpectedCertificateSha256 actual=$actualCertificateSha256"
    }

    $certificate = New-Object `
        System.Security.Cryptography.X509Certificates.X509Certificate2($CertificatePath)
    $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey(
        $certificate
    )
    if ($null -eq $rsa) {
        throw "Signer certificate does not contain an RSA public key: $CertificatePath"
    }
    try {
        $valid = $rsa.VerifyData(
            [System.IO.File]::ReadAllBytes($ManifestPath),
            [System.IO.File]::ReadAllBytes($SignaturePath),
            [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
        )
    } finally {
        $rsa.Dispose()
        $certificate.Dispose()
    }
    if (-not $valid) {
        throw "Detached MANIFEST.json RSA signature is invalid."
    }
}

$resolvedBundle = Resolve-FullPath $BundleDirectory
if (-not (Test-Path -LiteralPath $resolvedBundle -PathType Container)) {
    throw "Missing bundle directory: $resolvedBundle"
}

$circuits = @(
    [ordered]@{ circuit = "balance_circuit"; file = "balance.params" },
    [ordered]@{ circuit = "merkle_circuit"; file = "merkle.params" },
    [ordered]@{ circuit = "nullifier_circuit"; file = "nullifier.params" },
    [ordered]@{ circuit = "private_tx"; file = "private_tx.params" },
    [ordered]@{ circuit = "private_tx_v2_i1"; file = "private_tx_v2_i1.params" }
)
$allowedFiles = @(
    "balance.params",
    "merkle.params",
    "nullifier.params",
    "private_tx.params",
    "private_tx_v2_i1.params",
    "CIRCUIT-FREEZE.json",
    "DIGEST",
    "MANIFEST.json",
    "MANIFEST.sha256",
    "MANIFEST.json.sig",
    "MANIFEST.signer.cer"
)
foreach ($item in Get-ChildItem -LiteralPath $resolvedBundle -Force) {
    if ($item.PSIsContainer -or ($allowedFiles -notcontains $item.Name)) {
        throw "Unexpected file or directory in candidate bundle: $($item.FullName)"
    }
}

$digestPath = Join-Path $resolvedBundle "DIGEST"
$circuitFreezePath = Join-Path $resolvedBundle "CIRCUIT-FREEZE.json"
$manifestPath = Join-Path $resolvedBundle "MANIFEST.json"
$manifestHashPath = Join-Path $resolvedBundle "MANIFEST.sha256"
foreach ($path in @($digestPath, $circuitFreezePath, $manifestPath, $manifestHashPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing candidate metadata file: $path"
    }
}

$storedDigest = (Get-Content -LiteralPath $digestPath -Raw).Trim().ToLowerInvariant()
$actualDigest = Get-CanonicalBundleDigest $resolvedBundle $circuits
if ($storedDigest -ne $actualDigest) {
    throw "Candidate DIGEST mismatch: stored=$storedDigest actual=$actualDigest"
}

$storedManifestHash = (Get-Content -LiteralPath $manifestHashPath -Raw).Trim().ToLowerInvariant()
$actualManifestHash = Get-FileSha256Hex $manifestPath
if ($storedManifestHash -ne $actualManifestHash) {
    throw "MANIFEST.sha256 mismatch: stored=$storedManifestHash actual=$actualManifestHash"
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.schema_version -ne 1) {
    throw "Unsupported manifest schema_version: $($manifest.schema_version)"
}
if ($manifest.network -ne "zoka-mainnet" -or $manifest.profile -ne "private_tx_v2_i1") {
    throw "Manifest network/profile mismatch."
}
if ($manifest.status -eq "unsafe-rehearsal") {
    if (-not $AllowUnsafeRehearsal) {
        throw "Refusing unsafe rehearsal package. Pass -AllowUnsafeRehearsal only for a local rehearsal."
    }
} elseif ($manifest.status -ne "release-candidate") {
    throw "Unsupported manifest status: $($manifest.status)"
}
if ($manifest.canonical_bundle_digest -ne $actualDigest) {
    throw "Manifest canonical_bundle_digest mismatch."
}
$circuitFreezeManifestSha256 = Get-FileSha256Hex $circuitFreezePath
if ($manifest.circuit_freeze_manifest_sha256 -ne $circuitFreezeManifestSha256) {
    throw "Embedded CIRCUIT-FREEZE.json SHA-256 mismatch."
}
$circuitFreeze = Get-Content -LiteralPath $circuitFreezePath -Raw | ConvertFrom-Json
if ($circuitFreeze.schema_version -ne 1 -or $circuitFreeze.profile -ne "private_tx_v2_i1") {
    throw "Embedded circuit-freeze schema/profile mismatch."
}
if ($manifest.circuit_revision -ne $circuitFreeze.base_git_revision) {
    throw "Manifest circuit_revision does not match embedded circuit freeze."
}
if ($manifest.circuit_freeze_status -ne $circuitFreeze.status) {
    throw "Manifest circuit_freeze_status mismatch."
}
if ($manifest.circuit_freeze_source_snapshot_digest -ne $circuitFreeze.source_snapshot_digest) {
    throw "Manifest circuit-freeze source snapshot digest mismatch."
}
if ($manifest.status -eq "release-candidate") {
    if ($circuitFreeze.status -ne "reviewed" -or $circuitFreeze.source_tree_dirty) {
        throw "Release candidate requires an embedded reviewed clean circuit freeze."
    }
    if (-not $circuitFreeze.review.reviewer) {
        throw "Embedded reviewed circuit freeze is missing reviewer."
    }
    if ($circuitFreeze.review.review_record_sha256 -notmatch "^[0-9a-f]{64}$" -or
        $circuitFreeze.review.review_record_sha256 -eq ("0" * 64)) {
        throw "Embedded reviewed circuit freeze is missing a valid external review record digest."
    }
} elseif ($circuitFreeze.status -notin @("review-candidate", "reviewed")) {
    throw "Unsupported embedded circuit-freeze status: $($circuitFreeze.status)"
}
if (@($manifest.files).Count -ne $circuits.Count) {
    throw "Manifest file count mismatch."
}
for ($index = 0; $index -lt $circuits.Count; $index++) {
    $expected = $circuits[$index]
    $recorded = @($manifest.files)[$index]
    $path = Join-Path $resolvedBundle $expected.file
    if ($recorded.circuit -ne $expected.circuit -or $recorded.file -ne $expected.file) {
        throw "Manifest circuit order mismatch at index $index."
    }
    if ([long]$recorded.bytes -ne (Get-Item -LiteralPath $path).Length) {
        throw "Manifest byte count mismatch for $($expected.file)."
    }
    if ($recorded.sha256 -ne (Get-FileSha256Hex $path)) {
        throw "Manifest SHA-256 mismatch for $($expected.file)."
    }
}

if ($RequireSignature) {
    if (-not $ExpectedCertificateSha256) {
        throw "RequireSignature also requires ExpectedCertificateSha256 from an independent channel."
    }
    if (-not $SignaturePath) {
        $SignaturePath = Join-Path $resolvedBundle "MANIFEST.json.sig"
    }
    if (-not $CertificatePath) {
        $CertificatePath = Join-Path $resolvedBundle "MANIFEST.signer.cer"
    }
    Assert-DetachedSignature $manifestPath (Resolve-FullPath $SignaturePath) `
        (Resolve-FullPath $CertificatePath) $ExpectedCertificateSha256
}

Write-Host "Verified ZokaPriv v2 i1 bundle: $resolvedBundle"
Write-Host "Status: $($manifest.status)"
Write-Host "Canonical DIGEST: $actualDigest"
Write-Host "Manifest SHA-256: $actualManifestHash"
Write-Host "Detached signature required: $($RequireSignature.IsPresent)"
