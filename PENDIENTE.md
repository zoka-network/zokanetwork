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
- [ ] Minar bloques desde un nodo y confirmar propagacion/aplicacion en los
  demas.
- [ ] Arrancar un nodo atrasado desde DB vacia y verificar sync/IBD hasta la
  altura actual.
- [ ] Probar restart de nodos: apagar/arrancar, conservar DB, reconectar y
  continuar desde chain head correcto.
- [ ] Probar wallet real sobre la red local: crear, respaldar, restaurar,
  consultar balance, minar/recibir coinbase y respetar maturity.
- [ ] Probar transferencia privada entre wallets locales cuando haya fondos
  spendables.
- [ ] Documentar evidencia: comandos, logs minimos, alturas, hashes de head y
  resultado por nodo.

## Nuevo bloqueante pre-mainnet: auditoria total de CLI

- [ ] Ejecutar una matriz exhaustiva de todos los comandos `zoka` y `zokahd`,
  incluyendo casos OK, errores esperados, `--json`, passwords incorrectos,
  wallet inexistente, DB vacia, rutas invalidas y red sin peers.
- [ ] Convertir esa matriz en script reproducible para no depender de pruebas
  manuales.
- [ ] Revisar textos user-facing: mensajes claros, consistentes, sin ruido
  tecnico innecesario y con acciones sugeridas.
- [ ] Mejorar la presentacion visual de la consola: help, tablas, estados,
  errores y dashboard de nodo/mineria, manteniendo salida `--json` estable.

## Nuevo objetivo mayor: agente AI privado para consola

Objetivo: integrar un agente AI opcional para que el usuario pueda chatear por
consola y pedir acciones en lenguaje natural, por ejemplo crear wallet,
consultar balance, minar, explicar estado del nodo o preparar una transaccion.

Condiciones obligatorias:

- [ ] El agente nunca debe leer, imprimir, enviar ni almacenar seed phrases,
  spend keys, passphrases o material privado sensible.
- [ ] Toda accion destructiva o irreversible requiere confirmacion explicita del
  usuario.
- [ ] El agente debe mapear lenguaje natural a comandos internos auditables, no
  saltarse las validaciones del CLI.
- [ ] Modo offline/local primero; cualquier proveedor externo de AI debe ser
  opcional y estar bloqueado por defecto para proteger privacidad.
- [ ] El agente debe explicar que accion va a ejecutar antes de ejecutarla.
- [ ] Las rutas de wallet, transaccion, mineria y nodo deben tener tests de
  seguridad contra prompt injection y filtracion de secretos.

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
4. Disenar e implementar agente AI privado como feature opcional.
5. Para mainnet publica, reemplazar endpoints locales por operadores reales.
6. Pasar a auditoria externa + testnet publica.
