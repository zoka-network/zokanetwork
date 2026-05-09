# ZOKA Test Layout

Mapa de **donde vive cada test y por que**. Si querés ejecutar tests
relacionados a un area, usá la columna "Comando".

## Convencion de Rust (no es desorden, es la convencion estandar)

ZOKA sigue la convencion de Rust:

- **Tests unitarios inline** (`#[cfg(test)] mod tests` dentro del modulo
  que prueban) — viven dentro de `src/<modulo>/<archivo>.rs`. Tienen
  acceso a items privados (funciones, structs, constantes). La mayoria de
  los tests viven aca.
- **Tests de integracion** en este directorio (`tests/`) — solo ven la API
  publica del crate. Ejercitan flujos cross-module o setup pesado.

**No pueden mezclarse**: un test que usa `fn validate_coinbase_shape`
(privado dentro de `mining::block_validator`) no puede vivir en `tests/`
porque no veria la funcion. Mover todos los tests a un solo directorio
romperia ~80% de la suite. Por eso quedan inline.

## Tests inline (`src/...::tests`) por modulo

| Modulo | Cobertura | Comando |
|---|---|---|
| `economics::tests` | Halving, cap, maturity, parametros congelados | `cargo test --lib economics::` |
| `mining::pow::tests` | PowEngine, RandomX gating, fallback determinista, epoch seed | `cargo test --lib mining::pow::` |
| `mining::difficulty::tests` | Retarget step up/down, warmup, clamps, bounds | `cargo test --lib mining::difficulty::` |
| `mining::block_validator::tests` | Coinbase shape/amount/cap, private coinbase, private fee claims, PoW, chain link, header self-consistency | `cargo test --lib mining::block_validator::` |
| `state::chain::tests` | apply_block, fork choice, reorg, side branch lighter rejection, BlockChainMeta persistence | `cargo test --lib state::chain::` |
| `state::sync::tests` | PeerChainView, try_apply_synced_payload, gap/duplicate/sibling outcomes | `cargo test --lib state::sync::` |
| `state::tests` | StateManager, account proofs, private commitment tree, pruner | `cargo test --lib state::tests::` |
| `genesis::tests` | Schema validation, digest determinism, validator set keys, pre-mine cap, regression gate del genesis publicado | `cargo test --lib genesis::` |
| `snapshot::tests` | Snapshot roundtrip, sign/verify, byte tampering, idempotent import | `cargo test --lib snapshot::` |
| `wallet::storage::tests` | Encrypted storage, password-incorrecto messaging, missing wallet messages | `cargo test --lib wallet::storage::` |
| `private_wallet::tests` | PrivateWallet, claim coinbase/fees, decrypt notes | `cargo test --lib private_wallet::` |
| `transaction::*::tests` | CLSAG, nullifiers | `cargo test --lib transaction::` |
| `network::*::tests` | Message validation, peer discovery, dandelion | `cargo test --lib network::` |
| `consensus::*::tests` | PBFT message handling, view change | `cargo test --lib consensus::` |
| `crypto::tests` | Pedersen, Bulletproofs, MPC ceremony helpers | `cargo test --lib crypto::` |
| `api::tests` | HTTP RPC handlers, snapshot endpoints, WS routing | `cargo test --lib api::` |

Y el binario (`src/main.rs`) tiene su propio `#[cfg(test)] mod tests` con
8+ tests para `validate_user_private_address` + `mainnet_network_config`.

```sh
cargo test --bin zoka
```

## Tests de integracion (`tests/`)

| Archivo | Que ejercita |
|---|---|
| `integration_test.rs` | Flujos top-level del CLI / lib via API publica. |
| `crypto_integration_test.rs` | Roundtrips de circuitos Groth16 + commitments + Bulletproofs. |
| `network_integration_test.rs` | Dos nodos libp2p comunicandose, gossipsub, dandelion. |
| `pbft_transport_integration_test.rs` | PBFT message routing entre instancias. |
| `pbft_transport_multiprocess_test.rs` | PBFT multi-proceso (spawnea processes reales). |
| `tor_transport_test.rs` | Configuracion del transport Tor (smoke). |
| `nivel3_transactions_test.rs` | Transacciones privadas end-to-end (anonymity-set, mixer, claim). |

Estos tests **pueden** correr en paralelo con los unitarios pero algunos
(`pbft_transport_multiprocess_test.rs`, `network_integration_test.rs`)
levantan listeners reales y se sirven mejor con `--test-threads=1` cuando
fallan.

```sh
# Todos los tests de integracion
cargo test --tests

# Uno especifico
cargo test --test integration_test
cargo test --test pbft_transport_multiprocess_test -- --test-threads=1
```

## Suite completa

```sh
# Todo (lib + bin + integracion + doc-tests)
cargo test --workspace

# Solo lib (rapido, no toca network ni P2P spawn)
cargo test --lib

# Solo el bin principal
cargo test --bin zoka
```

## Tests con feature `randomx`

El backend RandomX se compila solo cuando el feature esta activo. Para
ejercitarlo (requiere toolchain MSVC + cmake en Windows, o gcc + cmake
en Linux):

```sh
powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict
cargo test --lib --features randomx mining::pow::
```

Sin la feature, `mining::pow::tests::block_for_mainnet_chain_without_randomx_feature_is_rejected_cleanly`
verifica que el rechazo sea explicito y orientativo (no panic obscuro).

## Artefactos en disco

Algunos tests crean directorios `.test_*` en la raiz del repo (RocksDB
fixtures). Estan en `.gitignore` y se regeneran en cada corrida. NO los
borres a mano si tests estan corriendo en paralelo — esos directorios
estan tomados por instancias activas de RocksDB.

Si querés un repo limpio post-test:

```sh
# Solo despues de que TODOS los tests hayan terminado
git clean -fdX  # borra solo archivos ignorados
```

## Property tests

`proptest` esta declarado en `dev-dependencies` (`Cargo.toml`). Hoy se usa
solo en algunos modulos `crypto::*`. Es un slot abierto en
[`agent-vs-human-roadmap.md`](../docs/agent-vs-human-roadmap.md) para
extender cobertura sobre `validate_pow_block` y `genesis::validate_static`.

## Fuzzing

El directorio `fuzz/` tiene targets cargo-fuzz para los modulos
criticos. Ver [`docs/fuzzing_coverage.md`](../docs/fuzzing_coverage.md)
para coverage actual.

```sh
cd fuzz
cargo fuzz list      # ver targets disponibles
cargo fuzz run <target>
```
