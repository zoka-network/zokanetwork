# ZOKA Mainnet Operational Roadmap v5

Objetivo: cerrar el tramo post-v4 con parametros economicos canonicos y requisitos minimos de experiencia de usuario antes de abrir mainnet.

## 1) Economia de red (propuesta canonica v1)

Nota v6: este documento queda como referencia historica. La fuente canonica actual es `src/economics.rs` junto con `docs/economics.mainnet.v2-atoms.json`.

### 1.1 Token

- Nombre: `ZOKA`
- Ticker: `ZKA`
- Decimales: `6` en la contabilidad activa.
- Unidad de contabilidad: atom (`1 ZKA = 1_000_000 atoms`).
- Max supply: `23,000,000 ZKA` (hard cap).

### 1.2 Emision y recompensa

- Tiempo objetivo de bloque: `120` segundos.
- Recompensa inicial por bloque (era 0): `12 ZKA`.
- Halving cada `1_051_200` bloques (aprox. 4 anios).
- Formula de recompensa por bloque:
  - `reward(height) = min(halving_reward(height), remaining_supply(height))`
  - `halving_reward(height) = (initial_reward >> era(height)) * atoms_per_zka`.
  - `era(height) = floor(height / 1_051_200)`
- Tail emission: `0` (deshabilitada para respetar hard cap).
- Regla de corte: cuando `remaining_supply = 0`, `reward = 0`.
- Tabla de emision validada y publicada: [docs/emission_schedule_v1.md](emission_schedule_v1.md)

### 1.3 Fees

- Fee minima por tx privada: `0` atoms cuando la red esta quieta; el estimador sube por pasos de `0.0001 ZKA` bajo carga.
- Politica de priorizacion de mempool:
  - `effective_fee = fee_per_input + fee_per_size`
  - desempate por antiguedad (FIFO).
- Destino de fees de bloque:
  - `100%` al productor/proposer del bloque via `private_fee_claims` cifrados.

### 1.4 Genesis y supply inicial

- `chain_id` exclusivo de mainnet (distinto de testnet/devnet).
- Genesis versionado y congelado por digest (SHA-256) antes de launch.
- Supply inicial de genesis + emision acumulada <= `23,000,000 ZKA`.
- Distribucion genesis documentada en archivo publico:
  - `genesis.mainnet.json`
  - `GENESIS_DIGEST.txt`

## 2) Consenso y mineria (decision operativa)

- Consenso mainnet: `PBFT` (no PoW competitivo como mecanismo de consenso final).
- El modulo `mining` actual queda catalogado como:
  - herramienta local/dev para simulacion y testing.
  - no fuente canonica de seguridad de consenso en mainnet PBFT.
- Recompensa en mainnet se acredita al productor PBFT del bloque finalizado.

## 3) Lo minimo que no puede faltar al usuario antes de launch

### 3.1 Nodo full (cero friccion)

- Arranque 1-comando que:
  - cree/cargue wallet automaticamente.
  - inicie nodo y sincronizacion sin pasos manuales.
- Comando para mostrar/exportar material de recuperacion de rewards:
  - direccion, seed, claves necesarias.
- Todos los textos de cara al usuario en ingles.

### 3.2 Wallet

- Create/import/restore por seed phrase verificados E2E.
- Envio y recepcion publica y privada.
- Historial local consistente despues de reinicio.
- Validacion estricta de direccion y checksum.
- Politica de fees visible y predecible para el usuario.

### 3.3 Red y privacidad por defecto

- Tor/I2P funcional por defecto para wallet y nodos seed publicos.
- Dandelion++ activo con parametros congelados para launch.
- Modo fallback documentado cuando no hay transporte anonimo disponible.

### 3.4 Operacion y soporte

- Logs sin fuga de secretos/PII.
- Mensajes de error accionables.
- Guia de backup/restore y recovery de fondos para usuario final.

## 4) Checklist operativa de salida (go-live)

1. Congelar `chain_id`, `genesis`, parametros economicos y formato tx.
2. Build reproducible + firma GPG + checksums publicos.
3. Dry-run de genesis con set real de validadores.
4. Testnet estable 8-12 semanas sin intervencion manual.
5. Chaos tests (caida de validadores, particion de red, stress mempool privada).
6. Release candidate firmado.
7. Lanzamiento mainnet.

## 5) Criterio de terminado (DoD v5)

Se considera completado cuando:

- Existe un unico documento canonico de economia (`v1`) publicado.
- El nodo full opera en modo "arrancar y usar" para usuario comun.
- El CLI de usuario en ingles cubre create/import/send/receive/history/backup.
- El runbook de release permite repetir build y verificar binarios sin ambiguedad.
