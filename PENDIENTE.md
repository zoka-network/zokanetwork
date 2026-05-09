# ZOKA - Pendientes reales para mainnet publica

Roadmap canonico: [docs/mainnet_launch_roadmap_v6.md](docs/mainnet_launch_roadmap_v6.md)

Estado al 2026-05-01: la base tecnica principal de v6 esta cerrada y
verificada localmente, pero ZOKA todavia no esta lista para mainnet publica.
Lo que falta ya no es "terminar wallet/PoW/genesis basico", sino cerrar
operacion real, auditoria, testnet publica y release firmado.

## Listo por codigo

- [x] Fase 1 / wallet de usuario: `wallet-create`,
  `wallet-restore-from-seed`, `wallet-restore-from-keys`,
  `wallet-change-passphrase`, backup, wipe, balance, history y export de
  secretos.
- [x] Fase 1 / recovery: wallets nuevas derivan `scan_key` y `spend_key`
  deterministicamente desde la recovery phrase.
- [x] Fase 1 / hardening: passphrase obligatoria para wallets nuevas,
  restauradas y autocreacion de fullnode.
- [x] Fase 2 / economia: constantes mainnet v1 congeladas en
  `src/economics.rs` y `docs/economics.mainnet.v1.json`.
- [x] Fase 2 / PoW: decision RandomX congelada en
  `docs/pow-mainnet-decision.md`; el binario sin feature `randomx` rechaza
  minar mainnet con error explicito.
- [x] Fase 2 / bloques: coinbase publica auditable, `private_coinbase`,
  private fee claims, validacion pura de bloque, retarget de dificultad,
  persistencia, fork choice/reorg por `cumulative_work` e IBD inverso.
- [x] Fase 3 / genesis: schema `zoka-genesis-v1`, verifier offline
  `zoka genesis-verify`, digest determinista y regression gate.
- [x] Fase 4 / red: formato `docs/mainnet-network.v1.json`, loader en
  `zoka fullnode`/`zokahd`, lint de readiness, runbook seed/RPC y politica
  de transporte.
- [x] Fase 4 / snapshots: `snapshot-export`/`snapshot-import` con replay de
  bloques, `snapshot-info` offline y firma opcional.
- [x] Operabilidad local: `zoka chain-stats` y test de integracion `mainnet
  boot` contra `docs/genesis.mainnet.v1.json`.
- [x] Fase 7 / release tooling: `scripts/package_release.ps1`,
  checksums, docs de build reproducible, verificacion y referencia CLI en
  ingles.
- [x] Fase 8 / runbooks: incidentes, key rotation, halt recovery y hard fork.

## Verificado localmente

- [x] `cargo test --lib`: 671 passed, 1 ignored.
- [x] `cargo test --tests`: integracion, network, PBFT transport, multiprocess
  smoke y Tor probe pasaron; 2 soak tests largos quedaron ignored por diseno.
- [x] `cargo check --bins`: OK.
- [x] `zoka genesis-verify docs/genesis.mainnet.v1.json --json`: OK,
  digest `f0d9e1261a3dec08d754f63e1bcfd2663f6cedfd476657125bfaebe7aefd5745`.
- [x] Smoke CLI sobre binarios `target/debug`: ver
  [docs/cli-smoke-status.md](docs/cli-smoke-status.md).

## Pendiente de codigo util, no bloqueante directo de launch

Estos items mejoran operabilidad, pero no cambian el hecho de que el bloqueo
principal ya es externo/operativo.

- [x] Test de integracion "mainnet boot" que arranca `zoka fullnode --genesis
  docs/genesis.mainnet.v1.json` contra DB temporal y verifica height 0.
- [x] `zoka chain-stats` read-only: altura, supply, work, dificultad y reward.
- [x] `zoka snapshot-info <path>` para inspeccion offline.
- [x] `zoka mainnet-network-lint` para rechazar configs de red con
  placeholders o sin quorum minimo.
- [x] Pruner CLI `zoka chain-prune --keep-last N`.
- [x] `docs/troubleshooting.md` con errores comunes de usuario.
- [x] `zoka release-keygen` para generar keypairs ed25519 de launch sin
  imprimir secretos por consola.
- [x] Template/aplicador de material real:
  `docs/mainnet-launch-material.template.json` y
  `scripts/apply_mainnet_launch_material.ps1`.
- [x] RC local completo: genesis sin placeholders, network lint OK, package,
  checksum y firma GPG local. Ver
  `docs/local-rc-complete-status.md`.

## Nuevo bloqueante pre-mainnet: rehearsal local intensivo

Antes de decir "mainnet para todos los usuarios", ZOKA debe pasar un ensayo
local controlado que simule operadores y usuarios reales sin depender todavia de
infraestructura publica.

- [x] Levantar 2-4 nodos locales independientes con directorios `.zoka`
  separados, puertos distintos y genesis/config mainnet RC.
- [x] Verificar discovery/conexion peer-to-peer entre nodos locales.
- [x] Minar un bloque desde un nodo y confirmar propagacion/aplicacion en peers
  locales. Evidencia 2 nodos:
  `.zoka\rehearsal\local-multinode-20260501-115813`.
- [x] Endurecer propagacion 3+ nodos. Se agrego reconnect automatico al seed y
  `--min-mine-peers` para que el minero espere peers suficientes antes de
  minar. Evidencia 3 nodos:
  `.zoka\rehearsal\local-multinode-20260501-123604`; evidencia 4 nodos:
  `.zoka\rehearsal\local-multinode-20260501-123747`.
- [x] Arrancar un nodo atrasado desde DB vacia y verificar sync/IBD hasta la
  altura observada del peer. Evidencia:
  `.zoka\rehearsal\late-sync-20260501-120928`.
- [x] Probar restart de nodos: apagar/arrancar, conservar DB, reconectar y
  continuar desde chain head correcto. Evidencia:
  `.zoka\rehearsal\restart-20260501-121200`.
- [x] Probar sincronizacion natural escalonada y competencia posterior: nodo 1
  mina solo hasta altura 5, nodo 2 entra y sincroniza hasta altura 3, luego
  entra nodo 3, todos alcanzan altura 5, compiten hasta al menos altura 8 y
  finalmente convergen al mismo head canonico por trabajo acumulado. Se aplico
  el patron a `fullnode`/`zokahd` con `--seed` repetible, fallback multi-seed,
  rebroadcast de bloques recientes, cadencia IBD no bloqueante, requests por
  hash esperado y promocion de ramas laterales conocidas. Evidencia:
  `.zoka\rehearsal\staged-sync-20260501-223605`.
- [x] Cerrar churn cotidiano de nodos antes de avanzar de fase: nodos que
  entran tarde, salen, vuelven con DB vieja, un nodo fresco entra desde cero,
  cae el seed original y luego vuelve. Evidencia:
  `.zoka\rehearsal\churn-20260502-184407\summary.json`. Resultado: node-1,
  node-2, node-3 y node-4 cerraron en height 5 con el mismo head
  `88c9ddf7eabec43883514009ca0737d90cdc5df36c2709011e8eb3601c6867ab`.
- [x] Probar wallet real sobre la red local: crear wallets aisladas, minar con
  wallet real del nodo, respetar coinbase maturity y sincronizar un verificador
  contra la cadena minada. Evidencia:
  `.zoka\rehearsal\wallet-network-20260502-211129`.
- [x] Probar transferencia privada entre wallets locales cuando hay fondos
  spendables y comprometer el hash en un bloque propagado. Evidencia:
  `.zoka\rehearsal\wallet-network-20260502-211129\manual-summary.json`;
  tx `a3f2c833aa851b8c730cb3c78e7d4e3f` quedo en block 65, ambos nodos
  terminaron en height 65/head
  `f2072ed2982854c22b49afb251177642afe68d66b1f215eb046debb2e1f24070`.
- [x] Documentar evidencia: comandos, logs minimos, alturas, hashes de head y
  resultado por nodo. El script reproducible base es
  `scripts/local_wallet_network_rehearsal.ps1`; usa dificultad local insegura y
  seeded setup dev-only explicitamente para no contaminar parametros mainnet.

## Nuevo bloqueante pre-mainnet: auditoria total de CLI

- [ ] Ejecutar una matriz exhaustiva de todos los comandos `zoka` y `zokahd`,
  incluyendo casos OK, errores esperados, `--json`, passwords incorrectos,
  wallet inexistente, DB vacia, rutas invalidas y red sin peers.
- [x] Convertir esa matriz en script reproducible para no depender de pruebas
  manuales: `scripts/cli_smoke_test.ps1` ahora cubre comandos largos y alias UX
  cortos.
- [x] Primera simplificacion UX: alias cortos (`create`, `wallets`,
  `balance`, `send`, `node`, `status`) y help principal orientado a usuarios.
- [x] Build operativo por defecto limitado a `zoka`/`zokahd`; herramientas de
  ceremonia y worker multiproceso quedan opt-in para operadores/tests.
- [ ] Completar revision de textos user-facing: mensajes claros, consistentes,
  sin ruido tecnico innecesario y con acciones sugeridas.
- [ ] Completar presentacion visual de tablas/errores; dashboards de
  nodo/mineria ya existen y deben mantener salida `--json` estable.

## Decision de alcance: sin asistente conversacional en release actual

Objetivo actualizado: simplificar la superficie publica y dejar solo el CLI
`zoka` y el daemon/wrapper `zokahd`. La capa conversacional queda fuera del
binario, fuera del smoke y fuera de la referencia de usuario.

- [x] Remover los comandos conversacionales del dispatcher.
- [x] Remover el menu interactivo del agente; `zoka` sin argumentos queda como
  consola guiada de Wallets, Transfers, Mining, Node, Status y Help.
- [x] Remover tests y pasos del asistente en el smoke CLI.
- [x] Mantener comandos reales de red/wallet/RPC: `rpc-list`, `rpc-status`,
  `rpc-height`, `rpc-mempool`, `create`, `restore`, `wallets`, `balance`,
  `send`, `mine`, `node`, `status`, `sync`, `backup`, `wipe` y `zokahd`.
- [ ] Reforzar ayuda y docs del CLI puro para que las tareas frecuentes no
  dependan de lenguaje natural.

## Pendiente humano / operativo

- [x] Copiar `docs/mainnet-launch-material.template.json` fuera del repo,
  llenarlo con claves/endpoints/fingerprint reales y aplicarlo con
  `scripts/apply_mainnet_launch_material.ps1 -InPlace` para RC local.
- [x] Generar keypairs ed25519 locales de launch en
  `C:\Users\mderramus\zoka-secrets` con
  `scripts/prepare_mainnet_launch_keys.ps1`.
- [x] Reemplazar placeholders de `docs/mainnet-network.v1.json` por endpoints
  locales/no-placeholder para RC tecnico.
- [x] Reemplazar claves placeholder de `docs/genesis.mainnet.v1.json` por claves
  ed25519 locales de validadores/operadores.
- [x] Ejecutar `scripts/mainnet_readiness_check.ps1` hasta obtener
  `Failures: 0`.
- [ ] Para mainnet publica: reemplazar endpoints locales por seed/RPC/onion/I2P
  de operadores reales.
- [ ] Decidir formalmente pre-mine: ninguno, o cantidades/direcciones/
  justificacion. El archivo actual tiene `pre_mine: []`.
- [ ] Completar roster real de operadores y on-call.
- [ ] Ejecutar dry-run genesis T-24/48h con validadores reales.
- [ ] Activar bug bounty publico.
- [ ] Anunciar ventana de launch.

## Pendiente externo / release

- [x] Instalar toolchain de release con `cmake` + MSVC Build Tools en Windows
  o toolchain equivalente Linux.
- [x] Construir binarios mainnet con:
  `cargo build --release --features randomx --bin zoka --bin zokahd`.
- [ ] Congelar commit/tag de release candidate.
- [x] Ejecutar `scripts/package_release.ps1` para RC local completo.
- [x] Firmar `SHA256SUMS.txt` con GPG local y publicar fingerprint en
  `docs/release-verification.md`.
- [ ] Ejecutar auditoria externa real sobre release candidate y cerrar findings.
- [ ] Ejecutar testnet publica 8-12 semanas con operadores, miners y usuarios
  reales, incluyendo chaos tests.
- [ ] Publicar artefactos finales y arrancar mainnet desde genesis final.

## Siguiente orden recomendado

1. Ejecutar rehearsal local multi-nodo con evidencia.
2. Ejecutar auditoria intensiva de todos los comandos `zoka`/`zokahd`.
3. Mejorar UX visual de consola sin romper `--json`.
4. Para mainnet publica, reemplazar endpoints locales por operadores reales.
5. Pasar a auditoria externa + testnet publica.
