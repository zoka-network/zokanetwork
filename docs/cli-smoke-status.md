# ZOKA CLI smoke status

Fecha: 2026-05-05

Objetivo: verificar que los comandos user-facing de `zoka` y el wrapper
`zokahd` arrancan y ejercitan su ruta real sobre un entorno temporal, sin tocar
wallets ni DBs del usuario.

Script reproducible: [`../scripts/cli_smoke_test.ps1`](../scripts/cli_smoke_test.ps1)

Ultima corrida local: `PowerShell -ExecutionPolicy Bypass -File
scripts\cli_smoke_test.ps1` paso 41/41 pasos.

## Resultado

| Comando / flujo | Estado | Nota |
|---|---|---|
| `zoka --help` | OK | Help expone la superficie CLI actual. |
| `zokahd --help` | OK | Help del wrapper automatico. |
| `zoka wallet-create` | OK | Crea wallet privada cifrada y exporta secretos. |
| `zoka wallet-list --json` | OK | Lista wallets creadas con passphrase. |
| `zoka wallet-balance --json` | OK | Devuelve balance/locked/total. |
| `zoka wallet-history --json` | OK | Devuelve historial vacio valido. |
| `zoka wallet-export-secrets` | OK | Escribe export de recovery/material privado. |
| `zoka wallet-backup` | OK | Copia el archivo de wallet cifrado. |
| `zoka wallet-restore-from-seed` | OK | Restaura desde frase BIP39 creada por wallet-create. |
| `zoka wallet-restore-from-keys` | OK | Restaura desde scan/spend keys exportadas. |
| `zoka wallet-change-passphrase` | OK | Re-cifra wallet y permite abrirla con la passphrase nueva. |
| `zoka rpc-list` | OK | Lista RPCs activos desde config sin contactar proveedores externos. |
| `zoka estimate-fee` | OK | Devuelve fee estimada. |
| `zoka tx-send` | OK esperado | En wallet sin fondos rechaza con `insufficient`, que es la respuesta correcta. |
| `zoka height --db ... --json` | OK | DB temporal vacia reporta height 0. |
| `zoka chain-stats --db ... --json` | OK | DB temporal vacia reporta supply/work/dificultad. |
| `zoka chain-prune --dry-run --json` | OK | Dry-run sobre DB temporal no borra nada y reporta `blocks_pruned`. |
| `zoka sync-status --json` | OK | Reporta progreso contra DB temporal. |
| `zoka genesis-verify docs/genesis.mainnet.v1.json --json` | OK | Digest verificado. |
| `zoka mainnet-network-lint docs/mainnet-network.v1.json` | OK | La config RC local esta `launch-ready` y sin placeholders. |
| `zoka release-keygen --json` | OK | Genera keypair ed25519 temporal y solo imprime la clave publica. |
| `zoka snapshot-export` | OK | Exporta snapshot vacio valido. |
| `zoka snapshot-info --json` | OK | Inspecciona header/firmas sin importar la DB. |
| `zoka snapshot-import` | OK | Importa snapshot vacio y reporta bloques aplicados/skipped. |
| `zoka mine-status --json` | OK | Reporta estado de mining. |
| `zoka mine-console --json` | OK | Reporta dashboard JSON. |
| `zoka mine --blocks 1` | OK esperado | Rechaza porque el binario debug actual no fue compilado con `--features randomx`. |
| `zoka fullnode --no-network-config --no-mine --seconds 1` | OK | Arranca, carga wallet, muestra dashboard y cierra por limite de tiempo. |
| `zokahd --inline --no-network-config --no-mine --seconds 1` | OK | Wrapper delega correctamente a `zoka fullnode`. |
| `zoka wallet-wipe --force` | OK | Delete local wallet: borra solo el archivo cifrado local. |

## Limitaciones del smoke

- No valida mineria exitosa en mainnet porque el binario probado es
  `target/debug` sin feature `randomx`.
- No valida transferencia exitosa porque el entorno temporal no tiene fondos
  maduros; si no hay fondos, `tx-send` debe rechazar.
- No valida peers reales porque `fullnode`/`zokahd` se ejecutaron con
  `--no-network-config` para evitar usar placeholders o red externa.
- El smoke no contacta RPC remoto; cubre listado/config local. Para probar RPC
  real, usar `zoka rpc-status --rpc http://host:port`.
- `mainnet-network-lint` valida el RC local. Para mainnet publica, reemplazar
  endpoints locales por seeds/RPC/snapshot signers de operadores reales.
- No reemplaza la testnet publica ni auditoria externa.

## Comandos de verificacion complementarios

```powershell
cargo test --lib
cargo test --tests
cargo check --bins
cargo run --bin zoka -- genesis-verify docs/genesis.mainnet.v1.json --json
PowerShell -ExecutionPolicy Bypass -File scripts\check_randomx_toolchain.ps1
PowerShell -ExecutionPolicy Bypass -File scripts\cli_smoke_test.ps1
```

Para cerrar mineria mainnet real:

```powershell
cargo build --release --features randomx --bin zoka --bin zokahd
```
