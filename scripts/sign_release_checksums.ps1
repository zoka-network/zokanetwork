param(
    [Parameter(Mandatory = $true)][string]$ChecksumsPath,
    [string]$GpgExe = "",
    [string]$LocalUser = "",
    [switch]$Verify
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

if (!(Test-Path -LiteralPath $ChecksumsPath -PathType Leaf)) {
    throw "Checksums file not found: $ChecksumsPath"
}

if ([string]::IsNullOrWhiteSpace($GpgExe)) {
    $GpgExe = Find-Gpg
}

$sigPath = "$ChecksumsPath.asc"
$signArgs = @("--detach-sign", "--armor")
if (![string]::IsNullOrWhiteSpace($LocalUser)) {
    $signArgs += @("--local-user", $LocalUser)
}
$signArgs += $ChecksumsPath

Write-Host "Signing checksums with: $GpgExe"
& $GpgExe @signArgs
if ($LASTEXITCODE -ne 0) {
    throw "gpg signing failed"
}

Write-Host "Signature written: $sigPath"

if ($Verify) {
    & $GpgExe --verify $sigPath $ChecksumsPath
    if ($LASTEXITCODE -ne 0) {
        throw "gpg verification failed"
    }
    Write-Host "Signature verification OK"
}
