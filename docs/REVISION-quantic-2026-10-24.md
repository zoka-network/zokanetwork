# Estado del desarrollo de ZOKA QUANTIC — 24/10/2026

Revisión del código local, ramas disponibles, binario instalado y notas de
Claude. Base del nodo: `860e4d8e`, rama `quantic-direcciones-cortas`. El trabajo
criptográfico más reciente del prototipo STARK está en `de9ce1dd` (21/10);
el TLS híbrido de iroh, en `01001969`. Las ramas locales revisadas no contienen
una implementación posterior del pool STARK de transferencias.

**Conclusión:** la cadena Quantic, su cifrado postcuántico de notas, las firmas
híbridas de autoridad y la minería con XMRig ya están implementados. El trabajo
principal pendiente para la mainnet es reemplazar el gasto y sus pruebas
clásicas por el pool privado postcuántico completo. El prototipo existente
permite medir STARK, pero todavía no demuestra una transferencia.

## Versiones comprobadas

| Componente | Versión | Evidencia |
| --- | --- | --- |
| ZSilent | 3.0.5 | `zsilent-core/build.gradle.kts`, `releaseVersion` |
| Nodo incluido en ZSilent | 3.0.4 | Metadatos de `%LOCALAPPDATA%/ZSilent Core/app/resources/bin/zoka.exe` y notas del MSI 3.0.5 |
| Código del nodo y paquete CLI | 3.0.4 | `Cargo.toml` y `packaging/cli-miner/LEEME.md` |

La versión 3.0.5 del producto de escritorio incluye el nodo 3.0.4. No hace falta
renumerar el nodo para explicar esta diferencia.

## Trabajo ya construido

| Parte | Estado y código |
| --- | --- |
| Base postcuántica | ML-KEM-768, ML-DSA-65 y X-Wing; vectores de respuesta conocida y casos negativos en `crates/zoka-pq` |
| Cifrado privado de notas | X-Wing integrado a los envíos y escaneo, recuperación del emisor y notas compactas; `crates/zoka-quantic/src/note_encryption.rs` |
| Pruebas privadas actuales | Ironwood/Orchard 0.16 con Halo 2, validación de pruebas, firmas y balance; `builder.rs`, `verify.rs` y `state.rs` del mismo crate |
| Autoridad híbrida | Checkpoints con Ed25519 + ML-DSA-65 obligatorias en Quantic; firma/verificación híbrida de snapshots implementada, aunque no exigida en todo bootstrap; `src/hybrid_signature.rs`, `src/checkpoint_authority.rs`, `src/snapshot.rs` |
| Transporte de snapshots | iroh usa un proveedor que prefiere X25519MLKEM768; conserva compatibilidad con pares clásicos; `src/network/pq_tls.rs` |
| Cadena Quantic | Génesis de testnet, emisión, ASERT, selección por trabajo, mempool, reorganizaciones, P2P, API y wallet; `src/quantic` |
| Wallet de escritorio | ZSilent 3.0.5: envíos, recepción, recuperación, direcciones cortas y publicación automática, minería y sección Stratum |
| XMRig | Blob de PoW v2 compatible con RandomX `rx/0`, nonce en posición 39, servidor Stratum y pago a la dirección del login; `src/mining/pow.rs`, `src/quantic/stratum.rs` |
| Motor móvil | Motor Rust de Quantic y puente JNI en ZKAPriv; la interfaz de Android aún no lo utiliza |

Que el cifrado de notas sea híbrido no convierte a Halo 2, las firmas RedPallas
ni los compromisos de Orchard en postcuánticos. La validación actual sigue
usando `orchard::bundle::BatchValidator`. Las garantías de privacidad completas
también tienen hipótesis pendientes de revisión, documentadas en
[REVIEW-zoka-quantic.md](REVIEW-zoka-quantic.md), especialmente H1 y H2.

## Trabajo pendiente principal, en orden

1. **Cerrar la arquitectura del nuevo pool y el costo de sus pruebas.** El crate
   `crates/zoka-pq-pool` contiene un único ejecutable de medición: prueba
   permutaciones de Poseidon2 sobre KoalaBear con Plonky3. No tiene circuito de
   transferencia, entradas públicas de una transacción ni integración al nodo.
   Falta decidir y medir la agregación de pruebas por bloque o una alternativa
   explícita de tamaños y almacenamiento. Es una decisión que condiciona el
   circuito, el formato y la sincronización.
2. **Implementar una transferencia postcuántica completa.** Primero una entrada
   y dos salidas: propiedad de la nota, camino de Merkle, nullifier derivado
   correctamente, autorización de gasto dentro de la prueba, rangos de montos,
   conservación del saldo y comisión. Vincularla a los efectos de la transacción
   y su red. El diseño propone hashes en lugar de compromisos y firmas de curva;
   todavía falta escribir esas restricciones y sus pruebas negativas. Las
   semillas fijas del benchmark son para medición, no aleatoriedad de producción.
3. **Integrar el pool al protocolo.** Notas, claves/direcciones, árbol, formato de
   transacciones, verificador, wallet, mempool, bloques, coinbase, plantillas de
   minería y sincronización. Conservar la compatibilidad de trabajo de XMRig
   mientras se adapta la construcción del bloque, y volver a ensayarla. El
   cambio del pool privado no obliga por sí solo a reemplazar RandomX.
4. **Validar rendimiento y seguridad del protocolo completo.** Medir RAM y
   tiempos en PC y ARMv7, escaneo, transacciones completas y agregación. Probar
   doble gasto, inflación, cambios en el mempool, separación de redes,
   reorganizaciones y ataques con las claves clásicas comprometidas. Revisar
   parámetros de seguridad y obtener revisión criptográfica externa.
5. **Nueva testnet y cierre de mainnet.** El plan aprobado reinicia la testnet
   con el nuevo pool. Antes de mainnet: clientes compatibles, ensayos de varios
   nodos, recuperación y sincronización, génesis y parámetros finales, y revisión
   externa. Aún no hay evidencia de que estos criterios estén cerrados para el
   pool postcuántico.

Referencia de diseño:
[QUANTIC-POOL-POSTCUANTICO.md](QUANTIC-POOL-POSTCUANTICO.md).

## Otros pendientes, sin confundirlos con el circuito

- **Autenticación de snapshots:** el bootstrap de iroh solo exige la firma
  del snapshot si se pasa `--iroh-require-signed-snapshot`
  (`src/main.rs:11568`, `src/main.rs:19574`, `src/main.rs:19619`). Por omisión
  confía en la identidad iroh del seed del manifiesto; esa identidad es clásica,
  aunque el intercambio de claves pueda ser híbrido. El manifiesto de testnet
  tiene `snapshot_signers: []`. Se vuelven a validar los bloques al importarlos,
  por lo que esto no significa que se acepten bloques inválidos. Sí falta cerrar
  la política de autenticación postcuántica y configurar/publicar los firmantes
  correspondientes antes de considerarla obligatoria en toda sincronización.
- **Transporte:** libp2p sigue usando Noise clásico. iroh prefiere el intercambio
  híbrido, pero permite X25519 con pares clásicos: tener soporte híbrido no
  equivale a exigirlo en todas las sesiones. Falta definir y validar la política
  de transporte de mainnet y el alcance de las promesas sobre metadatos. La
  evolución de Tor depende además del proyecto Tor.
- **Android:** falta conectar la interfaz Kotlin al motor Quantic, completar
  persistencia cifrada, manejo de envíos pendientes, memos y pruebas en teléfonos.
  El motor actual no verifica la prueba de trabajo de los encabezados. Según
  la decisión anterior del usuario, ZKAPriv va después de la cadena y ZSilent.
- **Pools de minería:** faltan RPC `get_block_template`/`submit_block` para
  operadores externos, adaptación del software del pool y prueba de
  `xmrig-proxy`. Un P2Pool propio es otro desarrollo, propuesto para después de
  mainnet. El Stratum actual ya permite XMRig y paga cada bloque al minero
  correspondiente; no implementa reparto proporcional entre todos los mineros.
- **Arranque de dificultad:** ASERT ya está implementado. Falta cerrar para
  mainnet la hora de su ancla y revisar los parámetros: la testnet mostró el
  efecto de crear el génesis horas antes de comenzar a minar.
- **Propagación:** actualmente el minero selecciona hasta 96 acciones; el límite
  de consenso es 512 acciones de transferencias. Gossip tiene un techo de 1 MiB.
  El nuevo formato STARK requiere medir los límites completos, incluyendo la
  codificación y descargas directas, no deducir capacidad solo del tamaño de la
  prueba. El diseño antiguo simplifica esto como «bloque de 1 MB».

## Comprobación ejecutada en esta revisión

- `zoka-pq`: **14 pruebas pasadas**, incluidos vectores NIST/X-Wing.
- `zoka-quantic --lib`: **40 pruebas pasadas**, incluido el caso que entrega al
  atacante las claves de visión de Orchard y comprueba que no puede abrir el
  sobre sin la mitad X-Wing correspondiente.
- Nodo con `--features quantic --lib`: **26 pruebas pasadas** de firmas
  híbridas, autoridad de checkpoints, TLS/iroh y Stratum. Incluye negociación
  real de iroh y rechazo de checkpoints con solo la clave Ed25519 comprometida.
- **2 pruebas de extremo a extremo pasadas** con Halo 2 real:
  `a_chain_from_the_genesis_with_payments_change_and_mining` y
  `transactions_survive_their_wire_format_and_any_change_breaks_them`. Cubren
  pagos, cambio, coinbase, maduración, conservación de valor, doble gasto y
  alteración de transacciones. Total de esta revisión: **82 pruebas pasadas**.
  La compilación del nodo emitió un aviso preexistente `unused_mut` en
  `src/api/mod.rs:3404`.
- Benchmark STARK comprobado por Cargo y ejecutado con 256
  permutaciones: **199,6 KiB, 205,7 ms para probar y 14,5 ms para verificar**, con
  100 consultas. El evaluador del benchmark informa 102 bits bajo su modelo;
  no es una evaluación de seguridad del futuro protocolo completo. Con 56
  consultas da 121,4 KiB y 71 bits: reducir tamaño tiene un costo de seguridad.

El benchmark mide permutaciones aisladas; estos tiempos no son tiempos de una
transferencia real. Los ensayos históricos de XMRig constan en
[QUANTIC-POOL-DE-MINERIA.md](QUANTIC-POOL-DE-MINERIA.md). En esta revisión no se
ejecutó minería nueva ni una prueba con un teléfono.

## Documentación que todavía arrastra estados anteriores

No usar el número de casillas abiertas de las hojas antiguas como medida de
avance. Varias aún dicen que faltan las firmas híbridas, el perfil del nodo,
ZSilent o el lanzamiento de testnet, aunque el código y los releases posteriores
los contienen. También queda una referencia antigua a una ceremonia de setup,
que no corresponde al diseño elegido de Halo 2 ni al prototipo STARK.

El inventario criptográfico y la especificación de revisión siguen siendo
útiles, pero «cerrado» en una tabla interna no sustituye una revisión externa,
y el borrador del pool empieza diciendo «nada está construido» antes de describir
el benchmark que sí existe. Esta revisión distingue ese prototipo del circuito
de transferencias que todavía falta.
