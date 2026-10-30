# Prueba del Modo Tor contra la testnet v2 real (docs/MODO-TOR.md, E4b): un
# nodo nuevo, en una carpeta temporal, con --tor por un tor.exe propio
# (cliente, SOCKS 19050) y las semillas de un manifiesto, sincroniza la cadena
# solo por Tor a traves de la semilla .onion. No toca los datos del usuario.
#
# Uso: powershell -File scripts\probar-modo-tor-testnet.ps1 [-Manifest docs\quantic-testnet-v2-network.v1.json] [-Seeds builtin]
param(
    [string]$Zoka = (Join-Path $PSScriptRoot "..\target\release\zoka.exe"),
    [string]$Tor = (Join-Path $PSScriptRoot "..\third_party\tor\windows-x86_64-0.4.9.14\tor.exe"),
    [string]$Seeds = (Join-Path $PSScriptRoot "..\docs\quantic-testnet-v2-network.v1.json"),
    [int]$Minutes = 10
)
$ErrorActionPreference = "Stop"
$Zoka = (Resolve-Path $Zoka).Path
$Tor = (Resolve-Path $Tor).Path
if ($Seeds -ne "builtin") { $Seeds = (Resolve-Path $Seeds).Path }
$work = Join-Path ([IO.Path]::GetTempPath()) ("zoka-tor-testnet-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $work | Out-Null
$procs = @()
function Say([string]$text) { Write-Host ("[{0}] {1}" -f (Get-Date -Format HH:mm:ss), $text) }
function Get-Status([string]$url) { try { Invoke-RestMethod $url -TimeoutSec 5 } catch { $null } }
try {
    $slash = { param($p) $p.Replace('\', '/') }
    $torrc = Join-Path $work "torrc"
    @("SocksPort 127.0.0.1:19050", "DataDirectory $(& $slash (Join-Path $work 'tor-data'))",
      "Log notice file $(& $slash (Join-Path $work 'tor.log'))") | Set-Content -Path $torrc -Encoding ascii
    $procs += Start-Process -FilePath $Tor -ArgumentList @("-f", "`"$torrc`"") -WindowStyle Hidden -PassThru
    $torLog = Join-Path $work "tor.log"
    $deadline = (Get-Date).AddMinutes(4)
    while (-not ((Test-Path $torLog) -and (Select-String -Path $torLog -Pattern "Bootstrapped 100%" -Quiet))) {
        if ((Get-Date) -gt $deadline) { throw "Tor no termino de arrancar en 4 minutos" }
        Start-Sleep -Seconds 2
    }
    Say "Tor (cliente) listo"
    $env:ZOKA_PQ_PASSWORD = "solo-prueba-tor-testnet"
    $env:RUST_LOG = "warn"
    $args_ = @("quantic-pq-node", "serve", "--network", "testnet", "--db", "`"$(Join-Path $work 'db')`"",
        "--identity", "`"$(Join-Path $work 'node.id')`"", "--api", "127.0.0.1:19142",
        "--tor", "127.0.0.1:19050", "--seeds", "`"$Seeds`"")
    $procs += Start-Process -FilePath $Zoka -ArgumentList $args_ -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $work "node.out") -RedirectStandardError (Join-Path $work "node.err")
    Remove-Item Env:RUST_LOG
    $target = (Get-Status "https://api.zokascan.cc/status").height
    Say "Nodo en Modo Tor arrancado; la red va por la altura $target"
    $started = Get-Date
    $deadline = $started.AddMinutes($Minutes)
    $report = $started
    while ($true) {
        $mine = Get-Status "http://127.0.0.1:19142/status"
        if ($mine -and $target -and $mine.height -ge $target) {
            Say ("OK: el nodo en Modo Tor bajo la testnet real hasta la altura {0} en {1:n0} s, solo por Tor (pares salientes {2})" -f $mine.height, ((Get-Date) - $started).TotalSeconds, $mine.peers.outbound)
            exit 0
        }
        if ($mine -and (Get-Date) -ge $report) {
            Say ("  altura {0} de {1}, pares salientes {2}" -f $mine.height, $target, $mine.peers.outbound)
            $report = (Get-Date).AddSeconds(30)
        }
        if ((Get-Date) -gt $deadline) {
            Say ("FALLO: en {0} minutos llego a {1} de {2}" -f $Minutes, $mine.height, $target)
            Get-Content (Join-Path $work "node.out") -Tail 3
            Get-Content (Join-Path $work "node.err") -Tail 20
            exit 1
        }
        Start-Sleep -Seconds 5
    }
} finally {
    Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
    foreach ($p in $procs) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } }
    Start-Sleep -Seconds 2
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
