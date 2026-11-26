# Respaldo de la clave de releases de Zoka (zoka release backup).
# Pide la contraseña dos veces sin mostrarla y deja el archivo cifrado en
# D:\Respaldo-Zoka-clave-releases. No borra ni pisa nada.

$ErrorActionPreference = "Stop"
$zoka = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\zoka.exe"
$carpeta = "D:\Respaldo-Zoka-clave-releases"
$archivo = Join-Path $carpeta "zoka-release-key.backup"

function Leer-Clave([string]$Texto) {
    $segura = Read-Host $Texto -AsSecureString
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($segura)
    try { return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr) }
}

Write-Host "Respaldo de la clave que firma las releases de Zoka" -ForegroundColor Cyan
Write-Host ""
if (Test-Path -LiteralPath $archivo) {
    Write-Host "Ya existe $archivo : el respaldo ya estaba hecho. No se toca nada." -ForegroundColor Yellow
    return
}
Write-Host "Elegí una contraseña de 12 caracteres o más. No se ve mientras la escribís."
$primera = Leer-Clave "Contraseña"
$segunda = Leer-Clave "Repetila"
if ($primera -ne $segunda) {
    Write-Host "Las dos no coinciden. No se hizo nada: volvé a abrir el archivo." -ForegroundColor Red
    return
}
if ($primera.Length -lt 12) {
    Write-Host "Tiene menos de 12 caracteres. No se hizo nada: volvé a abrir el archivo." -ForegroundColor Red
    return
}
New-Item -ItemType Directory -Force -Path $carpeta | Out-Null
$env:ZOKA_RELEASE_BACKUP_PASSWORD = $primera
try {
    & $zoka release backup --out $archivo
    if ($LASTEXITCODE -eq 0) {
        Write-Host ""
        Write-Host "Listo. Guardá la contraseña en un lugar seguro: sin ella el respaldo no sirve." -ForegroundColor Green
        Write-Host "Si podés, copiá también $archivo a un pendrive."
    } else {
        Write-Host "El respaldo falló (mirá el mensaje de arriba). No se borró nada." -ForegroundColor Red
    }
} finally {
    Remove-Item Env:ZOKA_RELEASE_BACKUP_PASSWORD -ErrorAction SilentlyContinue
    $primera = $null; $segunda = $null
}
