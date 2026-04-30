# ZOKA Mainnet Genesis Spec (`zoka-genesis-v1`)

Estado: schema canonico para Fase 3 del roadmap mainnet. Todo nodo que arranque
contra mainnet consume esta misma estructura; cualquier campo extra, faltante o
inconsistente con las constantes economicas hace que el loader rechace el archivo.

Implementacion: [`src/genesis.rs`](../src/genesis.rs). Tests:
`cargo test --lib genesis::tests` (22 casos).

## Identificadores

- `schema = "zoka-genesis-v1"` — discriminador de version. Cambiarlo es un hard
  fork y exige `genesis.mainnet.v2.spec.md` separado.
- `chain_id = "zoka-mainnet-v1"` para la red oficial. El loader rechaza cualquier
  `pow_algorithm` que no coincida con el canonico para ese chain id (mainnet =
  `zoka-randomx-v1`).

## Campos top-level

```json
{
  "schema": "zoka-genesis-v1",
  "chain_id": "zoka-mainnet-v1",
  "pow_algorithm": "zoka-randomx-v1",
  "genesis_timestamp": 1700000000,
  "economics": { ... },
  "validators": [ ... ],
  "pre_mine": [ ... ],
  "initial_accounts": [ ... ]
}
```

Campos opcionales adicionales: ninguno. Serde lleva `deny_unknown_fields`, por lo
que `genesis-verify` falla con error explicito ante cualquier campo no definido
aqui.

## `economics` — constantes restated

Repite los parametros economicos canonicos para que el loader detecte drift entre
el binario corriendo y el archivo publicado.

```json
{
  "max_supply_zka": 23000000,
  "initial_block_reward_zka": 12,
  "halving_interval_blocks": 1051200,
  "target_block_time_secs": 120,
  "coinbase_maturity_blocks": 60
}
```

Cualquier valor distinto al definido en `src/economics.rs` es rechazado con
`EconomicsDrift { field, got, expected }`. Cambiar uno de estos numeros sin
hard fork explicito es un bug de consenso.

## `validators` — set inicial

Cada entrada describe un validador participando en el consenso PBFT que finaliza
bloques publicados por PoW. La estructura separa **clave de consenso** (firma
mensajes PBFT) de **clave de operador** (ejecuta acciones administrativas
fuera-de-banda como rotacion). Reusar la misma clave para ambos roles es
rechazado.

```json
{
  "label": "validator-eu-1",
  "consensus_pubkey_hex": "<32 bytes ed25519 hex>",
  "operator_pubkey_hex":  "<32 bytes ed25519 hex>",
  "voting_power": 100,
  "role": "active"
}
```

- `role` valores: `"active"` (vota desde epoch 0) o `"standby"` (no vota hasta
  promocion por gobernanza).
- Validadores con la misma `label` o el mismo `consensus_pubkey_hex` /
  `operator_pubkey_hex` son rechazados.
- Ambas claves deben ser hex de exactamente 32 bytes (ed25519).
- Si la lista NO esta vacia, el total de `voting_power` de validadores
  `active` debe ser > 0. Una lista vacia es valida (cadenas pure-PoW que
  difieren PBFT a una fase posterior).

## `pre_mine` — emision pre-genesis

Salidas que cuentan contra `MAX_SUPPLY_ZKA`. Cada entrada DEBE publicar su
justificacion en texto plano dentro del mismo archivo — ese texto se hashea junto
con el resto en el digest.

```json
{
  "recipient_address": "zka1...",
  "amount_zka": 1000,
  "justification": "Reserva fundacion ZOKA — 5 anos lock-up off-chain"
}
```

Reglas:
- `amount_zka == 0` rechazado: si no hay monto, eliminar la entrada.
- `justification` vacio rechazado.
- Suma total de `pre_mine.amount_zka` no puede exceder `MAX_SUPPLY_ZKA`.
- Direcciones duplicadas (en `pre_mine`) rechazadas.
- Direccion presente en `pre_mine` Y en `initial_accounts` rechazada.

Para mainnet sin pre-mine: pasar `"pre_mine": []`.

## `initial_accounts` — bookkeeping

Cuentas con balance publicado (incluyendo cero) que el loader crea sin contar
contra el supply cap. Util para anclar direcciones bien conocidas
(ej. burn address, faucet, treasury post-launch).

```json
{
  "address": "zka1...",
  "balance_zka": 0
}
```

Reglas: direccion no vacia, no duplicada, no compartida con `pre_mine`.

## Digest

```
sha256(serde_json::to_vec_pretty(genesis)) -> 32 bytes -> hex
```

Determinista byte a byte. Se almacena en `chain:genesis:digest` al aplicar y se
publica junto con el archivo. Operadores corren `zoka genesis-verify <path>` y
comparan el digest impreso con el anuncio oficial antes de arrancar el nodo.

## Comando `genesis-verify`

```
$ zoka genesis-verify docs/genesis.mainnet.v1.json
Genesis file:        docs/genesis.mainnet.v1.json
Schema:              zoka-genesis-v1
Chain id:            zoka-mainnet-v1
PoW algorithm:       zoka-randomx-v1
SHA-256 digest:      <64-hex-chars>
Validator count:     7
Active voting power: 700
Pre-mine total ZKA:  500000
Initial accounts:    3

OK — genesis is structurally valid. Compare the digest against the published
value before applying it.
```

Con `--json` imprime la misma informacion como una linea de JSON apta para
pipelines de release/CI.

El comando es offline: no toca RocksDB, no toca la red, no muta nada. Solo carga
el archivo, corre `validate_static` y computa el digest. Sigue el principio de
"medir dos veces, cortar una": antes de aplicar el genesis a un DB de mainnet,
correr `genesis-verify` sobre el mismo archivo y verificar que el digest
coincide con el publicado por el equipo core.

## Pendientes operativos (no parte del schema)

Estos puntos son procedimentales y se documentaran en runbooks de Fase 8, NO en
este spec:

- Procedimiento concreto de rotacion de claves cuando un validador pierde la
  hot signing key.
- Procedimiento de halt recovery PBFT cuando se pierde quorum.
- Decision sobre existencia de pre-mine: politica del proyecto, no del schema.
- Dry-run del genesis: actividad operacional usando `genesis-verify` y
  `apply_genesis_to_db --force=false` contra un DB testnet limpio antes del
  launch real.

## Quien firma esto

Schema propuesto y mantenido por el equipo core de ZOKA, registrado como Fase 3
del roadmap mainnet v6. Cualquier cambio de campo (no de valor) requiere bumpear
`schema` a `zoka-genesis-v2` y publicar un spec equivalente.
