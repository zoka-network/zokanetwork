# Pasos 1 y 2 de docs/LANZAMIENTO-quantic.md, en la PC del equipo: crea la
# wallet "equipo" de la testnet de Quantic (la contrasena se pide oculta y llega
# al nodo por una variable de entorno que se borra al terminar) y la clave de la
# autoridad, y deja en el Escritorio solo las partes PUBLICAS: la direccion y las
# dos claves publicas. Las 24 palabras se muestran una sola vez: van al papel.
# Texto sin tildes a proposito: PowerShell 5.1 lee los .ps1 sin BOM como ANSI.
param(
    [string]$Zoka = "C:\Users\mderramus\ZOKA\target\release\zoka.exe",
    [string]$Network = "quantic-testnet",
    [string]$OutDir = [Environment]::GetFolderPath("Desktop")
)

$addressFile = Join-Path $OutDir "testnet-direccion.txt"
$authorityFile = Join-Path $OutDir "testnet-autoridad.txt"
$homeDir = if ($env:ZOKA_HOME) { $env:ZOKA_HOME } else { Join-Path $env:APPDATA "Zoka" }
$networkDir = Join-Path $homeDir ".zoka\$Network"

if (-not (Test-Path -LiteralPath $Zoka -PathType Leaf)) {
    Write-Host "No encuentro $Zoka"
    exit 1
}

# 1. La wallet del equipo (si ya existe, no se toca).
$existing = & $Zoka quantic-wallet address --network $Network --name equipo 2>$null
if ($LASTEXITCODE -eq 0 -and $existing) {
    Write-Host "La wallet 'equipo' ya existe: no la creo de nuevo."
} else {
    $a = Read-Host "Contrasena nueva de la wallet equipo (no se ve al escribirla)" -AsSecureString
    $b = Read-Host "Repetila" -AsSecureString
    $pa = [System.Net.NetworkCredential]::new('', $a).Password
    $pb = [System.Net.NetworkCredential]::new('', $b).Password
    if (-not ($pa -ceq $pb) -or $pa.Length -lt 8) {
        Remove-Variable a, b, pa, pb -ErrorAction SilentlyContinue
        Write-Host "No coinciden o tiene menos de 8 caracteres: no se creo nada. Volve a correr el script."
        exit 1
    }
    $env:ZOKA_EQUIPO_PW = $pa
    try {
        & $Zoka quantic-wallet new --network $Network --name equipo --password-env ZOKA_EQUIPO_PW
        $created = $LASTEXITCODE -eq 0
    } finally {
        Remove-Item Env:ZOKA_EQUIPO_PW -ErrorAction SilentlyContinue
        Remove-Variable a, b, pa, pb -ErrorAction SilentlyContinue
    }
    if (-not $created) {
        Write-Host "No se pudo crear la wallet."
        exit 1
    }
    Write-Host ""
    Write-Host "Anota las 24 palabras de arriba EN PAPEL. No les saques foto ni captura."
    Read-Host "Cuando las tengas anotadas, apreta Enter (se borra la pantalla)" | Out-Null
    Clear-Host
}
& $Zoka quantic-wallet address --network $Network --name equipo | Out-File -Encoding utf8 $addressFile

# 2. La clave de la autoridad (las dos mitades). No pisa una existente.
if (Test-Path -LiteralPath (Join-Path $networkDir "checkpoint-authority.key")) {
    Write-Host "La clave de la autoridad ya existe en $networkDir : no la creo de nuevo."
} else {
    & $Zoka checkpoint-keygen --network $Network | Out-File -Encoding utf8 $authorityFile
    if ($LASTEXITCODE -ne 0) {
        Write-Host "No se pudo crear la clave de la autoridad."
        exit 1
    }
}

Write-Host "Wallet 'equipo' lista. Direccion publica en: $addressFile"
Write-Host "Claves publicas de la autoridad en: $authorityFile"
Write-Host "Las privadas quedaron en $networkDir"
Write-Host "  Respalda fuera de la PC: checkpoint-authority.key y checkpoint-authority-mldsa.key"
Write-Host "Listo: avisale a Claude."
