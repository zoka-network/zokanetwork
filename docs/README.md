# ZOKA Documentation Index

Indice maestro de la documentacion. Cada entrada apunta al archivo y describe en
una linea para que sirve. Si vas a leer un solo doc, leer **`project-state.md`**
para tener el panorama y luego saltar al especifico.

## Punto de partida

| Doc | Para que |
|---|---|
| [`project-state.md`](project-state.md) | Snapshot de todo lo construido: modulos, tests, comandos CLI, arquitectura. |
| [`local-rc-complete-status.md`](local-rc-complete-status.md) | Estado del RC local completo: readiness, genesis digest, paquete, checksums y firma. |
| [`agent-vs-human-roadmap.md`](agent-vs-human-roadmap.md) | Que falta y quien puede hacerlo: agente (codigo) vs humanos/operadores. |
| [`mainnet_launch_roadmap_v6.md`](mainnet_launch_roadmap_v6.md) | Roadmap canonico de las 8 fases hasta mainnet publico. |

## Decisiones de diseno congeladas

| Doc | Decision |
|---|---|
| [`pow-mainnet-decision.md`](pow-mainnet-decision.md) | Mainnet usa **RandomX** (`zoka-randomx-v1`); fallback CPU es solo testnet/CI. |
| [`emission_schedule_v1.md`](emission_schedule_v1.md) | Tabla de emision con halving cada 1,051,200 bloques, max 23M ZKA. |
| [`private_fees.md`](private_fees.md) | Mecanismo de fees privadas con encrypted envelopes en bloque. |
| [`multi_asset.md`](multi_asset.md) | Politica de assets (un solo asset `ZOKA` en mainnet v1). |
| [`mixer_pool.md`](mixer_pool.md) | Diseno del private mixer pool. |
| [`clsag_pure.md`](clsag_pure.md) | Implementacion CLSAG sin C bindings. |
| [`membership_8_inputs.md`](membership_8_inputs.md) | Circuito Groth16 de membresia con 8 inputs. |
| [`remove_public_tx_path.md`](remove_public_tx_path.md) | Justificacion de quitar el path de tx publica obsoleto. |

## Especificaciones canonicas mainnet v1

| Doc / Archivo | Contenido |
|---|---|
| [`genesis.mainnet.v1.spec.md`](genesis.mainnet.v1.spec.md) | Schema de `zoka-genesis-v1` (validators, pre-mine, initial accounts, digest). |
| [`genesis.mainnet.v1.json`](genesis.mainnet.v1.json) | Genesis canonico publicado (digest verificado contra `genesis-dry-run-evidence.md`). |
| [`mainnet-network.v1.json`](mainnet-network.v1.json) | Configuracion de seeds/RPC/snapshot signers. |
| [`mainnet-launch-material.template.json`](mainnet-launch-material.template.json) | Template para reunir claves reales, endpoints reales y fingerprint GPG antes de aplicar el launch material. |
| [`economics.mainnet.v2-atoms.json`](economics.mainnet.v2-atoms.json) | Constantes economicas actuales: unidad atomica, 6 decimales y fee market fraccionario. |
| [`economics.mainnet.v1.json`](economics.mainnet.v1.json) | Artifact historico whole-ZKA retenido para trazabilidad pre-migracion. |
| [`pruning-snapshots-strategy.md`](pruning-snapshots-strategy.md) | Schema `zoka-snapshot-v1` y politica de fast sync. |
| [`infrastructure-logs-policy.md`](infrastructure-logs-policy.md) | Que se loguea y que no en infraestructura oficial. |
| [`tor_i2p_transport.md`](tor_i2p_transport.md) | Politica de transporte anonimo (`enforce_anonymous_transport`). |

## Operaciones y release

| Doc | Para que |
|---|---|
| [`reproducible-builds.md`](reproducible-builds.md) | Como reproducir un binario byte-a-byte. Toolchain pin, comando canonico. |
| [`release-verification.md`](release-verification.md) | Procedimiento de verificacion para usuarios (sha256 + GPG + genesis-verify). |
| [`troubleshooting.md`](troubleshooting.md) | Diagnostico de errores frecuentes: RandomX, placeholders, GPG, wallet, snapshots y peers. |
| [`user-cli-reference.en.md`](user-cli-reference.en.md) | Referencia oficial de los 6+ comandos minimos de mainnet. |
| [`user-cli-reference.md`](user-cli-reference.md) | Version en castellano del CLI reference. |
| [`mainnet-network-ops.md`](mainnet-network-ops.md) | Runbook de operacion de seeds/RPC. |
| [`mainnet-operators-roster.md`](mainnet-operators-roster.md) | Roster de operadores y on-call. |
| [`mainnet-bug-bounty-prelaunch.md`](mainnet-bug-bounty-prelaunch.md) | Programa de bug bounty pre-launch. |

## Runbooks de operacion (Fase 8)

| Doc | Cuando |
|---|---|
| [`runbook-incidents.md`](runbook-incidents.md) | Incidente generico en mainnet. |
| [`runbook-key-rotation.md`](runbook-key-rotation.md) | Rotacion de consensus / operator keys de un validador. |
| [`runbook-pbft-halt-recovery.md`](runbook-pbft-halt-recovery.md) | Halt de la cadena (perdida de quorum PBFT). |
| [`runbook-upgrade-hardfork.md`](runbook-upgrade-hardfork.md) | Upgrade coordinado / hard fork. |

## Auditoria y testnet

| Doc | Para que |
|---|---|
| [`audit_prep.md`](audit_prep.md) | Preparacion de paquete para auditoria externa. |
| [`mainnet-audit-closeout.md`](mainnet-audit-closeout.md) | Closeout de auditoria con findings y remediaciones. |
| [`mainnet-testnet-chaos-evidence.md`](mainnet-testnet-chaos-evidence.md) | Evidencias de chaos testing en testnet publica. |
| [`fuzzing_coverage.md`](fuzzing_coverage.md) | Estado de coverage del fuzzing. |
| [`genesis-dry-run-evidence.md`](genesis-dry-run-evidence.md) | Evidencia del dry-run del genesis. Incluye digest oficial. |

## Ceremony MPC (Groth16 trusted setup)

| Doc | Para que |
|---|---|
| [`ceremony_mpc.md`](ceremony_mpc.md) | Diseno general de la ceremonia MPC. |
| [`ceremony_runbook.md`](ceremony_runbook.md) | Runbook por contributor. |
| [`ceremony_contributor_kit.md`](ceremony_contributor_kit.md) | Kit de un contributor. |
| [`ceremony_execution_board.md`](ceremony_execution_board.md) | Board de ejecucion. |
| [`ceremony_closeout.md`](ceremony_closeout.md) | Closeout y publicacion final. |

## Launch readiness

| Doc | Para que |
|---|---|
| [`mainnet-launch-readiness.md`](mainnet-launch-readiness.md) | Gates de launch: ops, network, genesis, ejecucion. |
| [`mainnet-execution-board.md`](mainnet-execution-board.md) | Workstreams en flight para el launch. |
| [`mainnet-release-cutover.md`](mainnet-release-cutover.md) | Procedimiento del cutover release. |

## Roadmaps historicos / superseded

| Doc | Estado |
|---|---|
| [`mainnet_operational_roadmap_v5.md`](mainnet_operational_roadmap_v5.md) | Reemplazado por v6. Conservado para referencia historica. |
| [`cli_launch_mvp.md`](cli_launch_mvp.md) | Plan original del CLI MVP, ahora cerrado. |

## Como navegar este repo

- **"Quiero entender que esta construido"** → `project-state.md`.
- **"Quiero saber que falta"** → `agent-vs-human-roadmap.md`.
- **"Quiero correr un nodo mainnet"** → `release-verification.md` + `user-cli-reference.en.md`.
- **"Quiero auditar un genesis publicado"** → `zoka genesis-verify <path>` + `genesis.mainnet.v1.spec.md`.
- **"Quiero verificar un binario"** → `reproducible-builds.md` + `release-verification.md`.
- **"Algo se rompio en mainnet"** → `runbook-incidents.md` y de ahi al runbook especifico.
