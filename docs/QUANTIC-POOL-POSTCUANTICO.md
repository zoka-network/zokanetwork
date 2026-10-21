# Pool privado postcuántico de Zoka Quantic (borrador de diseño)

21/10/2026. Paso 3 del plan (ver
[QUANTIC-INVENTARIO-CRIPTOGRAFICO.md](QUANTIC-INVENTARIO-CRIPTOGRAFICO.md)).
Decisión del usuario del 21/10: **la mainnet espera a este pool**. La testnet
puede salir antes con Orchard.

Es un borrador: nada de esto está construido y ningún parámetro está fijo. El
primer paso es un prototipo que mida lo que acá se estima.

## Objetivo

Una transferencia privada (emisor, receptor y monto ocultos, como hoy) en la que
**ni la integridad ni la privacidad dependan de curvas elípticas**: con Shor no
se puede gastar lo ajeno, crear valor, cambiar una tx del mempool ni leer notas.

## Qué cambia respecto de Orchard

| Orchard (hoy) | Pool nuevo |
|---|---|
| Halo 2 (IPA sobre Pasta) | STARK con FRI: solo hashes |
| Compromiso de nota Sinsemilla, árbol Sinsemilla | Poseidon2 sobre un campo de 31 bits, árbol de profundidad 32 |
| `cv` de Pedersen + firma de balance | El balance se prueba dentro del circuito: una prueba por tx cubre todas sus entradas y salidas, y la comisión es pública |
| Firma RedPallas con clave re-aleatorizada | La prueba muestra que se conoce la clave que deriva el dueño de la nota y queda atada al txid (entrada pública): la prueba es la firma |
| Nullifier con escalares de Pallas | `nf = Poseidon2(nk, rho)`, un PRF con clave secreta |
| Dirección Orchard (`pk_d`) + X-Wing | Etiqueta del dueño (hash de las claves) + X-Wing |
| Sobres X-Wing | Igual (ya son postcuánticos) |

Bosquejo de lo que prueba cada transacción:

```text
públicas: ancla del árbol, nullifiers, compromisos de salida, comisión, txid
privadas: notas de entrada, sus caminos de Merkle, la clave de gasto, notas de salida

por cada entrada:  cm = Poseidon2(nota) está en el árbol del ancla
                   dueño(nota) = H(clave de gasto)
                   nf = Poseidon2(nk(clave), rho)
por cada salida:   cm_salida = Poseidon2(nota_salida); valor < 2^64
balance:           Σ entradas = Σ salidas + comisión
atadura:           el txid entra al transcript (Fiat-Shamir)
```

## Sistema de pruebas: candidatos

| | Plonky3 | Stwo | Winterfell |
|---|---|---|---|
| Campo | BabyBear / KoalaBear (31 bits), Goldilocks, M31 | M31 (Circle STARK) | Goldilocks (64 bits) |
| Teléfono de 32 bits | Campos de 31 bits, con NEON | M31, de 32 bits | Aritmética de 64 bits, más lenta en ARMv7 |
| Auditoría pública | Least Authority (2024) | El verificador Stwo-Cairo (zkSecurity, 2025), no el prover | No encontré |
| En producción | Base de SP1 y OpenVM | Prover de Starknet | — |

**Recomendación para el prototipo: Plonky3 sobre KoalaBear con Poseidon2.**
Tiene la auditoría pública más concreta, un campo cómodo para el teléfono de 32
bits y Poseidon2 y árboles de Merkle ya hechos. Stwo queda como segunda opción
si Plonky3 no da los tiempos.

## Lo que hay que medir antes de decidir

1. **Tamaño de la prueba por transacción.** Una STARK pesa del orden de cientos de
   KB contra los pocos KB de Halo 2. Con el límite actual de bloque (1 MB) eso deja
   pocas tx por bloque. Salidas: que el minero agregue las pruebas del bloque en
   una sola (recursión), o subir el límite. El mempool igual viaja con las pruebas
   enteras.
2. **Tiempo de prueba en el teléfono** (armeabi-v7a). Con Halo 2 son 4–14 s; si la
   STARK tarda minutos, la wallet móvil no sirve así.
3. **Tiempo de verificación en el nodo**, que fija cuántas tx por minuto valida.
4. **Nivel de seguridad**: la de FRI se apoya en conjeturas; parámetros
   conservadores (≥ 100 bits conjeturados, hash de 256 bits) aunque la prueba pese
   más.

## Lo que se pierde

- **Prueba delegada**: hoy un dispositivo podría probar sin tener la clave de
  gasto (solo con la de prueba). Si la autorización va dentro de la prueba, quien
  prueba tiene la clave. Para nuestras wallets (el teléfono y la PC prueban lo
  suyo) no cambia nada.
- **Lo probado de Orchard**: es diseño propio. No hay todavía un pool privado
  postcuántico en producción; Zcash apunta a 2027 (Tachyon y recuperabilidad
  cuántica) y entre lo que estudia están justamente las STARK basadas en hashes.
  **Antes del génesis de mainnet: auditoría externa obligatoria** y revisar si lo
  que publique Zcash conviene más que lo nuestro.

## Plan

1. **Prototipo** (`crates/zoka-pq-pool`, experimental, fuera del consenso): una
   transferencia de 1 entrada y 2 salidas con Plonky3 + Poseidon2. Medir tamaño,
   tiempo de prueba y de verificación en la PC.
2. **El mismo prototipo en el teléfono** (compilado para ARMv7; las medidas las
   toma el usuario con el teléfono conectado por adb).
3. Con esos números: elegir entre prueba por tx o agregada por bloque, fijar
   parámetros y escribir la especificación.
4. Implementar el pool en `zoka-quantic` (notas, árbol, nullifiers, wallet,
   bloques compactos), con vectores de prueba y el atacante simulado.
5. Testnet nueva con el pool, auditoría externa, y recién ahí el génesis de la
   mainnet.
