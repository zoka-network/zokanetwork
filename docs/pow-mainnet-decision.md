# ZOKA Mainnet PoW Decision (v1)

Estado: decision congelada para mainnet v1. Esta nota es la fuente unica de verdad para el algoritmo de proof-of-work del bloque y reemplaza cualquier descripcion contradictoria en codigo o roadmap viejo.

## Decision

Mainnet v1 usa **RandomX** (algoritmo de Tevador, mismo que Monero) como algoritmo canonico de PoW, identificado en el protocolo como `zoka-randomx-v1`.

Para desarrollo y testnet local existe un algoritmo separado, `zoka-cpu-fallback-v1`, basado en SHA3-256 con epoch keys. **No es valido en mainnet** y los nodos mainnet deben rechazar bloques que no usen `zoka-randomx-v1`.

La eleccion no es opcional ni configurable por nodo: cada chain ID lleva su algoritmo asociado en `src/economics.rs`. Mezclar algoritmos en una misma red es una falla de consenso y se trata como bug critico.

## Por que RandomX

Comparacion frente a las alternativas realistas:

| Criterio | RandomX | SHA3 fallback (lo que habia) | KawPoW / Ethash | Argon2d (Ravenchain-style) |
|---|---|---|---|---|
| Resistencia ASIC | Alta (memory-hard, AES + FP + branchy) | Baja (sha3 es ASIC-friendly) | Alta pero GPU-orientado | Media |
| Hardware objetivo | CPU general-purpose | CPU general-purpose | GPU | CPU |
| Madurez en produccion | 5+ anos en Monero | Ninguna (no esta en uso) | 4+ anos en RVN | Limitada |
| Auditoria publica | Si (Trail of Bits, paper de Tevador) | No | Parcial | No |
| Implementacion de referencia | C++ (Tevador) + binding `randomx-rs` | Inline en este repo | Multiples GPU miners | Bibliotecas dispersas |
| Costo de integracion | Alto (toolchain C++ + cmake) | Cero | Medio (acceso GPU) | Medio |
| Hashrate por nodo en CPU x86_64 | ~500-2500 H/s | ~500k H/s sin valor real | bajo en CPU | ~1k H/s |
| Riesgo de centralizacion en pools | Bajo | Alto (sha3 ASIC) | Alto (granjas GPU) | Medio |

ZOKA es una privacy chain orientada a usuarios reales con CPUs comunes. El requisito principal del PoW no es bruto (mas H/s es indiferente para la utilidad) sino **homogeneizar el costo de proponer un bloque** entre un usuario casero y una granja con hardware especializado. RandomX es el unico algoritmo en produccion que cumple eso con auditoria publica y 5+ anos de batalla real.

SHA3 corriendo en software siempre es ASIC-friendly: cualquier actor con presupuesto puede fabricar una unidad que reemplace a una CPU casera por cuatro o cinco ordenes de magnitud, romper la distribucion de bloques y centralizar la red. Eso ya es un patron documentado en otras chains (Bitcoin Cash con SHA-256 ASICs, Ravencoin con KawPoW antes de la migracion).

KawPoW resuelve la resistencia ASIC pero asume GPU. Forzar GPU como requisito basico contradice el target de usuarios privados con laptops y maquinas modestas. Argon2d tiene perfil similar pero nunca alcanzo masa critica en mainnets reales.

Conclusion: RandomX es la unica opcion racional para mainnet v1.

## Por que el fallback CPU sigue existiendo

`zoka-cpu-fallback-v1` se mantiene por dos motivos pragmaticos:

1. **Pipeline de desarrollo y testnet local sin toolchain C++**: nadie deberia necesitar `cmake` ni MSVC para correr la testnet en un laptop. El fallback es trivial de compilar en Rust puro.
2. **Pruebas continuas y CI**: los tests de regresion del flujo de mineria (no del algoritmo en si) corren con el fallback porque ejecutan en orden de microsegundos.

El fallback no es y no sera nunca un algoritmo de mainnet. El binario de mainnet:

- se compila con `cargo build --release --features randomx`,
- rechaza arrancar contra un genesis cuyo `chain_id` declare `zoka-randomx-v1` si el feature no esta presente,
- rechaza minar bloques con `algorithm = "zoka-cpu-fallback-v1"` si el chain ID es mainnet.

Esa separacion vive en `src/mining/pow.rs` y en `src/economics.rs::mainnet_algorithm()`.

## Que cambia operativamente

1. **Toolchain de release de mainnet**:
   - Linux x86_64: necesita `gcc`/`clang`, `cmake >= 3.5`, `make` y `pkg-config`.
   - Windows x86_64: necesita Visual Studio Build Tools (MSVC v143 o superior) y `cmake` en PATH.
   - Build: `cargo build --release --features randomx`.
   - Preflight: `powershell -ExecutionPolicy Bypass -File scripts/check_randomx_toolchain.ps1 -Strict`.
2. **Disclaimer al iniciar `zokahd`/`zoka mine` con feature off**: el binario imprime un warning explicito antes de minar contra una red distinta de mainnet. Si el chain_id es mainnet y el feature falta, falla en arranque, no permite degradacion silenciosa.
3. **Verificacion de bloques en otros nodos**: en mainnet v1, la verificacion de un bloque incluye recalcular el hash con la misma RandomX VM (mismo seed por epoch). Cualquier nodo que no compile con `--features randomx` no puede actuar como nodo mainnet.

## Que NO resuelve esta decision

Dejar congelada la decision de algoritmo no convierte la mineria en mainnet-grade por si sola. Estos puntos se tratan por separado en el roadmap:

- **Coinbase como transaccion en el bloque**: cerrado en Fase 2; `block.transactions[0]` es coinbase canónica y `Block.private_coinbase` contiene la salida privada spendable.
- **Validacion P2P del bloque**: cerrado para PoW/coinbase/fees/header; los peers aplican por `state::chain::apply_block`.
- **Reglas de fork choice**: cerrado para chain DB PoW con seleccion por `cumulative_work`.
- **Difficulty retargeting consensual**: cerrado con `mining::difficulty::expected_for_next_block`.
- **Ruteo de fees privadas**: cerrado con `private_fee_claims` en bloque y claim wallet-side.

Esta decision cierra solo el "que algoritmo". Habilita el resto del trabajo de Fase 2 sin reabrir la pregunta cada vez.

## Parametros canonicos asociados

Definidos en `src/economics.rs`:

- `POW_ALGORITHM_MAINNET = "zoka-randomx-v1"`
- `POW_ALGORITHM_TESTNET = "zoka-cpu-fallback-v1"`
- `POW_KEY_EPOCH_BLOCKS = 2_048` (cuanto vive una key/seed antes del re-hash; vale para ambos backends)
- `POW_TARGET_BLOCK_TIME_SECS = 120`
- `POW_INITIAL_DIFFICULTY_BITS = 16`
- `POW_MIN_DIFFICULTY_BITS = 12`
- `POW_MAX_DIFFICULTY_BITS = 30`

Los parametros economicos (`MAX_SUPPLY_ZKA`, `INITIAL_BLOCK_REWARD_ZKA`, `HALVING_INTERVAL_BLOCKS`, `COINBASE_MATURITY_BLOCKS`) son independientes del PoW y ya estaban congelados.

## Quien firma esto

Decision propuesta por el equipo core de ZOKA y registrada como parte del roadmap mainnet v6. Cualquier cambio posterior (por ejemplo migrar a un sucesor de RandomX) requiere hard fork y un documento equivalente en `docs/pow-mainnet-decision-vN.md`.
