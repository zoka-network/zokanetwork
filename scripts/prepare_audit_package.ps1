param(
    [Parameter(Mandatory = $true)]
    [string]$OutDir,
    [switch]$IncludeCargoCheck
)

$ErrorActionPreference = "Stop"

$utcNow = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$root = (Get-Location).Path

if (Test-Path -LiteralPath $OutDir) {
    Remove-Item -LiteralPath $OutDir -Recurse -Force
}
New-Item -ItemType Directory -Path $OutDir | Out-Null
New-Item -ItemType Directory -Path (Join-Path $OutDir "docs") | Out-Null
New-Item -ItemType Directory -Path (Join-Path $OutDir "scripts") | Out-Null

function Copy-IfExists {
    param(
        [string]$Source,
        [string]$Destination
    )
    if (Test-Path -LiteralPath $Source) {
        Copy-Item -LiteralPath $Source -Destination $Destination -Force
    }
}

$filesToCopy = @(
    "ROADMAP.md",
    "PENDIENTE.md",
    "Cargo.toml",
    "Cargo.lock",
    "docs/ceremony_mpc.md",
    "docs/ceremony_runbook.md",
    "docs/ceremony_closeout.md",
    "docs/ceremony_contributor_kit.md",
    "docs/ceremony_execution_board.md",
    "docs/templates/ceremony_round_checklist.md",
    "docs/templates/ceremony_attestation_template.md",
    "docs/templates/ceremony_contributor_invitation.md",
    "docs/audit_prep.md",
    "docs/tor_i2p_transport.md",
    "docs/multi_asset.md",
    "docs/fuzzing_coverage.md",
    "scripts/build_release_with_setup_digest.ps1",
    "scripts/publish_ceremony_bundle.ps1",
    "scripts/verify_ceremony_bundle.ps1",
    "scripts/ceremony_closeout.ps1",
    "scripts/init_ceremony_round.ps1",
    "scripts/export_ceremony_round_package.ps1",
    "scripts/finalize_ceremony_round.ps1",
    "scripts/process_ceremony_round_return.ps1",
    "scripts/process_ceremony_return_with_plan.ps1",
    "scripts/init_ceremony_round_plan.ps1"
)

foreach ($path in $filesToCopy) {
    $src = Join-Path $root $path
    if ($path.StartsWith("docs/")) {
        $dst = Join-Path (Join-Path $OutDir "docs") ([IO.Path]::GetFileName($path))
        Copy-IfExists -Source $src -Destination $dst
    } elseif ($path.StartsWith("scripts/")) {
        $dst = Join-Path (Join-Path $OutDir "scripts") ([IO.Path]::GetFileName($path))
        Copy-IfExists -Source $src -Destination $dst
    } else {
        $dst = Join-Path $OutDir ([IO.Path]::GetFileName($path))
        Copy-IfExists -Source $src -Destination $dst
    }
}

function Get-GitValue {
    param(
        [string[]]$Args,
        [string]$Fallback
    )
    try {
        $value = (& git @Args 2>$null)
        if ($LASTEXITCODE -ne 0) {
            return $Fallback
        }
        if ($null -eq $value) {
            return $Fallback
        }
        $text = ($value | Out-String).Trim()
        if ([string]::IsNullOrWhiteSpace($text)) {
            return $Fallback
        }
        return $text
    } catch {
        return $Fallback
    }
}

function Get-GitStatusLines {
    try {
        $value = (& git status --short 2>$null)
        if ($LASTEXITCODE -ne 0) {
            return @()
        }
        if ($null -eq $value) {
            return @()
        }
        return @($value)
    } catch {
        return @()
    }
}

$commit = Get-GitValue -Args @("rev-parse", "HEAD") -Fallback "unknown"
$branch = Get-GitValue -Args @("rev-parse", "--abbrev-ref", "HEAD") -Fallback "unknown"
$shortStatus = Get-GitStatusLines

$metadata = [ordered]@{
    generated_at_utc = $utcNow
    repo_root = $root
    git_commit = $commit
    git_branch = $branch
    has_uncommitted_changes = ($shortStatus.Count -gt 0)
    checklist = @{
        ceremony_blocker = "#5"
        external_audit_blocker = "#9"
    }
}

$metadataPath = Join-Path $OutDir "metadata.json"
$metadata | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $metadataPath -Encoding UTF8

$statusPath = Join-Path $OutDir "git_status.txt"
$shortStatus | Set-Content -LiteralPath $statusPath -Encoding UTF8

if ($IncludeCargoCheck) {
    $checkLog = Join-Path $OutDir "cargo_check.log"
    & cargo check *> $checkLog
}

$readmePath = Join-Path $OutDir "README_AUDIT_PACKAGE.txt"
@(
    "ZOKA External Audit Package",
    "",
    "Generated at: $utcNow",
    "Commit: $commit",
    "Branch: $branch",
    "",
    "Included files:",
    "- Project roadmap/pending snapshots",
    "- Ceremony runbooks and closeout scripts",
    "- Audit prep scope/checklist",
    "- Build and publication scripts",
    "- Git metadata and working tree status",
    "",
    "Suggested first documents:",
    "1. docs/audit_prep.md",
    "2. docs/ceremony_mpc.md",
    "3. docs/ceremony_runbook.md"
) | Set-Content -LiteralPath $readmePath -Encoding UTF8

Write-Host "Audit package ready at: $OutDir"
Write-Host "Metadata: $metadataPath"
Write-Host "Git status: $statusPath"
