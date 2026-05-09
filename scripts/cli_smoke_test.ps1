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

function Invoke-SmokeStdinStep {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$InputLines,
        [int[]]$OkCodes = @(0),
        [string]$Contains = ""
    )

    Write-Host "RUN $Name"
    $out = $InputLines | & $Exe 2>&1 | Out-String
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
    Invoke-SmokeStep "zoka --help" $ZokaExe @("--help") @(0) "ZOKA private console" | Out-Null
    Invoke-SmokeStep "zokahd --help" $ZokahdExe @("--help") @(0) "automatic fullnode" | Out-Null

    $createShort = Invoke-SmokeStep "create short-a alias" $ZokaExe @(
        "create", "short-a", "--password", "short-pass"
    ) @(0) "Private wallet created"
    $addrShort = [regex]::Match($createShort, "Address:\s+(\S+)").Groups[1].Value
    if ([string]::IsNullOrWhiteSpace($addrShort)) {
        throw "create alias output did not include address"
    }
    $oldAutopilotNoMine = $env:ZOKA_ZOKAHD_AUTOPILOT_NO_MINE
    $oldAutopilotNoNetwork = $env:ZOKA_ZOKAHD_AUTOPILOT_NO_NETWORK_CONFIG
    $oldAutopilotSeconds = $env:ZOKA_ZOKAHD_AUTOPILOT_SECONDS
    try {
        $env:ZOKA_ZOKAHD_AUTOPILOT_NO_MINE = "1"
        $env:ZOKA_ZOKAHD_AUTOPILOT_NO_NETWORK_CONFIG = "1"
        $env:ZOKA_ZOKAHD_AUTOPILOT_SECONDS = "1"
        Invoke-SmokeStdinStep "zokahd autopilot no-mine" $ZokahdExe @(
            "short-pass"
        ) @(0) "Fullnode session time limit reached" | Out-Null
    }
    finally {
        $env:ZOKA_ZOKAHD_AUTOPILOT_NO_MINE = $oldAutopilotNoMine
        $env:ZOKA_ZOKAHD_AUTOPILOT_NO_NETWORK_CONFIG = $oldAutopilotNoNetwork
        $env:ZOKA_ZOKAHD_AUTOPILOT_SECONDS = $oldAutopilotSeconds
    }
    Invoke-SmokeStep "wallets alias" $ZokaExe @("wallets", "--password", "short-pass") @(0) "short-a" | Out-Null
    Invoke-SmokeStep "balance alias" $ZokaExe @("balance", "short-a", "--password", "short-pass") @(0) "Available balance" | Out-Null
    Invoke-SmokeStep "receive alias" $ZokaExe @("receive", "short-a", "--password", "short-pass") @(0) "Receive ZKA" | Out-Null
    Invoke-SmokeStep "history alias" $ZokaExe @("history", "short-a", "--password", "short-pass") @(0) "No private transactions" | Out-Null
    Invoke-SmokeStep "fee alias" $ZokaExe @("fee", "1") @(0) "Estimated fee" | Out-Null
    Invoke-SmokeStep "pending alias" $ZokaExe @("pending", "--db", (Join-Path $root "chain-a.db")) @(0) "No pending private transfers" | Out-Null
    Invoke-SmokeStep "sync alias" $ZokaExe @("sync", "short-a", "--password", "short-pass", "--json") @(0) "progress_percent" | Out-Null
    Invoke-SmokeStep "status alias" $ZokaExe @("status", "--address", "short-a", "--password", "short-pass", "--json") @(0) "zoka-status-dashboard-v1" | Out-Null
    Invoke-SmokeStep "rpc-list" $ZokaExe @("rpc-list", "--network-config", (Join-Path $RepoRoot "docs\mainnet-network.v1.json"), "--json") @(0) "active_rpc" | Out-Null
    Invoke-SmokeStep "backup alias" $ZokaExe @("backup", "short-a", (Join-Path $root "short-a.private.json"), "--password", "short-pass") @(0) "Backup created" | Out-Null

    $secretsAPath = Join-Path $root "smoke-a-secrets.json"
    $createA = Invoke-SmokeStep "wallet-create smoke-a" $ZokaExe @(
        "wallet-create", "--name", "smoke-a", "--password", "smoke-pass",
        "--export-secrets", $secretsAPath
    ) @(0) "Private wallet created"
    $addrA = [regex]::Match($createA, "Address:\s+(\S+)").Groups[1].Value
    $seedA = [regex]::Match($createA, "Recovery phrase:\s+(.+)").Groups[1].Value.Trim()
    $secretsA = Get-Content -LiteralPath $secretsAPath -Raw | ConvertFrom-Json
    $scanA = $secretsA.scan_private_key_hex
    $spendA = $secretsA.spend_private_key_hex
    if ([string]::IsNullOrWhiteSpace($addrA) -or [string]::IsNullOrWhiteSpace($seedA) -or
        [string]::IsNullOrWhiteSpace($scanA) -or [string]::IsNullOrWhiteSpace($spendA)) {
        throw "wallet-create output/export did not include address, seed and keys"
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
    Invoke-SmokeStep "estimate-fee fractional explicit" $ZokaExe @("estimate-fee", "--amount", "1", "--fee", "0.0001") @(0) "0.0001 ZKA" | Out-Null
    Invoke-SmokeStep "send alias no-funds expected rejection" $ZokaExe @("send", "short-a", $addrB, "1", "--password", "short-pass", "--yes") @(1) "not enough spendable" | Out-Null
    Invoke-SmokeStep "tx-send no-funds expected rejection" $ZokaExe @("tx-send", "--from", $addrA, "--to", $addrB, "--amount", "1", "--password", "smoke-new", "--yes") @(1) "not enough spendable" | Out-Null
    Invoke-SmokeStep "height empty db" $ZokaExe @("height", "--db", (Join-Path $root "chain-a.db"), "--json") @(0) "height" | Out-Null
    Invoke-SmokeStep "chain-stats empty db" $ZokaExe @("chain-stats", "--db", (Join-Path $root "chain-a.db"), "--json") @(0) "total_supply_emitted" | Out-Null
    Invoke-SmokeStep "node-status --json" $ZokaExe @("node-status", "--db", (Join-Path $root "chain-a.db"), "--address", $addrA, "--password", "smoke-new", "--json") @(0) "available_cpu_threads" | Out-Null
    Invoke-SmokeStep "chain-prune dry-run empty db" $ZokaExe @("chain-prune", "--keep-last", "8", "--db", (Join-Path $root "chain-a.db"), "--dry-run", "--json") @(0) "blocks_pruned" | Out-Null
    Invoke-SmokeStep "sync-status" $ZokaExe @("sync-status", "--address", $addrA, "--password", "smoke-new", "--db", (Join-Path $root "chain-a.db"), "--json") @(0) "chain_height" | Out-Null
    Invoke-SmokeStep "genesis-verify" $ZokaExe @("genesis-verify", (Join-Path $RepoRoot "docs\genesis.mainnet.v1.json"), "--json") @(0) "digest_sha256_hex" | Out-Null
    Invoke-SmokeStep "mainnet-network-lint" $ZokaExe @("mainnet-network-lint", (Join-Path $RepoRoot "docs\mainnet-network.v1.json")) @(0) "launch-ready" | Out-Null
    Invoke-SmokeStep "release-keygen" $ZokaExe @("release-keygen", "--label", "smoke-release-key", "--out", (Join-Path $root "release-key.json"), "--json") @(0) "public_key_hex" | Out-Null
    Invoke-SmokeStep "snapshot-export" $ZokaExe @("snapshot-export", (Join-Path $root "snapshot.json"), "--db", (Join-Path $root "chain-a.db")) @(0) "zoka-snapshot-v1" | Out-Null
    Invoke-SmokeStep "snapshot-info" $ZokaExe @("snapshot-info", (Join-Path $root "snapshot.json"), "--json") @(0) "zoka-snapshot-v1" | Out-Null
    Invoke-SmokeStep "snapshot-import" $ZokaExe @("snapshot-import", (Join-Path $root "snapshot.json"), "--db", (Join-Path $root "chain-b.db")) @(0) "Blocks applied" | Out-Null
    Invoke-SmokeStep "mine-status --json" $ZokaExe @("mine-status", "--json") @(0) "available_cpu_threads" | Out-Null
    Invoke-SmokeStep "mine-console --json" $ZokaExe @("mine-console", "--address", $addrA, "--password", "smoke-new", "--json") @(0) "available_cpu_threads" | Out-Null
    Invoke-SmokeStep "mine expected RandomX gate" $ZokaExe @("mine", "--address", $addrA, "--password", "smoke-new", "--blocks", "1", "--power", "low") @(1) "not compiled into this binary" | Out-Null
    Invoke-SmokeStep "node alias no-mine" $ZokaExe @("node", "--address", $addrA, "--password", "smoke-new", "--no-network-config", "--no-mine", "--seconds", "1", "--db", (Join-Path $root "node.db"), "--power", "low") @(0) "Fullnode session time limit reached" | Out-Null
    Invoke-SmokeStep "fullnode no-mine" $ZokaExe @("fullnode", "--address", $addrA, "--password", "smoke-new", "--no-network-config", "--no-mine", "--seconds", "1", "--db", (Join-Path $root "fullnode.db")) @(0) "Fullnode session time limit reached" | Out-Null
    Invoke-SmokeStep "zokahd inline no-mine" $ZokahdExe @("--inline", "--address", $addrA, "--password", "smoke-new", "--no-network-config", "--no-mine", "--seconds", "1", "--db", (Join-Path $root "zokahd.db")) @(0) "Fullnode session time limit reached" | Out-Null
    Invoke-SmokeStep "wallet-wipe" $ZokaExe @("wallet-wipe", "--address", "smoke-keys", "--password", "keys-pass", "--force") @(0) "Local wallet deleted" | Out-Null

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
