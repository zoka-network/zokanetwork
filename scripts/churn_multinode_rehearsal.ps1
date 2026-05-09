param(
    [string]$RepoRoot = (Resolve-Path ".").Path,
    [string]$ZokaExe = "",
    [string]$OutDir = "",
    [int]$BasePort = 29700,
    [int]$InitialSeedHeight = 4,
    [int]$Node2JoinHeight = 3,
    [int]$SyncFloorHeight = 5,
    [int]$OfflineAdvanceHeight = 8,
    [int]$ReturnCatchupHeight = 8,
    [int]$LateJoinHeight = 4,
    [int]$SeedDownHeight = 9,
    [int]$FinalMinHeight = 9,
    [int]$MineIntervalSeconds = 2,
    [int]$UnsafeRehearsalDifficultyBits = 1,
    [int]$SyncRebroadcastRecentBlocks = 64,
    [int]$NodeSeconds = 1200,
    [int]$WaitTimeoutSeconds = 300,
    [int]$PeerLossRecoverySeconds = 60,
    [int]$SettleSeconds = 150,
    [int]$SettleAttempts = 2
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
    $OutDir = Join-Path $RepoRoot ".zoka\rehearsal\churn-$stamp"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Remove-Ansi {
    param([string]$Text)
    return (($Text -replace "\x1b\[[0-9;]*[A-Za-z]", "") -replace "\x1b\][^\x07]*\x07", "")
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

function Get-NodeConnectedPeersFromLog {
    param([string]$Path)

    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    $text = Remove-Ansi -Text (Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue)
    $matches = [regex]::Matches($text, "Connected peers\s+([0-9]+)")
    if ($matches.Count -eq 0) {
        return $null
    }
    $counts = foreach ($match in $matches) {
        [int]$match.Groups[1].Value
    }
    return ($counts | Measure-Object -Maximum).Maximum
}

function Get-ObservedNodeHeight {
    param($Node)

    $logHeight = Get-NodeHeightFromLog -Path $Node.Log
    if ($null -ne $logHeight) {
        return $logHeight
    }
    if ($Node.Process.HasExited) {
        return (Get-NodeHeight -Db $Node.Db)
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
    param(
        [int]$Index,
        [hashtable]$LatestByIndex
    )

    $node = $LatestByIndex[$Index]
    $listen = if ($null -ne $node) { $node.Listen } else { Get-NodeListen -Index $Index }
    if ([string]::IsNullOrWhiteSpace($listen) -or $listen -match '/p2p/') {
        return $listen
    }

    if ($null -ne $node) {
        $peerId = Get-NodePeerIdFromLog -Path $node.Log
        if (![string]::IsNullOrWhiteSpace($peerId)) {
            return "$listen/p2p/$peerId"
        }
    }
    return $listen
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
        Start-Sleep -Seconds 3
    }
    throw "timeout waiting for node $($Node.Name) to reach height $Height"
}

function Try-WaitForHeight {
    param(
        $Node,
        [int]$Height,
        [int]$TimeoutSeconds
    )

    try {
        return (Wait-ForHeight -Node $Node -Height $Height -TimeoutSeconds $TimeoutSeconds)
    } catch {
        return $null
    }
}

function Stop-Node {
    param($Node)
    if ($null -ne $Node -and $null -ne $Node.Process -and !$Node.Process.HasExited) {
        Stop-Process -Id $Node.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
    }
}

function Start-ChurnNode {
    param(
        [int]$Index,
        [string]$Phase,
        [switch]$NoMine,
        [string[]]$Seeds = @(),
        [int]$MinMinePeers = 1,
        [int]$RebroadcastRecentBlocks = $SyncRebroadcastRecentBlocks,
        [int]$SyncWarmupSeconds = 10
    )

    $nodeDir = Get-NodeDir -Index $Index
    New-Item -ItemType Directory -Force -Path $nodeDir | Out-Null
    $log = Join-Path $nodeDir "$Phase.node.log"
    $err = Join-Path $nodeDir "$Phase.node.err.log"
    $db = Get-NodeDb -Index $Index
    $profile = Join-Path $nodeDir "home"
    New-Item -ItemType Directory -Force -Path $profile | Out-Null

    $listen = Get-NodeListen -Index $Index
    $args = @(
        "fullnode",
        "--password", "churn-pass-$Index",
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
        Name = "node-$Index"
        Index = $Index
        Phase = $Phase
        Process = $process
        Dir = $nodeDir
        Log = $log
        ErrorLog = $err
        Db = $db
        Home = $profile
        Listen = $listen
    }
}

function Start-ChurnNodeWithRetry {
    param(
        [hashtable]$NodeArgs,
        [int]$Attempts = 4,
        [int]$ProbeSeconds = 25
    )

    $lastNode = $null
    $requiresPeer = $false
    if ($NodeArgs.ContainsKey("Seeds")) {
        $requiresPeer = @($NodeArgs["Seeds"] | Where-Object { ![string]::IsNullOrWhiteSpace($_) }).Count -gt 0
    }
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        $attemptArgs = @{}
        foreach ($key in $NodeArgs.Keys) {
            $attemptArgs[$key] = $NodeArgs[$key]
        }
        if ($attemptArgs.ContainsKey("Phase")) {
            $attemptArgs["Phase"] = "$($attemptArgs["Phase"])-try$attempt"
        }
        $node = Start-ChurnNode @attemptArgs
        Start-Sleep -Seconds $ProbeSeconds
        $node.Process.Refresh()
        $connectedPeers = Get-NodeConnectedPeersFromLog -Path $node.Log
        $observedHeight = Get-ObservedNodeHeight -Node $node
        if (!$node.Process.HasExited -and $requiresPeer -and $null -eq $connectedPeers) {
            Write-Host "node-$($NodeArgs["Index"]) attempt $attempt is still in bootstrap after $ProbeSeconds seconds; keeping it alive..."
            return $node
        }
        if (!$node.Process.HasExited -and (!$requiresPeer -or (($null -ne $connectedPeers -and $connectedPeers -gt 0) -or ($null -ne $observedHeight -and $observedHeight -gt 0)))) {
            return $node
        }
        $lastNode = $node
        if (!$node.Process.HasExited -and $requiresPeer) {
            Write-Host "node-$($NodeArgs["Index"]) attempt $attempt stayed isolated after $ProbeSeconds seconds; restarting with the same seeds..."
        }
        Stop-Node $node
        Start-Sleep -Seconds 2
    }
    return $lastNode
}

function Add-Event {
    param(
        [string]$Event,
        [int]$Index = 0,
        [object]$Height = $null,
        [string]$Note = ""
    )

    $entry = [ordered]@{
        event = $Event
        at = (Get-Date).ToString("o")
    }
    if ($Index -gt 0) {
        $entry.node = "node-$Index"
    }
    if ($null -ne $Height) {
        $entry.height = $Height
    }
    if (![string]::IsNullOrWhiteSpace($Note)) {
        $entry.note = $Note
    }
    $summary.events += [pscustomobject]$entry
}

function Read-NodeResult {
    param([int]$Index, $LatestNode = $null)

    $db = Get-NodeDb -Index $Index
    $log = if ($null -ne $LatestNode) { $LatestNode.Log } else { $null }
    if ([string]::IsNullOrWhiteSpace($log) -or !(Test-Path -LiteralPath $log -PathType Leaf)) {
        $latestLog = Get-ChildItem (Get-NodeDir -Index $Index) -Filter "*.node.log" -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1
        $log = if ($null -ne $latestLog) { $latestLog.FullName } else { "" }
    }
    [pscustomobject]@{
        index = $Index
        node = "node-$Index"
        height = (Get-NodeHeight -Db $db)
        head_hash = (Get-NodeHeadHash -Db $db)
        cumulative_work = (Get-NodeCumulativeWork -Db $db)
        wallet = (Get-WalletAddressFromLog -Path $log)
        log = $log
        db = $db
    }
}

function Read-AllNodeResults {
    param([int[]]$Indexes, [hashtable]$LatestByIndex)

    $results = @()
    foreach ($idx in $Indexes) {
        $latest = if ($LatestByIndex.ContainsKey($idx)) { $LatestByIndex[$idx] } else { $null }
        $results += Read-NodeResult -Index $idx -LatestNode $latest
    }
    return $results
}

function Stop-LiveNodes {
    param([hashtable]$LiveNodes)
    foreach ($key in @($LiveNodes.Keys)) {
        Stop-Node $LiveNodes[$key]
    }
    $LiveNodes.Clear()
    Start-Sleep -Seconds 2
}

$nodeIndexes = @(1, 2, 3, 4)
$live = @{}
$latest = @{}
$summary = [ordered]@{
    schema = "zoka-churn-multinode-rehearsal-v1"
    started_at = (Get-Date).ToString("o")
    base_port = $BasePort
    initial_seed_height = $InitialSeedHeight
    node2_join_height = $Node2JoinHeight
    sync_floor_height = $SyncFloorHeight
    offline_advance_height = $OfflineAdvanceHeight
    return_catchup_height = $ReturnCatchupHeight
    late_join_height = $LateJoinHeight
    seed_down_height = $SeedDownHeight
    final_min_height = $FinalMinHeight
    mine_interval_seconds = $MineIntervalSeconds
    unsafe_rehearsal_difficulty_bits = $UnsafeRehearsalDifficultyBits
    sync_rebroadcast_recent_blocks = $SyncRebroadcastRecentBlocks
    node_seconds = $NodeSeconds
    peer_loss_recovery_seconds = $PeerLossRecoverySeconds
    out_dir = $OutDir
    events = @()
    pre_settle_results = @()
    settle_attempts = @()
    results = @()
}

try {
    Write-Host "Starting node-1 as everyday seed/miner..."
    $node1 = Start-ChurnNode -Index 1 -Phase "seed-start" -MinMinePeers 0 -SyncWarmupSeconds 3
    $live[1] = $node1
    $latest[1] = $node1
    Wait-ForPattern -Path $node1.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30 | Out-Null
    $seedAddr = Get-NodeSeedAddress -Index 1 -LatestByIndex $latest
    Add-Event -Event "node_started" -Index 1 -Note $seedAddr
    $height = Wait-ForHeight -Node $node1 -Height $InitialSeedHeight -TimeoutSeconds $WaitTimeoutSeconds
    Add-Event -Event "seed_reached_initial_height" -Index 1 -Height $height

    Write-Host "Starting node-2 late, using node-1 as seed..."
    $node2 = Start-ChurnNodeWithRetry -NodeArgs @{
        Index = 2
        Phase = "late-join"
        Seeds = @((Get-NodeSeedAddress -Index 1 -LatestByIndex $latest))
        MinMinePeers = 1
        SyncWarmupSeconds = 15
    }
    $live[2] = $node2
    $latest[2] = $node2
    Add-Event -Event "node_started_late" -Index 2 -Note (Get-NodeSeedAddress -Index 2 -LatestByIndex $latest)
    $height = Wait-ForHeight -Node $node2 -Height $Node2JoinHeight -TimeoutSeconds $WaitTimeoutSeconds
    Add-Event -Event "node2_caught_partial_chain" -Index 2 -Height $height

    Write-Host "Starting node-3 through multi-seed fallback..."
    $node3 = Start-ChurnNodeWithRetry -NodeArgs @{
        Index = 3
        Phase = "multi-seed-join"
        Seeds = @(
            (Get-NodeSeedAddress -Index 1 -LatestByIndex $latest),
            (Get-NodeSeedAddress -Index 2 -LatestByIndex $latest)
        )
        MinMinePeers = 1
        SyncWarmupSeconds = 15
    }
    $live[3] = $node3
    $latest[3] = $node3
    Add-Event -Event "node_started_multi_seed" -Index 3 -Note (Get-NodeSeedAddress -Index 3 -LatestByIndex $latest)

    Write-Host "Waiting for nodes 1-3 to reach sync floor $SyncFloorHeight..."
    foreach ($idx in @(1, 2, 3)) {
        $height = Wait-ForHeight -Node $live[$idx] -Height $SyncFloorHeight -TimeoutSeconds $WaitTimeoutSeconds
        Add-Event -Event "node_reached_sync_floor" -Index $idx -Height $height
    }

    Write-Host "Simulating node-2 going offline while the network keeps moving..."
    Stop-Node $live[2]
    $live.Remove(2)
    Add-Event -Event "node_stopped_for_churn" -Index 2

    $height = Wait-ForHeight -Node $live[1] -Height $OfflineAdvanceHeight -TimeoutSeconds $WaitTimeoutSeconds
    Add-Event -Event "network_advanced_while_node2_offline" -Index 1 -Height $height

    $height = Try-WaitForHeight -Node $live[3] -Height $OfflineAdvanceHeight -TimeoutSeconds $PeerLossRecoverySeconds
    if ($null -eq $height) {
        Write-Host "node-3 lost progress after node-2 left; restarting node-3 with the remaining live seed..."
        Stop-Node $live[3]
        $live.Remove(3)
        Add-Event -Event "node_restarted_after_peer_loss" -Index 3 -Note "node-3 did not advance with node-2 offline"
        $node3 = Start-ChurnNodeWithRetry -NodeArgs @{
            Index = 3
            Phase = "recover-after-peer-loss"
            Seeds = @((Get-NodeSeedAddress -Index 1 -LatestByIndex $latest))
            MinMinePeers = 1
            SyncWarmupSeconds = 12
        }
        $live[3] = $node3
        $latest[3] = $node3
        $height = Wait-ForHeight -Node $node3 -Height $OfflineAdvanceHeight -TimeoutSeconds $WaitTimeoutSeconds
    }
    Add-Event -Event "node3_available_after_peer_loss" -Index 3 -Height $height

    Write-Host "Restarting node-2 with old DB and two live seeds..."
    $node2 = Start-ChurnNodeWithRetry -NodeArgs @{
        Index = 2
        Phase = "return-after-offline"
        Seeds = @(
            (Get-NodeSeedAddress -Index 1 -LatestByIndex $latest),
            (Get-NodeSeedAddress -Index 3 -LatestByIndex $latest)
        )
        MinMinePeers = 1
        SyncWarmupSeconds = 12
    }
    $live[2] = $node2
    $latest[2] = $node2
    Add-Event -Event "node_restarted_after_offline" -Index 2
    $height = Wait-ForHeight -Node $node2 -Height $ReturnCatchupHeight -TimeoutSeconds $WaitTimeoutSeconds
    Add-Event -Event "node2_caught_up_after_return" -Index 2 -Height $height

    Write-Host "Starting node-4 from an empty DB as a new human/operator..."
    $node4 = Start-ChurnNodeWithRetry -NodeArgs @{
        Index = 4
        Phase = "fresh-human-join"
        NoMine = $true
        Seeds = @(
            (Get-NodeSeedAddress -Index 1 -LatestByIndex $latest),
            (Get-NodeSeedAddress -Index 2 -LatestByIndex $latest),
            (Get-NodeSeedAddress -Index 3 -LatestByIndex $latest)
        )
        RebroadcastRecentBlocks = 0
        SyncWarmupSeconds = 12
    }
    $live[4] = $node4
    $latest[4] = $node4
    Add-Event -Event "fresh_node_started" -Index 4
    $height = Wait-ForHeight -Node $node4 -Height $LateJoinHeight -TimeoutSeconds $WaitTimeoutSeconds
    Add-Event -Event "fresh_node_synced_from_zero" -Index 4 -Height $height

    Write-Host "Stopping original seed node-1; peers should continue without it..."
    Stop-Node $live[1]
    $live.Remove(1)
    Add-Event -Event "original_seed_stopped" -Index 1

    foreach ($idx in @(2, 3)) {
        $height = Wait-ForHeight -Node $live[$idx] -Height $SeedDownHeight -TimeoutSeconds $WaitTimeoutSeconds
        Add-Event -Event "network_advanced_without_original_seed" -Index $idx -Height $height
    }
    $node4Height = Get-ObservedNodeHeight -Node $live[4]
    Add-Event -Event "fresh_node_survived_original_seed_loss" -Index 4 -Height $node4Height

    Write-Host "Bringing node-1 back as a normal returning peer..."
    $node1 = Start-ChurnNodeWithRetry -NodeArgs @{
        Index = 1
        Phase = "return-after-seed-down"
        NoMine = $true
        Seeds = @(
            (Get-NodeSeedAddress -Index 2 -LatestByIndex $latest),
            (Get-NodeSeedAddress -Index 3 -LatestByIndex $latest),
            (Get-NodeSeedAddress -Index 4 -LatestByIndex $latest)
        )
        RebroadcastRecentBlocks = 0
        SyncWarmupSeconds = 12
    }
    $live[1] = $node1
    $latest[1] = $node1
    Add-Event -Event "original_seed_returned_as_peer" -Index 1
    $height = Wait-ForHeight -Node $node1 -Height $FinalMinHeight -TimeoutSeconds $WaitTimeoutSeconds
    Add-Event -Event "original_seed_caught_up_after_return" -Index 1 -Height $height

    Write-Host "Stopping churn phase and reading heads..."
    Stop-LiveNodes -LiveNodes $live
    $summary.pre_settle_results = Read-AllNodeResults -Indexes $nodeIndexes -LatestByIndex $latest

    for ($attempt = 1; $attempt -le $SettleAttempts; $attempt++) {
        $results = Read-AllNodeResults -Indexes $nodeIndexes -LatestByIndex $latest
        $heads = @($results | Where-Object { ![string]::IsNullOrWhiteSpace($_.head_hash) } | Select-Object -ExpandProperty head_hash -Unique)
        if (@($heads).Count -eq 1) {
            Add-Event -Event "settle_not_needed" -Note "all nodes already share one head"
            break
        }

        $best = $results |
            Sort-Object @{ Expression = { $_.cumulative_work }; Descending = $true }, @{ Expression = { $_.height }; Descending = $true } |
            Select-Object -First 1
        Write-Host "Settle attempt ${attempt}: seeding from $($best.node) height $($best.height), work $($best.cumulative_work)..."
        $summary.settle_attempts += [pscustomobject]@{
            attempt = $attempt
            seed_node = $best.node
            seed_height = $best.height
            seed_work = $best.cumulative_work
            seed_head = $best.head_hash
        }

        $settleLive = @{}
        $seed = Start-ChurnNode -Index $best.index -Phase "settle-$attempt-seed" -NoMine -RebroadcastRecentBlocks $SyncRebroadcastRecentBlocks -SyncWarmupSeconds 3
        $settleLive[$best.index] = $seed
        $latest[$best.index] = $seed
        Wait-ForPattern -Path $seed.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30 | Out-Null
        Start-Sleep -Seconds 3

        foreach ($idx in ($nodeIndexes | Where-Object { $_ -ne $best.index })) {
            $peer = Start-ChurnNodeWithRetry -NodeArgs @{
                Index = $idx
                Phase = "settle-$attempt"
                NoMine = $true
                Seeds = @((Get-NodeSeedAddress -Index $best.index -LatestByIndex $latest))
                RebroadcastRecentBlocks = 0
                SyncWarmupSeconds = 5
            } -ProbeSeconds 10
            $settleLive[$idx] = $peer
            $latest[$idx] = $peer
        }

        Start-Sleep -Seconds $SettleSeconds
        Stop-LiveNodes -LiveNodes $settleLive
    }

    $summary.results = Read-AllNodeResults -Indexes $nodeIndexes -LatestByIndex $latest
}
finally {
    Stop-LiveNodes -LiveNodes $live
}

$summaryPath = Join-Path $OutDir "summary.json"
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8

$summary.results | Format-Table node, height, cumulative_work, head_hash -AutoSize
Write-Host "Summary: $summaryPath"

$allOk = $true
$finalHeads = @($summary.results | Where-Object { ![string]::IsNullOrWhiteSpace($_.head_hash) } | Select-Object -ExpandProperty head_hash -Unique)
if (@($finalHeads).Count -ne 1) {
    $allOk = $false
}
foreach ($result in $summary.results) {
    if ($null -eq $result.height -or $result.height -lt $FinalMinHeight) {
        $allOk = $false
    }
}
if (!$allOk) {
    throw "churn multi-node rehearsal failed; inspect $summaryPath"
}

Write-Host "Churn multi-node rehearsal passed."
