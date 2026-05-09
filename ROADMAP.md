# ZOKA Roadmap

Roadmap canonico de mainnet publica:
[docs/mainnet_launch_roadmap_v6.md](docs/mainnet_launch_roadmap_v6.md)

Este archivo es solo el resumen ejecutivo para no volver a mezclar la historia
v4/v5 con el estado real de launch. Para detalle tecnico construido, leer
[docs/project-state.md](docs/project-state.md). Para separar trabajo de agente
vs humano/sistema externo, leer
[docs/agent-vs-human-roadmap.md](docs/agent-vs-human-roadmap.md).

## Estado actual

ZOKA tiene cerrada la mayor parte del trabajo codeable de la v6:

- wallet privada completa con recovery deterministica desde seed;
- CLI de usuario `zoka` y wrapper `zokahd`;
- economia mainnet v1 congelada;
- RandomX elegido como PoW mainnet y gateado por feature `randomx`;
- validacion de bloque, coinbase publica auditable, private coinbase y private
  fee claims;
- chain DB con apply_block, fork choice/reorg por cumulative work, retarget de
  dificultad e IBD inverso;
- genesis schema/version/digest/verifier;
- configuracion mainnet network versionada y loader en fullnode;
- `mainnet-network-lint` para readiness de seeds/RPC/snapshot signers;
- snapshots export/import/info con replay;
- `chain-stats`, `chain-prune` y test de arranque mainnet contra genesis v1;
- scripts de release, checksums y docs de verificacion;
- runbooks operativos.

Verificacion local mas reciente:

- `cargo test --lib`: 671 passed, 1 ignored.
- `cargo test --tests`: OK; 2 soak tests largos ignored por diseno.
- `cargo check --bins`: OK.
- `zoka genesis-verify docs/genesis.mainnet.v1.json --json`: OK.
- Smoke CLI: ver [docs/cli-smoke-status.md](docs/cli-smoke-status.md).

## Fases v6

| Fase | Estado |
|---|---|
| Fase 1 - Wallet de usuario | Cerrada por codigo. |
| Fase 2 - Mineria y economia | Cerrada por codigo; build mainnet real con feature `randomx` verificado localmente. |
| Fase 3 - Genesis y validator set | Schema/verifier cerrados; claves reales, pre-mine final y dry-run dependen de humanos/operadores. |
| Fase 4 - Red publica y transporte | Loader/formato/lint cerrados; faltan endpoints reales de seed/RPC/snapshot signer. |
| Fase 5 - Seguridad criptografica y auditoria | Pendiente de auditoria externa real. |
| Fase 6 - Testnet publica y chaos | Pendiente de operadores/usuarios reales 8-12 semanas. |
| Fase 7 - Release reproducible | Tooling cerrado; faltan commit freeze, build `--features randomx`, checksums firmados y publicacion. |
| Fase 8 - Operacion de launch | Runbooks cerrados; faltan bug bounty, roster, ventana, dry-run y arranque real. |

## Bloqueantes reales de mainnet publica

- `docs/mainnet-network.v1.json` todavia contiene placeholders y estado
  `staged`; no hay seed/RPC reales activos.
- `docs/genesis.mainnet.v1.json` valida, pero contiene claves placeholder para
  validadores/operadores.
- No hay commit/tag congelado de release candidate.
- No hay firma GPG de release.
- Falta auditoria externa sobre RC.
- Falta testnet publica continua con chaos testing.
- Falta dry-run con operadores reales y publicacion de evidencia.

## Proximo tramo recomendado

1. Cerrar commits limpios de la base v6 y excluir artefactos locales del estado
   de release.
2. Reemplazar placeholders de red/genesis con operadores reales y pasar
   `zoka mainnet-network-lint`.
3. Cortar `v1.0.0-rc1`, firmar artefactos y pasar a auditoria/testnet.
4. Mantener helper pendiente de menor riesgo: `docs/troubleshooting.md`.
