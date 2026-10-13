# Etapa 5: gastar sin key images — elección de la construcción

13/10/2026. **Decisión confirmada por el usuario el mismo día; no hay código
integrado al nodo.** La propuso Claude con las decisiones que delegó el usuario y
queda sujeta a revisión externa antes del génesis de Quantic. En la hoja de traspaso esta etapa figura
como la 4 ("Circuito de privacidad y eliminación de key images").

## El problema

- **El grafo de gastos es público hoy, sin computadora cuántica.** Cada
  transacción publica `input_commitments` e `input_public_keys` de las notas que
  gasta; en la cadena real, 559 de 834 notas gastadas se unen directo con la
  salida donde nacieron (relevado el 11/10). El anillo CLSAG no lo tapa.
- **El circuito Groth16 actual no prueba lo necesario** para retirar CLSAG: ni la
  propiedad de la nota ni que el nullifier esté bien calculado
  ([revisión de la etapa 1](REVISION-postcuantica-etapa1.md)).
- **Los parámetros del trusted setup actual los generó una sola persona**: quien
  los tenga podría crear monedas sin que se note (punto 7.6 del plan).

## Requisitos

De la revisión de la etapa 1 y del principio del plan (la privacidad tiene que
resistir hacia atrás; la validez se puede migrar hacia adelante):

1. Probar dentro del circuito la propiedad de cada nota, su compromiso, su
   pertenencia al árbol, el nullifier y la conservación de valor; públicos solo
   la raíz, los nullifiers y las salidas; la prueba atada al contenido de la tx.
2. Privacidad que un adversario cuántico no pueda romper con la cadena guardada.
3. **No usar una construcción casera.**
4. Que el teléfono de pruebas (ARMv7 de 32 bits) pueda enviar.

## Lo que pasó con Orchard (mayo a julio de 2026)

Taylor Hornby encontró a fines de mayo una falla de solidez en el circuito de
Orchard, presente desde 2022, que en teoría permitía crear ZEC falsos sin que se
note (según la prensa especializada, una multiplicación de curva mal restringida).
Zcash la corrigió con un hard fork (NU6.2, 3/06) y el 28/07 activó **Ironwood**
(NU6.3, bloque 3.428.143): un pool nuevo con el circuito corregido sobre Halo 2,
con verificación formal declarada por sus autores, y un molinete que deja
encerrada en Orchard cualquier moneda falsificada. No hay evidencia de que se
haya explotado.

Dos lecciones para Zoka:

- Si el circuito más revisado del ecosistema tuvo una falla de solidez durante
  cuatro años, **un circuito propio de Zoka no es aceptable**.
- La validez también se rompe por errores clásicos, no solo por la cuántica:
  conviene que el suministro sea auditable (como el molinete de Zcash).

## Opciones

| | **Ironwood** (Orchard corregido, Halo 2) | **Sapling** (Groth16) con los parámetros de Zcash | **Circuito propio** (Groth16) |
|---|---|---|---|
| Trusted setup | Ninguno | El de la ceremonia multiparte de Zcash, solo si el circuito no se toca | Ceremonia propia obligatoria |
| Madurez | En producción en Zcash desde el 28/07/2026, circuito corregido | En producción desde 2018; Zcash recomienda pasar sus fondos a Ironwood | Nada |
| Recuperación ante la cuántica (ZIP 2005) | Notas recuperables desde ya (nota V3) | Zcash no la propone para Sapling | Habría que diseñarla |
| Curvas | Pallas/Vesta (nuevas en Zoka) | BLS12-381 + Jubjub (BLS12-381 ya está) | BLS12-381 |
| Código | Crate `orchard` 0.16.0 (30/09/2026), MIT o Apache-2.0 | `sapling-crypto` (no evaluado) | — |
| Teléfono ARMv7, medido | 40 s para armar la clave al abrir; 20 s por envío; 111 MB | No medido (el Groth16 actual de Zoka tarda 4–14 s) | — |

## Privacidad frente a una computadora cuántica

- El ZIP 2005 dice: *"The current privacy properties of the Orchard protocol
  would be retained against pre-quantum adversaries, and against post-quantum
  adversaries without knowledge of the notes' addresses."*
- El ataque conocido con la dirección en la mano es recuperar la `ivk` resolviendo
  un logaritmo discreto y descifrar las notas, porque su cifrado usa un
  intercambio de claves sobre curva. **En Zoka ese cifrado iría con el sobre
  X-Wing de la etapa 3**, que cierra ese camino.
- El crate ya diseña contra ese adversario: en los bundles sin transferencias
  cruzadas, la salida de valor cero que acompaña a un gasto lleva un ciphertext
  aleatorio para que *"no holder of the receiver's incoming viewing key --
  including a quantum adversary who recovered it from the published address --
  can use it to detect the spend"* (`orchard` 0.16.0, `src/builder.rs`).
- Queda un enlace que no afecta al grafo: un adversario cuántico que conozca dos
  direcciones diversificadas de la misma wallet puede ver que comparten `ivk`.
- **Hipótesis para la revisión externa:** Ironwood con el cifrado de notas X-Wing
  oculta grafo y montos ante un adversario cuántico aun conociendo las
  direcciones.
- **La validez sigue siendo clásica** (Pallas): un adversario cuántico podría
  falsificar pruebas o notas. Las notas recuperables del ZIP 2005 son el camino
  para migrar los fondos el día que haga falta.

## Decisión (confirmada por el usuario el 13/10)

**Zoka Quantic usa la construcción de Ironwood mediante el crate `orchard`, sin
modificar el circuito**, con el cifrado de notas reemplazado por el sobre X-Wing
y las notas en el formato recuperable. Se descarta el circuito propio. Sapling
queda como alternativa solo si Ironwood resultara inaceptable en los teléfonos
de los usuarios.

Consecuencias:

- Desaparecen CLSAG, las key images, `input_commitments`, `input_public_keys`, los
  caminos de inclusión públicos y Bulletproofs: el circuito prueba los valores.
- **Desaparece el trusted setup** de los gastos y con él la ceremonia del punto
  7.6, que hoy es el mayor riesgo de confianza del proyecto.
- Las direcciones de Quantic pasan a ser una dirección Orchard (43 bytes) más la
  clave X-Wing (1.216 bytes). La `zkq1` de Zenith sirve para probar el cifrado,
  no es el formato final.
- Zenith no adopta esto: sería migrar toda la cadena. Se prueba en una red de
  pruebas nueva de Quantic.
- En el teléfono hay que armar la clave de prueba en segundo plano al abrir la
  app (40 s en el equipo de pruebas; `halo2_proofs` 0.4 no la guarda en disco).

## Medición (13/10/2026)

[`crates/ironwood-bench`](../crates/ironwood-bench/README.md), datos sintéticos;
resultados en [benchmarks/2026-10-13-ironwood.json](benchmarks/2026-10-13-ironwood.json).
Bundle Ironwood de 2 acciones (Orchard rellena a 2 como mínimo):

| Equipo | Armar clave de prueba | Armar clave de verificación | Probar | Verificar | Prueba | Memoria |
|---|---|---|---|---|---|---|
| PC (x86_64, 4 hilos) | 2,8 s | 1,9 s | 2,0–2,1 s | 17 ms | 7.264 bytes | — |
| Galaxy A13 (ARMv7, 8 núcleos) | 39,7 s | 26,8 s | 19,4–20,2 s | 175 ms | 7.264 bytes | 111 MB |

Un teléfono de 64 bits debería andar bastante más rápido; no hay uno para medir.

## Próximos pasos

1. Leer la especificación de Orchard/Ironwood y las auditorías y la verificación
   formal publicadas, más allá de las notas de prensa.
2. Diseñar el reemplazo del cifrado de notas por X-Wing sin perder las
   comprobaciones de consistencia que hoy hace la clave efímera, y seguir el plan
   de Zcash con ML-KEM si lo publica.
3. Integrar `orchard` en un crate aparte del nodo (como `zoka-pq`), con pruebas de
   ida y vuelta de un envío completo.
4. Medir dentro de ZKAPriv (clave en segundo plano) y, si aparece, en un teléfono
   de 64 bits.
5. Revisión externa antes del génesis de Quantic.

## Fuentes

- [ZIP 2005: Ironwood Quantum Recoverability](https://zips.z.cash/zip-2005)
- [The Block, 28/07/2026: activación de Ironwood tras la falla de Orchard](https://www.theblock.co/post/409934/zcash-ironwood-upgrade-launching-new-shielded-pool-after-orchard-vulnerability)
- [Shielded Labs: Ironwood](https://shieldedlabs.net/ironwood/)
- [KuCoin: Ironwood NU6.3](https://www.kucoin.com/blog/zcash-ironwood-nu6-3-shielded-pool-migration)
- [Cryptonomist, 29/07/2026](https://en.cryptonomist.ch/2026/07/29/zcash-ironwood-upgrade-security/)
- [ironwood.live](https://ironwood.live/)
- [Crate `orchard` en crates.io](https://crates.io/crates/orchard) y su código 0.16.0
  (`src/builder.rs`, `src/note.rs`, `src/bundle.rs`)
