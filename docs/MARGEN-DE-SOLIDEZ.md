# Margen de solidez de la prueba de pago (punto 5 de la revisión)

11/11/2026, rama `endurecimiento-sin-consenso`. No cambia el protocolo: mide
la configuración actual y la compara con alternativas, para decidir en el
próximo reinicio de la testnet.

**No es una auditoría ni un análisis cuántico.** Son los números del
estimador de Plonky3 (`p3-security` 0.8.0) para el circuito real, término
por término, más lo que cuesta cada alternativa. **No usar estas cifras en
comunicación pública.**

Para reproducirlo:

```powershell
cargo test --release --manifest-path crates/zoka-pq-pool/Cargo.toml --lib transfer::margin -- --include-ignored --nocapture
```

El código está en
[transfer/margin.rs](../crates/zoka-pq-pool/src/transfer/margin.rs). La
prueba controla que los términos que muestra den exactamente los números que
reporta Plonky3.

## Qué mide el estimador

- **Solidez clásica ronda por ronda** del protocolo, en dos regímenes
  probados: decodificación única y decodificación por listas (cota de
  Johnson). Vale el mejor de los dos. También hay uno conjeturado (el de
  «palabras aleatorias», ePrint 2025/2010), que el mismo trabajo no recomienda
  para producción.
- **Cada término** es −log2 de una probabilidad de error. Hay cinco:
  - la composición del AIR;
  - DEEP-ALI;
  - la prueba de bajo grado (FRI);
  - la combinación de las aperturas;
  - el tope del hash de los compromisos.
- **Hipótesis que fijamos nosotros:** 128 bits de resistencia a colisiones
  para Poseidon2 en los compromisos (conservadora) y un campo de desafíos de
  247 bits (la extensión de grado 8 de KoalaBear).

## La configuración actual: blowup 8, 160 consultas, sin grinding

| Término | 1→2, única | 1→2, listas | 8→8, única | 8→8, listas |
| --- | ---: | ---: | ---: | ---: |
| Composición del AIR | 234,3 | 223,8 | 231,5 | 220,0 |
| DEEP-ALI | 233,8 | 223,3 | 232,8 | 221,4 |
| Bajo grado (FRI) | 132,8 | 202,6 | 132,8 | 198,7 |
| Combinación de las aperturas | 222,1 | **190,7** | 218,5 | **184,2** |
| Colisión del hash (hipótesis) | **128** | **128** | **128** | **128** |

- **La cota es 128 bits clásicos en las 64 formas de pago**, y lo que la fija
  es la hipótesis del hash.
- **Sin ese tope, el protocolo da unos 190 bits** (1→2) y 184 (8→8), en el
  régimen de listas, donde manda la combinación de las aperturas.
- **El «132» que dio Claude el 10/11** es el término de bajo grado del
  régimen único, que no es el que manda. La cuenta «66 cuánticos, nivel 1 de
  NIST» no tenía fundamento: ver más abajo.

## Las alternativas (pago 1→2, en la PC)

| Configuración | Sin el tope | Prueba | Probar | Verificar |
| --- | ---: | ---: | ---: | ---: |
| actual: blowup 8, 160 consultas | 190,7 | 1,93 MB | 4,8 s | 205 ms |
| + grinding de 16 bits | 190,7 | 1,93 MB | 4,9 s | 204 ms |
| 256 consultas, trazas de 1024 filas | 212,5 | 3,08 MB | 9,5 s | 272 ms |
| 320 consultas, trazas de 1024 filas | 221,1 | 3,77 MB | 9,5 s | 319 ms |
| blowup 16, 128 consultas | 188,7 | 1,67 MB | 9,1 s | 189 ms |
| blowup 16, 160 consultas, grinding 16 | 188,7 | 2,01 MB | 9,8 s | 206 ms |

«Sin el tope» es el mejor régimen dejando afuera la hipótesis del hash. Con
ella, todas dan 128.

1. **Ninguna mejora la cota completa.** El tope es el hash.
2. **El grinding no cambia nada.** Sube el régimen único, que no es el que
   manda.
3. **Más consultas no entran en la traza actual.** El ocultamiento de FRI (el
   conocimiento cero) necesita `2 · (8 · 2 + consultas)` valores de máscara.
   Con 512 filas, el máximo son **240 consultas**. Pasar de ahí obliga a
   trazas de 1024 filas para todos los pagos, que es cambio de formato. Cuesta
   entre 60 % y 95 % más de tamaño y el doble de tiempo, y gana 22 a 30 bits
   clásicos. Lo comprueba `more_than_240_queries_do_not_fit_a_512_row_trace`.
4. **Blowup 16 con 128 consultas achica la prueba un 14 %** (1,67 MB) con casi
   la misma cota (188,7). A cambio, duplica el tiempo y la memoria de la
   extensión: hay que medirlo en el teléfono de 32 bits antes de pensarlo.
5. **Más de unos 222 bits no se consiguen con estas perillas.** La
   combinación de las aperturas y la composición las limita el tamaño del
   campo de desafíos.

No se midió la memoria ni se probó nada en el teléfono.

## Y frente a una computadora cuántica

- **Lo que está demostrado:** Chiesa, Manohar y Spooner probaron en 2019
  («Succinct Arguments in the Quantum Random Oracle Model», TCC) que la
  compilación que usamos sigue siendo segura contra un atacante cuántico, en
  el modelo del oráculo aleatorio cuántico. La compilación es la de BCS:
  compromisos de Merkle y Fiat–Shamir. La condición es que el protocolo tenga
  solidez ronda por ronda, que es la que estima Plonky3. La cota pierde con el
  cuadrado de las consultas que el atacante le hace al hash.
- **La lectura de esa cota es solo orientativa:** deja del orden de la mitad
  de los bits del protocolo. Serían unos 95 con la configuración actual y unos
  110 con 320 consultas. No son mediciones ni constantes calculadas para este
  protocolo, y no se traducen a un nivel de NIST. NIST compara contra el costo
  de atacar AES o SHA-2, con límites de profundidad del circuito cuántico.
- **Para que esa lectura llegue a 128 haría falta un protocolo de unos 256
  bits clásicos.** Con un campo de desafíos de 247 bits no se llega: haría
  falta un campo más grande, que es otro formato de prueba y otras
  mediciones.
- **Aparte está el hash.** Poseidon2 es joven (punto 6 de la revisión), y su
  resistencia, clásica y cuántica, es la otra hipótesis de todo esto.

## Qué se decide en el próximo reinicio

- **A. Quedarse con 160 consultas** y la hipótesis de 128 bits del hash, sin
  prometer bits cuánticos. Si el teléfono lo banca, se puede achicar la
  prueba con blowup 16 y 128 consultas.
- **B. Subir el margen del protocolo** con 256 a 320 consultas y trazas de
  1024 filas. Pagos más pesados y lentos, 22 a 30 bits clásicos más, y la
  lectura cuántica sigue debajo de 128.
- **C. Apuntar a unos 128 en la lectura cuántica:** un campo de desafíos más
  grande. Es un rediseño que habría que medir primero.

En cualquiera de las tres, la revisión externa va antes de la mainnet. El
punto 6 (un hash conservador en el Merkle de FRI y en el transcript) se
decide junto con este, porque cambia la hipótesis del tope.
