param(
    [string]$RepoRoot = (Resolve-Path ".").Path,
    [string]$ZokaExe = "",
    [string]$OutDir = "",
    [int]$BasePort = 33300,
    [int]$SpendReadyHeight = 64,
    [int]$MineBlocksPerBatch = 32,
    [int]$MineIntervalSeconds = 0,
    [int]$UnsafeRehearsalDifficultyBits = 1,
    [int]$NodeSeconds = 1200,
    [int]$WaitTimeoutSeconds = 420,
    [string]$TransferAmount = "1",
    [string]$TransferFee = "0",
    [string]$WalletPassword = "wallet-rehearsal-pass"
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ZokaExe)) {
    $ZokaExe = Join-Path $RepoRoot "target\release\zoka.exe"
}
if (!(Test-Path -LiteralPath $ZokaExe -PathType Leaf)) {
    throw "zoka binary not found: $ZokaExe. Build with cargo build --release --features randomx --bin zoka --bin zokahd"
}
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $RepoRoot ".zoka\rehearsal\wallet-network-$stamp"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Remove-Ansi {
    param([string]$Text)
    return (($Text -replace "\x1b\[[0-9;]*[A-Za-z]", "") -replace "\x1b\][^\x07]*\x07", "")
}

function Convert-ZkaToAtoms {
    param([string]$Amount)

    $text = "$Amount".Trim()
    if ($text -notmatch '^[0-9]+(\.[0-9]{1,6})?$') {
        throw "invalid ZKA amount '$Amount'; expected up to 6 decimal places"
    }
    $parts = $text.Split('.', 2)
    $whole = [int64]$parts[0]
    $frac = 0
    if ($parts.Count -eq 2) {
        $fracText = $parts[1].PadRight(6, '0')
        $frac = [int64]$fracText
    }
    return ($whole * 1000000) + $frac
}

function Invoke-ZokaProfile {
    param(
        [string]$Profile,
        [string[]]$CmdArgs,
        [int[]]$OkCodes = @(0),
        [string]$Contains = ""
    )

    New-Item -ItemType Directory -Force -Path $Profile | Out-Null
    $oldUserProfile = $env:USERPROFILE
    $oldHome = $env:HOME
    $oldSeededSetup = $env:ZOKA_ALLOW_INSECURE_SEEDED_SETUP
    try {
        $env:USERPROFILE = $Profile
        $env:HOME = $Profile
        $env:ZOKA_ALLOW_INSECURE_SEEDED_SETUP = "1"
        $out = & $ZokaExe @CmdArgs 2>&1 | Out-String
        $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
        $ok = $OkCodes -contains $code
        if (![string]::IsNullOrWhiteSpace($Contains) -and ($out -notmatch [regex]::Escape($Contains))) {
            $ok = $false
        }
        if (!$ok) {
            throw "command failed code=$code expected=[$($OkCodes -join ',')] args='$($CmdArgs -join ' ')' output=$out"
        }
        return $out
    }
    finally {
        $env:USERPROFILE = $oldUserProfile
        $env:HOME = $oldHome
        $env:ZOKA_ALLOW_INSECURE_SEEDED_SETUP = $oldSeededSetup
    }
}

function Invoke-ZokaProfileJson {
    param(
        [string]$Profile,
        [string[]]$CmdArgs
    )

    $out = Invoke-ZokaProfile -Profile $Profile -CmdArgs $CmdArgs
    return ($out | ConvertFrom-Json)
}

function Wait-ForPattern {
    param(
        [string]$Path,
        [string]$Pattern,
        [int]$TimeoutSeconds = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $Path -PathType Leaf) {
            $text = Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue
            if ($text -match $Pattern) {
                return $Matches[1]
            }
        }
        Start-Sleep -Milliseconds 250
    }
    throw "timeout waiting for pattern '$Pattern' in $Path"
}

function Wait-ForReadyFile {
    param(
        $Node,
        [int]$TimeoutSeconds = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $Node.ReadyFile -PathType Leaf) {
            try {
                $ready = Get-Content -LiteralPath $Node.ReadyFile -Raw | ConvertFrom-Json
                if (![string]::IsNullOrWhiteSpace($ready.listen_address) -and
                    ![string]::IsNullOrWhiteSpace($ready.peer_id)) {
                    return $ready
                }
            } catch {
            }
        }
        if ($null -ne $Node.Process -and $Node.Process.HasExited) {
            $stderr = ""
            if (Test-Path -LiteralPath $Node.ErrorLog -PathType Leaf) {
                $stderr = Get-Content -LiteralPath $Node.ErrorLog -Raw -ErrorAction SilentlyContinue
            }
            throw "node $($Node.Name) exited before ready file was written: $stderr"
        }
        Start-Sleep -Milliseconds 250
    }
    throw "timeout waiting for node ready file $($Node.ReadyFile)"
}

function Get-NodeDir {
    param([int]$Index)
    return (Join-Path $OutDir "node-$Index")
}

function Get-NodeDb {
    param([int]$Index)
    return (Join-Path (Get-NodeDir -Index $Index) "blockchain.db")
}

function Get-NodeListen {
    param([int]$Index)
    $port = $BasePort + $Index
    return "/ip4/127.0.0.1/tcp/$port"
}

function Get-NodeHeight {
    param([string]$Db)

    try {
        $heightJson = & $ZokaExe height --db $Db --json 2>&1 | Out-String
    } catch {
        return $null
    }
    if ($LASTEXITCODE -ne 0) {
        return $null
    }
    try {
        return [int](($heightJson | ConvertFrom-Json).height)
    } catch {
        return $null
    }
}

function Get-NodeHeightFromLog {
    param([string]$Path)

    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    $text = Remove-Ansi -Text (Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue)
    $matches = [regex]::Matches($text, "Synced height\s+([0-9]+)")
    if ($matches.Count -eq 0) {
        return $null
    }
    $heights = foreach ($match in $matches) {
        [int]$match.Groups[1].Value
    }
    return ($heights | Measure-Object -Maximum).Maximum
}

function Get-ObservedNodeHeight {
    param($Node)

    $logHeight = Get-NodeHeightFromLog -Path $Node.Log
    $dbHeight = Get-NodeHeight -Db $Node.Db
    if ($null -ne $logHeight -and $null -ne $dbHeight) {
        return [Math]::Max($logHeight, $dbHeight)
    }
    if ($null -ne $logHeight) {
        return $logHeight
    }
    if ($null -ne $dbHeight) {
        return $dbHeight
    }
    return $null
}

function Get-NodeHeadHash {
    param([string]$Db)

    $height = Get-NodeHeight -Db $Db
    if ($null -eq $height -or $height -le 0) {
        return ""
    }
    try {
        $infoJson = & $ZokaExe block-info --height $height --db $Db --json 2>&1 | Out-String
    } catch {
        return ""
    }
    if ($LASTEXITCODE -ne 0) {
        return ""
    }
    try {
        return (($infoJson | ConvertFrom-Json).hash)
    } catch {
        return ""
    }
}

function Get-NodePeerIdFromLog {
    param([string]$Path)
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    $text = Remove-Ansi -Text (Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue)
    if ($text -match 'Peer ID\s+([1-9A-HJ-NP-Za-km-z]+)') {
        return $Matches[1]
    }
    return $null
}

function Get-NodeSeedAddress {
    param($Node)

    if (Test-Path -LiteralPath $Node.ReadyFile -PathType Leaf) {
        try {
            $ready = Get-Content -LiteralPath $Node.ReadyFile -Raw | ConvertFrom-Json
            if (![string]::IsNullOrWhiteSpace($ready.seed_address)) {
                return $ready.seed_address
            }
        } catch {
        }
    }

    $listen = $Node.Listen
    if ([string]::IsNullOrWhiteSpace($listen) -or $listen -match '/p2p/') {
        return $listen
    }
    $peerId = Get-NodePeerIdFromLog -Path $Node.Log
    if (![string]::IsNullOrWhiteSpace($peerId)) {
        return "$listen/p2p/$peerId"
    }
    return $listen
}

function Stop-Node {
    param($Node)
    if ($null -ne $Node -and $null -ne $Node.Process -and !$Node.Process.HasExited) {
        Stop-Process -Id $Node.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 750
    }
}

function Stop-LiveNodes {
    param([hashtable]$LiveNodes)
    foreach ($key in @($LiveNodes.Keys)) {
        Stop-Node $LiveNodes[$key]
    }
    $LiveNodes.Clear()
    Start-Sleep -Seconds 2
}

function Start-WalletNode {
    param(
        [int]$Index,
        [string]$Phase,
        [string]$Profile,
        [string]$WalletName,
        [switch]$NoMine,
        [string[]]$Seeds = @(),
        [int]$MineBlocks = $MineBlocksPerBatch,
        [int]$MinMinePeers = 0,
        [int]$SyncWarmupSeconds = 8,
        [int]$RebroadcastRecentBlocks = 64
    )

    $nodeDir = Get-NodeDir -Index $Index
    New-Item -ItemType Directory -Force -Path $nodeDir | Out-Null
    New-Item -ItemType Directory -Force -Path $Profile | Out-Null

    $log = Join-Path $nodeDir "$Phase.node.log"
    $err = Join-Path $nodeDir "$Phase.node.err.log"
    $ready = Join-Path $nodeDir "$Phase.ready.json"
    $db = Get-NodeDb -Index $Index
    $listen = Get-NodeListen -Index $Index
    $args = @(
        "fullnode",
        "--address", $WalletName,
        "--password", $WalletPassword,
        "--no-network-config",
        "--listen", $listen,
        "--db", $db,
        "--genesis", (Join-Path $RepoRoot "docs\genesis.mainnet.v1.json"),
        "--ready-file", $ready,
        "--seconds", "$NodeSeconds",
        "--sync-warmup-secs", "$SyncWarmupSeconds",
        "--sync-rebroadcast-recent-blocks", "$RebroadcastRecentBlocks",
        "--unsafe-rehearsal-difficulty-bits", "$UnsafeRehearsalDifficultyBits"
    )
    if ($NoMine) {
        $args += "--no-mine"
    } else {
        $args += @(
            "--mine-blocks", "$MineBlocks",
            "--mine-interval-secs", "$MineIntervalSeconds",
            "--min-mine-peers", "$MinMinePeers"
        )
    }
    if ($Seeds.Count -gt 0) {
        $args += "--allow-isolated-start"
    }
    foreach ($seed in $Seeds) {
        if (![string]::IsNullOrWhiteSpace($seed)) {
            $args += @("--seed", $seed)
        }
    }

    $oldUserProfile = $env:USERPROFILE
    $oldHome = $env:HOME
    $oldAllowUnsafe = $env:ZOKA_ALLOW_UNSAFE_REHEARSAL
    $oldSeededSetup = $env:ZOKA_ALLOW_INSECURE_SEEDED_SETUP
    try {
        $env:USERPROFILE = $Profile
        $env:HOME = $Profile
        $env:ZOKA_ALLOW_UNSAFE_REHEARSAL = "1"
        $env:ZOKA_ALLOW_INSECURE_SEEDED_SETUP = "1"
        $process = Start-Process -FilePath $ZokaExe `
            -ArgumentList $args `
            -WorkingDirectory $RepoRoot `
            -RedirectStandardOutput $log `
            -RedirectStandardError $err `
            -PassThru `
            -WindowStyle Hidden
    }
    finally {
        $env:USERPROFILE = $oldUserProfile
        $env:HOME = $oldHome
        $env:ZOKA_ALLOW_UNSAFE_REHEARSAL = $oldAllowUnsafe
        $env:ZOKA_ALLOW_INSECURE_SEEDED_SETUP = $oldSeededSetup
    }

    [pscustomobject]@{
        Name = "node-$Index"
        Index = $Index
        Phase = $Phase
        Process = $process
        Dir = $nodeDir
        Log = $log
        ErrorLog = $err
        ReadyFile = $ready
        Db = $db
        Home = $Profile
        Listen = $listen
        Wallet = $WalletName
    }
}

function Wait-ForHeight {
    param(
        $Node,
        [int]$Height,
        [int]$TimeoutSeconds
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $current = Get-ObservedNodeHeight -Node $Node
        if ($null -ne $current -and $current -ge $Height) {
            return $current
        }
        if ($Node.Process.HasExited) {
            throw "node $($Node.Name) exited before reaching height $Height"
        }
        Start-Sleep -Seconds 2
    }
    throw "timeout waiting for node $($Node.Name) to reach height $Height"
}

function Get-BlockInfo {
    param(
        [string]$Db,
        [int]$Height
    )

    $json = & $ZokaExe block-info --height $Height --db $Db --json 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "block-info failed for height $Height on $Db output=$json"
    }
    return ($json | ConvertFrom-Json)
}

$minerProfile = Join-Path $OutDir "miner-home"
$verifierProfile = Join-Path $OutDir "verifier-home"
$live = @{}
$summary = [ordered]@{
    schema = "zoka-local-wallet-network-rehearsal-v1"
    started_at = (Get-Date).ToString("o")
    out_dir = $OutDir
    spend_ready_height = $SpendReadyHeight
    transfer_amount = $TransferAmount
    transfer_fee = $TransferFee
    trusted_setup = "dev-only seeded setup allowed via ZOKA_ALLOW_INSECURE_SEEDED_SETUP=1"
    wallets = [ordered]@{}
    events = @()
    node_results = @()
    transfer = [ordered]@{}
    inclusion = [ordered]@{}
}

try {
    Write-Host "Creating isolated miner and receiver wallets..."
    $minerCreate = Invoke-ZokaProfile -Profile $minerProfile -CmdArgs @(
        "wallet-create", "--name", "miner", "--password", $WalletPassword,
        "--export-secrets", (Join-Path $OutDir "miner-secrets.json")
    ) -Contains "Private wallet created"
    $receiverCreate = Invoke-ZokaProfile -Profile $minerProfile -CmdArgs @(
        "wallet-create", "--name", "receiver", "--password", $WalletPassword,
        "--export-secrets", (Join-Path $OutDir "receiver-secrets.json")
    ) -Contains "Private wallet created"
    Invoke-ZokaProfile -Profile $verifierProfile -CmdArgs @(
        "wallet-create", "--name", "verifier", "--password", $WalletPassword
    ) -Contains "Private wallet created" | Out-Null

    $minerAddress = [regex]::Match($minerCreate, "Address:\s+(\S+)").Groups[1].Value
    $receiverAddress = [regex]::Match($receiverCreate, "Address:\s+(\S+)").Groups[1].Value
    if ([string]::IsNullOrWhiteSpace($minerAddress) -or [string]::IsNullOrWhiteSpace($receiverAddress)) {
        throw "wallet-create output did not expose expected addresses"
    }
    $summary["wallets"]["miner"] = [ordered]@{ name = "miner"; address = $minerAddress; profile = $minerProfile }
    $summary["wallets"]["receiver"] = [ordered]@{ name = "receiver"; address = $receiverAddress; profile = $minerProfile }

    Write-Host "Starting miner node..."
    $node1 = Start-WalletNode -Index 1 -Phase "mine-to-maturity" -Profile $minerProfile -WalletName "miner" -MineBlocks $MineBlocksPerBatch -MinMinePeers 0 -SyncWarmupSeconds 3
    $live[1] = $node1
    Wait-ForReadyFile -Node $node1 -TimeoutSeconds 30 | Out-Null
    $summary["events"] += [pscustomobject]@{ event = "miner_started"; at = (Get-Date).ToString("o") }

    Write-Host "Mining until rewards are spendable at height $SpendReadyHeight..."
    $node1Height = Wait-ForHeight -Node $node1 -Height $SpendReadyHeight -TimeoutSeconds $WaitTimeoutSeconds
    $summary["events"] += [pscustomobject]@{ event = "spend_ready_height_reached"; at = (Get-Date).ToString("o"); node1_height = $node1Height }

    Stop-LiveNodes -LiveNodes $live
    $seedHeight = Get-NodeHeight -Db $node1.Db
    if ($null -eq $seedHeight -or $seedHeight -lt $SpendReadyHeight) {
        throw "miner DB did not persist spend-ready height: observed=$seedHeight target=$SpendReadyHeight"
    }

    Write-Host "Restarting miner as no-mine seed and syncing verifier to height $seedHeight..."
    $node1Seed = Start-WalletNode -Index 1 -Phase "seed-after-maturity" -Profile $minerProfile -WalletName "miner" -NoMine -SyncWarmupSeconds 3
    $live[1] = $node1Seed
    Wait-ForReadyFile -Node $node1Seed -TimeoutSeconds 30 | Out-Null
    $seedAddr = Get-NodeSeedAddress -Node $node1Seed
    $node2 = Start-WalletNode -Index 2 -Phase "verifier-sync" -Profile $verifierProfile -WalletName "verifier" -NoMine -Seeds @($seedAddr) -SyncWarmupSeconds 12
    $live[2] = $node2
    $summary["events"] += [pscustomobject]@{ event = "verifier_started"; at = (Get-Date).ToString("o"); seed = $seedAddr }
    $node2Height = Wait-ForHeight -Node $node2 -Height $seedHeight -TimeoutSeconds $WaitTimeoutSeconds
    $summary["events"] += [pscustomobject]@{ event = "verifier_synced_to_spend_ready_chain"; at = (Get-Date).ToString("o"); node1_height = $seedHeight; node2_height = $node2Height }
    Stop-LiveNodes -LiveNodes $live

    $minerHead = Get-NodeHeadHash -Db $node1.Db
    $verifierHead = Get-NodeHeadHash -Db $node2.Db
    if ([string]::IsNullOrWhiteSpace($minerHead) -or $minerHead -ne $verifierHead) {
        throw "nodes did not settle to the same head before wallet send: node1=$minerHead node2=$verifierHead"
    }

    Write-Host "Checking matured miner balance and sending private transfer..."
    $transferAmountAtoms = Convert-ZkaToAtoms -Amount $TransferAmount
    $transferFeeAtoms = Convert-ZkaToAtoms -Amount $TransferFee
    $requiredAtoms = $transferAmountAtoms + $transferFeeAtoms
    $minerBefore = Invoke-ZokaProfileJson -Profile $minerProfile -CmdArgs @(
        "wallet-balance", "--address", "miner", "--password", $WalletPassword, "--json"
    )
    if ([int64]$minerBefore.balance_atoms -lt $requiredAtoms) {
        throw "miner balance is not spendable yet: balance_atoms=$($minerBefore.balance_atoms), required_atoms=$requiredAtoms"
    }

    $sendOut = Invoke-ZokaProfile -Profile $minerProfile -CmdArgs @(
        "tx-send",
        "--from", "miner",
        "--to", "receiver",
        "--amount", "$TransferAmount",
        "--fee", "$TransferFee",
        "--password", $WalletPassword,
        "--db", $node1.Db,
        "--yes"
    ) -Contains "Private transfer confirmed"
    $txHash = [regex]::Match($sendOut, "Hash:\s+(\S+)").Groups[1].Value
    if ([string]::IsNullOrWhiteSpace($txHash)) {
        throw "tx-send output did not include a private tx hash"
    }
    $audit = Invoke-ZokaProfileJson -Profile $minerProfile -CmdArgs @(
        "private-tx-audit",
        "--db", $node1.Db,
        "--hash", $txHash,
        "--json"
    )
    if (!$audit.audit_passed -or
        $audit.reveals_recipient_commitment -or
        $audit.reveals_output_blind -or
        $audit.reveals_change_output -or
        $audit.reveals_clear_fee -or
        $audit.reveals_fee_opening -or
        $audit.legacy_public_amount_stream) {
        throw "private tx privacy audit failed: $($audit | ConvertTo-Json -Compress)"
    }
    if ($transferFeeAtoms -gt 0 -and (!$audit.private_fee_commitment -or $audit.zero_fee)) {
        throw "private fee audit failed: $($audit | ConvertTo-Json -Compress)"
    }
    if ($transferFeeAtoms -eq 0 -and (!$audit.zero_fee -or $audit.private_fee_commitment)) {
        throw "zero fee audit failed: $($audit | ConvertTo-Json -Compress)"
    }

    $minerAfter = Invoke-ZokaProfileJson -Profile $minerProfile -CmdArgs @(
        "wallet-balance", "--address", "miner", "--password", $WalletPassword, "--json"
    )
    $receiverAfter = Invoke-ZokaProfileJson -Profile $minerProfile -CmdArgs @(
        "wallet-balance", "--address", "receiver", "--password", $WalletPassword, "--json"
    )
    if ([int64]$receiverAfter.balance_atoms -lt $transferAmountAtoms) {
        throw "receiver did not decrypt the private transfer: receiver_balance_atoms=$($receiverAfter.balance_atoms), expected_atoms=$transferAmountAtoms"
    }
    $summary["transfer"] = [ordered]@{
        tx_hash = $txHash
        privacy_audit = $audit
        miner_before = $minerBefore
        miner_after = $minerAfter
        receiver_after = $receiverAfter
    }

    $commitHeight = $seedHeight + 1
    Write-Host "Restarting local network to mine private tx hash into block $commitHeight..."
    $node1Commit = Start-WalletNode -Index 1 -Phase "commit-private-tx" -Profile $minerProfile -WalletName "miner" -MineBlocks 1 -MinMinePeers 0 -SyncWarmupSeconds 3
    $live[1] = $node1Commit
    Wait-ForReadyFile -Node $node1Commit -TimeoutSeconds 30 | Out-Null
    $commitSeed = Get-NodeSeedAddress -Node $node1Commit
    $node2Commit = Start-WalletNode -Index 2 -Phase "sync-private-tx" -Profile $verifierProfile -WalletName "verifier" -NoMine -Seeds @($commitSeed) -SyncWarmupSeconds 12
    $live[2] = $node2Commit

    $node1CommitHeight = Wait-ForHeight -Node $node1Commit -Height $commitHeight -TimeoutSeconds $WaitTimeoutSeconds
    $node2CommitHeight = Wait-ForHeight -Node $node2Commit -Height $commitHeight -TimeoutSeconds $WaitTimeoutSeconds
    Stop-LiveNodes -LiveNodes $live

    $commitInfoMiner = Get-BlockInfo -Db $node1Commit.Db -Height $commitHeight
    $commitInfoVerifier = Get-BlockInfo -Db $node2Commit.Db -Height $commitHeight
    $included = @($commitInfoMiner.private_tx_hashes) -contains $txHash
    if (!$included) {
        throw "private tx hash was not committed at height $commitHeight"
    }
    if ($transferFeeAtoms -gt 0 -and [int]$commitInfoMiner.private_fee_claim_count -lt 1) {
        throw "private fee was not claimed in the mined block: fee_atoms=$transferFeeAtoms private_fee_claim_count=$($commitInfoMiner.private_fee_claim_count)"
    }
    if ($transferFeeAtoms -eq 0 -and [int]$commitInfoMiner.private_fee_claim_count -ne 0) {
        throw "zero-fee transfer unexpectedly produced fee claims: private_fee_claim_count=$($commitInfoMiner.private_fee_claim_count)"
    }
    if ($commitInfoMiner.hash -ne $commitInfoVerifier.hash) {
        throw "verifier node does not share private-tx block head: miner=$($commitInfoMiner.hash) verifier=$($commitInfoVerifier.hash)"
    }
    $minerAfterCommit = Invoke-ZokaProfileJson -Profile $minerProfile -CmdArgs @(
        "wallet-balance", "--address", "miner", "--password", $WalletPassword, "--json"
    )
    $receiverAfterCommit = Invoke-ZokaProfileJson -Profile $minerProfile -CmdArgs @(
        "wallet-balance", "--address", "receiver", "--password", $WalletPassword, "--json"
    )
    $summary["inclusion"] = [ordered]@{
        block_height = $commitHeight
        tx_hash = $txHash
        included = $included
        miner_block = $commitInfoMiner
        verifier_block = $commitInfoVerifier
        node1_height = $node1CommitHeight
        node2_height = $node2CommitHeight
        miner_after_commit = $minerAfterCommit
        receiver_after_commit = $receiverAfterCommit
    }
    $summary["node_results"] = @(
        [pscustomobject]@{ node = "node-1"; height = (Get-NodeHeight -Db $node1Commit.Db); head_hash = (Get-NodeHeadHash -Db $node1Commit.Db); db = $node1Commit.Db; profile = $minerProfile },
        [pscustomobject]@{ node = "node-2"; height = (Get-NodeHeight -Db $node2Commit.Db); head_hash = (Get-NodeHeadHash -Db $node2Commit.Db); db = $node2Commit.Db; profile = $verifierProfile }
    )
}
finally {
    Stop-LiveNodes -LiveNodes $live
}

$summary["finished_at"] = (Get-Date).ToString("o")
$summaryPath = Join-Path $OutDir "summary.json"
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding utf8

$summary["node_results"] | Format-Table node, height, head_hash -AutoSize
$summaryTxHash = $summary["transfer"]["tx_hash"]
$summaryBlockHeight = $summary["inclusion"]["block_height"]
Write-Host "Wallet transfer hash: $summaryTxHash"
Write-Host "Committed at height: $summaryBlockHeight"
Write-Host "Summary: $summaryPath"
Write-Host "Local wallet network rehearsal passed."
