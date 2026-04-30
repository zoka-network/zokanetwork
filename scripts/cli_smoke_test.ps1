param(
    [string]$RepoRoot = (Resolve-Path ".").Path,
    [string]$ZokaExe = "",
    [string]$ZokahdExe = ""
)

$ErrorActionPreference = "Continue"

if ([string]::IsNullOrWhiteSpace($ZokaExe)) {
    $ZokaExe = Join-Path $RepoRoot "target\debug\zoka.exe"
}
if ([string]::IsNullOrWhiteSpace($ZokahdExe)) {
    $ZokahdExe = Join-Path $RepoRoot "target\debug\zokahd.exe"
}

if (!(Test-Path -LiteralPath $ZokaExe)) {
    throw "zoka binary not found: $ZokaExe. Run cargo build --bins first."
}
if (!(Test-Path -LiteralPath $ZokahdExe)) {
    throw "zokahd binary not found: $ZokahdExe. Run cargo build --bins first."
}

$oldUserProfile = $env:USERPROFILE
$oldHome = $env:HOME
$root = Join-Path $env:TEMP ("zoka-cli-smoke-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $root | Out-Null
$env:USERPROFILE = $root
$env:HOME = $root

$passed = New-Object System.Collections.Generic.List[string]

function Invoke-SmokeStep {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$CmdArgs,
        [int[]]$OkCodes = @(0),
        [string]$Contains = ""
    )

    Write-Host "RUN $Name"
    $out = & $Exe @CmdArgs 2>&1 | Out-String
    $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    $ok = $OkCodes -contains $code
    if (![string]::IsNullOrWhiteSpace($Contains) -and ($out -notmatch [regex]::Escape($Contains))) {
        $ok = $false
    }
    if (!$ok) {
        throw "Step failed: $Name code=$code expected=[$($OkCodes -join ',')] contains='$Contains' output=$out"
    }
    $script:passed.Add($Name) | Out-Null
    return $out
}

Push-Location $root
try {
    Invoke-SmokeStep "zoka --help" $ZokaExe @("--help") @(0) "wallet-create" | Out-Null
    Invoke-SmokeStep "zokahd --help" $ZokahdExe @("--help") @(0) "automatic fullnode" | Out-Null

    $createA = Invoke-SmokeStep "wallet-create smoke-a" $ZokaExe @(
        "wallet-create", "--name", "smoke-a", "--password", "smoke-pass",
        "--export-secrets", (Join-Path $root "smoke-a-secrets.json")
    ) @(0) "Private wallet created"
    $addrA = [regex]::Match($createA, "Address:\s+(\S+)").Groups[1].Value
    $seedA = [regex]::Match($createA, "Recovery phrase:\s+(.+)").Groups[1].Value.Trim()
    $scanA = [regex]::Match($createA, "Scan private key \(hex\):\s+([0-9a-f]+)").Groups[1].Value
    $spendA = [regex]::Match($createA, "Spend private key \(hex\):\s+([0-9a-f]+)").Groups[1].Value
    if ([string]::IsNullOrWhiteSpace($addrA) -or [string]::IsNullOrWhiteSpace($seedA) -or
        [string]::IsNullOrWhiteSpace($scanA) -or [string]::IsNullOrWhiteSpace($spendA)) {
        throw "wallet-create output did not include address, seed and keys"
    }

    $createB = Invoke-SmokeStep "wallet-create smoke-b" $ZokaExe @(
        "wallet-create", "--name", "smoke-b", "--password", "smoke-pass"
    ) @(0) "Private wallet created"
    $addrB = [regex]::Match($createB, "Address:\s+(\S+)").Groups[1].Value

    Invoke-SmokeStep "wallet-list --json" $ZokaExe @("wallet-list", "--password", "smoke-pass", "--json") @(0) "private_wallets" | Out-Null
    Invoke-SmokeStep "wallet-balance --json" $ZokaExe @("wallet-balance", "--address", $addrA, "--password", "smoke-pass", "--json") @(0) "balance" | Out-Null
    Invoke-SmokeStep "wallet-history --json" $ZokaExe @("wallet-history", "--address", $addrA, "--password", "smoke-pass", "--json") @(0) "transactions" | Out-Null
    Invoke-SmokeStep "wallet-export-secrets" $ZokaExe @("wallet-export-secrets", "--address", $addrA, "--password", "smoke-pass", "--out", (Join-Path $root "export-a.json")) @(0) "Secrets exported" | Out-Null
    Invoke-SmokeStep "wallet-backup" $ZokaExe @("wallet-backup", "--address", $addrA, "--password", "smoke-pass", "--out", (Join-Path $root "backup-a.private.json")) @(0) "Backup created" | Out-Null
    Invoke-SmokeStep "wallet-restore-from-seed" $ZokaExe @("wallet-restore-from-seed", "--name", "smoke-restored", "--seed", $seedA, "--password", "restore-pass", "--restore-height", "7") @(0) "Private wallet restored from seed" | Out-Null
    Invoke-SmokeStep "wallet-restore-from-keys" $ZokaExe @("wallet-restore-from-keys", "--name", "smoke-keys", "--scan-key-hex", $scanA, "--spend-key-hex", $spendA, "--password", "keys-pass", "--restore-height", "9") @(0) "Private wallet restored from keys" | Out-Null
    Invoke-SmokeStep "wallet-change-passphrase" $ZokaExe @("wallet-change-passphrase", "--address", $addrA, "--old-password", "smoke-pass", "--new-password", "smoke-new") @(0) "Wallet passphrase changed" | Out-Null
    Invoke-SmokeStep "wallet-balance after passphrase change" $ZokaExe @("wallet-balance", "--address", $addrA, "--password", "smoke-new", "--json") @(0) "balance" | Out-Null
    Invoke-SmokeStep "estimate-fee" $ZokaExe @("estimate-fee", "--amount", "1", "--priority", "normal", "--password", "smoke-new") @(0) "Estimated fee" | Out-Null
    Invoke-SmokeStep "tx-send no-funds expected rejection" $ZokaExe @("tx-send", "--from", $addrA, "--to", $addrB, "--amount", "1", "--password", "smoke-new", "--yes") @(1) "insufficient" | Out-Null
    Invoke-SmokeStep "height empty db" $ZokaExe @("height", "--db", (Join-Path $root "chain-a.db"), "--json") @(0) "height" | Out-Null
    Invoke-SmokeStep "chain-stats empty db" $ZokaExe @("chain-stats", "--db", (Join-Path $root "chain-a.db"), "--json") @(0) "total_supply_emitted" | Out-Null
    Invoke-SmokeStep "chain-prune dry-run empty db" $ZokaExe @("chain-prune", "--keep-last", "8", "--db", (Join-Path $root "chain-a.db"), "--dry-run", "--json") @(0) "blocks_pruned" | Out-Null
    Invoke-SmokeStep "sync-status" $ZokaExe @("sync-status", "--address", $addrA, "--password", "smoke-new", "--db", (Join-Path $root "chain-a.db"), "--json") @(0) "chain_height" | Out-Null
    Invoke-SmokeStep "genesis-verify" $ZokaExe @("genesis-verify", (Join-Path $RepoRoot "docs\genesis.mainnet.v1.json"), "--json") @(0) "digest_sha256_hex" | Out-Null
    Invoke-SmokeStep "mainnet-network-lint" $ZokaExe @("mainnet-network-lint", (Join-Path $RepoRoot "docs\mainnet-network.v1.json")) @(0) "launch-ready" | Out-Null
    Invoke-SmokeStep "release-keygen" $ZokaExe @("release-keygen", "--label", "smoke-release-key", "--out", (Join-Path $root "release-key.json"), "--json") @(0) "public_key_hex" | Out-Null
    Invoke-SmokeStep "snapshot-export" $ZokaExe @("snapshot-export", (Join-Path $root "snapshot.json"), "--db", (Join-Path $root "chain-a.db")) @(0) "zoka-snapshot-v1" | Out-Null
    Invoke-SmokeStep "snapshot-info" $ZokaExe @("snapshot-info", (Join-Path $root "snapshot.json"), "--json") @(0) "zoka-snapshot-v1" | Out-Null
    Invoke-SmokeStep "snapshot-import" $ZokaExe @("snapshot-import", (Join-Path $root "snapshot.json"), "--db", (Join-Path $root "chain-b.db")) @(0) "Blocks applied" | Out-Null
    Invoke-SmokeStep "mine-status --json" $ZokaExe @("mine-status", "--json") @(0) "running" | Out-Null
    Invoke-SmokeStep "mine-console --json" $ZokaExe @("mine-console", "--address", $addrA, "--password", "smoke-new", "--json") @(0) "hashrate_hps" | Out-Null
    Invoke-SmokeStep "mine expected RandomX gate" $ZokaExe @("mine", "--address", $addrA, "--password", "smoke-new", "--blocks", "1") @(1) "not compiled into this binary" | Out-Null
    Invoke-SmokeStep "fullnode no-mine" $ZokaExe @("fullnode", "--address", $addrA, "--password", "smoke-new", "--no-network-config", "--no-mine", "--seconds", "1", "--db", (Join-Path $root "fullnode.db")) @(0) "Fullnode session time limit reached" | Out-Null
    Invoke-SmokeStep "zokahd inline no-mine" $ZokahdExe @("--inline", "--address", $addrA, "--password", "smoke-new", "--no-network-config", "--no-mine", "--seconds", "1", "--db", (Join-Path $root "zokahd.db")) @(0) "Fullnode session time limit reached" | Out-Null
    Invoke-SmokeStep "wallet-wipe" $ZokaExe @("wallet-wipe", "--address", "smoke-keys", "--password", "keys-pass", "--force") @(0) "Wallet file wiped" | Out-Null

    Write-Host "CLI smoke passed: $($passed.Count) steps"
    $passed | ForEach-Object { Write-Host "  OK $_" }
}
finally {
    Pop-Location
    $env:USERPROFILE = $oldUserProfile
    $env:HOME = $oldHome
    if ($root -like (Join-Path $env:TEMP "zoka-cli-smoke-*")) {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    }
}
