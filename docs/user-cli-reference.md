# ZOKA User CLI Reference

Comandos minimos publicados en cada release de mainnet. Cobertura: usar una
wallet, transferir ZKA, minar, correr un fullnode, verificar genesis y
snapshots.

Requisitos previos: binarios verificados segun
[`release-verification.md`](release-verification.md).

> Convenciones: `<requerido>` significa argumento obligatorio,
> `[opcional]` significa argumento opcional, `--flag` activa una opcion.
> Las direcciones de pago empiezan con `zka1` y se imprimen sin redaction.
> Los montos `--amount` y `--fee` aceptan hasta 6 decimales:
> `0.000001 ZKA` es 1 atom.
> No hay modo de transferencia transparente en la superficie de usuario.

## Comandos cortos recomendados

```powershell
zoka
zoka create <label> --network mainnet|testnet --password <pwd>
zoka restore "<recovery phrase>" --network mainnet|testnet --password <pwd> [--name <label>]
zoka wallets [--network mainnet|testnet] [--password <pwd>]
zoka balance <wallet> [--network mainnet|testnet] [--password <pwd>]
zoka receive <wallet> [--network mainnet|testnet] [--password <pwd>]
zoka send <from> <to> <amount> [--network mainnet|testnet] [--password <pwd>] --yes
zoka pending [--network mainnet|testnet] [--db <path>]
zoka mine <wallet> [--network mainnet|testnet] [--password <pwd>] [--power low|normal|max]
zoka node [--network mainnet|testnet] [--password <pwd>] [--seed <multiaddr>] [--power low|normal|max]
zoka node-status [--db <path>] [--password <pwd>]
zoka status
zoka rpc-list
zoka rpc-status [--rpc http://host:port]
```

Los nombres largos (`wallet-create`, `tx-send`, `fullnode`, etc.) siguen
existiendo para scripts, pero los comandos cortos son la UX principal.
Las wallets y el estado de mineria se guardan por red dentro del proyecto:
`.zoka/mainnet/private_wallets`, `.zoka/testnet/private_wallets`,
`.zoka/mainnet/user_mining_state.json` y
`.zoka/testnet/user_mining_state.json`.

---

## RPC mainnet read-only

```
zoka rpc-list [--network-config <file>] [--json]
zoka rpc-status [--rpc http://host:port] [--network-config <file>] [--timeout-secs <n>] [--json]
zoka rpc-height [--rpc http://host:port] [--network-config <file>] [--timeout-secs <n>] [--json]
zoka rpc-mempool [--rpc http://host:port] [--network-config <file>] [--timeout-secs <n>] [--json]
```

Consultas read-only contra RPCs ZOKA. No leen wallets, no firman y no envian
transacciones. Si no se pasa `--rpc`, usan el primer RPC activo de
`docs/mainnet-network.v1.json` o del archivo indicado por `--network-config`.

En v0.4 el cliente embebido soporta `http://host:port` con timeouts cortos. Para
RPC publico HTTPS/onion/I2P, usar un proxy local HTTP confiable o esperar el
cliente TLS/proxy dedicado.

---

## Node - fullnode y estado

### `zoka node`

```powershell
zoka node [--address <wallet-address|name>] [--password <pwd>]
          [--seed <multiaddr>] [--power low|normal|max]
          [--no-mine] [--seconds <n>]
```

Arranca un fullnode con dashboard en consola. Por default sincroniza y mina con
la wallet local seleccionada. Usar `--no-mine` para modo verificador. `--power`
usa los mismos presets de CPU que `zoka mine`; `--threads <n>` sigue disponible
para operadores y tiene prioridad.

Si no se pasa `--seed`, el nodo usa la configuracion de red publicada. Para una
red local o pruebas aisladas, pasar `--no-network-config`; si se dan seeds
manuales y pueden fallar, `--allow-isolated-start` permite seguir en modo
standalone.

### `zoka node-status`

```powershell
zoka node-status [--db <path>] [--address <wallet-address|name>]
                 [--password <pwd>] [--json]
```

Muestra estado local: DB, altura, tx privadas pendientes, estado de mineria,
hilos CPU disponibles y snapshot de wallet si puede abrirse.

### `zokahd`

`zokahd` es el daemon estandar para correr un nodo full. Por default crea o
carga la wallet local del nodo, sincroniza desde peers, y mina si la
configuracion lo pide.

```
zokahd
zokahd [--network mainnet|testnet] [--address <wallet-address>] [--password <pwd>] [--seed <multiaddr>]
       [--seconds <n>] [--show-keys] [--no-mine]
       [--power low|normal|max] [--db <path>]
       [--genesis <file>] [--inline]
```

- Sin argumentos abre autopilot: elige la primera wallet local o crea una
  wallet cifrada, informa la ruta guardada, usa la red configurada para sync y
  activa mineria automatica cuando la cadena local esta lista. El usuario solo
  debe ingresar el password de la wallet.
- En modo no interactivo, `--password` es obligatorio cuando el daemon va a
  auto-crear o desbloquear una wallet cifrada.
- `--genesis <file>` aplica el genesis canonico (`docs/genesis.mainnet.v1.json`)
  contra el DB en el primer arranque. Solo se aplica una vez por DB; el
  digest queda anclado en `chain:genesis:digest`.
- `--no-mine` arranca el nodo en modo solo-verifier (sin PoW local).
- En Windows, el daemon abre su propia consola por default; pasar
  `--inline` para mantenerlo en la terminal actual.

---

## Status - dashboard general

```powershell
zoka status [--address <wallet-address|name>] [--password <pwd>]
            [--db <path>] [--network-config <file>] [--json]
```

Vista unica para usuario normal: wallet, saldo disponible/locked, tx privadas
pendientes, altura del DB local, mineria, recompensas locked y resumen de la
red configurada. `mine-console` queda como dashboard dedicado solo a mineria.

---

## Wallet - crear, restaurar, listar

### `zoka wallet-create`

```
zoka wallet-create [--name <label>] --password <pwd> [--export-secrets <path>]
```

Crea una wallet privada nueva. Imprime la direccion `zka1...`, la frase de
recuperacion (12 palabras BIP39), la ruta local del archivo cifrado y los
proximos pasos. **Guardar la frase fuera de linea.** El archivo queda cifrado
con `--password` en `.zoka/<network>/private_wallets/`. Puedes crear mas de una wallet;
cada una conserva su propio password, claves y frase. Las claves hex completas
solo se imprimen con `--show-keys` o exportando secretos con
`--export-secrets <path>`.

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

### Comandos simples

```powershell
zoka receive <wallet> [--password <pwd>]
zoka send <from> <to> <amount> [--password <pwd>] --yes
zoka pending [--db <path>]
zoka fee <amount>
```

`receive` imprime la direccion privada `zka1...` de una wallet local.
`send` estima la fee automaticamente y, si existe `.zoka/blockchain.db`, deja
la transferencia privada en cola para que el nodo local la mine. `pending`
muestra las transferencias privadas todavia no incluidas en bloques del DB
local del nodo.

### `zoka tx-send`

```
zoka tx-send --from <wallet-address|name> --to <wallet|address>
             --amount <n> [--fee <n>] [--priority low|normal|high]
             [--password <pwd>] [--db <path>] --yes
```

Envia ZKA. `--yes` es obligatorio para evitar broadcasts accidentales. La
transaccion es privada por construccion: monto y receptor quedan ocultos en
la cadena, el receptor descifra localmente. Si se pasa `--db <path>`, el CLI
tambien persiste el cuerpo privado y marca el hash como pendiente para que el
miner local lo comprometa en el siguiente bloque.

`--fee` puede ser fraccionario. En red quieta el estimador puede devolver `0`;
bajo carga sube en pasos de `0.0001 ZKA`. Las fees privadas se pagan al minero
mediante `private_fee_claims`, sin publicar el monto claro en la transaccion.

---

## Mining

### `zoka mine`

```
zoka mine --address <wallet-address> [--password <pwd>] [--minutes <n>]
          [--blocks <n>] [--power low|normal|max] [--threads <n>]
```

Si no se pasa `--minutes` ni `--blocks`, mina hasta interrumpir (Ctrl+C).
Mainnet usa el algoritmo `zoka-randomx-v1` (RandomX) cuando el binario fue
construido con `--features randomx`. Un binario sin la feature falla en
arranque contra `MAINNET_CHAIN_ID` con un error explicito.

`--power` es la opcion simple para usuarios normales:

- `low`: usa 1 hilo de CPU.
- `normal`: usa aproximadamente la mitad de los hilos disponibles.
- `max`: usa todos los hilos disponibles.

`--threads <n>` sigue disponible para operadores y scripts; cuando se pasa,
tiene prioridad sobre `--power`.

### `zoka mine-status`

```
zoka mine-status [--json]
```

Imprime altura local, dificultad actual, hashrate estimado, hilos CPU
disponibles, recompensas pendientes en maturity y proxima altura de unlock.

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
                     [--signer-key <hex>]
```

Carga un snapshot publicado por el equipo core o un mirror conocido.
`--signer-key` (raw 32 bytes hex de la verifying key del signer) hace que se
exija una firma valida del signer esperado. Tras la verificacion, cada bloque
se replaya por
`state::chain::apply_block`, asi que cualquier bloque invalido se rechaza
con la misma regla de consenso que aplicaria un nodo recibiendolo por
gossipsub.

### `zoka snapshot-export` (operadores avanzados)

```
zoka snapshot-export <out-path> [--db <path>] [--sign-key-hex <hex>]
```

Exporta el snapshot del chain DB local. `--sign-key-hex` (raw 32 bytes hex
del secret key ed25519) firma el blob; la verifying key queda incrustada en
el archivo bajo `signer_pubkey_hex`. Util para producir
mirrors privados o backups firmados.

---

## Comandos auxiliares

| Comando | Descripcion |
|---|---|
| `zoka status` | Dashboard general: wallet, transfers, mining y nodo local. |
| `zoka height` | Imprime la altura actual del chain DB. |
| `zoka node-status --db <path>` | Dashboard local del nodo y su DB. |
| `zoka sync-status --address <addr>` | Progreso de sync vs altura conocida. |
| `zoka estimate-fee --amount <n>` | Estimador de fee para `tx-send`. |
| `zoka tx-pending --db <path>` | Lista hashes privados pendientes de minar. |
| `zoka wallet-history --address <addr>` | Historial de transacciones. |
| `zoka wallet-receive --address <addr>` | Imprime la direccion para recibir. |
| `zoka wallet-backup --address <addr> --out <path>` | Copia el archivo cifrado. |
| `zoka wallet-export-secrets --address <addr>` | Exporta scan/spend keys (uso restringido). |
| `zoka wallet-change-passphrase --address <addr>` | Cambia el passphrase del archivo cifrado. |
| `zoka wallet-wipe --address <addr> --force` | Elimina la wallet local. Hacer backup antes. |

---

## Variables de ambiente / paths

- `~/.zoka/blockchain.db` - chain DB del fullnode (RocksDB).
- `~/.zoka/wallets/` - ruta legacy; no es la superficie normal de usuario.
- `.zoka/<network>/private_wallets/` - archivos de wallet privadas locales.
- `.zoka/<network>/user_mining_state.json` - estado local de mineria y recompensas locked.
- `~/.zoka/user_mining_state.json` - estado del miner local (recompensas en
  maturity, dificultad, contador).

Todos los paths se pueden overridear con `--db <path>` cuando aplique. Las
wallets viven SIEMPRE bajo `~/.zoka/`; no se puede mover el directorio sin
mover los archivos manualmente.
