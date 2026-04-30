# ZOKA Mainnet Launch Roadmap v6

Objetivo: convertir la base tecnica actual en una mainnet publica lanzable, verificable y operable por usuarios reales.

Estado honesto: ZOKA tiene una base avanzada de wallet, CLI, fullnode, privacidad por defecto, PoW local real y economia preliminar. No esta lista para mainnet publica hasta cerrar los bloqueantes de este documento.

---

## Criterio de salida

Mainnet publica solo se considera lista cuando:

- un usuario puede crear, respaldar, restaurar y usar una wallet sin ayuda tecnica;
- un nodo puede arrancar, sincronizar, minar y mantenerse conectado sin intervencion manual;
- el genesis, chain ID, economia y validator set estan congelados y publicados;
- los binarios son reproducibles, firmados y verificables;
- la red fue probada en testnet publica continua;
- las partes criptograficas y de consenso fueron revisadas externamente;
- existe procedimiento de recuperacion operativa ante halt, bug critico o hard fork.

---

## Fase 1 - Wallet de usuario completa

Bloqueante de mainnet.

- [x] Implementar `wallet-restore-from-seed`.
- [x] Garantizar que la recovery phrase de `wallet-create` derive deterministicamente scan/spend keys para wallets nuevas.
- [x] Validar checksum/longitud/formato de seed antes de restaurar.
- [x] Agregar tests create -> restore-from-seed -> same address.
- [x] Agregar `restore-height` persistido en restauracion desde seed.
- [x] Confirmar cifrado de wallet con passphrase no opcional para wallets nuevas/restauradas.
- [x] Agregar cambio de passphrase para wallets privadas.
- [x] Revisar que todos los textos user-facing del CLI esten en ingles.
- [x] Validar mensajes de error accionables para direccion invalida, password incorrecto y wallet inexistente.

Nota: las wallets creadas antes de este cambio tenian `spend_key` aleatoria separada de la frase. Esas wallets siguen siendo recuperables por `wallet-restore-from-keys`; la recuperacion completa por seed aplica a wallets nuevas creadas con la derivacion deterministica v1.

Nota: las wallets legacy sin passphrase todavia pueden cargarse para compatibilidad, pero los flujos nuevos de `wallet-create`, `wallet-restore-from-seed`, `wallet-restore-from-keys` y autocreacion de fullnode exigen passphrase.

Nota errores accionables: el password incorrecto se reporta como `incorrect password (or wallet file corrupted)` (AEAD tag mismatch), no como ruido criptografico interno; las direcciones invalidas devuelven la causa especifica (prefijo, base58, longitud, checksum) via `validate_user_private_address`; los `wallet not found` incluyen lista de wallets disponibles y sugerencia de `wallet-list`/`wallet-create`. Cobertura en `src/wallet/storage.rs` tests y `src/main.rs` tests modulo `tests`.

Salida de fase:

- `wallet-create`, `wallet-restore-from-seed`, `wallet-restore-from-keys`, `wallet-list`, `wallet-balance`, `wallet-history`, `wallet-backup`, `wallet-wipe` y `wallet-export-secrets` probados en binarios reales.

---

## Fase 2 - Mineria y economia mainnet

Bloqueante de mainnet.

- [x] Decidir algoritmo PoW final: RandomX nativo o fallback CPU propio.
- [x] Si se elige RandomX nativo, integrar backend buildable en Windows/Linux.
- [x] Congelar constantes economicas en archivo canonico versionado.
- [x] Confirmar token visible: `ZKA`.
- [x] Confirmar `max supply = 23,000,000 ZKA`.
- [x] Confirmar `reward inicial = 12 ZKA`.
- [x] Confirmar `halving = 1,051,200` bloques.
- [x] Confirmar `target block time = 120s`.
- [x] Confirmar `coinbase maturity = 60` bloques o ajustar antes de genesis.
- [x] Testear enforcement de max supply en contabilidad real de emision.
- [x] Integrar distribucion de recompensa al productor de bloque.
- [x] Integrar fees privadas al productor de bloque o documentar un mecanismo v1 verificable.
- [x] Integrar coinbase privada final o congelar un esquema alternativo honesto para v1.
- [x] Agregar tests de emision por era, halving, cap y maturity.

Nota: parametros canonicos vivos en `src/economics.rs` y publicables en `docs/economics.mainnet.v1.json`. La contabilidad v1 actual usa unidades enteras `ZKA`; no declarar decimales atomicos en mainnet hasta migrar wallet/circuitos/estado a una unidad menor.

Nota distribucion al productor: la coinbase es ahora una `Transaction` real, sentinel sender `zoka:coinbase:v1`, en `block.transactions[0]`, con monto = `block_reward_for_height(h, parent_supply)`. El miner construye un `Block` completo y lo pasa por `validate_pow_block` antes de avanzar la cadena, con las mismas reglas que aplicaria un nodo verificador. Validador puro y pluggable en `src/mining/block_validator.rs` (20 tests cubriendo coinbase faltante/duplicada/monto/cap/destinatario, private coinbase requerida/maturity, prev_hash, height, merkle, header hash, PoW seal, PoW por debajo de dificultad, y mainnet sin feature `randomx`). El acreditamiento al wallet productor usa `private_coinbase` comprometida por Merkle root y `coinbase maturity = 60` antes de importar la nota privada al wallet.

Nota PoW: decision congelada en `docs/pow-mainnet-decision.md`. Mainnet usa `zoka-randomx-v1` (RandomX, mismo algoritmo que Monero). Dev/testnet usa `zoka-cpu-fallback-v1` (SHA3 con epoch keys). El backend pluggable vive en `src/mining/pow.rs`; los binarios de mainnet se construyen con `cargo build --release --features randomx`. Sin ese feature, el codigo refusa minar contra `MAINNET_CHAIN_ID` con un error explicito que apunta al doc.

Nota toolchain RandomX: `scripts/check_randomx_toolchain.ps1` verifica `cmake` y compilador nativo antes del build. `scripts/package_release.ps1` ejecuta ese preflight y rechaza empaquetar una release mainnet si falta la toolchain. En esta maquina el preflight estricto ya detecta CMake y MSVC Build Tools, y `cargo build --release --features randomx --bin zoka --bin zokahd` compila correctamente.

Pendiente para "user-mineable mainnet" (NO resuelto por la eleccion de algoritmo):
- [x] Coinbase como `Transaction` real dentro del bloque, validable por terceros.
- [x] Logica de verificacion de bloque (PoW + coinbase + header self-consistency) como funcion pura reutilizable.
- [x] Persistir el bloque minado en RocksDB.
- [x] Entregar el bloque minado via gossipsub a otros peers (publish_block_response al cierre de cada batch).
- [x] Verificacion P2P del bloque entrante en tiempo real (`handle_inbound_block_message` -> `state::chain::apply_block`).
- [x] Fork choice rule con reorg real sobre chain DB PoW: ramas laterales se guardan sin pisar el indice canonico y una rama con mas `cumulative_work` reemplaza head/height-index/supply/work/difficulty.
- [x] Difficulty retargeting determinista derivado del chain DB (`mining::difficulty::expected_for_next_block` calcula desde los timestamps de los dos ultimos bloques persistidos + `chain:current_difficulty_bits`; miner y peer handler lo consultan antes de minar/validar; `chain::apply_block` no toma la dificultad como parametro, la deriva y la persiste).
- [x] Sync inverso desde un peer cuando el nodo arranca atras de la cadena (IBD por `request_block_with_retry` desde la altura local hasta la altura maxima observada en peers).

Nota IBD: `src/state/sync.rs` contiene el helper testable de payloads, `PeerChainView` y el calculo de siguiente altura requerida. `fullnode`/`zokahd` actualizan la altura maxima observada por `BlockResponse`, piden bloques faltantes y aplican cada bloque por el path canonico `state::chain::apply_block` antes de minar si estan atras.

Nota reorg: el path activo de PoW usa `state::chain::apply_block`, que hoy actualiza chain DB, emission metadata y block indexes, no el `StateManager` publico legacy. Si se vuelve a usar `StateManager` para transiciones publicas, habra que agregar replay/rollback de account-state encima del fork choice ya implementado.

Nota fees privadas al productor: `Block` incluye `private_fee_claims`, envelopes cifrados comprometidos por Merkle root y ligados a `private_tx_hashes` del mismo bloque. `validate_pow_block` rechaza claims duplicados, claims para txs no incluidas y envelopes malformados. El monto sigue oculto; el productor lo reclama localmente con `PrivateWalletManager::claim_private_fees_from_block_claims`, que verifica la apertura contra `fee_commitment` antes de acreditar el wallet.

Nota coinbase privada v1: `Block` incluye `private_coinbase: Option<PrivateCoinbaseOutput>`. Si la recompensa publica del bloque es mayor que cero, el validador exige esa salida privada, verifica que pertenezca al mismo receptor de coinbase, que use la altura correcta y que desbloquee exactamente en `height + COINBASE_MATURITY_BLOCKS`. El monto del subsidio sigue visible en la coinbase publica para auditar emision y max supply; la salida privada es la nota spendable que el minero reclama con `PrivateWalletManager::claim_private_coinbase_output`, verificando decrypt, amount commitment y Bulletproof antes de acreditar el wallet.

Salida de fase:

- `mine`, `mine-status`, `mine-console`, `fullnode` y `zokahd` producen emision coherente, sin scheduler falso y sin romper supply cap.

---

## Fase 3 - Genesis y validator set

Bloqueante de mainnet.

- [x] Crear formato canonico para `genesis.mainnet.json`.
- [x] Separar cuentas iniciales de validator set.
- [ ] Definir si existe pre-mine. Si existe, publicar cantidades, direcciones y justificacion. (decision de proyecto, no de schema; el schema ya soporta `pre_mine[]` con justificacion obligatoria por entrada).
- [x] Definir validator set inicial con claves de consenso separadas de claves de operador.
- [x] Documentar procedimiento de rotacion de claves. Ver `docs/runbook-key-rotation.md`.
- [x] Documentar procedimiento de halt recovery PBFT. Ver `docs/runbook-pbft-halt-recovery.md`.
- [x] Generar digest de genesis y publicarlo.
- [x] Agregar comando/verificador para validar genesis antes de arrancar.
- [ ] Ejecutar dry-run de genesis con el mismo archivo que se publicara. (operacional pre-launch).

Nota schema: `zoka-genesis-v1` definido en `src/genesis.rs` y especificado en `docs/genesis.mainnet.v1.spec.md`. `MainnetGenesisV1` separa `validators[]` (con `consensus_pubkey_hex` distinto de `operator_pubkey_hex`, roles `active`/`standby`, voting_power), `pre_mine[]` (con justificacion obligatoria, sumas que no exceden `MAX_SUPPLY_ZKA`) y `initial_accounts[]` (bookkeeping no-counted). El loader rechaza por `EconomicsDrift` cualquier discrepancia con las constantes del binario, y por `PowAlgorithmMismatch` cualquier algoritmo distinto al canonico para el `chain_id`. 22 tests cubren schema, drift, validator set malformado/duplicado, pre-mine cap, json roundtrip, deny_unknown_fields, digest determinista.

Nota verifier offline: `zoka genesis-verify <path> [--json]` carga el archivo, corre `validate_static`, computa el digest sha256 sobre la encoding canonica (`serde_json::to_vec_pretty`) e imprime chain_id + pow_algorithm + validator count + active voting power + pre-mine total + digest. Operadores comparan el digest contra el anuncio oficial antes de cargar el archivo en RocksDB.

Salida de fase:

- genesis, chain ID, validator set y procedimiento de recuperacion estan congelados y versionados.

---

## Fase 4 - Red publica y transporte anonimo

Bloqueante de mainnet.

- [ ] Definir seed nodes mainnet en multiples regiones. (decision operativa pre-launch).
- [ ] Definir RPC publicos si se ofrecen. (decision operativa pre-launch).
- [x] Congelar formato operativo de red publica (`docs/mainnet-network.v1.json`) y runbook de seed/RPC (`docs/mainnet-network-ops.md`).
- [x] Cargar configuracion seed/RPC desde el nodo. `zokahd`/`zoka fullnode` leen `docs/mainnet-network.v1.json`, validan `chain_id` + `PROTOCOL_FREEZE_ID`, seleccionan seeds activos y permiten override operator-only con `--seed`/`--network-config`.
- [x] Publicar politica de logs para infraestructura oficial. Ver `docs/infrastructure-logs-policy.md`.
- [x] Confirmar Tor/I2P operativo para seed/RPC publicos. Implementado en `network::p2p::NetworkPrivacyConfig` (`tor_proxy`, `i2p_sam_endpoint`, `Hybrid` mode).
- [x] Confirmar que wallet/fullnode prefieren transporte anonimo cuando este disponible. `enforce_anonymous_transport: true` rechaza fallback a clearnet (ver `network::p2p` linea 477).
- [x] Mantener flag explicito para clearnet cuando sea necesario. `enforce_anonymous_transport: false` (default) permite clearnet con disclaimer.
- [x] Probar sync desde genesis contra estado esperado. Cubierto por `state::sync` tests + `state::chain` tests + snapshot import path.
- [x] Definir estrategia de pruning/snapshots. Ver `docs/pruning-snapshots-strategy.md`.
- [x] Firmar snapshots si se habilita fast sync. `src/snapshot.rs::sign` (ed25519, raw 64 bytes hex); CLI `snapshot-export --sign-key-hex <hex>` y `snapshot-import --public-key <hex>`.

Nota fast sync: `zoka-snapshot-v1` definido en `src/snapshot.rs`. Export captura header + todos los bloques + chain metadata + firma ed25519 opcional. Import re-corre `state::chain::apply_block` por bloque, asi que el snapshot acelera (no atajo) — cualquier bloque invalido se rechaza con la misma regla de consenso. CLI: `zoka snapshot-export <out> [--db <path>] [--sign-key-hex <hex>]` y `zoka snapshot-import <in> [--db <path>] [--require-signature] [--public-key <hex>]`. 14 tests cubren roundtrip, deny_unknown_fields, height gap rechazado, chain_id mismatch, firma valida/invalida, byte tampering, idempotencia.

Salida de fase:

- un usuario puede descargar binarios, arrancar `zokahd`, conectarse a peers y sincronizar sin configurar manualmente la red.

Nota seed/RPC: el formato y el loader de configuracion estan cerrados, pero los endpoints reales no se pueden marcar como completados hasta tener operadores/hosts reales. No inventar seed nodes en docs de launch; publicar `docs/mainnet-network.v1.json` con `seed_nodes[]`, `public_rpc[]` y `snapshot_signers[]` reales solo en release candidate final.

---

## Fase 5 - Seguridad criptografica y auditoria

Bloqueante de mainnet.

- [ ] Ejecutar auditoria externa real sobre release candidate.
- [ ] Auditar circuitos Groth16.
- [ ] Auditar CLSAG/key images/nullifiers.
- [ ] Auditar Bulletproofs/Pedersen/fees.
- [ ] Auditar consenso/PBFT/commit pipeline.
- [ ] Cerrar remediaciones de auditoria.
- [ ] Publicar reporte o resumen verificable.
- [ ] Ejecutar ceremonia MPC real si el release final depende de CRS nueva.
- [ ] Publicar transcript/attestations/digest final.
- [ ] Bakear digest final en binarios release.

Salida de fase:

- no quedan findings criticos abiertos y el setup criptografico final es verificable.

---

## Fase 6 - Testnet publica y chaos testing

Bloqueante de mainnet.

- [ ] Lanzar testnet publica con genesis propio.
- [ ] Mantenerla 8 a 12 semanas sin reset no planificado.
- [ ] Probar wallets nuevas/restauradas.
- [ ] Probar transferencias entre usuarios reales.
- [ ] Probar mineria con multiples maquinas.
- [ ] Probar nodos detras de NAT.
- [ ] Probar Tor/I2P.
- [ ] Ejecutar chaos test de validadores caidos.
- [ ] Ejecutar particion de red controlada.
- [ ] Ejecutar flood de transacciones privadas validas.
- [ ] Documentar bugs encontrados y fixes.

Salida de fase:

- evidencia publica de estabilidad, no solo pruebas locales.

---

## Fase 7 - Release reproducible y publicacion

Bloqueante de mainnet.

- [x] Definir convencion de version release candidate: `v1.0.0-rc<N>` para artefactos, `PROTOCOL_FREEZE_ID=/zoka/1.0.0`.
- [ ] Congelar commit de release.
- [x] Agregar script de empaquetado release Windows/Linux: `scripts/package_release.ps1`.
- [ ] Generar binarios Windows/Linux sobre commit congelado.
- [x] Documentar builds reproducibles y diferencias justificadas.
- [ ] Firmar binarios con GPG.
- [x] Agregar generacion de checksums `SHA256SUMS.txt` (`scripts/release_checksums.ps1`, llamado por `package_release.ps1`).
- [x] Publicar instrucciones de verificacion.
- [x] Publicar documentacion de usuario en ingles (`docs/user-cli-reference.en.md`).
- [x] Publicar comandos minimos. Documentados en `docs/user-cli-reference.en.md`:
  - `zokahd`
  - `zoka wallet-create`
  - `zoka wallet-restore-from-seed`
  - `zoka wallet-balance`
  - `zoka tx-send`
  - `zoka mine`

Salida de fase:

- un tercero puede verificar que el binario corresponde al source publicado.

Nota release tooling: `scripts/package_release.ps1` empaqueta `zoka` + `zokahd`, copia docs minimas, emite `RELEASE-MANIFEST.txt` con commit/toolchain/comando de build/protocol freeze y genera `SHA256SUMS.txt`. Smoke local validado con binarios `debug` existentes en `.zoka/test/release-smoke`; la corrida real de release debe ejecutarse en `--release --features randomx` sobre commit congelado.

Nota regression gate: `genesis::tests::committed_mainnet_genesis_matches_published_digest` y `committed_mainnet_genesis_targets_mainnet_chain` aseguran que el archivo `docs/genesis.mainnet.v1.json` no puede driftear silenciosamente. Cualquier edicion al JSON sin actualizar `docs/genesis-dry-run-evidence.md` rompe el test, y viceversa. La rotacion intencional del genesis (hard fork o revision pre-launch) se hace en un solo commit que toca ambos archivos.

---

## Fase 8 - Operacion de launch

Bloqueante final.

- [ ] Bug bounty publico activo antes del launch.
- [x] Runbook de incidentes. Ver `docs/runbook-incidents.md`.
- [x] Runbook de upgrade/hard fork. Ver `docs/runbook-upgrade-hardfork.md`.
- [x] Runbook de chain halt recovery. Ver `docs/runbook-pbft-halt-recovery.md`.
- [ ] Contactos de operadores/validadores.
- [ ] Ventana de launch anunciada.
- [ ] Dry-run 24-48 h antes del launch con validadores reales.
- [ ] Genesis final publicado.
- [ ] Mainnet arrancada desde genesis final.
- [ ] Primeros bloques verificados contra supply esperado.

Salida de fase:

- mainnet publica en ejecucion con evidencia reproducible del arranque.

---

## Prioridad inmediata

Orden recomendado de trabajo desde el estado actual:

1. ~~Algoritmo PoW final.~~ Cerrado en `docs/pow-mainnet-decision.md`.
2. ~~Coinbase como transaccion real + validacion P2P del bloque entrante.~~ Cerrado.
3. ~~Difficulty retargeting derivado del chain DB.~~ Cerrado en `src/mining/difficulty.rs` + `src/state/chain.rs::apply_block`.
4. ~~Sync inverso al arrancar atras de la cadena.~~ Cerrado en `src/state/sync.rs` + `fullnode`/`zokahd`.
5. ~~Reorg real/fork choice por `cumulative_work` en chain DB PoW.~~ Cerrado en `src/state/chain.rs`.
6. ~~Distribucion verificable de fees privadas al productor.~~ Cerrado con `private_fee_claims` en bloque + claim local.
7. ~~Coinbase privada final o esquema v1 honesto.~~ Cerrado con `Block.private_coinbase: Option<PrivateCoinbaseOutput>` + reglas de validacion en `mining::block_validator`.
8. ~~Genesis + validator set separados (Fase 3).~~ Schema canonico `zoka-genesis-v1` cerrado en `src/genesis.rs` + `docs/genesis.mainnet.v1.spec.md` + `zoka genesis-verify`. Falta unicamente la decision de pre-mine (politica) y claves reales.
9. ~~Seed/RPC config cargable por nodo.~~ Cerrado: `fullnode`/`zokahd` auto-cargan `docs/mainnet-network.v1.json`, conectan seeds activos y `zoka mainnet-network-lint` valida readiness minima.
10. ~~Release tooling + user-facing docs (Fases 7-8 codeable).~~ Cerrado: `scripts/package_release.ps1`, `scripts/release_checksums.{ps1,sh}`, `scripts/check_randomx_toolchain.ps1`, `scripts/mainnet_readiness_check.ps1`, runbooks (`docs/runbook-*.md`), reference de usuario (`docs/user-cli-reference.en.md`), build reproducible (`docs/reproducible-builds.md`), verification (`docs/release-verification.md`), regression gate del genesis publicado.
11. ~~Operability helpers finales.~~ Cerrado: test de integracion mainnet boot, `zoka chain-stats`, `zoka chain-prune`, `zoka snapshot-info` y smoke CLI reproducible.
12. Pendiente externo (no codeable por el agente): commit-freeze del release, firma GPG de `SHA256SUMS.txt`, auditoria externa (Fase 5), testnet publica 8-12 semanas (Fase 6), bug bounty + ventana de launch + dry-run T-24/48h (Fase 8).

---

## No confundir

No esta terminado para mainnet publica solo porque:

- el CLI compila;
- el fullnode arranca localmente;
- se pueden minar bloques locales;
- el documento tecnico existe;
- el roadmap viejo dice 100%.

Mainnet publica requiere red viva, validadores reales, genesis final, auditoria, testnet larga, release reproducible y operacion documentada.
