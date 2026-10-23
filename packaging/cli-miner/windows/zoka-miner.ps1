# Minero de Zoka Quantic por línea de comandos, para Windows.
#
# Todo queda dentro de la carpeta del paquete: los datos (cadena, wallets, Tor)
# en .\data. Usa puertos propios (API 3180, P2P 28181, Tor 19150), así que puede
# convivir con ZSilent en la misma PC; pero no tiene sentido minar con los dos a
# la vez: se reparten la CPU.
#
# Uso: zoka-miner.ps1 <create-wallet | restore-wallet | address | mine | balance | xmrig-node | status> [-Wallet nombre] [-Threads n]

param(
    [Parameter(Position = 0)]
    [ValidateSet("create-wallet", "restore-wallet", "address", "mine", "balance", "xmrig-node", "status", "help")]
    [string]$Command = "help",
    [string]$Wallet = "miner",
    [int]$Threads = 0,
    [int]$StratumPort = 3333
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
$Zoka = Join-Path $Root "zoka.exe"
$Network = "quantic-testnet"
$Data = Join-Path $Root "data"
$ApiBind = "127.0.0.1:3180"
$Api = "http://$ApiBind"
$Listen = "/ip4/127.0.0.1/tcp/28181"
$TorSocks = "127.0.0.1:19150"
$StratumBind = "0.0.0.0:$StratumPort"

$env:ZOKA_HOME = $Data
New-Item -ItemType Directory -Force -Path $Data | Out-Null

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

function Test-WalletExists {
    $file = Join-Path $Data ".zoka\$Network\quantic-wallets\$Wallet.json"
    return (Test-Path -LiteralPath $file)
}

function Get-ShortAddress {
    $out = & $Zoka quantic-wallet address --network $Network --name $Wallet 2>&1
    if ($LASTEXITCODE -ne 0) { throw "Could not read the address of wallet '$Wallet': $out" }
    return ("" + $out).Trim()
}

function Test-NodeOnline {
    try { Invoke-RestMethod -Uri "$Api/quantic/info" -TimeoutSec 3 | Out-Null; return $true } catch { return $false }
}

# Como ZSilent: barras normales, y entre comillas si la ruta tiene espacios.
function TorPath([string]$Path) {
    $p = $Path -replace '\\', '/'
    if ($p -match '\s') { return "`"$p`"" }
    return $p
}

# Tor propio, solo para salir: el nodo llega a los demás por sus direcciones
# .onion. Sin Tor, un minero en otra casa no podría publicar sus bloques. El
# registro va por la salida estándar a un archivo: "Log notice file <ruta>" se
# rompe con una ruta con espacios.
function Start-Tor {
    $torExe = Join-Path $Root "tor\tor.exe"
    $torData = Join-Path $Data "tor"
    $torLog = Join-Path $torData "tor.log"
    New-Item -ItemType Directory -Force -Path $torData | Out-Null
    $torrc = Join-Path $torData "torrc"
    @(
        "SocksPort $TorSocks",
        "DataDirectory $(TorPath $torData)",
        "GeoIPFile $(TorPath (Join-Path $Root 'tor\geoip'))",
        "GeoIPv6File $(TorPath (Join-Path $Root 'tor\geoip6'))",
        "Log notice stdout"
    ) | Set-Content -LiteralPath $torrc -Encoding ASCII
    if (Test-Path -LiteralPath $torLog) { Remove-Item -LiteralPath $torLog -Force }
    $process = Start-Process -FilePath $torExe -ArgumentList @("-f", "`"$torrc`"") -NoNewWindow -RedirectStandardOutput $torLog -PassThru
    Say "Starting Tor (this takes up to a minute)..."
    $deadline = (Get-Date).AddMinutes(3)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        if ($process.HasExited) { throw "Tor stopped. See $torLog" }
        if ((Test-Path -LiteralPath $torLog) -and (Select-String -LiteralPath $torLog -Pattern "Bootstrapped 100%" -Quiet)) {
            Say "Tor is ready." "Green"
            return $process
        }
    }
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    throw "Tor did not finish starting in 3 minutes. Check your internet connection; details in $torLog"
}

function Start-Node([string[]]$Extra) {
    if (-not (Test-WalletExists)) {
        throw "There is no wallet '$Wallet' yet. Run 1-create-wallet.cmd (or restore-wallet.cmd) first."
    }
    if (Test-NodeOnline) { throw "A node is already running on $Api. Close it first." }
    $address = Get-ShortAddress
    $tor = Start-Tor
    try {
        $env:ZOKA_API_BIND = $ApiBind
        Say ""
        Say "Mining rewards go to wallet '$Wallet': $address" "Cyan"
        Say "Press Ctrl+C to stop." "Cyan"
        Say ""
        & $Zoka fullnode --network $Network --quantic-mine-to $address --listen $Listen --tor-proxy $TorSocks --min-mine-peers 1 @Extra
    } finally {
        Stop-Process -Id $tor.Id -Force -ErrorAction SilentlyContinue
        Say "Node and Tor stopped."
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
        Invoke-WithPassword $p1 { & $Zoka quantic-wallet new --network $Network --name $Wallet --password-env ZOKA_CLI_PASSWORD }
        if ($LASTEXITCODE -ne 0) { throw "The wallet was not created." }
    }
    "restore-wallet" {
        if (Test-WalletExists) { throw "Wallet '$Wallet' already exists. Use -Wallet <other name>." }
        $phrase = Read-Password "Type your 24 words, separated by spaces (hidden)"
        $p1 = Read-Password "Choose a password for this computer"
        $p2 = Read-Password "Repeat the password"
        if ($p1 -ne $p2) { throw "The two passwords are different. Nothing was restored." }
        $env:ZOKA_CLI_PHRASE = $phrase
        try {
            Invoke-WithPassword $p1 { & $Zoka quantic-wallet restore --network $Network --name $Wallet --phrase-env ZOKA_CLI_PHRASE --password-env ZOKA_CLI_PASSWORD }
        } finally { Remove-Item Env:ZOKA_CLI_PHRASE -ErrorAction SilentlyContinue }
        if ($LASTEXITCODE -ne 0) { throw "The wallet was not restored." }
    }
    "address" {
        Say "Your address (share this one to get paid):" "Cyan"
        Get-ShortAddress
    }
    "mine" {
        $cores = [Environment]::ProcessorCount
        if ($Threads -le 0) { $Threads = [Math]::Max(1, $cores - 1) }
        Say "Mining with $Threads of $cores CPU threads (use -Threads n to change it)."
        Start-Node @("--threads", "$Threads")
    }
    "xmrig-node" {
        Say "Node with a Stratum server for XMRig on port $StratumPort; the node itself does not mine." "Cyan"
        Say "If another program (for example ZSilent's Stratum) already uses it, pick another with -StratumPort <port>."
        Say "XMRig on this PC:  xmrig -o 127.0.0.1:$StratumPort -u <your address> -p x -a rx/0"
        Say "XMRig on another PC of your network: use this PC's local IP instead of 127.0.0.1."
        Say "Windows may ask whether zoka.exe can accept connections: allow private networks only."
        Start-Node @("--stratum", $StratumBind, "--no-mine")
    }
    "balance" {
        if (-not (Test-NodeOnline)) { throw "The node is not running. Start mining first (2-start-mining.cmd), then check the balance from another window." }
        $p = Read-Password "Password of wallet '$Wallet'"
        Invoke-WithPassword $p { & $Zoka quantic-wallet balance --network $Network --name $Wallet --password-env ZOKA_CLI_PASSWORD --rpc $Api }
    }
    "status" {
        if (-not (Test-NodeOnline)) { Say "The node is not running."; break }
        $info = Invoke-RestMethod -Uri "$Api/quantic/info" -TimeoutSec 5
        $mining = Invoke-RestMethod -Uri "$Api/quantic/mining" -TimeoutSec 5
        Say ("Height {0} · your blocks this session {1} · hashrate {2} H/s" -f $info.height, $mining.blocks_mined, $mining.hashrate_hps)
    }
    default {
        Say "Zoka Quantic CLI miner (testnet). Commands:"
        Say "  create-wallet   a new wallet (shows its 24 words once)"
        Say "  restore-wallet  a wallet from its 24 words"
        Say "  address         the address you share to get paid"
        Say "  mine            run the node and mine (-Threads n)"
        Say "  balance         your balance (with the node running)"
        Say "  xmrig-node      run the node for XMRig instead of mining"
        Say "  status          height and your blocks"
        Say "Options: -Wallet <name> (default: miner), -Threads n, -StratumPort n (default: 3333)."
        Say "Data lives in .\data next to this script."
    }
}
