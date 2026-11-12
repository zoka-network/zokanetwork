# Lanzamiento de la testnet v3, paso 2: el nodo 5.0.0 en los dos VPS.
#
# Por cada VPS:
#   - sube el binario de Linux y comprueba su SHA-256;
#   - para la semilla v2 y deja aparte su cadena (db-v2) y el binario 4.0.6
#     (zoka-4.0.6): no se borra nada de la v2;
#   - cifra la clave de autoridad del paso 1 con su contrasena
#     (/root/zoka-authority.env) y la suma al servicio (--authority);
#   - arranca la semilla v3 con la misma identidad, y recien cuando el nodo
#     arranco firmando borra la copia en claro de la clave;
#   - en el VPS 1, pasa el faucet a la v3: aparta la copia de la cadena v2 y su
#     base, toma el descriptor nuevo de seed2 e instala faucet.py.
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\vps-v3-2-instalar.ps1 -Binary <ruta al zoka de Linux 5.0.0>
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [string]$Key = (Join-Path $HOME '.ssh\zoka_vps'),
    [string]$KnownHosts = (Join-Path $HOME '.ssh\known_hosts_zoka_vps'),
    [string]$Faucet = (Join-Path $PSScriptRoot '..\zokafaucet\server')
)
$ErrorActionPreference = 'Stop'
$Binary = (Resolve-Path $Binary).Path
$sha = (Get-FileHash -Algorithm SHA256 $Binary).Hash.ToLower()
Write-Output "Binario: $Binary"
Write-Output "SHA-256: $sha"
$ssh = @('-p', '443', '-i', $Key, '-o', "UserKnownHostsFile=$KnownHosts")
$scp = @('-P', '443', '-i', $Key, '-o', "UserKnownHostsFile=$KnownHosts")

# Solo ASCII; __SHA__ y __ROLE__ se reemplazan antes de mandarlo.
$remote = @'
set -eu
SHA=__SHA__
ROLE=__ROLE__
echo "$SHA  /root/zoka-5.0.0.new" | sha256sum -c -
/root/zoka-5.0.0.new --version | grep -q '^zoka 5\.0\.0'
unit=/etc/systemd/system/zoka-seed.service
keys=/var/lib/zoka-authority/.zoka/quantic-testnet
test -s $keys/checkpoint-authority.key
test -s /root/zoka-authority.env

systemctl stop zoka-seed
[ -e /usr/local/bin/zoka-4.0.6 ] || cp -a /usr/local/bin/zoka /usr/local/bin/zoka-4.0.6
install -m 0755 /root/zoka-5.0.0.new /usr/local/bin/zoka
if [ -d /var/lib/zoka-seed/db ] && [ ! -e /var/lib/zoka-seed/db-v2 ]; then
  mv /var/lib/zoka-seed/db /var/lib/zoka-seed/db-v2
fi

# La clave, cifrada con la contrasena del paso 1 (el nodo la lee de ahi).
if [ "$(head -c 1 $keys/checkpoint-authority.key)" != "{" ]; then
  set -a; . /root/zoka-authority.env; set +a
  runuser -u zoka -w ZOKA_AUTHORITY_PASSWORD -- /usr/local/bin/zoka quantic-pq-node authority-encrypt --authority $keys > /dev/null
  unset ZOKA_AUTHORITY_PASSWORD
  echo CLAVE=cifrada
else
  echo CLAVE=ya-cifrada
fi

[ -e $unit.v2 ] || cp -a $unit $unit.v2
sed -i 's/^Description=.*/Description=Zoka Quantic Testnet v3 seed/' $unit
grep -q '^EnvironmentFile=/root/zoka-authority.env' $unit || sed -i 's#^EnvironmentFile=/etc/zoka-seed.env$#EnvironmentFile=/etc/zoka-seed.env\nEnvironmentFile=/root/zoka-authority.env#' $unit
grep -q -- '--authority ' $unit || sed -i "s#^\(ExecStart=.*[^ ]\) *\$#\1 --authority $keys#" $unit
grep -q '^ReadOnlyPaths=/var/lib/zoka-authority' $unit || sed -i 's#^ReadWritePaths=/var/lib/zoka-seed$#ReadWritePaths=/var/lib/zoka-seed\nReadOnlyPaths=/var/lib/zoka-authority#' $unit
systemctl daemon-reload
systemctl start zoka-seed
started=no
for i in $(seq 1 30); do
  sleep 4
  if journalctl -u zoka-seed -o cat --no-pager --since '-3 min' | grep '^{"announce"' | tail -1 | grep -q '"checkpoint_authority":true'; then
    started=yes; break
  fi
done
systemctl is-active zoka-seed
[ $started = yes ] || { echo 'la semilla no arranco como autoridad: la copia en claro queda'; exit 1; }
for f in $keys/*.plain-backup; do [ -e "$f" ] && shred -u "$f"; done
echo AUTORIDAD=firmando
api=$(grep -o -- '--api [0-9.:]*' $unit | head -1 | cut -d' ' -f2)
curl -s --max-time 10 http://$api/status | head -c 240; echo

if [ "$ROLE" = vps1 ]; then
  d=/var/lib/zoka-faucet
  systemctl stop zoka-faucet
  [ -d $d/wallet-mirror ] && [ ! -e $d/wallet-mirror.v2 ] && mv $d/wallet-mirror $d/wallet-mirror.v2
  for f in faucet.sqlite faucet.sqlite-shm faucet.sqlite-wal; do
    if [ -e $d/$f ] && [ ! -e $d/$f.v2 ]; then mv $d/$f $d/$f.v2; fi
  done
  journalctl -u zoka-seed -o cat --no-pager --since '-5 min' | grep '^{"announce"' | tail -1 \
    | python3 -c 'import json,sys; d=json.load(sys.stdin)["descriptor"]; assert d["chain_id"]=="zoka-quantic-testnet-v3"; json.dump(d, open("/var/lib/zoka-faucet/seed2.descriptor.json","w"), indent=2)'
  install -m 0644 /root/zoka-faucet-v3/faucet.py /opt/zoka-faucet/faucet.py
  install -m 0644 /root/zoka-faucet-v3/zoka-faucet.service /etc/systemd/system/zoka-faucet.service
  chown -R zoka-faucet:zoka-faucet $d
  systemctl daemon-reload
  systemctl start zoka-faucet
  sleep 5
  systemctl is-active zoka-faucet
  curl -s --max-time 10 http://127.0.0.1:9300/faucet/status | head -c 240; echo
  echo FAUCET=v3
fi
echo LISTO=$(hostname)
# fin
'@
$remote = $remote -replace "`r", ''

foreach ($target in @(@{ ip = '13.140.206.10'; role = 'vps1' }, @{ ip = '13.140.206.11'; role = 'vps2' })) {
    $ip = $target.ip
    Write-Output "== $ip ($($target.role))"
    & scp @scp $Binary "root@${ip}:/root/zoka-5.0.0.new"
    if ($LASTEXITCODE -ne 0) { throw "no se pudo subir el binario a $ip" }
    if ($target.role -eq 'vps1') {
        & ssh @ssh "root@$ip" 'mkdir -p /root/zoka-faucet-v3'
        & scp @scp (Join-Path $Faucet 'faucet.py') (Join-Path $Faucet 'zoka-faucet.service') "root@${ip}:/root/zoka-faucet-v3/"
        if ($LASTEXITCODE -ne 0) { throw "no se pudo subir el faucet a $ip" }
    }
    $script = $remote.Replace('__SHA__', $sha).Replace('__ROLE__', $target.role)
    $script | & ssh @ssh "root@$ip" 'bash -s'
    if ($LASTEXITCODE -ne 0) { throw "fallo la instalacion en $ip (codigo $LASTEXITCODE): lo de la v2 quedo intacto en db-v2 y zoka-4.0.6" }
}
Write-Output ''
Write-Output 'Listo: las dos semillas corren la testnet v3 y firman checkpoints.'
