# ZOKA User CLI Reference

Comandos minimos publicados en cada release de mainnet. Cobertura: usar una
wallet, transferir ZKA, minar, correr un fullnode, verificar genesis y
snapshots.

Requisitos previos: binarios verificados segun
[`release-verification.md`](release-verification.md).

> Convenciones: `<requerido>` significa argumento obligatorio,
> `[opcional]` significa argumento opcional, `--flag` activa una opcion.
> Las direcciones publicas empiezan con `zka1` y se imprimen sin
> redaction.

---

## `zokahd` — fullnode automatico

`zokahd` es el daemon estandar para correr un nodo full. Por default crea o
carga la wallet local del nodo, sincroniza desde peers, y mina si la
configuracion lo pide.

```
zokahd [--address <wallet-address>] [--password <pwd>] [--seed <multiaddr>]
       [--seconds <n>] [--show-keys] [--no-mine] [--db <path>]
       [--genesis <file>] [--inline]
```

- `--password` es obligatorio cuando el daemon va a auto-crear la wallet.
- `--genesis <file>` aplica el genesis canonico (`docs/genesis.mainnet.v1.json`)
  contra el DB en el primer arranque. Solo se aplica una vez por DB; el
  digest queda anclado en `chain:genesis:digest`.
- `--no-mine` arranca el nodo en modo solo-verifier (sin PoW local).
- En Windows, el daemon abre su propia consola por default; pasar
  `--inline` para mantenerlo en la terminal actual.

---

## Wallet — crear, restaurar, listar

### `zoka wallet-create`

```
zoka wallet-create [--name <label>] --password <pwd> [--export-secrets <path>]
```

Crea una wallet privada nueva. Imprime la direccion `zka1...`, la frase de
recuperacion (12 palabras BIP39) y las claves derivadas. **Guardar la frase
fuera de linea.** El archivo de wallet queda cifrado con `--password`.

### `zoka wallet-restore-from-seed`

```
zoka wallet-restore-from-seed [--name <label>] --seed "<bip39 words>"
                              --password <pwd> [--restore-height <n>]
```

Restaura una wallet desde la frase de 12 palabras. `--restore-height` evita
re-escanear desde genesis cuando sabes la altura de creacion aproximada.

### `zoka wallet-list`

```
zoka wallet-list [--password <pwd>] [--json]
```

Lista wallets locales. `--json` imprime maquina-legible.

### `zoka wallet-balance`

```
zoka wallet-balance --address <wallet-address> [--password <pwd>] [--json]
```

Imprime balance disponible + locked (recompensas en maturity).

---

## Transferencias

### `zoka tx-send`

```
zoka tx-send --from <wallet-address|name> --to <wallet|address>
             --amount <n> [--fee <n>] [--priority low|normal|high]
             [--password <pwd>] [--db <path>] --yes
```

Envia ZKA. `--yes` es obligatorio para evitar broadcasts accidentales. La
transaccion es privada por construccion: monto y receptor quedan ocultos en
la cadena, el receptor descifra localmente.

---

## Mining

### `zoka mine`

```
zoka mine --address <wallet-address> [--password <pwd>] [--minutes <n>]
          [--blocks <n>]
```

Si no se pasa `--minutes` ni `--blocks`, mina hasta interrumpir (Ctrl+C).
Mainnet usa el algoritmo `zoka-randomx-v1` (RandomX) cuando el binario fue
construido con `--features randomx`. Un binario sin la feature falla en
arranque contra `MAINNET_CHAIN_ID` con un error explicito.

### `zoka mine-status`

```
zoka mine-status [--json]
```

Imprime altura local, dificultad actual, hashrate estimado, recompensas
pendientes en maturity.

---

## Genesis

### `zoka genesis-verify`

```
zoka genesis-verify <path-to-genesis.json> [--json]
```

Validador offline. Carga el archivo, corre `validate_static`, computa el
SHA-256 digest sobre la encoding canonica, e imprime un reporte con chain
id, pow algorithm, validator count, voting power activo, total pre-mine, e
initial accounts. Comparar el digest contra el publicado en
[`genesis.mainnet.v1.spec.md`](genesis.mainnet.v1.spec.md) antes de aplicar
el genesis a un DB de mainnet.

---

## Fast sync (snapshot)

### `zoka snapshot-import`

```
zoka snapshot-import <in-path> [--db <path>] [--require-signature]
                     [--public-key <hex>]
```

Carga un snapshot publicado por el equipo core o un mirror conocido.
`--public-key` (raw 32 bytes hex) hace que se exija una firma valida del
signer esperado. Tras la verificacion, cada bloque se replaya por
`state::chain::apply_block`, asi que cualquier bloque invalido se rechaza
con la misma regla de consenso que aplicaria un nodo recibiendolo por
gossipsub.

### `zoka snapshot-export` (operadores avanzados)

```
zoka snapshot-export <out-path> [--db <path>] [--sign-key-hex <hex>]
```

Exporta el snapshot del chain DB local. `--sign-key-hex` (raw 32 bytes hex
del secret key ed25519) firma el blob; la verifying key publica queda
incrustada en el archivo bajo `signer_pubkey_hex`. Util para producir
mirrors privados o backups firmados.

---

## Comandos auxiliares

| Comando | Descripcion |
|---|---|
| `zoka height` | Imprime la altura actual del chain DB. |
| `zoka sync-status --address <addr>` | Progreso de sync vs altura conocida. |
| `zoka estimate-fee --amount <n>` | Estimador de fee para `tx-send`. |
| `zoka wallet-history --address <addr>` | Historial de transacciones. |
| `zoka wallet-backup --address <addr> --out <path>` | Copia el archivo cifrado. |
| `zoka wallet-export-secrets --address <addr>` | Exporta scan/spend keys (uso restringido). |
| `zoka wallet-change-passphrase --address <addr>` | Cambia el passphrase del archivo cifrado. |
| `zoka wallet-wipe --address <addr> --force` | Elimina la wallet local. Hacer backup antes. |

---

## Variables de ambiente / paths

- `~/.zoka/blockchain.db` — chain DB del fullnode (RocksDB).
- `~/.zoka/wallets/` — archivos de wallet publicas.
- `~/.zoka/private_wallets/` — archivos de wallet privadas (default).
- `~/.zoka/user_mining_state.json` — estado del miner local (recompensas en
  maturity, dificultad, contador).

Todos los paths se pueden overridear con `--db <path>` cuando aplique. Las
wallets viven SIEMPRE bajo `~/.zoka/`; no se puede mover el directorio sin
mover los archivos manualmente.
