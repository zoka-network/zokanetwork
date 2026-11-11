# Minero de Zoka Quantic Testnet v3 por línea de comandos, para Windows.
#
# Todo queda dentro de la carpeta del paquete, en .\data\quantic-testnet-v2,
# con el mismo orden que ZSilent 4.0: node (la cadena y la identidad del nodo),
# wallets y wallet-mirror (la copia de la cadena que leen las wallets). El nodo
# llega a la red por los relays de n0, sin abrir puertos. Su API va en
# 127.0.0.1:9180, así que puede convivir con ZSilent; el Stratum para XMRig, en
# 127.0.0.1:3333, el mismo puerto que el Stratum de ZSilent (-StratumPort lo
# cambia). No tiene sentido minar con los dos a la vez: se reparten la CPU.
#
# En la testnet el nodo no mina solo: mina XMRig a través de su Stratum. Si
# XMRig está al lado de este script, "mine" lo arranca.
#
# "mine" queda vigilado: si XMRig se cierra vuelve a los 10 s, y si se cae el
# nodo se rearman los dos. "autostart" lo hace arrancar al iniciar sesión.
#
# Uso: zoka-miner.ps1 <create-wallet | restore-wallet | address | view-key | mine | autostart [-Off] | balance | status | node-id> [-Wallet nombre] [-Threads n] [-StratumPort n] [-Xmrig ruta]

param(
    [Parameter(Position = 0)]
    [ValidateSet("create-wallet", "restore-wallet", "address", "view-key", "mine", "autostart", "balance", "status", "node-id", "help")]
    [string]$Command = "help",
    [string]$Wallet = "miner",
    [int]$Threads = 0,
    [int]$StratumPort = 3333,
    [string]$Xmrig = "",
    # Modo Tor (opcional): el SOCKS de un Tor que ya corre, 127.0.0.1:9150 el
    # de Tor Browser o 127.0.0.1:9050 el de tor.exe.
    [string]$Tor = "",
    # Con "autostart": deja de arrancar con Windows.
    [switch]$Off
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
$Zoka = Join-Path $Root "zoka.exe"
$Network = "testnet"
$Data = Join-Path $Root "data\quantic-testnet-v3"
# La testnet v3 es una cadena nueva: el nodo empieza de cero, y las wallets de
# la v2 (las mismas frases, las mismas direcciones) pasan una sola vez.
$Old = Join-Path $Root "data\quantic-testnet-v2"
$Marker = Join-Path $Data "migrated-from-v2"
if ((Test-Path -LiteralPath (Join-Path $Old "wallets")) -and -not (Test-Path -LiteralPath $Marker)) {
    New-Item -ItemType Directory -Force -Path (Join-Path $Data "wallets") | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $Old "wallets") -File | ForEach-Object {
        $to = Join-Path (Join-Path $Data "wallets") $_.Name
        if (-not (Test-Path -LiteralPath $to)) { Copy-Item -LiteralPath $_.FullName -Destination $to }
    }
    New-Item -ItemType File -Path $Marker | Out-Null
}
$NodeDir = Join-Path $Data "node"
$Descriptor = Join-Path $NodeDir "peer.json"
$ApiBind = "127.0.0.1:9180"
$Api = "http://$ApiBind"
$Stratum = "127.0.0.1:$StratumPort"
$NodeLog = Join-Path $Data "node.log"
$NodeErrLog = Join-Path $Data "node-errors.log"

New-Item -ItemType Directory -Force -Path $NodeDir | Out-Null

function Say([string]$Text, [string]$Color = "Gray") { Write-Host $Text -ForegroundColor $Color }

function Read-Password([string]$Prompt) {
    $secure = Read-Host -Prompt $Prompt -AsSecureString
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try { return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr) }
}

# La contraseña viaja a zoka.exe por una variable de entorno de este proceso:
# nunca como argumento (la vería cualquiera que liste los procesos) ni en pantalla.
function Invoke-WithPassword([string]$Password, [scriptblock]$Action) {
    $env:ZOKA_CLI_PASSWORD = $Password
    try { & $Action } finally { Remove-Item Env:ZOKA_CLI_PASSWORD -ErrorAction SilentlyContinue }
}

function Test-WalletExists { return (Test-Path -LiteralPath (Join-Path $Data "wallets\$Wallet.json")) }

function Get-Address([switch]$Full) {
    $extra = @()
    if ($Full) { $extra = @("--full") }
    $out = & $Zoka quantic-pq-wallet address --network $Network --dir $Data --name $Wallet @extra 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not read the address of wallet '$Wallet': $out" }
    return ("" + $out).Trim()
}

function Test-NodeOnline {
    try { Invoke-RestMethod -Uri "$Api/status" -TimeoutSec 3 | Out-Null; return $true } catch { return $false }
}

# La identidad del nodo se cifra con 32 bytes al azar que se crean la primera
# vez, como en ZSilent 4.0. Nunca se muestran.
function Get-NodeSecret {
    $file = Join-Path $NodeDir "node.identity.secret"
    if (-not (Test-Path -LiteralPath $file)) {
        $bytes = New-Object byte[] 32
        [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
        [System.IO.File]::WriteAllText($file, (($bytes | ForEach-Object { $_.ToString("x2") }) -join ""))
    }
    $secret = [System.IO.File]::ReadAllText($file).Trim()
    if ($secret.Length -eq 0) { throw "$file is empty" }
    return $secret
}

# XMRig no viene en el paquete: -Xmrig, o xmrig.exe al lado de este script (suelto
# o en la carpeta del zip oficial, xmrig-<versión>), o en el PATH.
function Find-Xmrig {
    if ($Xmrig) {
        if (-not (Test-Path -LiteralPath $Xmrig -PathType Leaf)) { throw "XMRig not found at $Xmrig" }
        return (Resolve-Path -LiteralPath $Xmrig).Path
    }
    $here = Join-Path $Root "xmrig.exe"
    if (Test-Path -LiteralPath $here -PathType Leaf) { return $here }
    $nested = Get-ChildItem -LiteralPath $Root -Directory -Filter "xmrig*" -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName "xmrig.exe" } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if ($nested) { return $nested }
    $onPath = Get-Command xmrig.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($onPath) { return $onPath.Source }
    return $null
}

# Start-Process junta los argumentos con espacios: los que tienen espacios van
# entre comillas (la carpeta del paquete puede tenerlos).
function Join-Arguments([string[]]$Values) {
    return ($Values | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join " "
}

function Start-NodeProcess([string]$address) {
    # El nodo nunca pisa un archivo: el descriptor de la corrida anterior se borra.
    if (Test-Path -LiteralPath $Descriptor) { Remove-Item -LiteralPath $Descriptor }
    $argv = @("quantic-pq-node", "serve", "--network", $Network,
        "--db", (Join-Path $NodeDir "db"), "--identity", (Join-Path $NodeDir "node.identity"),
        "--api", $ApiBind, "--descriptor-out", $Descriptor, "--stratum", $Stratum, "--mine-to", $address)
    if ($Tor) { $argv += @("--tor", $Tor) }
    $env:ZOKA_PQ_PASSWORD = Get-NodeSecret
    try {
        return Start-Process -FilePath $Zoka -ArgumentList (Join-Arguments $argv) -NoNewWindow -PassThru `
            -RedirectStandardOutput $NodeLog -RedirectStandardError $NodeErrLog
    } finally {
        Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
    }
}

function Stop-NodeProcess($node) {
    # Ctrl+C también le llega al nodo, que cierra su base solo; si no termina, se corta.
    if ($node -and -not $node.HasExited -and -not $node.WaitForExit(15000)) {
        Stop-Process -Id $node.Id -Force -ErrorAction SilentlyContinue
    }
}

# Mina vigilado: si XMRig se cierra, vuelve a los 10 s; si se cae el nodo, se
# rearman los dos a los 15 s. Solo Ctrl+C (o cerrar la ventana) lo para.
function Start-Mining {
    if (-not (Test-WalletExists)) {
        throw "There is no wallet '$Wallet' yet. Run 1-create-wallet.cmd (or restore-wallet.cmd) first."
    }
    if (Test-NodeOnline) { throw "A node is already running on $Api. Close it first." }
    $address = Get-Address -Full
    $short = Get-Address
    $cores = [Environment]::ProcessorCount
    if ($Threads -le 0) { $Threads = [Math]::Max(1, $cores - 1) }
    $miner = Find-Xmrig
    if ($Tor) { Say "Tor mode: the node connects only through Tor at $Tor, to the network's onion seeds." "Cyan" }

    $round = 0
    $xmrigProcess = $null
    while ($true) {
        $round++
        $node = Start-NodeProcess $address
        try {
            Say "Starting the node and joining the network..."
            $deadline = (Get-Date).AddMinutes(3)
            $online = $false
            while (-not $online) {
                $online = Test-NodeOnline
                if ($online) { break }
                if ($node.HasExited -or (Get-Date) -gt $deadline) {
                    # La primera vez es un error para ver; después, se reintenta solo.
                    if ($round -eq 1) { throw "The node did not start. Details in $NodeErrLog" }
                    break
                }
                Start-Sleep -Seconds 2
            }
            if ($online) {
                # Publica la dirección corta en la red, así cualquiera puede pagarte con ella.
                & $Zoka quantic-pq-wallet publish-address --network $Network --dir $Data --name $Wallet --api $Api 2>&1 | Out-Null
                if ($LASTEXITCODE -ne 0 -and $round -eq 1) { Say "Your short address could not be published yet; the full one always works." "Yellow" }
                if ($round -eq 1) {
                    Say ""
                    Say "Mining rewards go to wallet '$Wallet': $short" "Cyan"
                    Say "The node's log is $NodeLog" "Cyan"
                }
                if ($miner) {
                    if ($round -eq 1) {
                        Say "Mining with XMRig, $Threads of $cores CPU threads (-Threads n changes it). Ctrl+C stops both." "Cyan"
                        Say ""
                    }
                    # XMRig no termina si el nodo se cae (reintenta el Stratum
                    # para siempre): corre aparte, en esta misma consola, y se
                    # vigilan los dos.
                    while (-not $node.HasExited) {
                        $xmrigProcess = Start-Process -FilePath $miner -NoNewWindow -PassThru `
                            -ArgumentList (Join-Arguments @("-o", $Stratum, "-u", $Wallet, "-p", "x", "-a", "rx/0", "-t", "$Threads"))
                        $null = $xmrigProcess.Handle  # sin esto, ExitCode queda vacío
                        while (-not $node.HasExited -and -not $xmrigProcess.HasExited) { Start-Sleep -Seconds 2 }
                        if ($node.HasExited) { break }
                        Say "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') XMRig stopped (exit code $($xmrigProcess.ExitCode)). Starting it again in 10 s; Ctrl+C stops everything." "Yellow"
                        Start-Sleep -Seconds 10
                    }
                } else {
                    if ($round -eq 1) {
                        Say "XMRig was not found next to this script, so the node is waiting for miners. Start it with:" "Yellow"
                        Say "  xmrig -o $Stratum -u $Wallet -p x -a rx/0 -t $Threads" "Yellow"
                        Say "Ctrl+C stops the node." "Cyan"
                        Say ""
                    }
                    $shown = 0
                    while (-not $node.HasExited) {
                        Start-Sleep -Seconds 2
                        $lines = @(Get-Content -LiteralPath $NodeLog -ErrorAction SilentlyContinue)
                        if ($lines.Count -gt $shown) { $lines[$shown..($lines.Count - 1)] | ForEach-Object { Say $_ }; $shown = $lines.Count }
                    }
                }
            }
            Say "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') The node stopped. Details in $NodeErrLog. Starting it again in 15 s; Ctrl+C stops." "Yellow"
        } finally {
            if ($xmrigProcess -and -not $xmrigProcess.HasExited) {
                Stop-Process -Id $xmrigProcess.Id -Force -ErrorAction SilentlyContinue
            }
            $xmrigProcess = $null
            Stop-NodeProcess $node
        }
        Start-Sleep -Seconds 15
    }
}

# Arranque con Windows: un acceso directo en la carpeta Inicio de este usuario
# que abre "mine" minimizado al iniciar sesión, con las mismas opciones.
function Set-Autostart([bool]$On) {
    $startup = [Environment]::GetFolderPath("Startup")
    $link = Join-Path $startup "Zoka CLI miner ($Wallet).lnk"
    if (-not $On) {
        if (Test-Path -LiteralPath $link) {
            Remove-Item -LiteralPath $link
            Say "Mining no longer starts with Windows (wallet '$Wallet')." "Cyan"
        } else {
            Say "Mining was not set to start with Windows for wallet '$Wallet'." "Cyan"
        }
        return
    }
    if (-not (Test-WalletExists)) {
        throw "There is no wallet '$Wallet' yet. Run 1-create-wallet.cmd (or restore-wallet.cmd) first."
    }
    $mineArgs = @("mine", "-Wallet", $Wallet)
    if ($Threads -gt 0) { $mineArgs += @("-Threads", "$Threads") }
    if ($StratumPort -ne 3333) { $mineArgs += @("-StratumPort", "$StratumPort") }
    if ($Xmrig) { $mineArgs += @("-Xmrig", (Resolve-Path -LiteralPath $Xmrig).Path) }
    if ($Tor) { $mineArgs += @("-Tor", $Tor) }
    $shell = New-Object -ComObject WScript.Shell
    $lnk = $shell.CreateShortcut($link)
    $lnk.TargetPath = Join-Path $env:WINDIR "System32\WindowsPowerShell\v1.0\powershell.exe"
    $lnk.Arguments = "-NoProfile -ExecutionPolicy Bypass -File " + (Join-Arguments (@($PSCommandPath) + $mineArgs))
    $lnk.WorkingDirectory = $Root
    $lnk.WindowStyle = 7  # minimizada
    $lnk.Description = "Zoka Quantic CLI miner: mine with wallet '$Wallet'"
    $lnk.Save()
    Say "Mining now starts with Windows, minimized, when you sign in (wallet '$Wallet')." "Cyan"
    Say "It needs no password. To stop it from starting: dont-start-with-windows.cmd" "Cyan"
    Say "Shortcut: $link"
}

switch ($Command) {
    "create-wallet" {
        if (Test-WalletExists) { throw "Wallet '$Wallet' already exists. Use -Wallet <other name> for another one." }
        Say "A new wallet is protected by a password on this computer." "Cyan"
        $p1 = Read-Password "Choose a password"
        $p2 = Read-Password "Repeat the password"
        if ($p1 -ne $p2) { throw "The two passwords are different. Nothing was created." }
        if ($p1.Length -lt 8) { throw "Use at least 8 characters. Nothing was created." }
        Say ""
        Say "WRITE DOWN THE 24 WORDS BELOW, ON PAPER, AND KEEP THEM SAFE." "Yellow"
        Say "They are the only way to recover this wallet. They are shown only once," "Yellow"
        Say "and anyone who has them can spend your coins. Never type them on a website." "Yellow"
        Say ""
        Invoke-WithPassword $p1 { & $Zoka quantic-pq-wallet new --network $Network --dir $Data --name $Wallet --password-env ZOKA_CLI_PASSWORD }
        if ($LASTEXITCODE -ne 0) { throw "The wallet was not created." }
    }
    "restore-wallet" {
        if (Test-WalletExists) { throw "Wallet '$Wallet' already exists. Use -Wallet <other name>." }
        $phrase = Read-Password "Type your 24 words, separated by spaces (hidden)"
        $p1 = Read-Password "Choose a password for this computer"
        $p2 = Read-Password "Repeat the password"
        if ($p1 -ne $p2) { throw "The two passwords are different. Nothing was restored." }
        if ($p1.Length -lt 8) { throw "Use at least 8 characters. Nothing was restored." }
        $env:ZOKA_CLI_PHRASE = $phrase
        try {
            Say "Restored. Your address:" "Cyan"
            Invoke-WithPassword $p1 { & $Zoka quantic-pq-wallet restore --network $Network --dir $Data --name $Wallet --phrase-env ZOKA_CLI_PHRASE --password-env ZOKA_CLI_PASSWORD }
        } finally { Remove-Item Env:ZOKA_CLI_PHRASE -ErrorAction SilentlyContinue }
        if ($LASTEXITCODE -ne 0) { throw "The wallet was not restored." }
    }
    "address" {
        Say "Your address (share this one to get paid):" "Cyan"
        Get-Address
    }
    "view-key" {
        if (-not (Test-WalletExists)) { throw "There is no wallet '$Wallet' yet." }
        $p = Read-Password "Password of wallet '$Wallet'"
        Say "Your view key. It shows every payment and the balance of this wallet (paste it in" "Cyan"
        Say "https://zokascan.cc/wallet, which reads it inside your browser); it cannot spend." "Cyan"
        Say "Share it only as you would a bank statement: it cannot be revoked." "Yellow"
        Invoke-WithPassword $p { & $Zoka quantic-pq-wallet show-view-key --network $Network --dir $Data --name $Wallet --password-env ZOKA_CLI_PASSWORD --json } |
            ForEach-Object { if ($_ -match '"view_key":"([^"]+)"') { $Matches[1] } else { $_ } }
        if ($LASTEXITCODE -ne 0) { throw "Could not read the view key." }
    }
    "mine" { Start-Mining }
    "autostart" { Set-Autostart (-not $Off) }
    "balance" {
        if (-not (Test-WalletExists)) { throw "There is no wallet '$Wallet' yet." }
        # Con el nodo andando, la copia de la wallet se pone al día con él antes de contar.
        $peer = @()
        if ((Test-NodeOnline) -and (Test-Path -LiteralPath $Descriptor)) {
            $peer = @("--peer", $Descriptor)
        } else {
            Say "The node is not running: this is the balance as of the last time it ran." "Yellow"
        }
        $p = Read-Password "Password of wallet '$Wallet'"
        Invoke-WithPassword $p { & $Zoka quantic-pq-wallet balance --network $Network --dir $Data --name $Wallet --password-env ZOKA_CLI_PASSWORD --api $Api @peer }
    }
    "status" {
        if (-not (Test-NodeOnline)) { Say "The node is not running."; break }
        $s = Invoke-RestMethod -Uri "$Api/status" -TimeoutSec 5
        Say ("Height {0} | peers {1} out, {2} in | XMRig miners {3} | blocks found this session {4}" -f
            $s.height, $s.peers.outbound, $s.peers.inbound, $s.stratum.miners, $s.stratum.blocks_found)
    }
    "node-id" {
        # Los dos identificadores públicos que fija el manifiesto de una semilla.
        # Crea la identidad del nodo si todavía no existe; no muestra secretos.
        New-Item -ItemType Directory -Force -Path $NodeDir | Out-Null
        $env:ZOKA_PQ_PASSWORD = Get-NodeSecret
        try {
            $out = (& $Zoka quantic-pq-node identity --network $Network --identity (Join-Path $NodeDir "node.identity") 2>&1 | Out-String)
        } finally {
            Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
        }
        if ($LASTEXITCODE -ne 0) { throw "Could not read the node's identity: $out" }
        $entry = ($out | ConvertFrom-Json).seed_entry
        Say "Endpoint id:   $($entry.endpoint_id)"
        Say "ML-DSA key id: $($entry.ml_dsa_key_id)"
        Say "Both are public. To offer this node as a seed, keep it running all day and send them to the Zoka team." "Cyan"
    }
    default {
        Say "Zoka Quantic Testnet v3 CLI miner. Commands:"
        Say "  create-wallet   a new wallet (shows its 24 words once)"
        Say "  restore-wallet  a wallet from its 24 words"
        Say "  address         the address you share to get paid"
        Say "  view-key        a key that shows your balance (zokascan.cc/wallet) and cannot spend"
        Say "  mine            run the node, and XMRig if it is next to this script (-Threads n); restarts them if they stop"
        Say "  autostart       start mining when you sign in to Windows (autostart -Off undoes it)"
        Say "  balance         your balance"
        Say "  status          height, peers and miners"
        Say "  node-id         this node's public ids, to offer it as a seed"
        Say "Options: -Wallet <name> (default: miner), -Threads n, -StratumPort n (default: 3333), -Xmrig <path to xmrig.exe>,"
        Say "  -Tor 127.0.0.1:9150 (optional Tor mode, through a running Tor such as Tor Browser's)."
        Say "Data lives in .\data next to this script."
    }
}
