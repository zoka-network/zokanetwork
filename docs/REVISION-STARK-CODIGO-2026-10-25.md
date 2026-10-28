# Revisión asistida del pool STARK

Fecha: 25 de octubre de 2026, Argentina. Rama: `quantic-protocolo-postcuantico`.

Esta es una revisión del código realizada durante su implementación, con ayuda
de un segundo agente. **No es una auditoría externa ni una demostración formal
de seguridad, conocimiento cero o resistencia cuántica.** El resultado describe
el prototipo local; no certifica la red pública ni habilita mainnet.

## Alcance y evidencia

Se leyeron `crypto.rs`, `model.rs`, `transfer.rs`, `wallet.rs`, `transaction.rs`
y `ledger.rs` de [zoka-pq-pool](../crates/zoka-pq-pool/src/lib.rs), además de las
partes pertinentes del código fuente local de Plonky3 0.8.0 y postcard 1.1.3.
Se inspeccionó también el adaptador [stark.rs](../src/quantic/stark.rs).

La revisión fue principalmente estática. El agente principal comunicó los
resultados de las pruebas ejecutadas en este trabajo: transferencia cifrada,
propiedad y pertenencia, rechazo de doble gasto y recompensa inflada,
reorganización mediante el validador real del nodo y recuperación del estado
desde RocksDB. La prueba del nodo pasó en aproximadamente 22,5 segundos. El
revisor no ejecutó una segunda compilación ni una campaña independiente de fuzzing.

## Hallazgo corregido: codificaciones ambiguas

`Transaction::decode` y `verify_transfer` usaban `postcard::from_bytes`.
Según la implementación de postcard 1.1.3, esa función deserializa un objeto
e ignora el resto del buffer. Por tanto, añadir bytes a una transacción o al
buffer de una prueba no bastaba para hacerla inválida. El identificador de
efectos de la transacción no cambia por esos bytes.

Esto permite representaciones múltiples del mismo objeto y complica los
identificadores, límites de tamaño y deduplicación. No se demostró creación
de dinero ni gasto ajeno mediante este defecto.

El agente principal lo corrigió en
[transaction.rs](../crates/zoka-pq-pool/src/transaction.rs) y
[transfer.rs](../crates/zoka-pq-pool/src/transfer.rs): `take_from_bytes`, resto
vacío y comparación con la reserialización canónica. La comparación también
rechaza variantes no canónicas de los enteros de longitud variable. Se añadieron
regresiones para bytes sobrantes y una codificación alternativa de la versión.

## Restricciones del circuito

En la lectura no se encontró una omisión evidente de las siguientes reglas:

- El dueño de cada nota de entrada deriva del secreto de gasto mediante hash.
- El compromiso de la nota pertenece al árbol público mediante un camino de
  profundidad 32, con los bits del índice restringidos a valores booleanos.
- El nullifier público deriva de la clave y de `rho` de la nota.
- Los compromisos de salida se calculan dentro del AIR.
- Los montos son enteros de 64 bits; cuatro limbs de 16 bits y carries
  restringidos imponen conservación del saldo incluyendo comisión y emisión.
- Los efectos públicos, incluido el digest de los sobres cifrados, se incluyen
  en el transcript de la prueba. El consumidor verifica además ese digest.

Con un máximo de ocho entradas y ocho salidas, la magnitud de cada ecuación de
balance queda muy por debajo del módulo del campo. No se identificó una vía
para crear saldo usando un wrap modular en esas ecuaciones.

La primera prueba adversarial modificaba registros privados sin reconstruir
las permutaciones de hash. Podía fallar por inconsistencia del hash sin
aislar la regla de balance. Se añadió y pasó una prueba más específica:
incrementa una salida, recalcula las permutaciones y compromisos públicos y
comprueba que el AIR rechaza la inflación. También comprueba el rechazo de
un bit de monto no booleano. Esto mejora la cobertura; no sustituye la revisión
formal de todas las restricciones ni pruebas con testigos arbitrarios.

## Ocultamiento y estimador de soundness

La configuración usa `HidingFriPcs`, MMCS con ocultamiento tanto para la traza
como para FRI, ocho codewords aleatorios y RNG criptográficos sembrados por el
sistema operativo. Las implementaciones locales de `Clone` de esos componentes
bifurcan sus streams aleatorios. No se observó reutilización de máscaras entre
las pruebas construidas por esta biblioteca.

El límite conservador de ocultamiento que verifica `HidingFriPcs` es:

```text
N >= 2 * (consultas + dimensión_extensión * puntos_abiertos)
  >= 2 * (160 + 8 * 2)
  >= 352
```

La traza mínima de 512 filas cumple ese límite; ocho codewords cumplen el
mínimo de la extensión de grado ocho. Es una comprobación de configuración,
no una demostración de conocimiento cero del protocolo compuesto.

Para el estimador `ProvenSecurity` de Plonky3:

- Derivar la forma real con `StarkSecurityParams::from_air`.
- Usar `max_combo = 2`, porque el AIR abre filas local y siguiente. El valor
  `1` del antiguo benchmark de permutaciones no corresponde a este circuito.
- Usar 247 como piso de `log2(p^8)` para KoalaBear, no 248.
- Incluir `OpeningShape::hiding(8)` y únicamente el grinding realmente
  ejecutado; aquí no hay grinding.
- Pasar la altura comprometida después del ocultamiento:
  `degree_bits = log2(filas_originales) + 1`.

Se añadió una prueba del estimador sobre todas las formas admitidas. Su
hipótesis de resistencia a colisiones se fija explícitamente en 128 bits.
El término de consultas UDR calculado directamente es aproximadamente
132,76 bits para la forma 1→2; una cota hash de 128 limita el resultado total
como máximo a 128. No se debe publicar ese número como «128 bits cuánticos».
El estimador calcula soundness algebraico bajo sus hipótesis, y no acredita
por sí solo la seguridad de Fiat–Shamir en QROM, de Poseidon2, del cifrado ni
la privacidad del protocolo completo. Tampoco basta con ampliar la longitud
de salida de un hash para aumentar la seguridad de su construcción interna.

## Coste y límites del prototipo

La medida de aproximadamente 1,9 MB y 6,4 segundos corresponde a una prueba
real de una entrada y dos salidas en la PC usada en este trabajo. El antiguo
resultado de unos 200 KB medía permutaciones aisladas y no debe presentarse
como tamaño de una transferencia.

Por lectura del layout se obtiene:

```text
columnas = 319 + 1200 * entradas + 112 * salidas + 15
filas    = next_power_of_two(max(512, 104 * entradas + 3 * salidas))
```

| Forma | Columnas | Filas originales | LDE principal, estimación |
| --- | ---: | ---: | ---: |
| 0→1 | 446 | 512 | 14,19 MiB |
| 1→2 | 1.758 | 512 | 55,19 MiB |
| 8→8 | 10.830 | 1.024 | 677,38 MiB |

La estimación cuenta campos de cuatro bytes, ocho columnas aleatorias, la
duplicación de altura del ocultamiento y blowup ocho. **No es el pico de RAM**:
faltan otras trazas, buffers de DFT, quotient, Merkle y serialización.
El agente principal ejecutó después una prueba completa 8→8: pasó,
ocupó **8.341.990 bytes** y tardó **87,15 s** en probar en esta PC, durante
otras compilaciones. También ejercita montos próximos a `u64::MAX` y carries.
No se midió su ejecución en Android de 32 bits.
Aceptar esa forma en el modelo no prueba que resulte utilizable en todos los
dispositivos admitidos por las wallets actuales.

El adaptador del nodo conserva el estado completo del ledger por hash de
bloque. Eso permite validar ramas y probar reorganizaciones, pero repite
árboles y conjuntos crecientes en disco. La escalabilidad de este esquema no
equivale a la frontera compacta que empleaba el pool anterior. Ni este
almacenamiento ni la propagación de bloques de decenas de megabytes quedaron
validados para una red pública de larga duración.

## Implementado frente a validación todavía ausente

Existe código de transferencia STARK, sobres X-Wing, direcciones v2, ledger,
mempool acotado y adaptación al validador, PoW y almacenamiento del nodo.
La prueba de integración usa la identidad separada
`zoka-quantic-regtest-v2`. Las cadenas públicas existentes no se han migrado.

Las herramientas de wallet local se desarrollan en esta misma rama. Este
informe no acredita su interfaz, importación de semillas, gestión de archivos
o integración con ZSilent y Android: están fuera de la revisión del circuito.
Tampoco se ejecutaron en esta revisión sincronización entre nodos remotos,
propagación bajo carga, recuperación ante cortes de proceso, fuzzing de la
API/P2P, interoperabilidad real de XMRig con el nuevo pool ni un ataque con
claves clásicas comprometidas sobre el sistema completo. Reutilizar el formato
de blob de minería de 76 bytes no demuestra por sí solo esa interoperabilidad.

La privacidad implementada oculta montos y propietarios de las notas frente
a observadores sin claves. Continúan siendo públicos la comisión, cantidades
de entradas y salidas, nullifiers, compromisos, alturas y tiempos de bloques.
Este trabajo no demuestra anonimato de las direcciones IP ni resistencia al
análisis de tráfico.

## Fuentes primarias inspeccionadas

- Código de esta rama: [pool](../crates/zoka-pq-pool/src/lib.rs),
  [adaptador del nodo](../src/quantic/stark.rs) y
  [consenso compartido](../src/state/chain.rs).
- `postcard-1.1.3/src/de/mod.rs`: semántica de `from_bytes` y `take_from_bytes`.
- `p3-fri-0.8.0/src/hiding_pcs.rs`: RNG, presupuesto de ocultamiento,
  codewords aleatorios y uso único de compromisos privados.
- `p3-merkle-tree-0.8.0/src/hiding_mmcs.rs`: salts y separación de streams RNG.
- `p3-uni-stark-0.8.0/src/verifier.rs`: forma de prueba, transcript, valores
  públicos y apertura local/siguiente.
- `p3-uni-stark-0.8.0/src/security.rs` y `p3-security-0.8.0/src/{air,deep,fri,
  proximity,stark}.rs`: parámetros y alcance del estimador.
- `p3-poseidon2-air-0.8.0/src/columns.rs` y
  `p3-koala-bear-0.8.0/src/poseidon2.rs`: layout y constantes canónicas.

Esas dependencias se leyeron del registro local de Cargo. Las referencias a
papers presentes en sus comentarios no implican que se haya revisado o
reproducido aquí toda la demostración de cada paper.
