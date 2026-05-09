# ZOKA Project State

Snapshot tecnico del repo al 2026-04-28. Para el roadmap canonico de launch,
leer [mainnet_launch_roadmap_v6.md](mainnet_launch_roadmap_v6.md). Para "que
falta y quien lo puede hacer", leer
[agent-vs-human-roadmap.md](agent-vs-human-roadmap.md).

## Estado ejecutivo

La base codeable de mainnet v6 esta mayormente cerrada:

- wallet privada por defecto;
- CLI `zoka` y wrapper `zokahd`;
- economia mainnet `zoka-economics-v2-atoms` con 6 decimales;
- PoW mainnet definido como RandomX;
- validacion de bloque, coinbase publica auditable, private coinbase y private
  fee claims;
- chain DB con fork choice/reorg por cumulative work, retarget e IBD inverso;
- genesis schema/digest/verifier;
- network config mainnet, loader y lint de readiness;
- snapshots export/import/info;
- `chain-stats`, `chain-prune` y test de boot mainnet contra genesis v1;
- tooling de release y runbooks.

Mainnet abierta todavia no esta lista porque faltan operadores reales, claves
reales, commit/tag congelado, firma GPG, auditoria externa, testnet publica y
launch operacional.

## Privacidad y consenso auditable

**Por defecto privado.** Wallets, balances, recipients, transferencias y fees
van por el path privado (`private_wallet::*`, CLSAG, Bulletproofs, Pedersen,
nullifiers, stealth addresses). El CLI de usuario no expone una ruta normal de
transferencia transparente.

**Auditable por consenso:**

- coinbase auditable de cada bloque para auditar emision y max supply;
- `block.private_coinbase` como nota privada spendable por el minero;
- `block.private_fee_claims[]` cifrados y comprometidos por Merkle/Pedersen;
- header de bloque para PoW, altura, enlaces y merkle root.

**Eliminado o fuera de superficie user-facing:**

- comandos publicos legacy de wallet/transfer;
- binarios legacy `user-cli`, `user-cli-private`, `user-node`;
- binarios operativos fuera de `zoka`/`zokahd` en builds de usuario;
- modulo `user_node`;
- modulo `transaction_pool` legacy;
- handlers operator/dev antiguos en `src/main.rs`.

## Layout actual

```text
src/
  main.rs                    # CLI principal zoka
  lib.rs                     # Re-exports del crate
  economics.rs               # Constantes canonicas mainnet, atoms y fees
  genesis.rs                 # zoka-genesis-v1 schema + validate + digest
  snapshot.rs                # zoka-snapshot-v1 export/import/sign
  admin/, api/, health/      # Superficie operacional/RPC
  block/, types/             # Tipos y helpers de bloque
  cli/                       # Infra CLI legacy/test helpers
  consensus/                 # PBFT module/tests, no finality gate en apply_block
  crypto/                    # Hashing, KDF, Pedersen, MPC helpers
  identity/                  # BIP39 / HD wallet helpers
  mempool/                   # Mempool support code
  metrics/                   # Metricas
  mining/                    # UserMiner, pow, difficulty, block_validator
  network/                   # libp2p, messages, handler, Tor/I2P config
  private_wallet/            # Wallet privada, CLSAG, claims
  state/                     # StateManager, chain DB, sync
  storage/                   # RocksDB wrapper
  transaction/               # Commitments/nullifiers/modelo tx
  wallet/                    # Storage cifrado y formatos de archivo
  zk/                        # Groth16/range proofs/setup io
  bin/                       # zokahd, ceremony, trusted setup, p2p worker
```

## Modulos mainnet-critical

`economics`
: Parametros hard-fork sensitive: `ZKA`, `1 ZKA = 1,000,000 atoms`, max
  supply `23,000,000`, reward inicial `12`, halving `1,051,200`, target block
  time `120s`, maturity `60`.

`mining::pow`
: Backend pluggable. Mainnet requiere `zoka-randomx-v1`; sin feature `randomx`,
  el binario rechaza minar chain `zoka-mainnet-v1`.

`mining::block_validator`
: Valida coinbase, private coinbase, private fee claims, prev/hash/height,
  merkle, header self-consistency, seal PoW y dificultad.

`state::chain`
: Punto canonico de entrada para bloques nuevos. Persiste canonical/side
  branches, calcula cumulative work y promueve la rama mas pesada.

`state::sync`
: IBD inverso desde peers: observa altura maxima, pide bloques faltantes,
  verifica hash anunciado y delega a `apply_block`.

`genesis`
: Schema `zoka-genesis-v1`, `deny_unknown_fields`, validacion de economia,
  validator set, pre-mine e initial accounts. `zoka genesis-verify` computa el
  digest canonico.

`snapshot`
: Export/import de `zoka-snapshot-v1` e inspeccion offline con
  `snapshot-info`. Import no salta consenso: re-ejecuta
  `state::chain::apply_block` por bloque.

`network::p2p`
: libp2p gossipsub/Kad/Identify, freeze de protocolo, Tor/I2P config y loader
  de `docs/mainnet-network.v1.json`.

## CLI publicado

| Comando | Estado smoke |
|---|---|
| `zoka wallet-create` | OK |
| `zoka wallet-restore-from-seed` | OK |
| `zoka wallet-restore-from-keys` | OK |
| `zoka wallet-list` | OK |
| `zoka wallet-balance` | OK |
| `zoka wallet-history` | OK |
| `zoka wallet-backup` | OK |
| `zoka wallet-export-secrets` | OK |
| `zoka wallet-change-passphrase` | OK |
| `zoka wallet-wipe` | OK |
| `zoka tx-send` | Path validado; rechaza sin fondos como corresponde. |
| `zoka estimate-fee` | OK |
| `zoka height` | OK |
| `zoka chain-stats` | OK |
| `zoka chain-prune` | OK |
| `zoka rpc-list` / `rpc-status` / `rpc-height` / `rpc-mempool` | v0.4 read-only; lista RPCs de config y consulta HTTP simple contra endpoints ZOKA sin wallet/firma/envio. |
| `zoka sync-status` | OK |
| `zoka mine` | Gate RandomX correcto en binario debug sin feature. |
| `zoka mine-status` / `mine-console` | OK |
| `zoka genesis-verify` | OK |
| `zoka mainnet-network-lint` | OK; la config RC local actual esta `launch-ready` y sin placeholders tecnicos. |
| `zoka release-keygen` | OK; genera keypair ed25519 y solo imprime la clave publica. |
| `zoka snapshot-export` / `snapshot-info` / `snapshot-import` | OK |
| `zoka fullnode` | OK en modo `--no-network-config --no-mine`. |
| `zokahd --inline` | OK delegando a `zoka fullnode`. |

Alias UX publicados: `zoka create`, `restore`, `wallets`, `balance`, `send`,
`fee`, `mine`, `node`, `status`, `sync`, `backup` y `wipe`.

Detalle: [cli-smoke-status.md](cli-smoke-status.md).

## Scripts relevantes

| Script | Uso |
|---|---|
| `scripts/check_randomx_toolchain.ps1` | Preflight de `cmake` + compilador nativo. |
| `scripts/package_release.ps1` | Empaqueta `zoka`/`zokahd`, docs, manifest y checksums. |
| `scripts/prepare_mainnet_launch_keys.ps1` | Genera keypairs ed25519 fuera del repo y pre-rellena `mainnet-launch-material.json`. |
| `scripts/prepare_release_gpg_key.ps1` | Intenta preparar una GPG release key local; requiere un GPG con agente funcional. |
| `scripts/sign_release_checksums.ps1` | Firma/verifica `SHA256SUMS.txt` localizando `gpg.exe` aunque no este en PATH. |
| `scripts/apply_mainnet_launch_material.ps1` | Valida y aplica claves/endpoints/fingerprint reales desde `docs/mainnet-launch-material.template.json`. |
| `scripts/release_checksums.ps1` / `.sh` | Genera `SHA256SUMS.txt`. |
| `scripts/mainnet_readiness_check.ps1 [-BuildRelease]` | Preflight estricto de release: docs, RandomX, genesis, placeholders, network lint, GPG y fingerprint. |
| `scripts/cli_smoke_test.ps1` | Smoke test reproducible de `zoka`/`zokahd` sobre perfil temporal. |
| `scripts/local_wallet_network_rehearsal.ps1` | Rehearsal local de wallet real sobre red local: coinbase maturity, transferencia privada e inclusion del hash en bloque sincronizado. |
| `scripts/build_release_with_setup_digest.ps1` | Build con digest de trusted setup. |
| `scripts/prepare_audit_package.ps1` | Paquete para auditoria externa. |
| `scripts/init_ceremony_round*.ps1` | Inicializa rondas de ceremonia MPC. |
| `scripts/process_ceremony_*.ps1` | Procesa retornos de ceremonia. |
| `scripts/publish_ceremony_bundle.ps1` | Publica bundle verificable. |
| `scripts/verify_ceremony_bundle.ps1` | Verifica bundle publicado. |
| `scripts/ceremony_closeout.ps1` | Closeout integral de ceremonia. |

## Verificacion local

- `cargo test --lib`: 671 passed, 1 ignored.
- `cargo test --tests`: OK. Integracion, network, PBFT transport,
  multiprocess smoke y Tor probe pasaron; 2 soak tests largos quedaron ignored
  por diseno.
- `cargo check --bins`: OK.
- `zoka genesis-verify docs/genesis.mainnet.v1.json --json`: OK, digest
  `f0d9e1261a3dec08d754f63e1bcfd2663f6cedfd476657125bfaebe7aefd5745`.
- `scripts/mainnet_readiness_check.ps1`: RC local completo termina con
  `Failures: 0`; para mainnet publica real aun faltan endpoints/operadores
  reales y cierre externo.
- `scripts/prepare_mainnet_launch_keys.ps1 -Force`: genero keypairs ed25519
  locales fuera del repo en `C:\Users\mderramus\zoka-secrets`.
- RC local completo: `scripts/mainnet_readiness_check.ps1` termina con
  `Failures: 0`; paquete y firma en
  `.zoka/test/release-complete/local-rc-complete`.
- `scripts/check_randomx_toolchain.ps1 -Strict`: OK, detecta CMake y MSVC Build
  Tools.
- `cargo build --release --features randomx --bin zoka --bin zokahd`: OK.

## Estado por fase v6

| Fase | Estado |
|---|---|
| Fase 1 - Wallet | Cerrada por codigo y smoke CLI. |
| Fase 2 - Mineria/economia | Cerrada por codigo; build release con RandomX nativo verificado. |
| Fase 3 - Genesis/validators | Schema/verifier cerrados; faltan claves reales, decision final de pre-mine y dry-run. |
| Fase 4 - Red publica | Formato/loader/lint cerrados; faltan endpoints reales activos. |
| Fase 5 - Auditoria | Pendiente externa. |
| Fase 6 - Testnet publica | Pendiente externa/operativa. |
| Fase 7 - Release | Tooling cerrado; faltan commit freeze, paquete release, firma GPG y publicacion. |
| Fase 8 - Launch ops | Runbooks cerrados; falta ejecucion real. |
