# Instala en el ZSilent de esta PC el nodo compilado (zoka.exe y zokahd.exe) y
# el Tor fijado en zsilent-core, con respaldo de lo anterior y verificacion.
#
# Uso, con ZSilent cerrado:
#   powershell -ExecutionPolicy Bypass -File <repo ZOKA>\scripts\instalar-en-pc.ps1
# Opciones: -SoloNodo, -SoloTor, -NodeDir <carpeta con zoka.exe>.
#
# No toca los datos (%APPDATA%\Zoka): la cadena, las wallets y la clave de
# checkpoints quedan donde estan. Volver atras es copiar los respaldos.
param(
    [string]$NodeDir = (Join-Path $PSScriptRoot "..\target\release"),
    [switch]$SoloNodo,
    [switch]$SoloTor,
    [string]$Stamp = (Get-Date -Format "yyyyMMdd")
)

$ErrorActionPreference = "Stop"
$app = Join-Path $env:LOCALAPPDATA "ZSilent Core"
$bin = Join-Path $app "app\resources\bin"
$torDir = Join-Path $app "app\resources\tor\windows-x86_64"
$informe = @()

# Windows bloquea un ejecutable en uso: con ZSilent abierto la copia saldria a
# medias. Mejor no empezar.
$enUso = Get-Process | Where-Object {
    $_.ProcessName -match '^(zoka|zokahd|tor|lyrebird|conjure-client|ZSilent.*)$'
}
if ($enUso) {
    throw ("Cerra ZSilent antes de instalar. Siguen corriendo: " +
        (($enUso | ForEach-Object { "$($_.ProcessName) ($($_.Id))" }) -join ", "))
}

if (-not $SoloTor) {
    foreach ($nombre in @("zoka.exe", "zokahd.exe")) {
        $nuevo = Join-Path $NodeDir $nombre
        $destino = Join-Path $bin $nombre
        $hashNuevo = (Get-FileHash $nuevo -Algorithm SHA256).Hash
        $hashViejo = (Get-FileHash $destino -Algorithm SHA256).Hash
        if ($hashNuevo -eq $hashViejo) {
            $informe += "$nombre ya estaba instalado ($hashNuevo)"
            continue
        }
        $respaldo = "$destino.bak-$Stamp"
        if (Test-Path $respaldo) { throw "Ya existe $respaldo; no piso un respaldo." }
        Copy-Item $destino $respaldo
        Copy-Item $nuevo $destino -Force
        if ((Get-FileHash $destino -Algorithm SHA256).Hash -ne $hashNuevo) {
            Copy-Item $respaldo $destino -Force
            throw "La copia de $nombre no coincide; deje el anterior."
        }
        $informe += "$nombre $hashViejo -> $hashNuevo (respaldo $nombre.bak-$Stamp)"
    }
}

if (-not $SoloNodo) {
    # Los valores fijados viven en prepare-release.ps1 (uno de los cuatro
    # lugares de docs/zsilent-managed-tor-onion-seed.md); se leen de ahi para
    # no sumar un quinto.
    $pins = Get-Content (Join-Path $PSScriptRoot "..\zsilent-core\scripts\prepare-release.ps1") -Raw
    $pin = { param($var) [regex]::Match($pins, "\`$$var = `"([^`"]+)`"").Groups[1].Value }
    $torUrl = & $pin "expectedTorUrl"
    $torSha = & $pin "expectedTorArchiveSha256"
    $torVersion = & $pin "expectedTorVersion"
    if (-not ($torUrl -and $torSha -and $torVersion)) {
        throw "No pude leer la version de Tor fijada en prepare-release.ps1"
    }
    $instalado = Get-Content (Join-Path $torDir ".zsilent-bundle.json") -Raw | ConvertFrom-Json
    if ($instalado.archive_sha256 -eq $torSha) {
        $informe += "Tor $torVersion ya estaba instalado"
    } else {
        $respaldoTor = Join-Path $app "backup-tor-$($instalado.tor_version)-$Stamp"
        if (Test-Path $respaldoTor) { throw "Ya existe $respaldoTor; no piso un respaldo." }
        Copy-Item $torDir $respaldoTor -Recurse
        # gpg viene con Git; la verificacion de la firma lo necesita en el PATH.
        if (-not (Get-Command gpg -ErrorAction SilentlyContinue)) {
            $env:PATH = "C:\Program Files\Git\usr\bin;$env:PATH"
        }
        try {
            & (Join-Path $PSScriptRoot "..\zsilent-core\scripts\prepare-tor-expert-bundle-windows.ps1") `
                -Url $torUrl -Sha256 $torSha -AscUrl "$torUrl.asc" `
                -ExpectedTorVersion $torVersion -VerifySignature -OutDir $torDir
        } catch {
            if (Test-Path $torDir) { Remove-Item -LiteralPath $torDir -Recurse -Force }
            Copy-Item $respaldoTor $torDir -Recurse
            throw "Fallo la instalacion de Tor; deje el $($instalado.tor_version). $_"
        }
        $informe += "Tor $($instalado.tor_version) -> $torVersion (respaldo $(Split-Path $respaldoTor -Leaf))"
    }
}

$clave = Join-Path $env:APPDATA "Zoka\.zoka\mainnet\checkpoint-authority.key"
if (Test-Path $clave) {
    $informe += "Esta PC tiene la clave de autoridad: el nodo va a firmar checkpoints. Respaldala fuera de la PC."
}
$nota = Join-Path $app "INSTALADO-$Stamp.txt"
Add-Content -Path $nota -Value (@("Instalado el $(Get-Date -Format 'dd/MM/yyyy HH:mm') con scripts/instalar-en-pc.ps1") + $informe + "") -Encoding UTF8
$informe
"Listo. Abri ZSilent y desbloquea la wallet como siempre. Detalle en $nota"
