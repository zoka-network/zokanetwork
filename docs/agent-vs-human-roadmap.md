# ZOKA Roadmap: Agent vs Human

Hoja de ruta explicita de **que falta** para mainnet publico, separando lo que
un agente de codigo (yo) puede entregar vs lo que requiere intervencion humana
o sistemas externos.

Para el roadmap canonico de fases ver [`mainnet_launch_roadmap_v6.md`](mainnet_launch_roadmap_v6.md).
Para un snapshot de lo construido ver [`project-state.md`](project-state.md).

---

## Convenciones

- ðŸ¤– **Agente** = yo (Claude). Lo puedo construir directamente: codigo,
  tests, docs canonicos, scripts deterministicos.
- ðŸ‘¤ **Humano** = vos o el equipo core. Requiere decision politica, acceso a
  hardware/credenciales, o ejecucion en tiempo real.
- âš™ï¸ **Sistema externo** = auditores, validadores, infraestructura de red,
  GPG keyholders, operadores de testnet.

Cada item lleva un prefijo. Si dice ðŸ¤– y todavia esta abierto, decime y lo
hago. Si dice ðŸ‘¤ o âš™ï¸, depende de actores fuera del repo.

---

## âœ… Cerrado (todas las fases)

Lo que ya se construyo. Detalle en [`project-state.md`](project-state.md). En
una linea:

- ðŸ¤– **Cleanup public-by-default**: borrado de CLI legacy con flujos publicos
  (`user-cli`, `user-cli-private`, `user-node`), modulo `user_node` huerfano,
  modulo `transaction_pool` (mempool publica huerfana), `WalletManager`
  publico. Lo que quedo publico es publico-por-consenso (coinbase para
  audit de supply, fee claims comprometidas Pedersen, header de bloque
  para PoW verification). Ver "Que es publico y que es privado" en
  `project-state.md`.

- ðŸ¤– Wallet privada completa (BIP39, encryption, restore-from-seed/keys).
- ðŸ¤– Token + economia (ZKA, 23M cap, halving, maturity, validate constants).
- ðŸ¤– PoW pluggable (RandomX + CPU fallback) con decision congelada en
  `pow-mainnet-decision.md`.
- ðŸ¤– Block validator (coinbase, private coinbase, private fee claims, PoW,
  chain link, header self-consistency).
- ðŸ¤– Chain DB con apply_block + fork choice por cumulative_work + reorg.
- ðŸ¤– Difficulty retarget header-driven.
- ðŸ¤– IBD + sync inverso desde peers.
- ðŸ¤– Genesis schema canonico + verifier offline + regression gate.
- ðŸ¤– Snapshots con firma ed25519 + import con replay completo.
- ðŸ¤– Network privacy config (Tor/I2P/clearnet flag, mainnet-network.v1.json).
- 🤖 Mainnet network lint (`zoka mainnet-network-lint`).
- 🤖 Mainnet boot integration test contra `docs/genesis.mainnet.v1.json`.
- 🤖 Operability helpers (`zoka chain-stats`, `zoka snapshot-info`).
- ðŸ¤– Release tooling (package_release.ps1, checksums, readiness check).
- ðŸ¤– Runbooks (incidents, key-rotation, halt-recovery, hard-fork).
- ðŸ¤– User-facing CLI reference en ingles.
- ðŸ¤– Reproducible build doc + verification doc.

Total: 600+ tests pasando.

---

## ðŸ¤– Pendiente â€” Agente puede ejecutar

Items que solo dependen de codigo / docs y no requieren actores externos.

### Pequenas mejoras (low-effort)

- ðŸ¤– **Property tests con `proptest`**: ya esta en `dev-dependencies`. Util
  para `validate_pow_block` (random blocks should not panic, only reject) y
  para `genesis::validate_static`.
- ðŸ¤– **Mensajes de error verbose en `wallet-restore-from-seed` cuando la
  frase tiene typos**: detectar palabra-no-en-BIP39 vs longitud mal vs
  checksum incorrecto.
- ðŸ¤– **`docs/troubleshooting.md`**: catalogo de errores user-facing comunes
  y como diagnosticarlos. Ya tenemos los textos en codigo, falta indexarlos.

### Medianas (medium-effort)

- ~~Pruner CLI `zoka chain-prune --keep-last N`~~: ✅ **Cerrado.**
  `state::chain::prune_below_head` con `PRUNE_MIN_RETAINED_BLOCKS=8` como
  piso (no se puede romper retarget de dificultad). CLI
  `zoka chain-prune --keep-last <N> [--db <path>] [--dry-run] [--json]`.
  8 tests cubren empty chain, floor clamp, retain head metadata, retarget
  post-prune, apply post-prune, dry-run no-modify, multi-keys cleanup
  (block body + height index + meta).
- ðŸ¤– **Snapshot diferencial v3**: delta sobre un snapshot base, util para
  releases menores. Spec ya esta en
  `pruning-snapshots-strategy.md` como roadmap del modulo. Solo codigo.
- ðŸ¤– **Mempool publica para fees pre-coinbase-claim**: hoy las fees
  privadas se acreditan al miner via `private_fee_claims` post-validacion.
  Faltaria una mempool publica para que los TXs lleguen al miner antes de
  formar bloque. La estructura existe en `transaction_pool/`, falta
  conectarla al fullnode loop.
- ðŸ¤– **Health endpoints expandidos**: `/health/chain` con altura + hash +
  fork-choice status. `src/health/` existe; falta extender.

### Grandes (refactor / arquitectura)

- ðŸ¤– **PBFT encadenado a PoW**: hoy el modulo `consensus::pbft` existe pero
  no esta en el path de `apply_block`. Para mainnet finalidad efectiva
  necesita: validators firman PRE-COMMIT/COMMIT por hash de bloque,
  `apply_block` requiere quorum 2/3 antes de canonicalizar, fork-choice
  respeta finalizados sobre cumulative_work. Es un proyecto en si.
- ðŸ¤– **Account state replay durante reorgs**: `apply_block` actualiza chain
  DB pero **no rolls back account state** en re-orgs (nota explicita en
  `chain.rs`). Para que reorgs sean seguros sobre balances, falta integrar
  con `StateManager`. Decision: hacerlo si/cuando se vuelva a usar el
  ledger publico vs solo el privado.
- ðŸ¤– **Migracion explicita de DBs legacy whole-ZKA**: la contabilidad atomica
  ya esta en codigo como `zoka-economics-v2-atoms` y los DBs nuevos quedan
  marcados. Los DBs locales antiguos sin marker se bloquean para no mezclar
  whole-ZKA con atoms. Si se quiere conservar historial viejo, falta una
  herramienta de migracion explicita; para launch local limpio, usar genesis
  nuevo.

---

## ðŸ‘¤ Pendiente â€” Humano debe ejecutar

Items que requieren decisiones politicas o credenciales fuera del repo.

### Pre-launch decisiones

- ðŸ‘¤ **Pre-mine: existe o no? cuanto, a quien, justificacion?** El schema de
  genesis soporta `pre_mine[]` con justificacion obligatoria. La decision
  es politica del proyecto, no de codigo.
- ðŸ‘¤ **Lista de validadores iniciales**: 3 publicados como placeholder en
  `genesis.mainnet.v1.json`, pero las claves consensus + operator reales
  las generan los validadores en sus propios HSMs.
- ðŸ‘¤ **Roster de operadores y on-call**: `mainnet-operators-roster.md`
  existe como template. Falta llenarlo con personas y contactos reales.
- ðŸ‘¤ **Seed nodes reales**: `mainnet-network.v1.json` tiene placeholders
  (`12D3KooWQo11111...`). Falta correr 3+ seed nodes reales en regiones
  distintas y publicar sus peer ids reales.
- ðŸ‘¤ **Public RPC reales o decision de no publicar**: idem. El schema lo
  soporta opcional.
- ðŸ‘¤ **GPG release signing key**: generar la key, publicar el fingerprint
  en `release-verification.md`, mantenerla en cold storage.

### Build + release fisico

- ðŸ‘¤ **Toolchain de release con MSVC + cmake**: esta maquina actual no
  tiene MSVC build tools instalado, asi que no puedo correr
  `cargo build --release --features randomx` aqui. Hay que ejecutarlo en
  una maquina con la toolchain. `scripts/check_randomx_toolchain.ps1`
  preflight ya existe.
- ðŸ‘¤ **Commit-freeze del release**: tag `v1.0.0-rc1`, `v1.0.0` etc. Decision
  + `git tag -s`.
- ðŸ‘¤ **Firmado GPG de `SHA256SUMS.txt`**: requiere GPG key + acto manual.
  `release-verification.md` documenta el flujo.
- ðŸ‘¤ **Subida de artefactos a GitHub Releases**: requiere acceso al repo
  oficial.

### Launch operacional

- ðŸ‘¤ **Bug bounty publico activado**: `mainnet-bug-bounty-prelaunch.md`
  describe el programa. Falta lanzarlo en plataforma (HackerOne /
  Immunefi / programa propio).
- ðŸ‘¤ **Ventana de launch anunciada publicamente**: comunicacion con
  comunidad/stakeholders.
- ðŸ‘¤ **Dry-run T-24/48h con validadores reales**: requiere validadores
  reales ejecutando contra el genesis publicado.
- ðŸ‘¤ **Mainnet arrancada desde genesis final**: el momento real del launch.

---

## âš™ï¸ Pendiente â€” Sistema externo

Items que dependen de actores que no son parte del equipo core.

### Auditoria (Fase 5)

- âš™ï¸ **Auditoria criptografica externa real**: contratar firma reputable
  (Trail of Bits, Quarkslab, Cure53, NCC Group). Cubrir Groth16, CLSAG,
  Bulletproofs, PoW, consenso. Plazo tipico: 6-12 semanas. Costo: $50k-
  $300k segun alcance.
- âš™ï¸ **Cierre de findings**: el equipo core arregla, los auditores
  reverifican, publica reporte (`mainnet-audit-closeout.md` template
  existe).
- âš™ï¸ **Ceremonia MPC con contributors externos** (si el setup Groth16
  cambia entre la ultima ceremonia y release): scripts existen
  (`scripts/init_ceremony_round*.ps1` etc.), falta correrla con
  contribuidores publicos.

### Testnet publica (Fase 6)

- âš™ï¸ **Testnet publica corriendo 8-12 semanas**: requiere validadores +
  miners + usuarios reales generando trafico durante el periodo. No es
  algo que el agente pueda simular.
- âš™ï¸ **Chaos testing en testnet**: validators caidos, particion de red,
  flood de tx privadas. Requiere coordinacion entre operadores reales.

### Distribucion y ecosistema

- âš™ï¸ **Wallets de terceros / integraciones**: requiere proyectos
  independientes adoptando el binario o el RPC.
- âš™ï¸ **Exchanges / on-ramps**: regulatorio + integracion individual con
  cada operador.
- âš™ï¸ **Block explorers publicos**: alguien tiene que ofrecerlos como
  servicio con SLA.

---

## Como pedir avance

Si querÃ©s que el agente avance, decime cual de los items ðŸ¤– abrir. Mi
recomendacion en orden de impacto:

1. ~~Pruner CLI~~ ✅ Cerrado en `state::chain::prune_below_head`.
2. **Property tests** (low-effort, robustez contra inputs adversariales).
3. **`docs/troubleshooting.md`** (low-effort, reduce friccion de usuarios).
4. **Mempool publica wireada al fullnode** (medium-effort, operability).
5. **Health endpoints expandidos** (medium-effort, monitoreo de operadores).
PBFT encadenado a PoW y account state replay son grandes y dependen de
decisiones de arquitectura (ej. mantener PBFT como capa optional vs
required, mantener account state publica vs eliminarla del mainnet v1).
Mejor discutirlos antes de codear.

Si lo abierto es ðŸ‘¤ o âš™ï¸, no te puedo ayudar directamente â€” pero si puedo
preparar checklists / scripts / docs que faciliten la ejecucion.
