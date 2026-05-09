param(
    [string]$RepoRoot = (Resolve-Path ".").Path,
    [string]$ZokaExe = "",
    [string]$OutDir = "",
    [int]$NodeCount = 3,
    [int]$BasePort = 29100,
    [int]$RunSeconds = 180,
    [int]$StopGraceSeconds = 45,
    [int]$SyncWarmupSeconds = 8,
    [int]$MineIntervalSeconds = 5,
    [int]$SyncRebroadcastRecentBlocks = 8,
    [int]$UnsafeRehearsalDifficultyBits = 4,
    [int]$TargetHeight = 8,
    [switch]$NoMine
)

$ErrorActionPreference = "Stop"

if ($NodeCount -lt 2) {
    throw "NodeCount must be at least 2 for a multi-node rehearsal."
}
if ([string]::IsNullOrWhiteSpace($ZokaExe)) {
    $ZokaExe = Join-Path $RepoRoot "target\release\zoka.exe"
}
if (!(Test-Path -LiteralPath $ZokaExe -PathType Leaf)) {
    throw "zoka binary not found: $ZokaExe. Build with cargo build --release --features randomx --bin zoka --bin zokahd"
}
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $RepoRoot ".zoka\rehearsal\local-multinode-$stamp"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Wait-ForPattern {
    param(
        [string]$Path,
        [string]$Pattern,
        [int]$TimeoutSeconds = 20
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

function Start-ZokaNode {
    param(
        [int]$Index,
        [string[]]$Seeds = @()
    )

    $nodeDir = Join-Path $OutDir "node-$Index"
    New-Item -ItemType Directory -Force -Path $nodeDir | Out-Null
    $log = Join-Path $nodeDir "node.log"
    $err = Join-Path $nodeDir "node.err.log"
    $db = Join-Path $nodeDir "blockchain.db"
    $profile = Join-Path $nodeDir "home"
    New-Item -ItemType Directory -Force -Path $profile | Out-Null

    $port = $BasePort + $Index
    $listen = "/ip4/127.0.0.1/tcp/$port"
    $args = @(
        "fullnode",
        "--password", "local-rehearsal-pass-$Index",
        "--no-network-config",
        "--listen", $listen,
        "--db", $db,
        "--genesis", (Join-Path $RepoRoot "docs\genesis.mainnet.v1.json"),
        "--seconds", "$RunSeconds",
        "--sync-warmup-secs", "$SyncWarmupSeconds",
        "--sync-rebroadcast-recent-blocks", "$SyncRebroadcastRecentBlocks",
        "--unsafe-rehearsal-difficulty-bits", "$UnsafeRehearsalDifficultyBits"
    )
    if ($NoMine) {
        $args += "--no-mine"
    } else {
        # Every node competes to mine — all are miners, none is privileged.
        $args += @(
            "--mine-blocks", "1",
            "--mine-interval-secs", "$MineIntervalSeconds",
            "--min-mine-peers", "0"
        )
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

function Stop-Node {
    param($Node)
    if ($null -ne $Node.Process -and !$Node.Process.HasExited) {
        Stop-Process -Id $Node.Process.Id -Force -ErrorAction SilentlyContinue
    }
}

function Get-NodeHeight {
    param([string]$Db)
    $heightJson = & $ZokaExe height --db $Db --json 2>&1 | Out-String
    try { return [int](($heightJson | ConvertFrom-Json).height) } catch { return $null }
}

function Get-BlockWinners {
    param([string]$Db, [int]$MaxHeight)
    $winners = @()
    for ($h = 1; $h -le $MaxHeight; $h++) {
        $infoJson = & $ZokaExe block-info --height $h --db $Db --json 2>&1 | Out-String
        try {
            $info = $infoJson | ConvertFrom-Json
            $winners += [pscustomobject]@{
                height     = $info.height
                miner      = $info.miner
                reward_zka = $info.reward_zka
                hash       = $info.hash.Substring(0, [Math]::Min(16, $info.hash.Length)) + "..."
            }
        } catch {
            $winners += [pscustomobject]@{
                height     = $h
                miner      = "(unreadable)"
                reward_zka = 0
                hash       = ""
            }
        }
    }
    return $winners
}

$nodes = @()
$summary = [ordered]@{
    schema = "zoka-local-multinode-rehearsal-v1"
    started_at = (Get-Date).ToString("o")
    node_count = $NodeCount
    base_port = $BasePort
    run_seconds = $RunSeconds
    stop_grace_seconds = $StopGraceSeconds
    sync_warmup_seconds = $SyncWarmupSeconds
    mine_interval_seconds = $MineIntervalSeconds
    sync_rebroadcast_recent_blocks = $SyncRebroadcastRecentBlocks
    unsafe_rehearsal_difficulty_bits = $UnsafeRehearsalDifficultyBits
    target_height = $TargetHeight
    no_mine = [bool]$NoMine
    out_dir = $OutDir
    seed_addr = $null
    block_winners = @()
    results = @()
}

try {
    Write-Host "Starting seed node (node-1)..."
    $seedNode = Start-ZokaNode -Index 1
    $nodes += $seedNode
    $seedAddr = Wait-ForPattern -Path $seedNode.Log -Pattern 'Listen address.*?((/ip4|/ip6)/[^\s]+)' -TimeoutSeconds 30
    $summary.seed_addr = $seedAddr
    Write-Host "Seed node address: $seedAddr"

    $knownSeedAddrs = @($seedAddr)
    for ($i = 2; $i -le $NodeCount; $i++) {
        Write-Host "Starting node $i -> seeding from known peers..."
        $node = Start-ZokaNode -Index $i -Seeds $knownSeedAddrs
        $nodes += $node
        $knownSeedAddrs += $node.Listen
        Start-Sleep -Seconds 1
    }

    Write-Host "All $NodeCount nodes running and competing to mine. Waiting up to $($RunSeconds + $StopGraceSeconds)s..."
    $deadline = (Get-Date).AddSeconds($RunSeconds + $StopGraceSeconds)
    foreach ($node in $nodes) {
        while (!$node.Process.HasExited -and (Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 500
        }
    }

    foreach ($node in $nodes) {
        if (!$node.Process.HasExited) {
            Stop-Node $node
        }
        $node.Process.Refresh()
        $stdout = if (Test-Path -LiteralPath $node.Log) { Get-Content -LiteralPath $node.Log -Raw } else { "" }
        $stderr = if (Test-Path -LiteralPath $node.ErrorLog) { Get-Content -LiteralPath $node.ErrorLog -Raw } else { "" }
        $height = Get-NodeHeight -Db $node.Db

        $connected = $stdout -match "Connected to seed peer" -or [regex]::IsMatch($stdout, "Connected peers.*?[1-9]")
        $listening = $stdout -match "Listen address"
        $timedOutCleanly = $stdout -match "Fullnode session time limit reached"
        $startupOk = $stdout -match "Node wallet" -and $stdout -match "Listen address"
        $exitCode = $node.Process.ExitCode
        if ($null -eq $exitCode -and $timedOutCleanly) {
            $exitCode = 0
        }

        $summary.results += [pscustomobject]@{
            node = $node.Index
            exit_code = $exitCode
            listening = $listening
            connected = $connected
            startup_ok = $startupOk
            clean_time_limit = $timedOutCleanly
            height = $height
            log = $node.Log
            error_log = $node.ErrorLog
        }

        if ($exitCode -ne 0 -or !$listening -or !$startupOk -or !$timedOutCleanly) {
            Write-Warning "Node $($node.Index) did not pass all startup checks. See $($node.Log)"
        }
        if (!$NoMine -and $height -lt 1) {
            Write-Warning "Node $($node.Index) did not reach height >= 1. See $($node.Log)"
        }
    }

    # --- Block winner table ---
    # Pick the node with the highest chain to query the canonical history.
    $bestNode = $nodes | Sort-Object { (Get-NodeHeight -Db $_.Db) } -Descending | Select-Object -First 1
    $bestHeight = Get-NodeHeight -Db $bestNode.Db
    if ($null -ne $bestHeight -and $bestHeight -gt 0) {
        Write-Host ""
        Write-Host "Block winner table (from node-$($bestNode.Index) at height $bestHeight):"
        $winners = Get-BlockWinners -Db $bestNode.Db -MaxHeight $bestHeight
        $summary.block_winners = $winners
        $winners | Format-Table height, miner, reward_zka, hash -AutoSize
    }
}
finally {
    foreach ($node in $nodes) {
        Stop-Node $node
    }
}

$summaryPath = Join-Path $OutDir "summary.json"
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8

$allOk = $true
foreach ($result in $summary.results) {
    if ($result.exit_code -ne 0 -or !$result.listening -or !$result.startup_ok -or !$result.clean_time_limit) {
        $allOk = $false
    }
    if (!$NoMine -and $result.height -lt $TargetHeight) {
        Write-Warning "Node $($result.node) reached height $($result.height), target was $TargetHeight"
        $allOk = $false
    }
}
$connectedNodes = @($summary.results | Where-Object { $_.node -gt 1 -and $_.connected }).Count
if ($connectedNodes -lt ($NodeCount - 1)) {
    $allOk = $false
}

Write-Host ""
Write-Host "Summary: $summaryPath"
$summary.results | Format-Table node, exit_code, listening, connected, startup_ok, clean_time_limit, height -AutoSize

if (!$allOk) {
    throw "local multi-node rehearsal failed; inspect $summaryPath"
}

Write-Host "Local multi-node rehearsal passed."
