param(
    [string]$RepoRoot = ".",
    [string]$ZokaExe = "",
    [switch]$BuildRelease
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$failures = New-Object System.Collections.Generic.List[string]
$warnings = New-Object System.Collections.Generic.List[string]

function Add-Failure {
    param([string]$Message)
    $script:failures.Add($Message) | Out-Null
    Write-Host "FAIL $Message"
}

function Add-Warning {
    param([string]$Message)
    $script:warnings.Add($Message) | Out-Null
    Write-Host "WARN $Message"
}

function Test-Step {
    param(
        [string]$Name,
        [scriptblock]$Body
    )

    try {
        & $Body
        Write-Host "OK   $Name"
    }
    catch {
        Add-Failure "$Name - $($_.Exception.Message)"
    }
}

function Test-Placeholder {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $false
    }
    $lower = $Value.ToLowerInvariant()
    return $lower.Contains("111111") -or
        $lower.Contains("222222") -or
        $lower.Contains("333333") -or
        $lower.Contains("aaaaaaaa") -or
        $lower.Contains("bbbbbbbb") -or
        $lower.Contains("cccccccc") -or
        $lower.Contains("placeholder") -or
        $lower.Contains("example")
}

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
    return $null
}

Push-Location $RepoRoot
try {
    $repo = (Resolve-Path ".").Path
    if ([string]::IsNullOrWhiteSpace($ZokaExe)) {
        $exeName = if ($env:OS -eq "Windows_NT") { "zoka.exe" } else { "zoka" }
        $ZokaExe = Join-Path $repo (Join-Path "target\release" $exeName)
    }

    Write-Host "ZOKA mainnet readiness check"
    Write-Host "Repo: $repo"
    Write-Host "Binary: $ZokaExe"
    Write-Host "--------------------------------"

    $requiredFiles = @(
        "docs/mainnet-network.v1.json",
        "docs/genesis.mainnet.v1.json",
        "docs/runbook-key-rotation.md",
        "docs/runbook-pbft-halt-recovery.md",
        "docs/runbook-incidents.md",
        "docs/runbook-upgrade-hardfork.md",
        "docs/mainnet-audit-closeout.md",
        "docs/mainnet-testnet-chaos-evidence.md",
        "docs/mainnet-release-cutover.md",
        "docs/mainnet-launch-readiness.md",
        "docs/release-verification.md"
    )

    Test-Step "required launch files exist" {
        foreach ($file in $requiredFiles) {
            if (!(Test-Path -LiteralPath $file -PathType Leaf)) {
                throw "missing $file"
            }
        }
    }

    Test-Step "RandomX native toolchain preflight" {
        & powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict | Out-Host
        if ($LASTEXITCODE -ne 0) {
            throw "scripts/check_randomx_toolchain.ps1 -Strict failed"
        }
    }

    if ($BuildRelease) {
        Test-Step "release build with RandomX" {
            & powershell -ExecutionPolicy Bypass -File scripts/build_mainnet_release.ps1 | Out-Host
            if ($LASTEXITCODE -ne 0) {
                throw "scripts/build_mainnet_release.ps1 failed"
            }
        }
    }

    Test-Step "release zoka binary exists" {
        if (!(Test-Path -LiteralPath $ZokaExe -PathType Leaf)) {
            throw "missing release binary; run powershell -ExecutionPolicy Bypass -File scripts/build_mainnet_release.ps1"
        }
    }

    Test-Step "genesis verifies offline" {
        $out = & $ZokaExe genesis-verify docs/genesis.mainnet.v1.json --json 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) {
            throw $out
        }
        $report = $out | ConvertFrom-Json
        if ($report.chain_id -ne "zoka-mainnet-v1") {
            throw "unexpected chain_id $($report.chain_id)"
        }
        if ([string]::IsNullOrWhiteSpace($report.digest_sha256_hex)) {
            throw "missing genesis digest"
        }
        Write-Host "     genesis digest: $($report.digest_sha256_hex)"
    }

    Test-Step "genesis has no placeholder validator keys" {
        $genesis = Get-Content -LiteralPath "docs/genesis.mainnet.v1.json" -Raw | ConvertFrom-Json
        foreach ($validator in $genesis.validators) {
            if (Test-Placeholder $validator.consensus_pubkey_hex) {
                throw "validator '$($validator.label)' consensus_pubkey_hex is placeholder"
            }
            if (Test-Placeholder $validator.operator_pubkey_hex) {
                throw "validator '$($validator.label)' operator_pubkey_hex is placeholder"
            }
        }
    }

    Test-Step "mainnet network lint passes" {
        $oldErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $out = & $ZokaExe mainnet-network-lint docs/mainnet-network.v1.json --json 2>&1 | Out-String
        $code = $LASTEXITCODE
        $ErrorActionPreference = $oldErrorActionPreference
        $firstJsonLine = ($out -split "`r?`n" | Where-Object { $_.Trim().StartsWith("{") } | Select-Object -First 1)
        if ([string]::IsNullOrWhiteSpace($firstJsonLine)) {
            throw $out
        }
        $report = $firstJsonLine | ConvertFrom-Json
        if ($code -ne 0) {
            throw (($report.errors | Select-Object -First 5) -join "; ")
        }
        if (-not $report.ok) {
            throw "network lint returned ok=false"
        }
    }

    Test-Step "GPG command is available" {
        $gpg = Find-Gpg
        if ([string]::IsNullOrWhiteSpace($gpg)) {
            throw "gpg is not installed or not on PATH"
        }
        Write-Host "     gpg: $gpg"
    }

    Test-Step "release verification doc has final GPG fingerprint" {
        $releaseDoc = Get-Content -LiteralPath "docs/release-verification.md" -Raw
        if ($releaseDoc -match "Pendiente:\s*<FFFF" -or $releaseDoc -match "TODO antes de mainnet launch") {
            throw "docs/release-verification.md still has placeholder fingerprint"
        }
    }

    $gitStatus = & git status --porcelain 2>$null
    if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace(($gitStatus | Out-String))) {
        Add-Warning "working tree is dirty; freeze/tag only after source changes are committed"
    }

    Write-Host ""
    Write-Host "Readiness summary"
    Write-Host "-----------------"
    Write-Host "Failures: $($failures.Count)"
    foreach ($failure in $failures) {
        Write-Host "  - $failure"
    }
    Write-Host "Warnings: $($warnings.Count)"
    foreach ($warning in $warnings) {
        Write-Host "  - $warning"
    }

    if ($failures.Count -gt 0) {
        exit 1
    }
    exit 0
}
finally {
    Pop-Location
}
