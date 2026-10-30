# Minero de Zoka Quantic Testnet v2 por línea de comandos, para Windows.
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
# Uso: zoka-miner.ps1 <create-wallet | restore-wallet | address | view-key | mine | balance | status> [-Wallet nombre] [-Threads n] [-StratumPort n] [-Xmrig ruta]

param(
    [Parameter(Position = 0)]
    [ValidateSet("create-wallet", "restore-wallet", "address", "view-key", "mine", "balance", "status", "help")]
    [string]$Command = "help",
    [string]$Wallet = "miner",
    [int]$Threads = 0,
    [int]$StratumPort = 3333,
    [string]$Xmrig = ""
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
$Zoka = Join-Path $Root "zoka.exe"
$Network = "testnet"
$V2 = Join-Path $Root "data\quantic-testnet-v2"
$NodeDir = Join-Path $V2 "node"
$Descriptor = Join-Path $NodeDir "peer.json"
$ApiBind = "127.0.0.1:9180"
$Api = "http://$ApiBind"
$Stratum = "127.0.0.1:$StratumPort"

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

function Test-WalletExists { return (Test-Path -LiteralPath (Join-Path $V2 "wallets\$Wallet.json")) }

function Get-Address([switch]$Full) {
    $extra = @()
    if ($Full) { $extra = @("--full") }
    $out = & $Zoka quantic-pq-wallet address --network $Network --dir $V2 --name $Wallet @extra 2>&1
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

    # El nodo nunca pisa un archivo: el descriptor de la corrida anterior se borra.
    if (Test-Path -LiteralPath $Descriptor) { Remove-Item -LiteralPath $Descriptor }
    $log = Join-Path $V2 "node.log"
    $errLog = Join-Path $V2 "node-errors.log"
    $argv = @("quantic-pq-node", "serve", "--network", $Network,
        "--db", (Join-Path $NodeDir "db"), "--identity", (Join-Path $NodeDir "node.identity"),
        "--api", $ApiBind, "--descriptor-out", $Descriptor, "--stratum", $Stratum, "--mine-to", $address)

    $env:ZOKA_PQ_PASSWORD = Get-NodeSecret
    try {
        $node = Start-Process -FilePath $Zoka -ArgumentList (Join-Arguments $argv) -NoNewWindow -PassThru `
            -RedirectStandardOutput $log -RedirectStandardError $errLog
    } finally {
        Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
    }
    try {
        Say "Starting the node and joining the network..."
        $deadline = (Get-Date).AddMinutes(3)
        while (-not (Test-NodeOnline)) {
            if ($node.HasExited) { throw "The node stopped. Details in $errLog" }
            if ((Get-Date) -gt $deadline) { throw "The node did not start in 3 minutes. Details in $errLog" }
            Start-Sleep -Seconds 2
        }
        # Publica la dirección corta en la red, así cualquiera puede pagarte con ella.
        & $Zoka quantic-pq-wallet publish-address --network $Network --dir $V2 --name $Wallet --api $Api 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { Say "Your short address could not be published yet; the full one always works." "Yellow" }

        Say ""
        Say "Mining rewards go to wallet '$Wallet': $short" "Cyan"
        Say "The node's log is $log" "Cyan"
        if ($miner) {
            Say "Mining with XMRig, $Threads of $cores CPU threads (-Threads n changes it). Ctrl+C stops both." "Cyan"
            Say ""
            & $miner -o $Stratum -u $Wallet -p x -a rx/0 -t $Threads
            Say "XMRig stopped (exit code $LASTEXITCODE)." "Yellow"
        } else {
            Say "XMRig was not found next to this script, so the node is waiting for miners. Start it with:" "Yellow"
            Say "  xmrig -o $Stratum -u $Wallet -p x -a rx/0 -t $Threads" "Yellow"
            Say "Ctrl+C stops the node." "Cyan"
            Say ""
            $shown = 0
            while (-not $node.HasExited) {
                Start-Sleep -Seconds 2
                $lines = @(Get-Content -LiteralPath $log -ErrorAction SilentlyContinue)
                if ($lines.Count -gt $shown) { $lines[$shown..($lines.Count - 1)] | ForEach-Object { Say $_ }; $shown = $lines.Count }
            }
            Say "The node stopped. Details in $errLog" "Yellow"
        }
    } finally {
        # Ctrl+C también le llega al nodo, que cierra su base solo; si no termina, se corta.
        if (-not $node.HasExited -and -not $node.WaitForExit(15000)) {
            Stop-Process -Id $node.Id -Force -ErrorAction SilentlyContinue
        }
        Say "Node stopped."
    }
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
        Invoke-WithPassword $p1 { & $Zoka quantic-pq-wallet new --network $Network --dir $V2 --name $Wallet --password-env ZOKA_CLI_PASSWORD }
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
            Invoke-WithPassword $p1 { & $Zoka quantic-pq-wallet restore --network $Network --dir $V2 --name $Wallet --phrase-env ZOKA_CLI_PHRASE --password-env ZOKA_CLI_PASSWORD }
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
        Invoke-WithPassword $p { & $Zoka quantic-pq-wallet show-view-key --network $Network --dir $V2 --name $Wallet --password-env ZOKA_CLI_PASSWORD --json } |
            ForEach-Object { if ($_ -match '"view_key":"([^"]+)"') { $Matches[1] } else { $_ } }
        if ($LASTEXITCODE -ne 0) { throw "Could not read the view key." }
    }
    "mine" { Start-Mining }
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
        Invoke-WithPassword $p { & $Zoka quantic-pq-wallet balance --network $Network --dir $V2 --name $Wallet --password-env ZOKA_CLI_PASSWORD --api $Api @peer }
    }
    "status" {
        if (-not (Test-NodeOnline)) { Say "The node is not running."; break }
        $s = Invoke-RestMethod -Uri "$Api/status" -TimeoutSec 5
        Say ("Height {0} | peers {1} out, {2} in | XMRig miners {3} | blocks found this session {4}" -f
            $s.height, $s.peers.outbound, $s.peers.inbound, $s.stratum.miners, $s.stratum.blocks_found)
    }
    default {
        Say "Zoka Quantic Testnet v2 CLI miner. Commands:"
        Say "  create-wallet   a new wallet (shows its 24 words once)"
        Say "  restore-wallet  a wallet from its 24 words"
        Say "  address         the address you share to get paid"
        Say "  view-key        a key that shows your balance (zokascan.cc/wallet) and cannot spend"
        Say "  mine            run the node, and XMRig if it is next to this script (-Threads n)"
        Say "  balance         your balance"
        Say "  status          height, peers and miners"
        Say "Options: -Wallet <name> (default: miner), -Threads n, -StratumPort n (default: 3333), -Xmrig <path to xmrig.exe>."
        Say "Data lives in .\data next to this script."
    }
}
