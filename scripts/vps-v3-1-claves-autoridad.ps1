# Lanzamiento de la testnet v3, paso 1 (decision del 11/11/2026: autoridad de
# checkpoints 2 de 3 con la PC, el VPS 1 y el VPS 2).
#
# En cada VPS crea su clave de autoridad (Ed25519 + ML-DSA-65) con el zoka que
# ya tiene instalado, en /var/lib/zoka-authority (usuario zoka, permisos 0700),
# y la contrasena que la va a cifrar cuando se instale el nodo 5.0.0, en
# /root/zoka-authority.env (0600, solo root). La clave privada no sale del VPS:
# a esta PC vuelven solo las publicas, en scripts\autoridad-v3-publicas.txt,
# para compilarlas en los programas.
#
# Correrlo dos veces no crea otra clave: muestra la que ya hay.
# No toca la semilla v2 ni el faucet, que siguen andando.
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\vps-v3-1-claves-autoridad.ps1
param(
    [string]$Key = (Join-Path $HOME '.ssh\zoka_vps'),
    [string]$KnownHosts = (Join-Path $HOME '.ssh\known_hosts_zoka_vps'),
    [string[]]$Hosts = @('13.140.206.10', '13.140.206.11')
)
$ErrorActionPreference = 'Stop'

# Solo ASCII y sin \r: va por stdin a bash.
$remote = @'
set -eu
dir=/var/lib/zoka-authority
keys=$dir/.zoka/quantic-testnet
if [ -e $keys/checkpoint-authority.key ] || [ -e $keys/checkpoint-authority-mldsa.key ]; then
  echo ESTADO=ya-existia
else
  install -d -o zoka -g zoka -m 0700 $dir
  cd $dir
  runuser -u zoka -- env HOME=$dir ZOKA_HOME=$dir /usr/local/bin/zoka checkpoint-keygen --network quantic-testnet > $dir/publicas.txt
  chown zoka:zoka $dir/publicas.txt
  echo ESTADO=creada
fi
test -s $keys/checkpoint-authority.key
test -s $keys/checkpoint-authority-mldsa.key
if [ ! -e /root/zoka-authority.env ]; then
  umask 077
  printf 'ZOKA_AUTHORITY_PASSWORD=%s\n' $(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n') > /root/zoka-authority.env
  echo CONTRASENA=creada
else
  echo CONTRASENA=ya-existia
fi
echo HOST=$(hostname)
grep -E '^Public key: |^[0-9a-f]{3904}$' $dir/publicas.txt
# fin
'@
$remote = $remote -replace "`r", ''

$out = Join-Path $PSScriptRoot 'autoridad-v3-publicas.txt'
$lines = @("# Claves publicas de autoridad de la testnet v3, creadas el $(Get-Date -Format s)")
foreach ($ip in $Hosts) {
    Write-Output "== $ip"
    $reply = $remote | & ssh -p 443 -i $Key -o "UserKnownHostsFile=$KnownHosts" "root@$ip" 'bash -s'
    if ($LASTEXITCODE -ne 0) { throw "fallo en $ip (codigo $LASTEXITCODE)" }
    $ed = ($reply | Where-Object { $_ -like 'Public key: *' }) -replace '^Public key: ', ''
    $ml = $reply | Where-Object { $_ -match '^[0-9a-f]{3904}$' }
    if (-not $ed -or -not $ml) { throw "no llegaron las dos claves publicas de $ip" }
    $reply | Where-Object { $_ -like 'ESTADO=*' -or $_ -like 'CONTRASENA=*' -or $_ -like 'HOST=*' } | Write-Output
    Write-Output "Ed25519: $ed"
    Write-Output "ML-DSA-65: $($ml.Substring(0, 24))... ($($ml.Length / 2) bytes)"
    $lines += "$ip ed25519 $ed"
    $lines += "$ip mldsa $ml"
}
Set-Content -Path $out -Value $lines -Encoding ascii
Write-Output ''
Write-Output "Listo. Las publicas quedaron en $out (no hay nada secreto ahi)."
