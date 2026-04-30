# ZOKA - Pendientes reales para mainnet publica

Roadmap canonico: [docs/mainnet_launch_roadmap_v6.md](docs/mainnet_launch_roadmap_v6.md)

Estado al 2026-04-28: la base tecnica principal de v6 esta cerrada y
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

1. Limpiar el working tree: separar codigo/doc fuente de artefactos locales
   (`target`, `.zoka`, DBs de prueba) y preparar commits auditables.
2. Limpiar/commitear el working tree y congelar tag si este RC local queda
   como base.
3. Para mainnet publica, reemplazar endpoints locales por operadores reales.
4. Pasar a auditoria externa + testnet publica.
