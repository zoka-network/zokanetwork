# Pool privado postcuántico de ZOKA QUANTIC

## Estado de la implementación: 28/10/2026

Ya existe una implementación STARK v2 en
[zoka-pq-pool](../crates/zoka-pq-pool/src/lib.rs), integrada con consenso,
persistencia, wallet, minería, API y transporte de un nodo en la cadena separada
`zoka-quantic-regtest-v2`. La [guía de uso](QUANTIC-STARK-USO.md) documenta el
build con `--features quantic-pq`, dos nodos, sincronización, pagos y los límites
actuales. El regtest usa PoW CPU; no es el protocolo `rx/0` de XMRig ni una
migración de la red pública o de wallets Orchard.

La [revisión asistida del código STARK](REVISION-STARK-CODIGO-2026-10-25.md)
describe las restricciones verificadas, las correcciones y las hipótesis de
seguridad. Es una revisión de implementación, no una auditoría externa ni una
certificación postcuántica. Los bits del estimador son una cota bajo su modelo
clásico; no deben publicarse como bits de seguridad cuántica. La
[revisión de Quantic del 24/10](REVISION-quantic-2026-10-24.md) aporta el contexto
anterior, pero no sustituye la evaluación de la ruta STARK v2.

El texto que sigue conserva el diseño y las mediciones históricas del 21/10.
Sus candidatos, parámetros y decisiones eran propuestas de esa fecha. En
particular, los aproximadamente 200 KB medían permutaciones aisladas, no una
transferencia completa; la implementación actual no agrega recursivamente las
pruebas del bloque y admite hasta 32 MiB de transacciones binarias por bloque.
Para comportamiento y comandos vigentes, prevalecen la guía de uso y el código.

## Diseño y medidas históricos: 21/10/2026

21/10/2026. Paso 3 del plan (ver
[QUANTIC-INVENTARIO-CRIPTOGRAFICO.md](QUANTIC-INVENTARIO-CRIPTOGRAFICO.md)).
Decisión del usuario del 21/10: **la mainnet espera a este pool**. La testnet
puede salir antes con Orchard.

En esa fecha era un borrador sin parámetros fijados. Se proponía construir un
prototipo para medir las estimaciones siguientes; véase arriba el estado actual.

## Objetivo

Una transferencia privada (emisor, receptor y monto ocultos, como hoy) en la que
**ni la integridad ni la privacidad dependan de curvas elípticas**: con Shor no
se puede gastar lo ajeno, crear valor, cambiar una tx del mempool ni leer notas.

## Qué cambia respecto de Orchard

| Orchard (hoy) | Pool nuevo |
| --- | --- |
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
| --- | --- | --- | --- |
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

## Primeras medidas (21/10, PC, un hilo)

`crates/zoka-pq-pool`: Plonky3 0.8.0, KoalaBear⁴, Poseidon2 en la traza, en el
árbol de FRI y en el transcript, FRI con ocultamiento (blowup 4, 16 bits de
grinding en las consultas). La traza son solo permutaciones de Poseidon2, sin las
restricciones que las unen en una transferencia: es una cota de abajo.

| Permutaciones | Consultas | Prueba | Probar | Verificar | Seguridad demostrada |
| --- | --- | --- | --- | --- | --- |
| 64–128 | 100 | no entra | — | — | — |
| 256 | 100 | 200 KB | 0,20 s | 13 ms | 102 bits |
| 256 | 56 | 121 KB | 0,34 s | 12 ms | 71 bits |
| 512 | 100 | 224 KB | 0,41 s | 14 ms | 101 bits |
| 1024 | 100 | 248 KB | 0,78 s | 16 ms | 100 bits |

Lo que dicen:

- **El ocultamiento pone un piso.** Con conocimiento cero, una traza de menos de
  unas 256 filas no alcanza para esconder 100 consultas (`HidingBudgetExceeded`):
  la prueba de una transferencia, que necesita unas 40 permutaciones, se rellena
  hasta ahí igual. Por eso el tamaño casi no depende de la transferencia.
- **Unos 200 KB por transferencia** con ~100 bits demostrados, contra unos pocos
  KB de Halo 2. Con el límite de bloque de 1 MB entrarían ~5 transferencias por
  bloque, y la cadena crecería 200 KB por cada una. **Agregar las pruebas del
  bloque en una sola (recursión) no es opcional**: el bloque guarda una prueba y,
  por transferencia, solo nullifiers, compromisos y sobres X-Wing (~1,7 KB por
  salida). Es lo que hace Quantus con Plonky2. El mempool igual viaja con las
  pruebas enteras.
- **Probar es rápido en la PC** (0,2 s con un hilo). Falta medirlo en el teléfono
  de 32 bits; si fuera 30 veces más lento, serían unos 6 s, del orden de lo que
  tarda hoy Halo 2 ahí.
- **Verificar es barato** (13 ms): un nodo valida muchas por segundo.

**El riesgo principal es la agregación.** Para Plonky3 no hay recursión publicada
y estable: `p3-recursion` 0.1.0 en crates.io (agosto de 2025) es un marcador sin
código, y lo que existe está en el repositorio Plonky3-recursion: activo (commits
del 17/10/2026), sin versiones, hecho sobre Plonky3 0.9.0-rc.2, con verificación
recursiva de lotes de pruebas y conocimiento cero, y con un aviso propio de que no
está auditado ni es para producción. Caminos:

- (a) Depender de ese repositorio por git, fijar un commit y esperar su
  auditoría.
- (b) Plonky2, con recursión madura (la usa Quantus), pero en modo de
  mantenimiento y con un campo de 64 bits, más lento en el teléfono de 32 bits
  (que igual solo prueba su propia transferencia: agrega el minero, en la PC).
- (c) Stwo, cuya recursión pasa por Cairo.
- (d) Sin recursión: pruebas por transferencia, bloques más grandes (unos 8 MB:
  ~40 transferencias cada 120 s) y los nodos descartan las pruebas viejas después
  de validarlas. Un nodo nuevo no las necesita: arranca del snapshot firmado por
  la autoridad híbrida. Es lo más simple de auditar, a cambio de ancho de banda y
  de que un nodo nuevo confíe en la autoridad para la historia que ya no puede
  re-verificar (hoy ya obedece sus checkpoints, pero podía revisar las pruebas).

Hay que elegir antes de escribir el circuito de la transferencia, porque fija el
campo y el hash.

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

1. **Prototipo** (`crates/zoka-pq-pool`, experimental, fuera del consenso).
   Hecho para las permutaciones (tabla de arriba). Falta el circuito de una
   transferencia de 1 entrada y 2 salidas con sus restricciones.
2. **El mismo prototipo en el teléfono** (compilado para ARMv7; las medidas las
   toma el usuario con el teléfono conectado por adb).
3. **Agregación por bloque**: medir una prueba recursiva que verifique N pruebas
   de transferencia (lo arma el minero, en la PC). Con eso, fijar parámetros y
   escribir la especificación.
4. Implementar el pool en `zoka-quantic` (notas, árbol, nullifiers, wallet,
   bloques compactos), con vectores de prueba y el atacante simulado.
5. Testnet nueva con el pool, auditoría externa, y recién ahí el génesis de la
   mainnet.
