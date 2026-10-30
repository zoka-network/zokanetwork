# Prueba de punta a punta del Modo Tor (docs/MODO-TOR.md, E4), con dos nodos
# reales del binario en una red regtest propia y un tor.exe propio:
#   - la semilla mina y escucha detras de una .onion (--stream-listen y
#     --stream-address);
#   - el nodo en Modo Tor (--tor) solo sale por el SOCKS de ese Tor y tiene
#     que alcanzar la altura de la semilla.
# No toca la testnet ni ningun dato del usuario: todo vive en una carpeta
# temporal que se borra al final. Necesita la red Tor.
#
# Uso: powershell -File scripts\probar-modo-tor.ps1 [-Zoka target\release\zoka.exe] [-Tor ...\tor.exe]
param(
    [string]$Zoka = (Join-Path $PSScriptRoot "..\target\release\zoka.exe"),
    [string]$Tor = (Join-Path $PSScriptRoot "..\third_party\tor\windows-x86_64-0.4.9.14\tor.exe"),
    [int]$Minutes = 8
)
$ErrorActionPreference = "Stop"
$Zoka = (Resolve-Path $Zoka).Path
$Tor = (Resolve-Path $Tor).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("zoka-modo-tor-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $work | Out-Null
$procs = @()
function Say([string]$text) { Write-Host ("[{0}] {1}" -f (Get-Date -Format HH:mm:ss), $text) }
function Get-Status([int]$port) { try { Invoke-RestMethod "http://127.0.0.1:$port/status" -TimeoutSec 3 } catch { $null } }
try {
    # 1. Tor: cliente (SOCKS 19050) y servicio .onion hacia el puerto de la semilla.
    $slash = { param($p) $p.Replace('\', '/') }
    $torrc = Join-Path $work "torrc"
    @(
        "SocksPort 127.0.0.1:19050",
        "DataDirectory $(& $slash (Join-Path $work 'tor-data'))",
        "HiddenServiceDir $(& $slash (Join-Path $work 'hs'))",
        "HiddenServicePort 9444 127.0.0.1:19444",
        "Log notice file $(& $slash (Join-Path $work 'tor.log'))"
    ) | Set-Content -Path $torrc -Encoding ascii
    $procs += Start-Process -FilePath $Tor -ArgumentList @("-f", "`"$torrc`"") -WindowStyle Hidden -PassThru
    $deadline = (Get-Date).AddMinutes(4)
    $torLog = Join-Path $work "tor.log"
    while (-not ((Test-Path $torLog) -and (Select-String -Path $torLog -Pattern "Bootstrapped 100%" -Quiet))) {
        if ((Get-Date) -gt $deadline) { throw "Tor no termino de arrancar en 4 minutos" }
        Start-Sleep -Seconds 2
    }
    $onion = (Get-Content (Join-Path $work "hs\hostname")).Trim()
    Say "Tor listo; la semilla va a ser $onion`:9444"

    # 2. Wallet de regtest para la semilla, y la semilla.
    $env:ZOKA_PQ_PASSWORD = "solo-prueba-modo-tor"
    $wallet = & $Zoka quantic-pq-wallet new --network regtest --dir (Join-Path $work "wallets") --name semilla --password-env ZOKA_PQ_PASSWORD --json | ConvertFrom-Json
    $seedArgs = @("quantic-pq-node", "serve", "--network", "regtest", "--db", "`"$(Join-Path $work 'semilla-db')`"",
        "--identity", "`"$(Join-Path $work 'semilla.id')`"", "--api", "127.0.0.1:19140",
        "--descriptor-out", "`"$(Join-Path $work 'semilla.json')`"", "--mine-to", $wallet.address, "--interval", "10",
        "--stream-listen", "127.0.0.1:19444", "--stream-address", "$onion`:9444")
    $procs += Start-Process -FilePath $Zoka -ArgumentList $seedArgs -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $work "semilla.out") -RedirectStandardError (Join-Path $work "semilla.err")
    $deadline = (Get-Date).AddMinutes(2)
    while (-not (Test-Path (Join-Path $work "semilla.json")) -or -not (Get-Status 19140)) {
        if ((Get-Date) -gt $deadline) { throw "La semilla no arranco: $(Get-Content (Join-Path $work 'semilla.err') -Raw)" }
        Start-Sleep -Seconds 1
    }
    $descriptor = Get-Content (Join-Path $work "semilla.json") -Raw
    if ($descriptor -notmatch "Custom|custom") { Say "OJO: el descriptor no muestra la direccion de flujo: $descriptor" }
    Say "Semilla minando (altura $((Get-Status 19140).height))"

    # Una .onion nueva tarda en publicarse; una semilla real lleva tiempo
    # prendida. Se espera a que su puerto responda por el SOCKS de Tor.
    $onionReady = (Get-Date).AddMinutes(4)
    while ($true) {
        $probe = New-Object Net.Sockets.TcpClient
        try {
            $probe.Connect("127.0.0.1", 19050)
            $s = $probe.GetStream()
            $s.Write([byte[]](5, 1, 0), 0, 3); $r = New-Object byte[] 2; [void]$s.Read($r, 0, 2)
            $host_ = [Text.Encoding]::ASCII.GetBytes($onion)
            $req = [byte[]](@(5, 1, 0, 3, $host_.Length) + $host_ + @(0x24, 0xE4))
            $s.Write($req, 0, $req.Length); $rep = New-Object byte[] 10; [void]$s.Read($rep, 0, 10)
            if ($rep[1] -eq 0) { break }
        } catch {} finally { $probe.Close() }
        if ((Get-Date) -gt $onionReady) { throw "La .onion de la semilla no respondio en 4 minutos" }
        Start-Sleep -Seconds 5
    }
    Say "La .onion de la semilla ya responde por Tor"

    # 3. El nodo en Modo Tor, solo con el descriptor de la semilla.
    $torArgs = @("quantic-pq-node", "serve", "--network", "regtest", "--db", "`"$(Join-Path $work 'tor-db')`"",
        "--identity", "`"$(Join-Path $work 'tor.id')`"", "--api", "127.0.0.1:19141",
        "--tor", "127.0.0.1:19050", "--peer", "`"$(Join-Path $work 'semilla.json')`"")
    $env:RUST_LOG = "warn,iroh=info"
    $procs += Start-Process -FilePath $Zoka -ArgumentList $torArgs -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $work "tor-node.out") -RedirectStandardError (Join-Path $work "tor-node.err")
    Remove-Item Env:RUST_LOG
    $started = Get-Date
    $deadline = $started.AddMinutes($Minutes)
    $report = $started
    while ($true) {
        $seed = Get-Status 19140
        $mine = Get-Status 19141
        # Tiene que seguir a la semilla todo el tiempo de la prueba, no solo
        # alcanzarla una vez: al final, a 2 bloques como mucho.
        if ((Get-Date) -gt $deadline -and $seed -and $mine -and $mine.height -ge 10 -and $mine.height -ge $seed.height - 2) {
            Say ("OK: durante {0} minutos el nodo en Modo Tor siguio a la semilla solo por Tor: altura {1} (semilla {2})" -f $Minutes, $mine.height, $seed.height)
            Say ("Pares del nodo Tor: salientes {0}, entrantes {1}" -f $mine.peers.outbound, $mine.peers.inbound)
            exit 0
        }
        if ($mine -and $seed -and (Get-Date) -ge $report) {
            Say ("  nodo Tor: altura {0}, salientes {1}, entrantes {2} | semilla: altura {3}, entrantes {4}" -f $mine.height, $mine.peers.outbound, $mine.peers.inbound, $seed.height, $seed.peers.inbound)
            $report = (Get-Date).AddSeconds(20)
        }
        if ((Get-Date) -gt $deadline) {
            Say ("FALLO: el nodo Tor no alcanzo a la semilla en {0} minutos (semilla {1}, nodo Tor {2})" -f $Minutes, $seed.height, $mine.height)
            Say "--- semilla.err"; Get-Content (Join-Path $work "semilla.err") -Tail 15
            Say "--- tor-node.out"; Get-Content (Join-Path $work "tor-node.out") -Tail 5
            Say "--- tor-node.err"; Get-Content (Join-Path $work "tor-node.err") -Tail 25
            $script:keep = $true
            exit 1
        }
        Start-Sleep -Seconds 5
    }
} finally {
    Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
    foreach ($p in $procs) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } }
    Start-Sleep -Seconds 2
    if ($script:keep) { Say "La carpeta queda para revisar: $work" }
    else { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}
