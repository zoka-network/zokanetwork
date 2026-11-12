# Cambia el binario de zoka en los dos VPS por una version nueva de la misma
# red (por ejemplo, 5.0.0 -> 5.0.1). No toca la cadena, las claves ni el faucet.
#
# Por cada VPS: sube el binario y comprueba su SHA-256 y su version, guarda el
# anterior como /usr/local/bin/zoka-<version anterior>, reinicia la semilla y
# espera a que vuelva a anunciarse como autoridad. Si no vuelve, deja el
# anterior y corta antes de pasar al otro VPS.
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\vps-actualizar-binario.ps1 -Binary <zoka de Linux> -Version 5.0.1
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Key = (Join-Path $HOME '.ssh\zoka_vps'),
    [string]$KnownHosts = (Join-Path $HOME '.ssh\known_hosts_zoka_vps'),
    [string[]]$Hosts = @('13.140.206.10', '13.140.206.11')
)
$ErrorActionPreference = 'Stop'
# PowerShell 5.1 antepone igual una marca BOM a lo que manda por stdin, y bash
# leia "<BOM>set -eu" y no cortaba ante un error: el VPS la saca con sed.
$OutputEncoding = New-Object System.Text.UTF8Encoding $false
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "version invalida: $Version" }
$Binary = (Resolve-Path $Binary).Path
$sha = (Get-FileHash -Algorithm SHA256 $Binary).Hash.ToLower()
Write-Output "Binario: $Binary"
Write-Output "SHA-256: $sha"
$ssh = @('-p', '443', '-i', $Key, '-o', "UserKnownHostsFile=$KnownHosts")
$scp = @('-P', '443', '-i', $Key, '-o', "UserKnownHostsFile=$KnownHosts")

# Solo ASCII; __SHA__ y __VERSION__ se reemplazan antes de mandarlo.
$remote = @'
set -eu
SHA=__SHA__
VERSION=__VERSION__
new=/root/zoka-$VERSION.new
echo "$SHA  $new" | sha256sum -c -
chmod 0755 $new
$new --version | grep -qx "zoka $VERSION"
old=$(/usr/local/bin/zoka --version | awk '{print $2}')
echo ANTERIOR=$old
if [ "$old" = "$VERSION" ]; then rm -f $new; echo YA-ESTABA; echo LISTO=$(hostname); exit 0; fi
[ -e /usr/local/bin/zoka-$old ] || cp -a /usr/local/bin/zoka /usr/local/bin/zoka-$old
since=$(date '+%Y-%m-%d %H:%M:%S')
systemctl stop zoka-seed
install -m 0755 $new /usr/local/bin/zoka
systemctl start zoka-seed
started=no
for i in $(seq 1 30); do
  sleep 4
  if journalctl -u zoka-seed -o cat --no-pager --since "$since" | grep '^{"announce"' | tail -1 | grep -q '"checkpoint_authority":true'; then
    started=yes; break
  fi
done
if [ $started != yes ]; then
  systemctl stop zoka-seed
  install -m 0755 /usr/local/bin/zoka-$old /usr/local/bin/zoka
  systemctl start zoka-seed
  echo "la semilla $VERSION no arranco como autoridad: volvio la $old"
  exit 1
fi
rm -f $new
systemctl is-active zoka-seed
api=$(grep -o -- '--api [0-9.:]*' /etc/systemd/system/zoka-seed.service | head -1 | cut -d' ' -f2)
curl -s --max-time 10 http://$api/status | head -c 200; echo
echo LISTO=$(hostname)
# fin
'@
$remote = $remote -replace "`r", ''

foreach ($ip in $Hosts) {
    Write-Output "== $ip"
    & scp @scp $Binary "root@${ip}:/root/zoka-$Version.new"
    if ($LASTEXITCODE -ne 0) { throw "no se pudo subir el binario a $ip" }
    $script = $remote.Replace('__SHA__', $sha).Replace('__VERSION__', $Version)
    $script | & ssh @ssh "root@$ip" 'sed 1s/^$(printf ''\357\273\277'')// | bash -s'
    if ($LASTEXITCODE -ne 0) { throw "fallo en $ip (codigo $LASTEXITCODE): no se sigue con el resto" }
}
Write-Output ''
Write-Output "Listo: las semillas corren zoka $Version."
