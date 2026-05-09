param(
    [string]$RepoRoot = (Resolve-Path ".").Path,
    [string]$ZokaExe = "",
    [string]$OutDir = "",
    [int]$BasePort = 29500,
    [int]$SeedTargetHeight = 5,
    [int]$LaunchNode3AtHeight = 3,
    [int]$SyncFloorHeight = 5,
    [int]$FinalTargetHeight = 8,
    [int]$SeedMineIntervalSeconds = 5,
    [int]$CompetitionMineIntervalSeconds = 5,
    [int]$CompetitionMinMinePeers = 1,
    [int]$SyncRebroadcastRecentBlocks = 16,
    [int]$NodeSeconds = 300,
    [int]$WaitTimeoutSeconds = 240,
    [int]$UnsafeRehearsalDifficultyBits = 2
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
    $OutDir = Join-Path $RepoRoot ".zoka\rehearsal\staged-sync-$stamp"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

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

function Remove-Ansi {
    param([string]$Text)
    return (($Text -replace "\x1b\[[0-9;]*[A-Za-z]", "") -replace "\x1b\][^\x07]*\x07", "")
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
    return [int]$matches[$matches.Count - 1].Groups[1].Value
}

function Get-ObservedNodeHeight {
    param($Node)

    $dbHeight = Get-NodeHeight -Db $Node.Db
    $logHeight = Get-NodeHeightFromLog -Path $Node.Log
    $heights = @($dbHeight, $logHeight) | Where-Object { $null -ne $_ }
    if (@($heights).Count -eq 0) {
        return $null
    }
    return ($heights | Measure-Object -Maximum).Maximum
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

function Get-NodeCumulativeWork {
    param([string]$Db)
    try {
        $statsJson = & $ZokaExe chain-stats --db $Db --json 2>&1 | Out-String
    } catch {
        return 0
    }
    if ($LASTEXITCODE -ne 0) {
        return 0
    }
    try {
        return [int64](($statsJson | ConvertFrom-Json).cumulative_work)
    } catch {
        return 0
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
        Start-Sleep -Seconds 4
    }
    throw "timeout waiting for node $($Node.Name) to reach height $Height"
}

function Wait-ForHeadHash {
    param(
        $Node,
        [string]$ExpectedHash,
        [int]$TimeoutSeconds
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $current = Get-NodeHeadHash -Db $Node.Db
        if (![string]::IsNullOrWhiteSpace($current) -and $current -eq $ExpectedHash) {
            return $current
        }
        if ($Node.Process.HasExited) {
            throw "node $($Node.Name) exited before adopting head $ExpectedHash"
        }
        Start-Sleep -Seconds 4
    }
    throw "timeout waiting for node $($Node.Name) to adopt head $ExpectedHash"
}

function Get-BlockWinners {
    param([string]$Db, [int]$MaxHeight, [hashtable]$WalletToNode = @{})
    $winners = @()
    for ($h = 1; $h -le $MaxHeight; $h++) {
        $infoJson = & $ZokaExe block-info --height $h --db $Db --json 2>&1 | Out-String
        try {
            $info = $infoJson | ConvertFrom-Json
            $minerNode = if ($WalletToNode.ContainsKey($info.miner)) { $WalletToNode[$info.miner] } else { "(unknown)" }
            $winners += [pscustomobject]@{
                height     = $info.height
                miner_node = $minerNode
                miner      = $info.miner
                reward_zka = $info.reward_zka
                hash       = $info.hash.Substring(0, [Math]::Min(16, $info.hash.Length)) + "..."
            }
        } catch {
            $winners += [pscustomobject]@{
                height     = $h
                miner_node = "(unreadable)"
                miner      = "(unreadable)"
                reward_zka = 0
                hash       = ""
            }
        }
    }
    return $winners
}

function Start-StagedNode {
    param(
        [string]$Name,
        [int]$Index,
        [string]$Phase = "run",
        [switch]$NoMine,
        [string[]]$Seeds = @(),
        [int]$MineIntervalSeconds = $SeedMineIntervalSeconds,
        [int]$MinMinePeers = 0,
        [int]$RebroadcastRecentBlocks = $SyncRebroadcastRecentBlocks,
        [int]$SyncWarmupSeconds = 3
    )

    $nodeDir = Join-Path $OutDir $Name
    New-Item -ItemType Directory -Force -Path $nodeDir | Out-Null
    $log = Join-Path $nodeDir "$Phase.node.log"
    $err = Join-Path $nodeDir "$Phase.node.err.log"
    $db = Join-Path $nodeDir "blockchain.db"
    $profile = Join-Path $nodeDir "home"
    New-Item -ItemType Directory -Force -Path $profile | Out-Null

    $port = $BasePort + $Index
    $listen = "/ip4/127.0.0.1/tcp/$port"
    $args = @(
        "fullnode",
        "--password", "staged-sync-pass-$Index",
        "--no-network-config",
        "--listen", $listen,
        "--db", $db,
        "--genesis", (Join-Path $RepoRoot "docs\genesis.mainnet.v1.json"),
        "--seconds", "$NodeSeconds",
        "--sync-warmup-secs", "$SyncWarmupSeconds",
        "--sync-rebroadcast-recent-blocks", "$RebroadcastRecentBlocks",
        "--unsafe-rehearsal-difficulty-bits", "$UnsafeRehearsalDifficultyBits"
    )
    if ($NoMine) {
        $args += "--no-mine"
    } else {
        # All nodes compete; min-mine-peers=0 so they don't wait for a full
        # peer set before their first attempt unless the caller asks for it.
        $args += @(
            "--mine-blocks", "1",
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
    try {
        $env:USERPROFILE = $profile
        $env:HOME = $profile
        $env:ZOKA_ALLOW_UNSAFE_REHEARSAL = "1"
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
    }

    [pscustomobject]@{
        Name = $Name
        Index = $Index
        Process = $process
        Dir = $nodeDir
        Log = $log
        ErrorLog = $err
        Db = $db
        Home = $profile
        Listen = $listen
    }
}

function Start-StagedNodeWithRetry {
    param(
        [hashtable]$NodeArgs,
        [int]$Attempts = 5,
        [int]$ProbeSeconds = 22
    )

    $lastNode = $null
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        $node = Start-StagedNode @NodeArgs
        Start-Sleep -Seconds $ProbeSeconds
        $node.Process.Refresh()
        if (!$node.Process.HasExited) {
            return $node
        }
        $lastNode = $node
        Stop-Node $node
        Start-Sleep -Seconds 3
    }
    return $lastNode
}

function Get-WalletAddressFromLog {
    param([string]$Path)
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    $text = Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue
    if ($text -match 'Wallet address:\s*(zka1[^\s]+)') {
        return $Matches[1]
    }
    return $null
}

function Stop-Node {
    param($Node)
    if ($null -ne $Node.Process -and !$Node.Process.HasExited) {
        Stop-Process -Id $Node.Process.Id -Force -ErrorAction SilentlyContinue
    }
}

$nodes = @()
$summary = [ordered]@{
    schema = "zoka-staged-sync-rehearsal-v1"
    started_at = (Get-Date).ToString("o")
    seed_target_height = $SeedTargetHeight
    launch_node3_at_height = $LaunchNode3AtHeight
    sync_floor_height = $SyncFloorHeight
    final_target_height = $FinalTargetHeight
    seed_mine_interval_seconds = $SeedMineIntervalSeconds
    competition_mine_interval_seconds = $CompetitionMineIntervalSeconds
    competition_min_mine_peers = $CompetitionMinMinePeers
    sync_rebroadcast_recent_blocks = $SyncRebroadcastRecentBlocks
    node_seconds = $NodeSeconds
    unsafe_rehearsal_difficulty_bits = $UnsafeRehearsalDifficultyBits
    out_dir = $OutDir
    seed_addr = $null
    competition_seed_addr = $null
    milestones = @()
    node_wallets = @()
    competition_results = @()
    block_winners = @()
    results = @()
}

try {
    Write-Host "Starting node 1 as initial miner..."
    $node1 = Start-StagedNode -Name "node-1" -Index 1 -Phase "sync" -MineIntervalSeconds $SeedMineIntervalSeconds -MinMinePeers 0
    $nodes += $node1
    $seedAddr = Wait-ForPattern -Path $node1.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30
    $summary.seed_addr = $seedAddr
    Write-Host "Node 1 address: $seedAddr"

    $height1 = Wait-ForHeight -Node $node1 -Height $SeedTargetHeight -TimeoutSeconds $WaitTimeoutSeconds
    $summary.milestones += [pscustomobject]@{
        event = "node1_reached_seed_target"
        node = "node-1"
        height = $height1
        at = (Get-Date).ToString("o")
    }
    Write-Host "Node 1 reached height $height1. Freezing seed mining and restarting node 1 as sync-only..."

    Stop-Node $node1
    Start-Sleep -Seconds 3
    $nodes = @()
    $node1 = Start-StagedNode -Name "node-1" -Index 1 -Phase "sync-frozen" -NoMine
    $nodes += $node1
    $seedAddr = Wait-ForPattern -Path $node1.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30
    $summary.seed_addr = $seedAddr
    $summary.milestones += [pscustomobject]@{
        event = "node1_restarted_sync_only"
        node = "node-1"
        height = (Get-NodeHeight -Db $node1.Db)
        at = (Get-Date).ToString("o")
    }
    Write-Host "Node 1 restarted sync-only at $seedAddr. Starting node 2 as sync-only..."

    $node2 = Start-StagedNodeWithRetry -NodeArgs @{
        Name = "node-2"
        Index = 2
        Phase = "sync"
        NoMine = $true
        Seeds = @($seedAddr)
    }
    $nodes += $node2
    $height2 = Wait-ForHeight -Node $node2 -Height $LaunchNode3AtHeight -TimeoutSeconds $WaitTimeoutSeconds
    $summary.milestones += [pscustomobject]@{
        event = "node2_reached_launch_threshold"
        node = "node-2"
        height = $height2
        at = (Get-Date).ToString("o")
    }
    Write-Host "Node 2 reached height $height2. Starting node 3 as sync-only..."

    $node3 = Start-StagedNodeWithRetry -NodeArgs @{
        Name = "node-3"
        Index = 3
        Phase = "sync"
        NoMine = $true
        Seeds = @($seedAddr, $node2.Listen)
    }
    $nodes += $node3

    Write-Host "Waiting for all 3 nodes to sync to height $SyncFloorHeight before competition..."
    foreach ($node in $nodes) {
        $height = Wait-ForHeight -Node $node -Height $SyncFloorHeight -TimeoutSeconds $WaitTimeoutSeconds
        $summary.milestones += [pscustomobject]@{
            event = "node_reached_sync_floor"
            node = $node.Name
            height = $height
            at = (Get-Date).ToString("o")
        }
        Write-Host "$($node.Name) reached height $height."
    }

    Write-Host "Sync floor reached. Restarting all 3 nodes with mining enabled for competition..."
    foreach ($node in $nodes) {
        Stop-Node $node
    }
    Start-Sleep -Seconds 3

    $nodes = @()
    $node2 = Start-StagedNode -Name "node-2" -Index 2 -Phase "competition" -MineIntervalSeconds $CompetitionMineIntervalSeconds -MinMinePeers $CompetitionMinMinePeers -SyncWarmupSeconds 20
    $nodes += $node2
    $competitionSeedAddr = Wait-ForPattern -Path $node2.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30
    $summary.competition_seed_addr = $competitionSeedAddr

    $node1 = Start-StagedNode -Name "node-1" -Index 1 -Phase "competition" -Seeds @($competitionSeedAddr) -MineIntervalSeconds $CompetitionMineIntervalSeconds -MinMinePeers $CompetitionMinMinePeers -SyncWarmupSeconds 30
    $nodes += $node1
    Start-Sleep -Seconds 1
    $node3 = Start-StagedNode -Name "node-3" -Index 3 -Phase "competition" -Seeds @($competitionSeedAddr, $node1.Listen) -MineIntervalSeconds $CompetitionMineIntervalSeconds -MinMinePeers $CompetitionMinMinePeers -SyncWarmupSeconds 30
    $nodes += $node3

    Start-Sleep -Seconds 25
    foreach ($node in $nodes) {
        $node.Process.Refresh()
        if ($node.Process.HasExited) {
            throw "node $($node.Name) exited during competition warmup; inspect $($node.ErrorLog)"
        }
    }

    Write-Host "All 3 nodes now competing. Waiting for everyone to reach height $FinalTargetHeight..."
    foreach ($node in $nodes) {
        $height = Wait-ForHeight -Node $node -Height $FinalTargetHeight -TimeoutSeconds $WaitTimeoutSeconds
        $summary.milestones += [pscustomobject]@{
            event = "node_reached_competition_target"
            node = $node.Name
            height = $height
            at = (Get-Date).ToString("o")
        }
        Write-Host "$($node.Name) reached height $height."
    }

    Start-Sleep -Seconds 12
    foreach ($node in $nodes) {
        $summary.competition_results += [pscustomobject]@{
            node = $node.Name
            height = (Get-ObservedNodeHeight -Node $node)
            head_hash = (Get-NodeHeadHash -Db $node.Db)
            cumulative_work = (Get-NodeCumulativeWork -Db $node.Db)
        }
    }

    foreach ($node in $nodes) {
        Stop-Node $node
    }
    Start-Sleep -Seconds 3

    $bestStoppedNode = $nodes | Sort-Object @{ Expression = { Get-NodeCumulativeWork -Db $_.Db }; Descending = $true }, @{ Expression = { Get-NodeHeight -Db $_.Db }; Descending = $true } | Select-Object -First 1
    $settleTargetHeight = Get-NodeHeight -Db $bestStoppedNode.Db
    $settleTargetHash = Get-NodeHeadHash -Db $bestStoppedNode.Db
    $settleTargetWork = Get-NodeCumulativeWork -Db $bestStoppedNode.Db
    Write-Host "Competition stopped. Settling all nodes to height $settleTargetHeight / work $settleTargetWork from $($bestStoppedNode.Name)..."

    $settleSeed = Start-StagedNode -Name $bestStoppedNode.Name -Index $bestStoppedNode.Index -Phase "settle" -NoMine -RebroadcastRecentBlocks $SyncRebroadcastRecentBlocks
    $settleNodes = @($settleSeed)
    $settleSeedAddr = Wait-ForPattern -Path $settleSeed.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30
    foreach ($oldNode in ($nodes | Where-Object { $_.Name -ne $bestStoppedNode.Name })) {
        $settleNode = Start-StagedNodeWithRetry -NodeArgs @{
            Name = $oldNode.Name
            Index = $oldNode.Index
            Phase = "settle"
            NoMine = $true
            Seeds = @($settleSeedAddr)
            RebroadcastRecentBlocks = 0
        }
        $headHash = Wait-ForHeadHash -Node $settleNode -ExpectedHash $settleTargetHash -TimeoutSeconds $WaitTimeoutSeconds
        $height = Get-ObservedNodeHeight -Node $settleNode
        $summary.milestones += [pscustomobject]@{
            event = "node_reached_settle_target"
            node = $settleNode.Name
            height = $height
            head_hash = $headHash
            at = (Get-Date).ToString("o")
        }
        Write-Host "$($settleNode.Name) settled at height $height / $($headHash.Substring(0, [Math]::Min(16, $headHash.Length)))..."
        Stop-Node $settleNode
        $settleNodes += $settleNode
        Start-Sleep -Seconds 2
    }

    Start-Sleep -Seconds 8
    foreach ($node in $settleNodes) {
        Stop-Node $node
    }
    Start-Sleep -Seconds 2
    $nodes = $settleNodes

    $finalHeads = @($nodes | ForEach-Object { Get-NodeHeadHash -Db $_.Db } | Where-Object { ![string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
    if (@($finalHeads).Count -ne 1) {
        throw "settle phase did not converge to one canonical head; inspect $OutDir"
    }

    foreach ($node in $nodes) {
        $node.Process.Refresh()
        $stdout = if (Test-Path -LiteralPath $node.Log) { Get-Content -LiteralPath $node.Log -Raw } else { "" }
        $stderr = if (Test-Path -LiteralPath $node.ErrorLog) { Get-Content -LiteralPath $node.ErrorLog -Raw } else { "" }
        $height = Get-NodeHeight -Db $node.Db
        $wallet = Get-WalletAddressFromLog -Path $node.Log
        $summary.node_wallets += [pscustomobject]@{
            node = $node.Name
            wallet = $wallet
        }
        $summary.results += [pscustomobject]@{
            node = $node.Name
            exit_code = $node.Process.ExitCode
            height = $height
            head_hash = (Get-NodeHeadHash -Db $node.Db)
            wallet = $wallet
            connected = ($stdout -match "Connected to seed peer" -or [regex]::IsMatch($stdout, "Connected peers.*?[1-9]"))
            observed_sync_state = ($stdout -match "Observed peer sync state")
            served_block_request = ($stdout -match "Served block request")
            accepted_or_known_block = ($stdout -match "Accepted peer block" -or $stdout -match "Peer block @ height")
            auto_mined = ($stdout -match "Auto-mined")
            clean_time_limit = ($stdout -match "Fullnode session time limit reached")
            log = $node.Log
            error_log = $node.ErrorLog
            stderr_tail = if ($stderr.Length -gt 600) { $stderr.Substring($stderr.Length - 600) } else { $stderr }
        }
    }

    # --- Block winner table ---
    # All nodes should share the same canonical chain; query node-1's DB.
    $bestNode = $nodes | Sort-Object { (Get-NodeHeight -Db $_.Db) } -Descending | Select-Object -First 1
    $bestHeight = Get-NodeHeight -Db $bestNode.Db
    if ($null -ne $bestHeight -and $bestHeight -gt 0) {
        Write-Host ""
        Write-Host "=== Block winner table (from $($bestNode.Name) at height $bestHeight) ==="
        $walletToNode = @{}
        foreach ($entry in $summary.node_wallets) {
            if (![string]::IsNullOrWhiteSpace($entry.wallet)) {
                $walletToNode[$entry.wallet] = $entry.node
            }
        }
        $winners = Get-BlockWinners -Db $bestNode.Db -MaxHeight $bestHeight -WalletToNode $walletToNode
        $summary.block_winners = $winners
        $winners | Format-Table height, miner_node, reward_zka, miner, hash -AutoSize
    }
}
finally {
    foreach ($node in $nodes) {
        Stop-Node $node
    }
}

$summaryPath = Join-Path $OutDir "summary.json"
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8

$summary.results | Format-Table node, exit_code, height, connected, observed_sync_state, served_block_request, accepted_or_known_block, auto_mined -AutoSize
Write-Host "Summary: $summaryPath"

$allOk = $true
foreach ($result in $summary.results) {
    if ($result.height -lt $FinalTargetHeight) {
        $allOk = $false
    }
}
if (@($summary.results).Count -ne 3) {
    $allOk = $false
}
if (!$allOk) {
    throw "staged sync rehearsal failed; inspect $summaryPath"
}

Write-Host "Staged sync rehearsal passed."
