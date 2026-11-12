# Lanzamiento de la testnet v3: el genesis, en la PC del equipo (seed1).
#
# Arranca el nodo de la testnet v3 tal como lo va a arrancar ZSilent 5.0: la
# misma carpeta (%APPDATA%\Zoka\.zoka\quantic-testnet-v3\node), la misma
# identidad y el mismo secreto que la cifra. Cuando instales ZSilent 5.0, su
# nodo sigue esta misma cadena. La autoridad de checkpoints es la de esta PC
# (la carpeta de la v1, como hoy).
#
# El bloque 1 (el premine) cobra a -Wallet, la wallet de ZSilent que recibio
# el premine de la v2: la misma frase da la misma direccion en la v3. Lo mina
# XMRig por el Stratum del nodo: abri XMRig contra 127.0.0.1:3333 como siempre.
#
#   1. Cerra ZSilent (su nodo usa el mismo puerto de la API).
#   2. .\scripts\genesis-testnet-v3.ps1            (lista las wallets)
#      .\scripts\genesis-testnet-v3.ps1 -Wallet NOMBRE
#   3. Abri XMRig. Cuando sale el bloque 1, el script escribe su hash y la
#      identidad publica de esta semilla en scripts\genesis-v3.txt.
#
# El nodo queda en su propia ventana: cerrala (Ctrl+C) cuando instales
# ZSilent 5.0, que lo reemplaza.
param(
    [string]$Wallet = '',
    [string]$Zoka = (Join-Path $PSScriptRoot '..\target\release\zoka.exe'),
    [string]$Stratum = '127.0.0.1:3333',
    [string]$Api = '127.0.0.1:9140'
)
$ErrorActionPreference = 'Stop'

$Zoka = (Resolve-Path $Zoka).Path
$ErrorActionPreference = 'Continue'
$help = (& $Zoka quantic-pq-node help 2>&1 | ForEach-Object { "$_" } | Out-String)
$ErrorActionPreference = 'Stop'
if ($help -notmatch 'zoka-quantic-testnet-v3') {
    throw "$Zoka no es el nodo de la testnet v3"
}
if (Get-Process -Name 'ZSilent Core' -ErrorAction SilentlyContinue) {
    throw 'Cerra ZSilent primero: su nodo usa el puerto de la API.'
}

$home_ = if ($env:ZOKA_HOME) { $env:ZOKA_HOME } else { Join-Path $env:APPDATA 'Zoka' }
$v2 = Join-Path $home_ '.zoka\quantic-testnet-v2'
$v3 = Join-Path $home_ '.zoka\quantic-testnet-v3'
$node = Join-Path $v3 'node'

if (-not $Wallet) {
    Write-Output 'Wallets de la testnet v2 en esta PC (elegi la del premine con -Wallet NOMBRE):'
    & $Zoka quantic-pq-wallet list --network testnet --dir $v2
    exit 0
}
$reply = & $Zoka quantic-pq-wallet address --network testnet --dir $v2 --name $Wallet --json | Out-String
$info = $reply | ConvertFrom-Json
$address = $info.address
if (-not $address -or -not $address.StartsWith('zqtest2') -or $address.Length -lt 1000) {
    throw "No se pudo leer la direccion completa de la wallet $Wallet"
}
Write-Output "El bloque 1 cobra a la wallet $Wallet ($($info.short_address))."

New-Item -ItemType Directory -Force $node | Out-Null
if (Test-Path (Join-Path $node 'db')) {
    throw "$node\db ya existe: el genesis ya se mino. No se mina otro."
}
# El secreto de la identidad, como lo crea ZSilent (32 bytes al azar en hex).
$secretFile = Join-Path $node 'node.identity.secret'
if (-not (Test-Path $secretFile)) {
    $bytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    [System.IO.File]::WriteAllText($secretFile, (($bytes | ForEach-Object { $_.ToString('x2') }) -join ''))
}
$secret = [System.IO.File]::ReadAllText($secretFile).Trim()
$descriptor = Join-Path $node 'peer.json'
if (Test-Path $descriptor) { Remove-Item $descriptor }

$authority = Join-Path $home_ '.zoka\quantic-testnet'
$argv = @('quantic-pq-node', 'serve', '--network', 'testnet',
    '--db', (Join-Path $node 'db'), '--identity', (Join-Path $node 'node.identity'),
    '--api', $Api, '--descriptor-out', $descriptor, '--stratum', $Stratum, '--mine-to', $address)
if ((Test-Path (Join-Path $authority 'checkpoint-authority.key')) -and (Test-Path (Join-Path $authority 'checkpoint-authority-mldsa.key'))) {
    $argv += @('--authority', $authority)
    Write-Output "Autoridad de checkpoints: $authority (una de las tres claves)."
} else {
    throw "Faltan las claves de autoridad en $authority"
}

# El nodo en su ventana, con el secreto solo en su entorno.
$env:ZOKA_PQ_PASSWORD = $secret
try {
    $quoted = $argv | ForEach-Object { '"' + $_ + '"' }
    Start-Process -FilePath $Zoka -ArgumentList $quoted -WorkingDirectory $node | Out-Null
} finally {
    Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
}
Write-Output "Nodo v3 arrancado. API http://$Api, Stratum $Stratum."
Write-Output 'Ahora abri XMRig contra 127.0.0.1:3333. Espero el bloque 1...'

$hash = $null
for ($i = 0; $i -lt 720 -and -not $hash; $i++) {
    Start-Sleep -Seconds 5
    try {
        $block = Invoke-RestMethod -Uri "http://$Api/blocks/1/summary" -TimeoutSec 5
        if ($block.hash) { $hash = $block.hash }
    } catch { }
}
if (-not $hash) { throw 'En una hora no salio el bloque 1: revisa que XMRig este minando contra 127.0.0.1:3333.' }

$env:ZOKA_PQ_PASSWORD = $secret
try {
    $identity = & $Zoka quantic-pq-node identity --network testnet --identity (Join-Path $node 'node.identity') 2>&1 | Out-String
} finally {
    Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
}
$out = Join-Path $PSScriptRoot 'genesis-v3.txt'
Set-Content -Path $out -Encoding ascii -Value @(
    "# Genesis de la testnet v3, $(Get-Date -Format s)",
    "genesis $hash",
    "premine $($info.short_address)",
    'identity:',
    $identity.Trim()
)
Write-Output ''
Write-Output "Bloque 1: $hash"
Write-Output "Listo: el hash y la identidad publica de esta semilla quedaron en $out."
Write-Output 'Deja el nodo y XMRig andando.'
