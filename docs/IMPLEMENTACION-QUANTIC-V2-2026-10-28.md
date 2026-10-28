# Implementación de ZOKA QUANTIC STARK v2

28 de octubre de 2026. Rama `quantic-protocolo-postcuantico`.

Existe un circuito de transferencias privadas integrado al validador y un
nodo ejecutable para `zoka-quantic-regtest-v2`. La implementación incluye
wallet, pagos, minería local, sincronización de forks, persistencia, API y
autenticación ML-DSA del servidor. La
[guía de uso](QUANTIC-STARK-USO.md) permite ejecutar el flujo completo.

Esto amplía el benchmark de permutaciones que existía al iniciar la revisión.
No convierte automáticamente la testnet pública, ZSilent ni Android al nuevo
pool. La red pública completa solicitada todavía no está terminada.

## Código implementado

| Componente | Implementación |
| --- | --- |
| Transferencia | [zoka-pq-pool](../crates/zoka-pq-pool/src/lib.rs): propiedad derivada por hash, árbol Poseidon2, nullifiers, pertenencia, rango de 64 bits y conservación de saldo restringidos dentro del AIR; STARK con ocultamiento |
| Notas y wallet | Cifrado X-Wing y ChaCha20-Poly1305, dirección v2, recuperación para destinatario y emisor, memos de tamaño fijo |
| Reglas de cadena | Emisión máxima, recompensa exacta, comisiones, maduración, anclas recientes, límites de tamaño y rechazo de doble gasto |
| Nodo | [stark.rs](../src/quantic/stark.rs): integración con el validador real, PoW de regtest, RocksDB, reorganizaciones y mempool |
| Herramientas | [stark_cli.rs](../src/quantic/stark_cli.rs): creación/restauración cifrada, escaneo, saldo, reservas, pagos y archivos de bloques |
| API | [stark_api.rs](../src/quantic/stark_api.rs): HTTP loopback, límites antes de cargar cuerpos, protección de origen, estado, transacciones, bloques y plantillas |
| Transporte | [stark_net.rs](../src/quantic/stark_net.rs): iroh con intercambio híbrido obligatorio, clave ML-DSA fijada y firma ligada a cada conexión TLS, sincronización por lotes y ramas divergentes |
| Proceso de nodo | [stark_node.rs](../src/quantic/stark_node.rs): `serve`, `sync`, `broadcast`, identidad cifrada, minero y retransmisión con estado compartido |
| Endurecimiento existente | Snapshots híbridos obligatorios para Quantic, rechazo de degradación TLS y de archivos modernos truncados que antes podían activar XOR legado |

Las pruebas de gasto nuevas no usan las firmas sobre curvas de Orchard.
X-Wing sigue siendo un esquema híbrido e iroh conserva su identidad
convencional, complementada en este protocolo por la firma ML-DSA.

## Validación ejecutada

| Comprobación | Resultado observado |
| --- | --- |
| `cargo test --offline --release --manifest-path crates/zoka-pq-pool/Cargo.toml --lib -- --test-threads=1` | 10 pruebas pasadas, 99,83 s |
| `cargo test --offline --release --features quantic-pq --lib quantic:: -- --test-threads=1` | 48 pruebas pasadas, 354,66 s |
| Pruebas de biblioteca con filtros `snapshot::tests`, `network::pq_tls::tests`, `wallet::storage::tests` | 42 pruebas pasadas, 27,55 s |
| `cargo test --offline --release --features quantic-pq --bin zoka snapshot_ -- --test-threads=1` | 15 pruebas pasadas, 0,12 s |
| Pruebas del binario con filtro `mainnet_network_config` | 3 pruebas pasadas |
| Repetición de `quantic::stark_cli::tests` tras corregir el bloqueo en Windows | 4 pruebas pasadas, 10,83 s; incluye una regresión adicional |
| Regresión `startup_directory_policy` | Pasó: los comandos v2 conservan el directorio del invocador y los comandos anteriores mantienen su política |
| `cargo check --offline --features quantic-pq --tests` | Pasó |
| `cargo check --offline --lib` sin la feature nueva | Pasó |
| `cargo build --offline --release --features quantic-pq --bin zoka` | Pasó; ejecutable en `target/release/zoka.exe` |
| Ejecutable con dos procesos y wallet separada | Pago de 1 ZKA retransmitido, minado y recibido; ambos nodos en altura 3 |
| Biblioteca del pool para `armv7-linux-androideabi` y `aarch64-linux-android` | Ambos `cargo check --lib --target ...` pasaron en la sesión anterior; no son pruebas en teléfono |

Las pruebas del circuito incluyen inflación con hashes y salidas públicas
recalculados, testigos privados adulterados, destinatarios/red incorrectos,
doble gasto, expiración/maduración y codificaciones no canónicas. La forma
máxima 8→8 prueba montos próximos a `u64::MAX` y acarreos.

Las pruebas del nodo ejercitan pagos y minería mediante API, reorganización
y reapertura de RocksDB, intercambio QUIC por loopback, pin ML-DSA incorrecto,
replay de autenticación entre conexiones con las mismas identidades,
pruebas adulteradas y continuación de forks en lotes de un bloque. Se verificó
que la cancelación de una solicitud conserva ocupado el permiso de su trabajo
criptográfico hasta que ese trabajo termina.

La prueba posterior del ejecutable con dos procesos encontró un fallo de
bloqueo obligatorio de archivos en Windows: el comando de pago no podía
reabrir la wallet que acababa de bloquear. Se cambió a un archivo persistente
`.stark-lock`, con ruta canónica compartida también por la reserva pendiente.
Las cuatro pruebas focalizadas de wallet pasaron después de la corrección y
el ejecutable se volvió a compilar. En total se aprobaron **120 pruebas
distintas**; las tres pruebas existentes de wallet se repitieron junto a la
nueva regresión sin contarlas dos veces.

La repetición con procesos reales también pasó: A minó, B sincronizó desde A,
una copia separada de la cadena permitió construir el pago, `broadcast` lo
entregó a B, B lo retransmitió a A y el pago apareció minado y recibido al
sincronizar nuevamente desde B. Resultado:

- Cadena: `zoka-quantic-regtest-v2`; dos procesos locales sin relay.
- Transferencia: 1.000.000 átomos (1 ZKA); comisión: 200 átomos.
- Transacción serializada: 1.875.385 bytes, incluyendo sobres y prueba.
- Saldo disponible del destinatario: 1.000.000 átomos; saldo inmaduro: 0.
- Ambos nodos terminaron en altura 3 y fueron detenidos al finalizar la prueba.
- Txid: `61ce502a6b2c1f162690f4cba8ff950d367d93df8eabdc7daedd002d48eacd67`.

Al reabrir las dos bases por sus rutas absolutas se comprobó la misma altura
y hash `2c56194540a4c9a61cb338d2f2b5fa15327acf0e0b016c76e2757813a99f1120`.
La comprobación con rutas relativas detectó que el arranque heredado cambiaba
el directorio de trabajo. Se corrigió para que los comandos v2 conserven el
directorio del invocador y omitan la migración automática y la lectura de
configuración pública. Su regresión pasó; no fue una pérdida de los datos
originales. La repetición del ejecutable con rutas relativas también pasó:
ambos nodos llegaron a altura 3 y conservaron el mismo hash al reabrirse,
`0f2773d1ab1480dd7303f6a46704c5c0b39a1799a860e36a2950f50465c3e514`.
El pago de esa repetición fue
`e7c489a0546941191142f83e85b509b0d9638f77e3be09b46602a05d0c7f4631`,
de 1.875.258 bytes, con el mismo monto y comisión.

Los logs y datos temporales están bajo `target/quantic-validation-2026-10-28/`,
fuera del control de versiones. Se usaron wallets de prueba nuevas.

Los avisos de compilación observados son `unused_mut` en `src/api/mod.rs` y
`dead_code` de `shuffled_peer_addrs` en `src/main.rs`, preexistentes y ajenos
a este cambio.

## Alcance criptográfico y operativo

La configuración y sus hipótesis se describen en la
[revisión asistida](REVISION-STARK-CODIGO-2026-10-25.md). El estimador de
Plonky3 dio 128 bits de soundness clásico bajo la hipótesis de hash elegida.
Ese resultado no constituye una demostración de 128 bits de seguridad
cuántica, una prueba formal de conocimiento cero de toda la composición ni
una auditoría externa.

Las mediciones anteriores dieron 1.868.731 bytes para una prueba 1→2 y
8.341.990 bytes para una 8→8. La segunda tardó 87,15 s al medirla mientras
había otras compilaciones. El LDE principal de la forma máxima se estima en
677,38 MiB antes de sumar otras trazas y buffers; no es una medida del pico
de RAM. No se verificó la viabilidad del prover en un teléfono.

La privacidad oculta claves de gasto, propietarios, notas, caminos del árbol
y montos privados frente a quien no tenga las claves correspondientes.
Comisiones, emisión, cantidades de entradas/salidas, compromisos, nullifiers,
alturas, tiempos y metadatos de conexión siguen siendo observables. Una
recompensa pública con una sola salida permite inferir el monto de esa nota.

El adaptador conserva una copia completa del ledger por bloque. La wallet
escanea la cadena canónica sin podar. Ni ese almacenamiento ni la propagación
de pruebas de varios megabytes se validaron para una red pública sostenida.

## Integraciones que aún faltan

| Área | Límite concreto del código actual |
| --- | --- |
| Perfiles públicos v2 | Los adaptadores fijan red 3 y cadena regtest. Requieren reglas de activación, génesis fijo y perfiles/manifiestos; la selección de envío por altura debe cambiar para dificultad variable |
| XMRig | Regtest v2 usa PoW CPU. El Stratum existente construye coinbase y trabajos del pool Orchard. Falta integrar trabajos STARK, direcciones v2, RandomX/ASERT y comprobar shares, extranonce y bloques de XMRig |
| ZSilent | `WalletService`, `NodeController` y la configuración siguen los comandos, prefijos y contratos v1. Falta conectar el flujo v2 y sus modelos de saldo/historial |
| Android | `mobile_quantic`, JNI y las pantallas no están conectados al pool STARK. Compilar para ARM no sustituye esa integración ni la medición de memoria en dispositivos |
| Almacenamiento y revisión | La representación por copias completas necesita reemplazo para escalar; faltan pruebas prolongadas, fuzzing y revisión criptográfica independiente |

La misma frase aplicada a esquemas diferentes no migra las notas ni los saldos.
No se publicó un instalador, no se activó un génesis público nuevo y no se
sustituyeron los JNI existentes como parte de este trabajo.
