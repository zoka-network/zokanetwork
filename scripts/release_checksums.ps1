# Generates SHA-256 checksums for the release artifacts in a release directory.
# Usage:
#   ./scripts/release_checksums.ps1 -ReleaseDir ./release -OutFile ./release/SHA256SUMS.txt
#
# The output file follows the GNU coreutils `sha256sum` format so that any
# verifier can run `sha256sum -c SHA256SUMS.txt` (or PowerShell `Get-FileHash`)
# and confirm every artifact matches.

param(
    [Parameter(Mandatory = $true)][string]$ReleaseDir,
    [string]$OutFile = ""
)

if (-not (Test-Path $ReleaseDir -PathType Container)) {
    Write-Error "release directory not found: $ReleaseDir"
    exit 1
}

$artifacts = Get-ChildItem -File -Path $ReleaseDir | Where-Object {
    $_.Name -ne "SHA256SUMS.txt" -and $_.Name -ne "SHA256SUMS.txt.asc"
} | Sort-Object Name

if ($artifacts.Count -eq 0) {
    Write-Error "no release artifacts found in: $ReleaseDir"
    exit 1
}

$lines = New-Object System.Collections.Generic.List[string]
foreach ($file in $artifacts) {
    $hash = (Get-FileHash -Algorithm SHA256 -Path $file.FullName).Hash.ToLower()
    # GNU coreutils format: "<hash>  <filename>"
    $lines.Add("$hash  $($file.Name)")
}

if ([string]::IsNullOrWhiteSpace($OutFile)) {
    $lines | ForEach-Object { Write-Output $_ }
} else {
    $lines | Out-File -Encoding ascii -NoNewline:$false -FilePath $OutFile
    Write-Host "Wrote $($lines.Count) checksums to $OutFile"
    Write-Host ""
    Write-Host "Next steps:"
    Write-Host "  1. Sign the file with GPG: gpg --detach-sign --armor $OutFile"
    Write-Host "  2. Publish $OutFile and $OutFile.asc together with the artifacts"
    Write-Host "  3. Document the signing key fingerprint in docs/release-verification.md"
}
