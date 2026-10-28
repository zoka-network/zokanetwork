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
| Perfiles públicos v2 | Desde la continuación de abajo existe el perfil `zoka-quantic-testnet-v2` (RandomX, ASERT, `zqtest2`), con manifiesto, intercambio de pares y relays (sección final). Falta el génesis fijo y la primera semilla (decisiones de lanzamiento), y revisar la selección de envío por altura para dificultad variable |
| XMRig | Hecho en local: Stratum sobre plantillas STARK, probado con XMRig 6.25.0 real (ver abajo). Falta el RPC `get_block_template`/`submit_block` para software de pools y probarlo con varios mineros y xmrig-proxy |
| ZSilent | `WalletService`, `NodeController` y la configuración siguen los comandos, prefijos y contratos v1. Falta conectar el flujo v2 y sus modelos de saldo/historial |
| Android | `mobile_quantic`, JNI y las pantallas no están conectados al pool STARK. Compilar para ARM no sustituye esa integración ni la medición de memoria en dispositivos |
| Almacenamiento y revisión | Desde el formato final (abajo) el estado por bloque es compacto. Faltan reducir el tamaño de las pruebas, guardar los bloques en binario, pruebas prolongadas, fuzzing y revisión criptográfica independiente |

## Continuación del 28/10: testnet v2 con RandomX y XMRig

Rama `quantic-stark-xmrig`, sobre `quantic-protocolo-postcuantico`. Retoma el
paso que había quedado anunciado («conectar la minería de XMRig al nuevo
formato privado»).

- **Perfiles** (`stark::Profile`): regtest v2 (red 3, PoW CPU, dificultad fija)
  y testnet v2 (`zoka-quantic-testnet-v2`, red 2, RandomX y la ASERT de la v1).
  El perfil sale de la marca de la base y viaja en `NodeState`. Wallets,
  identidades de nodo y descriptores guardan su cadena y se rechazan en otra;
  el mensaje que el servidor firma con ML-DSA incluye la cadena.
  `--network regtest|testnet` (regtest por omisión, como antes). La testnet v2
  no entra en `network_for_chain`: hereda solo la dificultad, para que nunca
  le llegue el génesis embebido de la v1. Su génesis es
  `QUANTIC_PQ_TESTNET_GENESIS = None` hasta el lanzamiento.
- **Stratum**: el servidor de la v1 pasó a tener un `Backend` (login → dirección
  de cobro, plantilla, bloque encontrado). El de Ironwood conserva su
  comportamiento; `stark_stratum.rs` arma plantillas STARK con un permiso de
  trabajo, hasta 4 direcciones y refresco cada 60 s, e importa los bloques por
  el mismo candado que API, red y minero. `quantic-pq-node serve --stratum`.
  En la testnet `--mine-to` solo paga logins que no son direcciones.
- **Fallas del Stratum compartido que destapó XMRig real** (también afectaban a
  la v1, aunque ahí no se veían): con la cadena vacía la punta guardada es `"0"`
  y cada plantilla se daba por vieja, una prueba por segundo; mientras una
  sesión esperaba la respuesta de un acierto se acumulaban avisos y XMRig
  recibía el mismo trabajo dos veces y se reconectaba; y el verificador
  RandomX recién se armaba con el primer acierto (unos 37 s de espera).
  Arreglados, con una regresión que falla sin el primer arreglo.
- **XMRig 6.25.0 real** contra un nodo de testnet v2 local y aislado, wallet
  descartable, en `target/stark-xmrig-2026-10-28/xmrig-run5`: 20 aciertos
  aceptados y ninguno rechazado, respuestas de 5 ms (unos 240 ms los que eran
  bloque), 4 bloques canónicos, un trabajo por altura y sin reconexiones; la
  wallet ve 5.000.000 ZKA disponibles y 36 inmaduros. Unos 685 H/s con 2 hilos.
  Las corridas 2 a 4 son las que mostraron las fallas.

## Formato final antes del génesis (28/10, rama `quantic-v2-formato-final`)

Tres cambios de consenso, especificados en
[QUANTIC-V2-FORMATO.md](QUANTIC-V2-FORMATO.md):

- **`rho` único por nota.** Quien enviaba elegía `rho` al azar y el circuito no
  lo ataba: dos notas con el mismo `rho` compartían nullifier («Faerie Gold»).
  Ahora sale del primer nullifier de la transferencia (forzado por el AIR) o de
  la altura del coinbase, y `rcm` da el ocultamiento.
- **Coinbase sin prueba STARK:** publica su valor y `H(owner ‖ rcm)`; el nodo
  recalcula el compromiso. De ~947 KB a menos de 4 KB por bloque.
- **Estado compacto:** `state_root` compromete altura, emisión, raíz y tamaño
  del árbol y un acumulador de nullifiers; por bloque se guarda un registro de
  cientos de bytes, y las ramas se validan con esos registros y el índice
  canónico de nullifiers. La wallet reconstruye el árbol con `replay`.

Validado: 14 pruebas del pool y 105 del nodo en release; pago por CLI de punta
a punta (saldo, emisión y rechazo del doble gasto); XMRig 6.25.0 real, 25
aciertos aceptados y 0 rechazados, 5 bloques canónicos, base de 119 KB para 5
bloques (antes 7,4 MB para 4).

## Red pública (28/10, rama `quantic-v2-red-publica`)

- **TLS:** la exigencia de X25519MLKEM768 pasó del proveedor de rustls a un hook
  `after_handshake` de iroh. El proveedor también sirve al HTTPS de relays y
  pkarr, y los relays de n0 rechazaban un ClientHello solo híbrido
  (`HandshakeFailure`). Así, un nodo con relays nunca quedaba en línea, y pasaba
  lo mismo en la v1 de Quantic. Las conexiones entre nodos siguen siendo solo
  híbridas, en los dos sentidos.
- **Descriptor v3:** fija la huella blake3 de 32 bytes de la clave ML-DSA-65.
  Una semilla del manifiesto es su EndpointId y esa huella.
- **Manifiesto** `docs/quantic-testnet-v2-network.v1.json`, embebido en el
  binario. Está en pre-génesis y sin semillas.
- **Intercambio:**
  - `ANNOUNCE` solo acepta el endpoint de la propia conexión;
  - `PEERS` devuelve hasta 16 pares que el nodo llamó y autenticó en el último
    día, o pares fijos con sesión, con una respuesta por conexión y minuto;
  - cada par aprendido se llama y se autentica antes de entrar en la tabla;
  - con relays se guardan solo los EndpointId.
- **Nodo:**
  - sesiones fijas con `--peer` y las semillas;
  - hasta 8 sesiones con pares aprendidos;
  - tabla de 256 conocidos y 64 pendientes, guardada en la base;
  - 64 sesiones entrantes;
  - relay N0 por omisión en la testnet.

Validado:
- 19 pruebas de red y 238 de `quantic::`, `state::chain`, `economics` y
  `network::`, en release;
- tres procesos en loopback: el nodo que conocía un solo par encontró al
  tercero y siguió su cadena;
- dos procesos por los relays reales de n0, con la semilla fijada solo por id y
  huella.

Falta:
- Tor (haría falta un transporte TCP);
- un modo de semilla que solo presente pares;
- un relay propio para la mainnet;
- defensa contra quien ocupa las sesiones entrantes con muchos nodos reales.

La misma frase aplicada a esquemas diferentes no migra las notas ni los saldos.
No se publicó un instalador, no se activó un génesis público nuevo y no se
sustituyeron los JNI existentes como parte de este trabajo.
