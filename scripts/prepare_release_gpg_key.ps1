param(
    [string]$OutDir = "",
    [string]$GpgExe = "",
    [string]$Name = "ZOKA Release",
    [string]$Email = "release@zoka.local",
    [string]$MaterialPath = "",
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Find-Gpg {
    $cmd = Get-Command gpg -ErrorAction SilentlyContinue
    if ($null -ne $cmd) {
        return $cmd.Source
    }
    $candidates = @(
        "C:\Program Files\Git\usr\bin\gpg.exe",
        "C:\Program Files\GnuPG\bin\gpg.exe",
        "C:\Program Files (x86)\GnuPG\bin\gpg.exe",
        (Join-Path $env:LOCALAPPDATA "Programs\GnuPG\bin\gpg.exe")
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw "gpg not found. Install GPG or pass -GpgExe <path-to-gpg.exe>."
}

if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $env:USERPROFILE "zoka-secrets"
}
if ([string]::IsNullOrWhiteSpace($GpgExe)) {
    $GpgExe = Find-Gpg
}
$gpgDir = Split-Path -Parent $GpgExe
$oldPath = $env:PATH
if ($env:PATH -notlike "*$gpgDir*") {
    $env:PATH = "$gpgDir;$env:PATH"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$gnupgHome = Join-Path $OutDir "gnupg-release"
$publicKeyPath = Join-Path $OutDir "zoka-release-signing.asc"
$fingerprintPath = Join-Path $OutDir "zoka-release-signing.fingerprint.txt"

if ((Test-Path -LiteralPath $gnupgHome) -and $Force) {
    Remove-Item -LiteralPath $gnupgHome -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $gnupgHome | Out-Null

$oldErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"

$existing = & $GpgExe --homedir $gnupgHome --batch --with-colons --list-secret-keys $Email 2>$null | Out-String
if ([string]::IsNullOrWhiteSpace($existing)) {
    $batchPath = Join-Path $OutDir "zoka-release-gpg-batch.txt"
    @"
%no-protection
Key-Type: RSA
Key-Length: 4096
Name-Real: $Name
Name-Email: $Email
Expire-Date: 0
%commit
"@ | Set-Content -LiteralPath $batchPath -Encoding ascii

    & $GpgExe --homedir $gnupgHome --batch --generate-key $batchPath
    if ($LASTEXITCODE -ne 0) {
        $ErrorActionPreference = $oldErrorActionPreference
        throw "gpg key generation failed"
    }
    Remove-Item -LiteralPath $batchPath -Force -ErrorAction SilentlyContinue
}

$keyInfo = & $GpgExe --homedir $gnupgHome --batch --with-colons --fingerprint $Email | Out-String
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($keyInfo)) {
    $ErrorActionPreference = $oldErrorActionPreference
    throw "failed to read generated GPG fingerprint"
}
$fingerprint = ($keyInfo -split "`r?`n" | Where-Object { $_ -like "fpr:*" } | Select-Object -First 1).Split(":")[9]
if ([string]::IsNullOrWhiteSpace($fingerprint)) {
    $ErrorActionPreference = $oldErrorActionPreference
    throw "fingerprint not found in gpg output"
}
$fingerprintSpaced = (($fingerprint.ToUpperInvariant() -split '(.{4})' | Where-Object { $_ }) -join ' ')
$fingerprintSpaced | Set-Content -LiteralPath $fingerprintPath -Encoding ascii

& $GpgExe --homedir $gnupgHome --armor --export $Email | Set-Content -LiteralPath $publicKeyPath -Encoding ascii
if ($LASTEXITCODE -ne 0) {
    $ErrorActionPreference = $oldErrorActionPreference
    throw "failed to export public GPG key"
}

$ErrorActionPreference = $oldErrorActionPreference

if (![string]::IsNullOrWhiteSpace($MaterialPath)) {
    if (!(Test-Path -LiteralPath $MaterialPath -PathType Leaf)) {
        throw "Material file not found: $MaterialPath"
    }
    $material = Get-Content -LiteralPath $MaterialPath -Raw | ConvertFrom-Json
    $material.release.gpg_fingerprint = $fingerprintSpaced
    $material | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $MaterialPath -Encoding UTF8
}

Write-Host "Prepared local release GPG key."
Write-Host "GPG home:      $gnupgHome"
Write-Host "Public key:    $publicKeyPath"
Write-Host "Fingerprint:   $fingerprintSpaced"
Write-Host "Fingerprint file: $fingerprintPath"
if (![string]::IsNullOrWhiteSpace($MaterialPath)) {
    Write-Host "Updated material: $MaterialPath"
}

$env:PATH = $oldPath
