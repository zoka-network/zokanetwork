# Arranca el nodo de la testnet v2 de esta PC como lo arranca ZSilent 4.0:
# misma carpeta, misma identidad y el mismo secreto que la cifra. Lo que mina
# o firma este nodo lo sigue haciendo el de ZSilent 4.0 cuando se instale.
#
#   .\scripts\semilla-testnet-v2.ps1 -MineTo <direccion completa del equipo>
#
# - Carpeta: %APPDATA%\Zoka\.zoka\quantic-testnet-v2\node (o $env:ZOKA_HOME).
# - Identidad: node.identity, cifrada con node.identity.secret (32 bytes al
#   azar que se crean la primera vez). Nunca se imprimen.
# - Autoridad de checkpoints: la carpeta de la v1 si tiene las dos claves
#   (`-Authority none` la apaga).
# - Stratum para XMRig en -Stratum; los logins que no son direcciones cobran a
#   -MineTo (el bloque 1, con el premine, incluido).
# Ctrl+C detiene el nodo.
param(
    [string]$Zoka = (Join-Path $PSScriptRoot '..\target\release\zoka.exe'),
    [Parameter(Mandatory = $true)][string]$MineTo,
    [string]$Stratum = '127.0.0.1:3333',
    [string]$Api = '127.0.0.1:9140',
    [string]$Authority = ''
)
$ErrorActionPreference = 'Stop'

$Zoka = (Resolve-Path $Zoka).Path
$ErrorActionPreference = 'Continue'
$help = (& $Zoka quantic-pq-node help 2>&1 | ForEach-Object { "$_" } | Out-String)
$ErrorActionPreference = 'Stop'
if ($help -notmatch 'serve --db') {
    throw "$Zoka no trae el nodo v2: compilarlo con cargo build --release --features randomx,quantic-pq --bins"
}
if (-not $MineTo.StartsWith('zqtest2') -or $MineTo.Length -lt 1000) {
    throw '-MineTo tiene que ser la direccion COMPLETA (zqtest2..., unos 1.760 caracteres) de la wallet del equipo'
}

$home_ = if ($env:ZOKA_HOME) { $env:ZOKA_HOME } else { Join-Path $env:APPDATA 'Zoka' }
$node = Join-Path $home_ '.zoka\quantic-testnet-v2\node'
New-Item -ItemType Directory -Force $node | Out-Null
$secretFile = Join-Path $node 'node.identity.secret'
if (-not (Test-Path $secretFile)) {
    $bytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    [System.IO.File]::WriteAllText($secretFile, (($bytes | ForEach-Object { $_.ToString('x2') }) -join ''))
    Write-Output "Secreto de la identidad creado en $secretFile (no se muestra)."
}
$secret = [System.IO.File]::ReadAllText($secretFile).Trim()
if ($secret.Length -eq 0) { throw "$secretFile esta vacio" }

# El nodo nunca pisa un archivo: el descriptor de la corrida anterior se borra.
$descriptor = Join-Path $node 'peer.json'
if (Test-Path $descriptor) { Remove-Item $descriptor }

$argv = @('quantic-pq-node', 'serve', '--network', 'testnet',
    '--db', (Join-Path $node 'db'), '--identity', (Join-Path $node 'node.identity'),
    '--api', $Api, '--descriptor-out', $descriptor, '--stratum', $Stratum, '--mine-to', $MineTo)
if ($Authority -ne 'none') {
    $keys = if ($Authority) { $Authority } else { Join-Path $home_ '.zoka\quantic-testnet' }
    if ((Test-Path (Join-Path $keys 'checkpoint-authority.key')) -and (Test-Path (Join-Path $keys 'checkpoint-authority-mldsa.key'))) {
        $argv += @('--authority', $keys)
        Write-Output "Autoridad de checkpoints: $keys"
    } elseif ($Authority) {
        throw "$keys no tiene checkpoint-authority.key y checkpoint-authority-mldsa.key"
    } else {
        Write-Output 'Sin claves de autoridad: este nodo no firma checkpoints.'
    }
}
Write-Output "Nodo v2: $node"
Write-Output "API: http://$Api   Stratum: $Stratum   (Ctrl+C lo detiene)"
$env:ZOKA_PQ_PASSWORD = $secret
try {
    & $Zoka @argv
} finally {
    Remove-Item Env:ZOKA_PQ_PASSWORD -ErrorAction SilentlyContinue
}
