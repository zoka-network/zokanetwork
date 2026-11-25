# Prueba v4 y actualizaciones de red (25/11/2026)

Rama `prueba-v4` (sobre `billetera-motor-liviano`, sobre `protocolo-v3`).
Código: [transfer/narrow.rs](../crates/zoka-pq-pool/src/transfer/narrow.rs),
reglas de activación en [ledger.rs](../crates/zoka-pq-pool/src/ledger.rs).

## El problema

Un pago de la testnet v3 lleva una prueba STARK de 2,59 MB si gasta 2 notas y
de 7,85 MB si gasta 8. La v3 copia el testigo entero en columnas constantes
(unas 1.100 por nota gastada: 32 niveles × 16 palabras de hermanos y otros
tantos nodos intermedios) y cada una de las 160 consultas de FRI abre la fila
completa. Las mediciones lo confirman: cada nota gastada de más suma 0,88 MB.

Eso hacía que la cadena creciera ~2,6 MB por pago, que entraran ~12 pagos en un
bloque de 32 MiB, que un pago de 8 notas tardara 90 s y 741 MB en el teléfono
de 32 bits, y que la billetera de la PC releyera 1 GB por consulta.

## Qué se hizo

### Circuito angosto (versión de prueba 4)

El mismo `Program` de la v3 (mismos trabajos, mensajes, dominios, entradas de
relleno y valores públicos: las dos versiones prueban los mismos enunciados),
en pocas columnas:

- Cada valor vive en una columna desde la fila que lo escribe un trabajo, o en
  la que lo carga el testigo, hasta la última fila que lo lee; después la
  columna lleva otro valor.
- Las filas en que una columna mantiene, recibe o carga un valor son columnas
  periódicas fijadas por la forma del pago: el probador no elige nada.
- El balance suma los límbs de 16 bits de cada nota en cuatro sumas corridas
  (las salidas restan) y se controla en una fila final con los acarreos.
- El ancla de cada entrada usa la suma de sus límbs, guardada en una columna
  hasta el final de su camino de Merkle.
- Transcript propio (`ZOKA QUANTIC transfer proof v4`): una prueba v4 nunca
  vale como v3 ni al revés.

### Medido en la PC (release, 25/11)

| Forma | Tamaño v3 | Tamaño v4 | Probar v3 | Probar v4 | Verificar v3 | Verificar v4 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2→2 | 2,59 MB | 0,74 MB | 2,09 s | 0,98 s | 167 ms | 126 ms |
| 4→2 | 4,33 MB | 0,74 MB | 3,72 s | 1,67 s | 226 ms | 143 ms |
| 8→2 | 7,85 MB | 0,79 MB | 13,95 s | 5,16 s | 343 ms | 199 ms |

Columnas: 452 en las tres formas (v3: 3.086, 5.550 y 10.478).

### Medido en el teléfono de 32 bits (25/11)

Samsung A13 (SM-A135M, armeabi-v7a, 3,8 GB). Las pruebas del pool compiladas
con `cargo ndk -t armeabi-v7a -P 26 test --release --lib --no-run`, copiadas
a una carpeta temporal de `/data/local/tmp` y borradas al terminar;
`what_each_version_costs`, con la memoria pico leída de `VmHWM` en `/proc`.

| Forma | Probar v3 | Probar v4 | Verificar v3 | Verificar v4 |
| --- | ---: | ---: | ---: | ---: |
| 2→2 | 14,06 s | 5,83 s | 553 ms | 375 ms |
| 4→2 | 26,51 s | 10,33 s | 787 ms | 437 ms |
| 8→2 | 90,11 s | 27,01 s | 1.308 ms | 634 ms |

- Tamaños iguales a los de la PC (2.593.885 / 737.480 bytes en 2→2;
  7.853.113 / 796.449 en 8→2).
- Memoria pico: 741 MB con la v3 8→2 (lo mismo que el 11/11) y **100 MB con
  la v4 8→2** (`the_largest_shape_with_carries_in_every_limb_proves_and_verifies`,
  44 s con la verificación).

De qué está hecha una prueba v4 de 2→2 (739.903 bytes):

| Parte | Bytes |
| --- | ---: |
| Filas de la traza abiertas (460 valores por consulta) | 294.882 |
| FRI (10 rondas) | 174.160 |
| Cociente (128 valores por consulta) + su Merkle | 155.476 |
| Caminos de Merkle de traza y máscaras | 70.128 |
| Aperturas fuera del dominio y de las máscaras | 34.091 |
| Filas de las máscaras | 10.562 |

## Decisiones

1. **Circuito v4 angosto.** Es la palanca grande y no cambia notas, árbol,
   nullifiers ni direcciones.
2. **FRI igual que la v3** (blowup 8, 160 consultas, plegado de a 2, sin
   grinding). El estimador de Plonky3 da 128 bits clásicos en las tres formas,
   igual que la v3: el tope sigue siendo la hipótesis de 128 bits de Keccak.
   Las otras perillas medidas el 11/11 sacaban como mucho un 14 % (blowup 16 y
   128 consultas) a cambio del doble de tiempo de prueba.
3. **Keccak en el árbol de FRI y en el transcript**, como decidió la v3.
4. **Codificación postcard, sin cambios.** Una codificación de ancho fijo
   ahorraría ~15 % (los valores de campo ocupan ~4,9 bytes en vez de 4), pero
   agrega código propio en el camino de decodificación del consenso. Queda
   como posible versión futura.
5. **Sin agregación en el lanzamiento de la mainnet.** Ver abajo.
6. **Margen cuántico sin cambios** (la revisión de la solidez postcuántica
   sigue en el mapa de 2028). Un campo de desafíos más grande sería otra
   versión de prueba, activable por altura.

## Agregación: por qué no ahora

Para agregar los pagos de un bloque, el minero tiene que verificar cada prueba
dentro de otra. Lo caro es rehashear lo abierto:

- Por prueba v4: ~97.000 valores de campo en las hojas abiertas, ~3.700
  compresiones en los caminos de Merkle y ~3.000 más en FRI. Del orden de
  10.000 a 15.000 permutaciones de Keccak-f por prueba; unas 120.000 a 180.000
  por bloque de 12 pagos.
- El AIR de Keccak de Plonky3 usa 24 filas y unas 2.600 columnas por
  permutación: miles de millones de celdas por bloque, contra las 460.000 de
  una prueba v4 (1 a 5 s). No entra en una PC cada 2 minutos.
- Con Poseidon2 en el árbol de la prueba interna (una fila de ~300 columnas por
  permutación) serían unas 60 a 90 millones de celdas: varios minutos por
  bloque en esta PC. Posible para un probador dedicado, no para un minero
  común.

Conclusión: la mainnet sale con una prueba por pago (v4). Para el disco, cada
nodo podría descartar las pruebas viejas ya validadas, con nodos de archivo
que las guardan todas. La agregación queda como una actualización futura (una
versión de prueba nueva) si la recursión de Plonky3 madura.

## Actualizaciones de red en vez de reinicios

Decisión del usuario (25/11): no se vuelve a empezar la cadena de cero por
cada cambio, y la mainnet nunca se reinicia. Los cambios de formato entran como
actualizaciones a una altura anunciada, como hicieron Zcash (Sapling, Orchard)
y Monero (RingCT, Bulletproofs, CLSAG).

La regla (consenso, `ledger::proof_version_allowed`):

- un pago del bloque `h` puede llevar la prueba v4 si `h ≥ H`;
- la v3 vale hasta `H + 100` (gracia para los pagos armados justo antes);
- `H` es 10.000 en la testnet (decidido el 25/11 a la altura 9.865, para verla
  activarse esa misma tarde), 8 en regtest, y toda red nueva (la mainnet)
  tiene solo v4 desde el génesis.

Las billeteras prueban con `ledger::proof_version_for(red, próximo bloque)`:
nadie elige la versión. El mempool vuelve a validar con cada bloque, así que
un pago v3 que quedó afuera de la gracia sale solo.

Notas, árbol, nullifiers y direcciones no cambian: los saldos y las billeteras
siguen igual después de la activación.

## Qué falta

- Ver la activación en el bloque 10.000 y un pago real con la v4 (todo lo
  demás se publicó el 25/11: ver [RELEASE-5.1.0.md](RELEASE-5.1.0.md)).

Hechos el 25/11: la `.so` de ZKAPriv (arm64-v8a y armeabi-v7a) y su APK 4.0.1,
el WASM de la billetera web y el ensayo de la activación en regtest. ZokaScan no
se recompila: solo escanea, no verifica pruebas.
