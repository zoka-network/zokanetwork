param(
    [string]$Manifest = "docs/mainnet-network.v1.json",
    [int]$TimeoutSeconds = 5,
    [int]$ReadyWaitSeconds = 0,
    [switch]$RequireReady,
    [switch]$Json
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$manifestPath = if ([System.IO.Path]::IsPathRooted($Manifest)) {
    $Manifest
} else {
    Join-Path $root $Manifest
}

if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "Manifest file not found: $manifestPath"
}

function ConvertFrom-MultiaddrEndpoint {
    param([string]$Multiaddr)

    if ([string]::IsNullOrWhiteSpace($Multiaddr)) {
        return $null
    }

    $match = [regex]::Match(
        $Multiaddr,
        "/(?<kind>ip4|ip6|dns4|dns6|dns)/(?<host>[^/]+)/tcp/(?<port>[0-9]+)"
    )
    if (-not $match.Success) {
        return $null
    }

    [pscustomobject]@{
        Host = $match.Groups["host"].Value
        Port = [int]$match.Groups["port"].Value
    }
}

function Format-ErrorSummary {
    param([string]$Message)

    if ([string]::IsNullOrWhiteSpace($Message)) {
        return $null
    }

    $summary = ($Message -replace "\s+", " ").Trim()
    if ($summary -match "timed out|tiempo de espera|timeout") {
        return "timeout"
    }
    if ($summary.Length -gt 120) {
        return $summary.Substring(0, 117) + "..."
    }
    $summary
}

function Test-TcpEndpoint {
    param(
        [string]$HostName,
        [int]$Port,
        [int]$TimeoutMs
    )

    $client = [System.Net.Sockets.TcpClient]::new()
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $connect = $client.BeginConnect($HostName, $Port, $null, $null)
        if (-not $connect.AsyncWaitHandle.WaitOne($TimeoutMs)) {
            return [pscustomobject]@{
                Ok = $false
                Status = "timeout"
                LatencyMs = $timer.ElapsedMilliseconds
                Error = "tcp timeout after ${TimeoutMs}ms"
            }
        }
        $client.EndConnect($connect)
        return [pscustomobject]@{
            Ok = $true
            Status = "open"
            LatencyMs = $timer.ElapsedMilliseconds
            Error = $null
        }
    } catch {
        return [pscustomobject]@{
            Ok = $false
            Status = "failed"
            LatencyMs = $timer.ElapsedMilliseconds
            Error = Format-ErrorSummary $_.Exception.Message
        }
    } finally {
        $timer.Stop()
        $client.Close()
    }
}

function Invoke-HttpCheck {
    param(
        [string]$BaseUrl,
        [string]$Path,
        [int]$TimeoutSeconds
    )

    $uri = ($BaseUrl.TrimEnd("/") + $Path)
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $response = Invoke-WebRequest -UseBasicParsing -Uri $uri -TimeoutSec $TimeoutSeconds
        return [pscustomobject]@{
            Ok = $true
            Url = $uri
            StatusCode = [int]$response.StatusCode
            LatencyMs = $timer.ElapsedMilliseconds
            Error = $null
        }
    } catch {
        $statusCode = $null
        if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
            $statusCode = [int]$_.Exception.Response.StatusCode
        }
        return [pscustomobject]@{
            Ok = $false
            Url = $uri
            StatusCode = $statusCode
            LatencyMs = $timer.ElapsedMilliseconds
            Error = Format-ErrorSummary $_.Exception.Message
        }
    } finally {
        $timer.Stop()
    }
}

function Wait-Ready {
    param(
        [string]$BaseUrl,
        [int]$TimeoutSeconds,
        [int]$ReadyWaitSeconds
    )

    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($ReadyWaitSeconds)
    do {
        $check = Invoke-HttpCheck -BaseUrl $BaseUrl -Path "/ready" -TimeoutSeconds $TimeoutSeconds
        if ($check.StatusCode -eq 200) {
            return $check
        }
        if ($ReadyWaitSeconds -le 0) {
            return $check
        }
        Start-Sleep -Seconds 2
    } while ([DateTimeOffset]::UtcNow -lt $deadline)

    $check
}

$manifestJson = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$timeoutMs = [Math]::Max(1, $TimeoutSeconds) * 1000

$seedResults = @()
foreach ($seed in @($manifestJson.seed_nodes)) {
    if ($seed.status -and $seed.status -ne "active") {
        continue
    }
    $endpoint = ConvertFrom-MultiaddrEndpoint -Multiaddr $seed.multiaddr
    if ($null -eq $endpoint) {
        $seedResults += [pscustomobject]@{
            Type = "p2p"
            Label = $seed.label
            Target = $seed.multiaddr
            Ok = $false
            Status = "invalid_multiaddr"
            LatencyMs = 0
            Error = "unsupported multiaddr"
        }
        continue
    }

    $tcp = Test-TcpEndpoint -HostName $endpoint.Host -Port $endpoint.Port -TimeoutMs $timeoutMs
    $seedResults += [pscustomobject]@{
        Type = "p2p"
        Label = $seed.label
        Target = "$($endpoint.Host):$($endpoint.Port)"
        Ok = $tcp.Ok
        Status = $tcp.Status
        LatencyMs = $tcp.LatencyMs
        Error = $tcp.Error
    }
}

$rpcResults = @()
foreach ($rpc in @($manifestJson.public_rpc)) {
    if ($rpc.status -and $rpc.status -ne "active") {
        continue
    }

    $health = Invoke-HttpCheck -BaseUrl $rpc.url -Path "/health" -TimeoutSeconds $TimeoutSeconds
    $ready = Wait-Ready -BaseUrl $rpc.url -TimeoutSeconds $TimeoutSeconds -ReadyWaitSeconds $ReadyWaitSeconds
    $sync = Invoke-HttpCheck -BaseUrl $rpc.url -Path "/sync/status" -TimeoutSeconds $TimeoutSeconds
    $runtime = Invoke-HttpCheck -BaseUrl $rpc.url -Path "/network/runtime" -TimeoutSeconds $TimeoutSeconds

    foreach ($check in @(
        [pscustomobject]@{ Name = "health"; Check = $health; RequiredStatus = 200 },
        [pscustomobject]@{ Name = "ready"; Check = $ready; RequiredStatus = 200 },
        [pscustomobject]@{ Name = "sync_status"; Check = $sync; RequiredStatus = 200 },
        [pscustomobject]@{ Name = "network_runtime"; Check = $runtime; RequiredStatus = 200 }
    )) {
        $isReadyCheck = $check.Name -eq "ready"
        $ok = $check.Check.StatusCode -eq $check.RequiredStatus
        if ($isReadyCheck -and -not $RequireReady) {
            $ok = $check.Check.StatusCode -in @(200, 503)
        }
        $rpcResults += [pscustomobject]@{
            Type = "rpc"
            Label = $rpc.label
            Target = $check.Check.Url
            Check = $check.Name
            Ok = $ok
            Status = $check.Check.StatusCode
            LatencyMs = $check.Check.LatencyMs
            Error = $check.Check.Error
        }
    }
}

$allResults = @($seedResults + $rpcResults)
$failed = @($allResults | Where-Object { -not $_.Ok })

if ($Json) {
    [pscustomobject]@{
        manifest = $manifestPath
        ok = $failed.Count -eq 0
        failed = $failed.Count
        results = $allResults
    } | ConvertTo-Json -Depth 8
} else {
    Write-Host "Manifest: $manifestPath"
    Write-Host "Checks: $($allResults.Count), failed: $($failed.Count)"
    Write-Host ""
    $allResults | Format-Table Type, Label, Check, Target, Ok, Status, LatencyMs, Error -AutoSize
}

if ($failed.Count -gt 0) {
    exit 1
}
