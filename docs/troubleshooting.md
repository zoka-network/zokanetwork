# ZOKA Troubleshooting

Guia rapida para diagnosticar errores frecuentes antes de abrir un issue o
incidente operacional.

## Mainnet no esta launch-ready

Comando:

```powershell
target\release\zoka.exe mainnet-network-lint docs\mainnet-network.v1.json
```

Errores tipicos:

- `expected active or launch-ready`: `status` sigue en modo prelaunch.
- `only 0 active seed node(s)`: los seeds no estan marcados como `active`.
- `looks like a placeholder`: hay peer IDs, onion/I2P, RPC o snapshot signers
  de ejemplo.

Accion: reemplazar placeholders con datos reales de operadores y volver a
correr el lint hasta que no emita errores.

## Genesis valida, pero no es final

Comando:

```powershell
target\release\zoka.exe genesis-verify docs\genesis.mainnet.v1.json --json
```

Si el comando pasa, el archivo es estructuralmente valido. Eso no significa que
sea final: tambien deben reemplazarse las claves placeholder de
`validators[].consensus_pubkey_hex` y `validators[].operator_pubkey_hex`.

Accion: correr:

```powershell
PowerShell -ExecutionPolicy Bypass -File scripts\mainnet_readiness_check.ps1
```

Ese preflight falla si detecta claves placeholder.

Para generar keypairs ed25519 de launch sin imprimir secretos por consola:

```powershell
zoka release-keygen --label validator-na-1-consensus --out C:\zoka-secrets\validator-na-1-consensus.json
```

El comando imprime la clave publica. El archivo escrito contiene el secreto y
no debe commitearse. Copiar solo `public_key_hex` al material de launch.

Para aplicar material real:

```powershell
Copy-Item docs\mainnet-launch-material.template.json C:\zoka-secrets\mainnet-launch-material.json
# editar C:\zoka-secrets\mainnet-launch-material.json con claves/endpoints/fingerprint reales
PowerShell -ExecutionPolicy Bypass -File scripts\apply_mainnet_launch_material.ps1 -MaterialPath C:\zoka-secrets\mainnet-launch-material.json -InPlace
```

## RandomX no esta compilado

Error:

```text
not compiled into this binary
```

Causa: estas usando un binario sin feature `randomx`, normalmente
`target\debug\zoka.exe`.

Accion:

```powershell
cargo build --release --features randomx --bin zoka --bin zokahd
```

Usar despues:

```powershell
target\release\zoka.exe
target\release\zokahd.exe
```

## Falla el build RandomX

Comando de diagnostico:

```powershell
PowerShell -ExecutionPolicy Bypass -File scripts\check_randomx_toolchain.ps1 -Strict
```

En Windows debe detectar CMake y Visual Studio Build Tools con C++ workload.
Si falla, instalar esas dependencias y repetir el build release.

## GPG no esta instalado o la firma falta

Error del readiness:

```text
gpg is not installed or not on PATH
```

Accion:

1. Instalar GPG.
2. Generar o importar la clave oficial de release.
3. Publicar el fingerprint real en `docs/release-verification.md`.
4. Firmar checksums:

```powershell
PowerShell -ExecutionPolicy Bypass -File scripts\sign_release_checksums.ps1 -ChecksumsPath releases\<version>\SHA256SUMS.txt -Verify
```

Nota Windows: el `gpg.exe` incluido con Git puede servir para verificar firmas,
pero en algunas instalaciones no puede generar claves porque no arranca
`gpg-agent`. Para crear la clave oficial de release, usar Gpg4win/full GPG o
una maquina de signing dedicada.

## Wallet sin fondos

Error:

```text
insufficient
```

Causa: la wallet existe, pero no tiene fondos spendables. En mainnet, las
recompensas de mineria tambien respetan `coinbase_maturity_blocks = 60`.

Accion: esperar madurez de coinbase o recibir fondos confirmados.

## Passphrase incorrecta

Sintomas: comandos de wallet fallan al abrir o descifrar la wallet.

Accion: usar la passphrase correcta. Si no se recuerda, restaurar desde seed o
desde scan/spend keys:

```powershell
zoka wallet-restore-from-seed --name restored --seed "<words>" --password "<new-pass>"
```

## Snapshot rechazado

Errores tipicos:

- firma ausente con `--require-signature`;
- signer distinto del `--public-key`;
- bloque invalido durante replay.

Accion: inspeccionar primero:

```powershell
zoka snapshot-info <snapshot.json> --json
```

Importar solo snapshots firmados por un signer publicado en
`docs/mainnet-network.v1.json`.

## Fullnode no conecta a peers

Verificar:

- `docs/mainnet-network.v1.json` debe tener seeds `active`;
- si solo hay clearnet, ejecutar con decision explicita de operador:

```powershell
zoka fullnode --allow-clearnet-seeds
```

Para pruebas locales sin red externa:

```powershell
zoka fullnode --no-network-config --no-mine --seconds 1
```

## Preflight canonico

Antes de cortar un release candidate:

```powershell
PowerShell -ExecutionPolicy Bypass -File scripts\mainnet_readiness_check.ps1 -BuildRelease
```

El resultado debe terminar con `Failures: 0`.
