# ZOKA Pruning & Snapshots Strategy (v1)

Estado: estrategia inicial para Fase 4 del roadmap mainnet. Define como un nodo
nuevo se pone al dia rapido y como un nodo viejo libera disco sin perder la
capacidad de servir a la red.

Implementacion: [`src/snapshot.rs`](../src/snapshot.rs) + comandos
`zoka snapshot-export` / `zoka snapshot-import`.

## Modelo de almacenamiento actual

ZOKA persiste cada bloque completo bajo `block:<hash>` en RocksDB, con
indices auxiliares `chain:block_height:<n>`, `chain:block_meta:<hash>`,
`chain:height`, `chain:last_hash`, `chain:total_supply_emitted`,
`chain:cumulative_work`, `chain:current_difficulty_bits`,
`chain:genesis:digest`. Cuentas individuales viven en `state:<address>`.

Sin pruning ni snapshots, el disco crece linealmente con la altura de la cadena
(~size_bloque * 720_blocks_por_dia). A 23M ZKA con halving a 1,051,200 bloques
por era, una era completa dura ~4 anos y produce ~3M bloques.

## Fast sync via snapshots

### Objetivo

Permitir que un usuario nuevo arranque su nodo y este en la cadena en minutos,
no en horas, **sin sacrificar la verificacion completa** de cada bloque que
recibe.

### Modelo de confianza

Un snapshot es un blob deterministico que captura `(head, blocks 1..head)`.
**No** se confia ciegamente en el blob:

1. El importador corre `zoka::snapshot::verify` para validar header (schema,
   chain_id, alturas contiguas, head metadata).
2. El importador llama `zoka::snapshot::import_into_storage`, que **replaya
   cada bloque** por `state::chain::apply_block`. Eso recomputa el PoW,
   valida la coinbase, valida la dificultad esperada y rechaza cualquier
   bloque que no pase. Es el mismo ingress path que un bloque recibido por
   gossipsub.

El unico atajo que da el snapshot es no hacer round-trips de red por bloque.
Las reglas de consenso se aplican byte-a-byte.

### Firma opcional

Un snapshot **puede** llevar firma ed25519 raw 64 bytes (`signature_hex`) +
verifying key publica (`signer_pubkey_hex`). El operador del nodo nuevo pasa
`--public-key <hex>` al importar para garantizar que el blob viene del firmante
esperado (ej. el equipo core o un mirror conocido). Sin `--public-key`, el
importador acepta blobs no firmados pero igual replaya cada bloque, asi que un
blob hostil solo puede demorar (sirviendo bloques validos), nunca corromper.

### Generacion

Un nodo full corriendo contra mainnet:

```sh
zoka snapshot-export ./mainnet.zoka-snapshot.json \
  --db ~/.zoka/blockchain.db \
  --sign-key-hex <32-bytes-hex>
```

El comando imprime el digest SHA-256, que se publica junto al archivo. Cualquier
mirror puede comparar `sha256sum` contra el digest publicado.

### Importacion

Nodo nuevo:

```sh
zoka snapshot-import ./mainnet.zoka-snapshot.json \
  --db ~/.zoka/blockchain.db \
  --public-key <signer-pubkey-hex>
```

Tras la verificacion, el nodo arranca normal (`zokahd`) y completa por IBD via
`request_block_with_retry` cualquier bloque emitido despues del snapshot.

### Cadencia recomendada

- **Mainnet release**: snapshot firmado por el equipo core en cada release de
  binario, anunciado junto al digest en [`docs/release-verification.md`](release-verification.md).
- **Operacionales**: cualquier nodo full puede generar snapshots por su cuenta
  (sin firma) para tooling interno o backup.

## Pruning

### Implementacion v2

`zoka chain-prune --keep-last <N> [--db <path>] [--dry-run]` borra los block
bodies con altura `< head - N` de RocksDB, junto con sus entradas en
`chain:block_height:<n>` y `chain:block_meta:<hash>`. La logica vive en
`state::chain::prune_below_head` y los helpers de eliminacion (`delete_block_body`,
`delete_block_hash_at_height`, `delete_raw`) en `storage::RocksDbStorage`.

Comando recomendado para nodos casuales que no quieren retener historia
profunda (4 dias bajo block time = 120s):

```sh
zoka chain-prune --keep-last 2880 --dry-run    # ver que se borraria
zoka chain-prune --keep-last 2880              # aplicar
```

### Reglas de invariantes que el pruner NO toca

- `chain:genesis:digest`, `chain:height`, `chain:last_hash`,
  `chain:total_supply_emitted`, `chain:cumulative_work`,
  `chain:current_difficulty_bits`: chain-level metadata, nunca tocada.
- Bloques en `[max(1, head - keep_last + 1), head]`: se mantienen completos
  (cuerpo + height index + block_meta) para servir IBD reciente y
  retargeting de dificultad.
- `keep_last` se eleva silenciosamente a `PRUNE_MIN_RETAINED_BLOCKS = 8`
  cuando el caller pide menos. El piso lo dicta el algoritmo de retarget
  (`mining::difficulty::expected_for_next_block` necesita parent +
  grandparent), no es una preferencia de operador.

### Implicancias operacionales

Un nodo prunado **no puede servir IBD para heights < head - keep_last**. Si
el operador quiere contribuir a la red ayudando a otros nodos a sincronizar,
mejor mantener el chain DB completo. Pruning es para nodos casuales con
constraint de disco que solo necesitan participar (minar, transferir,
verificar) sin ser archivo.

### Cuentas en el state layer

`state::pruner` ya soporta el pruning de cuentas con balance 0 cuyo
`last_updated_height` < head - N (ver `state::tests::test_should_prune_old_accounts`).
Es ortogonal al chain-prune; se puede correr separadamente.

## Roadmap del modulo

- [x] v1: snapshot export/import con firma ed25519 opcional + replay completo
      por `apply_block`.
- [x] v2: pruner automatico con CLI `zoka chain-prune --keep-last <n>`. Implementado en `state::chain::prune_below_head` con `PRUNE_MIN_RETAINED_BLOCKS=8` como piso (no se puede romper el retarget de dificultad). 8 tests cubren empty chain, floor, retain head, difficulty retarget post-prune, apply post-prune, dry-run, clamp.
- [ ] v3: snapshot diferencial (delta sobre un snapshot base) para releases
      menores.

Quien firma esto: equipo core de ZOKA. Cualquier cambio en el formato del
snapshot bumpea `SNAPSHOT_SCHEMA_ID` y obliga a re-firmar.
