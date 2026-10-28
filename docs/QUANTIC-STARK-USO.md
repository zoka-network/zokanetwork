# ZOKA QUANTIC STARK v2: uso local

Actualizado el 28 de octubre de 2026. Esta guía corresponde a la implementación
de las cadenas **`zoka-quantic-regtest-v2`** y **`zoka-quantic-testnet-v2`**,
activadas con la feature `quantic-pq`.
Incluye transferencias privadas con STARK, wallet cifrada, consenso y persistencia,
nodo con transporte autenticado, sincronización y API local. Es una cadena de
pruebas separada: estos comandos no despliegan una mainnet ni convierten la
testnet Orchard existente.

Las opciones se contrastaron con [stark_cli.rs](../src/quantic/stark_cli.rs),
[stark_node.rs](../src/quantic/stark_node.rs),
[stark_api.rs](../src/quantic/stark_api.rs),
[stark_net.rs](../src/quantic/stark_net.rs) y
[stark_stratum.rs](../src/quantic/stark_stratum.rs). La
[revisión del pool STARK](REVISION-STARK-CODIGO-2026-10-25.md) explica las
hipótesis criptográficas y los límites de la evidencia. Esta guía no constituye
un informe de ejecución de las nuevas pruebas de API o transporte.

## Compilar

Desde la raíz del repositorio:

```powershell
cargo build --release --bin zoka --features quantic-pq
$Zoka = ".\target\release\zoka.exe"
& $Zoka quantic-pq help
& $Zoka quantic-pq-node help
```

Se necesitan las herramientas nativas del proyecto: en Windows, Rust para MSVC,
Visual Studio Build Tools para C++, CMake y el entorno de Clang/libclang requerido
por RocksDB. La feature predeterminada `randomx` sigue incluida en ese build.
Para un binario dedicado al regtest se puede compilar con
`--no-default-features --features quantic-pq`; esto no elimina los requisitos
nativos de RocksDB.

### Dos cadenas v2

| `--network` | Cadena | Prueba de trabajo | Direcciones | Minería |
| --- | --- | --- | --- | --- |
| `regtest` (por omisión) | `zoka-quantic-regtest-v2` | CPU de pruebas (SHA3), dificultad fija | `zqregtest2…` | Minero interno (`--mine-to`) o `quantic-pq mine` |
| `testnet` | `zoka-quantic-testnet-v2` | RandomX (`rx/0`) con la ASERT de la testnet v1 | `zqtest2…` | XMRig por el Stratum del nodo (`--stratum`) |

La testnet v2 es la que reemplazará a la v1 cuando se lance. **Todavía no tiene
génesis fijo** (`QUANTIC_PQ_TESTNET_GENESIS` en `economics.rs` vale `None`):
hasta que el lanzamiento lo fije en el binario, quien mina el bloque 1 arma su
propia cadena local con esas reglas y se lleva el premine de esa cadena. No hay
semillas ni manifiesto público de la v2. La API y `serve` informan
`xmrig_compatible` según la cadena.

Una base, una wallet y una identidad de nodo guardan su cadena y nunca se
abren como otra. `--network` elige la cadena de una base **nueva**; una base
existente conserva la suya, y pedir otra con `--network` es un error. La
wallet de un envío decide su cadena. Un descriptor de par nombra la cadena que
sirve, y esa cadena forma parte del mensaje que firma el servidor con ML-DSA.

Los ejemplos siguientes son PowerShell. Cada terminal debe estar en la raíz
del repositorio y definir `$Zoka` como arriba. En Linux se usa el mismo conjunto
de argumentos con `./target/release/zoka` y las rutas propias del sistema.

## Archivos, contraseña y wallets

Use un directorio nuevo, por ejemplo `stark-lab`. Todos los comandos exigen rutas
explícitas; los ejemplos no usan una wallet existente. Los archivos de salida
se crean sin sobrescribir archivos previos.

Para cargar una contraseña sin escribirla literalmente en el historial de
PowerShell:

```powershell
$env:ZOKA_PQ_PASSWORD = [System.Net.NetworkCredential]::new("", (Read-Host "Contraseña de prueba" -AsSecureString)).Password
& $Zoka quantic-pq wallet-create --wallet .\stark-lab\alice.wallet.json
& $Zoka quantic-pq wallet-create --wallet .\stark-lab\bob.wallet.json
$Alice = (& $Zoka quantic-pq address --wallet .\stark-lab\alice.wallet.json).Trim()
$Bob = (& $Zoka quantic-pq address --wallet .\stark-lab\bob.wallet.json).Trim()
```

Las dos wallets del ejemplo usan la contraseña cargada. `address` solo lee la
dirección pública y no necesita contraseña. Las operaciones privadas de wallet
aceptan `--password-stdin` como alternativa a la variable de entorno: leen una
línea desde la entrada estándar; esa opción no implementa un prompt que oculte
el eco de la terminal. El nodo `serve` usa `ZOKA_PQ_PASSWORD`.

`wallet-create` no imprime semilla ni frase de recuperación. Haga una copia del
archivo cifrado y conserve su contraseña. Las semillas se guardan con Argon2 y
AES-256-GCM, formato `ZOKAENC1`; la dirección y el nombre de la cadena permanecen
visibles en el JSON. Al terminar la sesión que usa la contraseña:

```powershell
Remove-Item Env:ZOKA_PQ_PASSWORD
```

La restauración desde una frase BIP-39 **de este esquema v2** se realiza así:

```powershell
& $Zoka quantic-pq wallet-restore --wallet .\stark-lab\restored.wallet.json --phrase-stdin
```

El comando lee de stdin 24 palabras inglesas, equivalentes a 256 bits de
entropía. Con `--password-stdin`, la primera línea contiene la contraseña y la
segunda, la frase. No incluya la frase en argumentos ni en el historial.

Las wallets usan el esquema `zoka-quantic-stark-wallet-v2`; las identidades de
nodo usan `zoka-quantic-stark-node-identity-v2`. Son archivos diferentes. Las
direcciones completas comienzan por `zqregtest2` (regtest) o `zqtest2`
(testnet) y contienen las claves públicas necesarias para cifrar. Una versión
abreviada de la dirección no sirve como destinatario. El prefijo `zq2` de la
mainnet existe en la biblioteca, pero ningún comando abre esa cadena. Para una
wallet de la testnet:

```powershell
& $Zoka quantic-pq wallet-create --network testnet --wallet .\stark-lab\minera.wallet.json
```

**No existe migración de saldo ni de wallet v1 a v2 mediante estos comandos.**
La misma frase aplicada a otro esquema no implica la misma cuenta. No cambie
manualmente el esquema de un JSON. Las bases de datos v2 llevan su propio
marcador de cadena y rechazan una base existente de otra cadena o sin ese
marcador.

## Preparar fondos y arrancar dos nodos

Antes de abrir el nodo A, mine tres bloques de prueba en su base:

```powershell
1..3 | ForEach-Object { & $Zoka quantic-pq mine --db .\stark-lab\node-a --to $Alice }
& $Zoka quantic-pq balance --db .\stark-lab\node-a --wallet .\stark-lab\alice.wallet.json
```

Un ZKA equivale a 1.000.000 de átomos. Las recompensas ordinarias de regtest
maduran dos alturas después de su creación; la emisión inicial del bloque 1
se incorpora directamente al árbol. `balance` separa `available_atoms`,
`immature_atoms` y `reserved_atoms`.

En la terminal A, manteniendo la contraseña de su identidad en el entorno:

```powershell
& $Zoka quantic-pq-node serve --db .\stark-lab\node-a --identity .\stark-lab\node-a.identity.json --api 127.0.0.1:9140 --descriptor-out .\stark-lab\node-a.peer.json --mine-to $Alice --interval 120
```

`serve` crea una identidad cifrada si no existe o abre la existente con la
contraseña. La identidad conserva las claves del nodo entre reinicios.
`--mine-to` activa el minero interno: intenta minar al arrancar y vuelve a
intentarlo después de cada intervalo indicado, en segundos. Sin esa opción,
el nodo valida y distribuye datos pero no mina. Ctrl+C inicia el cierre.

El descriptor `node-a.peer.json` es público: contiene chain ID, identidad iroh,
direcciones de conexión y clave pública ML-DSA-65 del servidor. Compártalo por
un canal autenticado; el cliente confía en la clave fijada allí. No comparta el
archivo `node-a.identity.json`. En un reinicio, use otro nombre para
`--descriptor-out`, porque el comando no sobrescribe archivos, y distribuya el
descriptor nuevo: el puerto de transporte puede cambiar.

En otra terminal, cargue `ZOKA_PQ_PASSWORD` para la identidad del nodo B y ejecute:

```powershell
& $Zoka quantic-pq-node serve --db .\stark-lab\node-b --identity .\stark-lab\node-b.identity.json --api 127.0.0.1:9141 --descriptor-out .\stark-lab\node-b.peer.json --peer .\stark-lab\node-a.peer.json
```

El nodo B sincroniza desde A, comparte bloques y retransmite las transacciones
de su mempool. Se pueden indicar hasta ocho opciones `--peer`; los ciclos de
sincronización trabajan en lotes de hasta 64 bloques y se repiten cada cinco
segundos. La búsqueda de ancestro común permite procesar ramas divergentes;
cada bloque recibido pasa por la validación local y la selección de cadena por
trabajo acumulado. Un bloque válido de una rama todavía menos pesada puede
quedar almacenado sin cambiar la punta canónica.

Por defecto se usa transporte directo sin relays. El descriptor incluye
sockets loopback para que los dos nodos del ejemplo funcionen en la misma PC.
Para conectar equipos distintos se necesitan direcciones alcanzables, o la
opción explícita `--relay=N0` tanto al servir como al conectar. Con N0 el nodo
espera que el relay esté disponible antes de publicar su descriptor. Eso usa
infraestructura pública de transporte; la cadena sigue siendo regtest y el
relay no proporciona anonimato.

Las conexiones exigen intercambio TLS híbrido postcuántico. Además, el
servidor firma con ML-DSA-65 un mensaje ligado al exporter de esa conexión TLS,
al protocolo, a la cadena y a las identidades de ambos extremos. El cliente
comprueba la clave fijada en el descriptor. La identidad Ed25519 de iroh por
sí sola no es la garantía postcuántica de este protocolo. El ALPN separado es
`/zoka/quantic-stark/2`.

## Sincronizar una copia para la wallet y enviar

RocksDB abre una base por proceso. Mientras A o B estén activos, use otra base
para consultar saldo o construir pagos. En una tercera terminal:

```powershell
& $Zoka quantic-pq-node sync --db .\stark-lab\wallet-mirror --peer .\stark-lab\node-a.peer.json
& $Zoka quantic-pq balance --db .\stark-lab\wallet-mirror --wallet .\stark-lab\alice.wallet.json
$Bob = (& $Zoka quantic-pq address --wallet .\stark-lab\bob.wallet.json).Trim()
& $Zoka quantic-pq send --db .\stark-lab\wallet-mirror --wallet .\stark-lab\alice.wallet.json --to $Bob --amount-atoms 1000000 --memo "prueba privada" --out .\stark-lab\alice-to-bob.tx
& $Zoka quantic-pq-node broadcast --peer .\stark-lab\node-a.peer.json --tx .\stark-lab\alice-to-bob.tx
```

Esta terminal necesita la contraseña de Alice para `balance` y `send`.
`sync` y `broadcast` no necesitan una wallet ni identidad privada del servidor.
`sync` importa como máximo 8192 bloques por invocación; `--max-blocks N` permite
entre 1 y 1.000.000. Si se alcanza el presupuesto antes de completar el trabajo,
se puede repetir el comando: los bloques validados ya están en la base.

`send` construye y verifica la prueba, reserva las entradas y exporta una
transacción binaria. No la transmite automáticamente. Cobra la comisión
convencional calculada por el protocolo, que aparece como `fee_atoms` en su
salida. El memo admite hasta 512 bytes UTF-8. La selección admite hasta ocho
entradas maduras; un saldo total suficiente puede requerir más entradas de
las admitidas. El pago caduca veinte alturas después de la altura de la copia
local usada para crearlo, por lo que conviene sincronizar inmediatamente antes.

`broadcast` confirma la admisión en el mempool del nodo, no la inclusión en un
bloque. Una vez que el minero haya incluido el pago, actualice la copia y consulte:

```powershell
& $Zoka quantic-pq-node sync --db .\stark-lab\wallet-mirror --peer .\stark-lab\node-a.peer.json
& $Zoka quantic-pq balance --db .\stark-lab\wallet-mirror --wallet .\stark-lab\bob.wallet.json
```

La contraseña debe corresponder a la wallet consultada. El ejemplo inicial dio
a ambas la misma contraseña únicamente por comodidad.

La reserva se guarda cifrada junto a la wallet, con sufijo `.stark-pending`.
Las operaciones privadas usan su ruta canónica y un archivo `.stark-lock`
separado y persistente: permite leer la wallet en Windows mientras impide que
dos escritores modifiquen sus reservas al mismo tiempo. No elimine ese archivo
durante una operación.
Solo se admite un pago pendiente por wallet. Para recuperar exactamente la
misma transacción después de un fallo de exportación:

```powershell
& $Zoka quantic-pq pending-export --wallet .\stark-lab\alice.wallet.json --out .\stark-lab\recovered-payment.tx
```

`quantic-pq abandon --wallet FILE` elimina la reserva local. **No revoca un pago
ya exportado**: ese pago todavía puede entrar en un bloque hasta caducar o
hasta que una de sus entradas sea gastada. Sincronice antes de decidir volver
a gastar fondos reservados.

## Minar la testnet v2 con XMRig

En la testnet v2 el nodo no mina solo: abre un Stratum y XMRig (o cualquier
minero `rx/0` que hable el protocolo de pools de Monero) mina sus plantillas.
El protocolo es el mismo de la testnet v1 (blob de 76 bytes, nonce de 32 bits
en la posición 39 y extranonce propio por conexión); cambian la plantilla, que
lleva el coinbase con su prueba STARK, y el bloque encontrado, que entra por la
misma validación que uno recibido.

```powershell
$Minera = (& $Zoka quantic-pq address --wallet .\stark-lab\minera.wallet.json).Trim()
& $Zoka quantic-pq-node serve --network testnet --db .\stark-lab\testnet-a --identity .\stark-lab\testnet-a.identity.json --api 127.0.0.1:9140 --descriptor-out .\stark-lab\testnet-a.peer.json --stratum 127.0.0.1:3333
```

En otra terminal:

```powershell
xmrig.exe -o 127.0.0.1:3333 -u $Minera -p x -a rx/0
```

- El login de XMRig (`-u`) es la **dirección v2 completa** de la wallet que
  cobra. Una dirección de otra red, una de la v1 (`zqtest1…`) o una corta se
  rechazan con un error, para no minar para otro sin saberlo. Con
  `--mine-to DIRECCION` en `serve`, un login que no es una dirección (el
  nombre de un equipo) cobra a esa.
- `--mine-to` sin `--stratum` es un error en la testnet: el minero interno
  resuelve una plantilla hasta el final con un hilo y es para la regtest.
  `--stratum` en la regtest también es un error: XMRig calcula RandomX y la
  regtest usa otra prueba de trabajo.
- Cada dirección de cobro necesita su plantilla. El coinbase no lleva prueba
  ([formato](QUANTIC-V2-FORMATO.md)), así que una plantilla cuesta las reglas
  del ledger y volver a verificar las transferencias del mempool que incluye.
  El nodo atiende hasta 16 direcciones a la vez, rehace cada plantilla al
  cambiar la punta o cada 30 segundos y la arma con uno de los dos permisos de
  trabajo criptográfico que comparte con la API y la red.
- Los aciertos son 16 veces más fáciles que un bloque; el nodo recalcula el
  hash de cada uno y rechaza los que no coinciden, los repetidos y los de
  trabajos viejos. Al encontrar un bloque imprime una línea
  `{"stratum_block":…}` y lo importa; los pares lo reciben en su ciclo.
- `GET /status` incluye `stratum`: dirección, mineros conectados, aciertos
  aceptados y rechazados y bloques encontrados.
- Al abrir el Stratum el nodo arma su verificador de RandomX: el dataset de
  2 GB, uno por proceso y época, que también valida los bloques. En la PC de
  pruebas (i5-6400) tarda unos 37 segundos. Un acierto que llega antes espera
  a que termine; después cada uno se responde en milisegundos.

Probado el 28/10 con XMRig 6.25.0 oficial (2 hilos, sin huge pages, unos
685 H/s) contra un nodo de la testnet v2 local y aislado, con el formato
final: 25 aciertos aceptados y ninguno rechazado, respondidos en 5 a 15 ms, y
5 bloques canónicos que pagaron a la wallet del login. Cada trabajo nuevo salió
en menos de un segundo después del bloque anterior. Con la dificultad en el
piso, un acierto que llega en ese intervalo puede ser otro bloque de la misma
altura: queda guardado como rama lateral (`"canonical":false`).

El Stratum escucha en la dirección que se le pida. Abrirlo a la red local
(`0.0.0.0:3333`) es decisión de quien corre el nodo: cualquiera que llegue al
puerto puede pedir plantillas.

Windows Defender detecta XMRig como `Trojan:Win64/HashvaultMiner.A` y lo cierra
a los pocos segundos; una exclusión de carpeta no alcanza. Ver
[QUANTIC-POOL-DE-MINERIA.md](QUANTIC-POOL-DE-MINERIA.md).

## Minería e intercambio de bloques sin un nodo activo

Con la base indicada cerrada en otros procesos se puede incluir un pago
exportado, minar y guardar el bloque resultante:

```powershell
& $Zoka quantic-pq mine --db .\stark-lab\node-a --to $Alice --tx .\stark-lab\alice-to-bob.tx --out .\stark-lab\mined-block.json
& $Zoka quantic-pq export-block --db .\stark-lab\node-a --height 1 --out .\stark-lab\block-1.json
& $Zoka quantic-pq import-block --db .\stark-lab\offline-copy --in .\stark-lab\block-1.json
& $Zoka quantic-pq status --db .\stark-lab\offline-copy
```

El primer comando es una alternativa al minero de `serve`: la transacción debe
seguir sin confirmar, ser válida y no haber caducado. Para reconstruir una
copia por archivos, importe todos los bloques en orden desde el bloque 1.
`import-block` valida prueba de trabajo, estructura, pruebas STARK, estado y
reglas de cadena. Exportar/importar JSON no autoriza a omitir esas verificaciones.

## API HTTP local

`serve` abre por defecto `http://127.0.0.1:9140`. Solo permite bind loopback.
Comprueba Host y origen de solicitudes de navegador y rechaza solicitudes
cross-site; no ofrece CORS general. No es una API con autenticación de usuarios
para exponer directamente a Internet.

| Operación | Solicitud | Resultado |
| --- | --- | --- |
| Estado | `GET /status` | Cadena, altura, hash, emisión, cantidad de notas y mempool, algoritmo, límites y estado del Stratum |
| Bloque canónico | `GET /blocks/ALTURA` | JSON del bloque; 404 si no existe |
| Admitir pago | `POST /transactions`, `application/octet-stream` | Transacción v2 binaria; 202 con txid si se acepta |
| Plantilla de minería | `GET /mining/template?address=DIRECCION` | Bloque candidato, blob, target, seed y posición del nonce |
| Admitir bloque | `POST /blocks`, `application/json` | Valida y almacena el bloque; conserva ramas válidas según fork choice |

Ejemplos con el nodo A activo:

```powershell
Invoke-RestMethod http://127.0.0.1:9140/status
Invoke-RestMethod http://127.0.0.1:9140/blocks/1
$Alice = (& $Zoka quantic-pq address --wallet .\stark-lab\alice.wallet.json).Trim()
$AddressQuery = [Uri]::EscapeDataString($Alice)
Invoke-RestMethod "http://127.0.0.1:9140/mining/template?address=$AddressQuery"
curl.exe --fail-with-body -H "Content-Type: application/octet-stream" --data-binary "@stark-lab/alice-to-bob.tx" http://127.0.0.1:9140/transactions
```

El último comando es una alternativa a `broadcast`, no un paso obligatorio
posterior: un pago ya conocido puede ser rechazado como duplicado. La plantilla
devuelta todavía necesita minarse; el `block` no es automáticamente un bloque
aceptable. Para enviar un bloque ya minado se usa `POST /blocks` con su JSON.

API, red y minero comparten el control de mutaciones y el límite de dos trabajos
criptográficos. La API puede responder 503 cuando están ocupados; un timeout
del cliente no libera anticipadamente el permiso de un verificador que sigue
ejecutándose. Las transacciones admiten hasta 16 MiB; el bloque admite hasta
32 MiB de transacciones binarias, con hasta 16 transferencias más coinbase.
La envoltura JSON del bloque tiene un límite de 65 MiB por la codificación hex.
El mempool tiene un límite de 128 MiB. Son límites máximos, no una promesa de
rendimiento ni de memoria constante por debajo de esos tamaños.

## Qué es privado y qué se observa

| Dato | Tratamiento en v2 |
| --- | --- |
| Clave de gasto, notas de entrada y caminos de pertenencia | Testigo privado de la STARK; no se publican |
| Dueño y monto de las notas, memo | No se publican en claro; se usan compromisos y sobres cifrados que el destinatario puede descifrar |
| Relación entre un nullifier y la nota concreta gastada | Se demuestra sin publicar el índice ni el camino de esa nota |
| Ancla del árbol, nullifiers y compromisos de salida | Públicos para validar pertenencia, estado y evitar doble gasto |
| Comisión, emisión de coinbase y efectos públicos de la transacción | Públicos y ligados a la prueba |
| Número de entradas y salidas, tamaños, txid y sobres cifrados | Públicos |
| Alturas, tiempos, prueba de trabajo y cabeceras | Públicos |
| IP, identidad del peer y patrones de conexión/transmisión | No los oculta la STARK ni el relay; permiten observación y posibles correlaciones |

El cifrado de notas usa X-Wing, combinación de ML-KEM-768 y X25519, junto con
ChaCha20-Poly1305. La propiedad de fondos, pertenencia y conservación de saldo
se prueban con el circuito STARK basado en hashes; esta ruta de gasto no usa
las firmas sobre curvas de Orchard. Eso no significa que todos los componentes
de la aplicación hayan dejado de usar curvas: X-Wing es híbrido e iroh conserva
su identidad convencional, reforzada aquí por la autenticación ML-DSA descrita.

Conocer una dirección por fuera de la cadena, controlar el equipo de una wallet
o correlacionar tráfico queda fuera de la ocultación del testigo. «Privado por
defecto» describe los datos de las notas y sus gastos; no significa que cada
metadato de la red o del consenso sea secreto.

El cifrado tampoco impide deducir un dato a partir de otros datos públicos:
por ejemplo, si una coinbase tiene una sola salida y su recompensa total es
pública, su monto puede inferirse aunque la nota esté cifrada. La privacidad
debe evaluarse con esas relaciones y con los patrones de uso, no solo leyendo
el campo cifrado.

## Alcance y límites actuales

El nodo guarda por bloque un **registro compacto** (unos cientos de bytes) y
`state_root` compromete un estado de tamaño constante: altura, emisión, raíz y
tamaño del árbol de notas y un acumulador de los nullifiers gastados. Las
ramas laterales se validan con esos registros y el índice canónico de
nullifiers. Detalle en [QUANTIC-V2-FORMATO.md](QUANTIC-V2-FORMATO.md). Las
bases creadas antes de ese formato no se pueden abrir: hay que usar una nueva.

La wallet reconstruye el árbol completo recorriendo los bloques canónicos (sin
podar) cada vez que calcula saldo o arma un pago; eso crece con la cadena. Las
pruebas se transportan por transacción, unos 1,9 MB por pago, sin agregación
recursiva de todo el bloque.

La configuración de STARK y el estimador de Plonky3 se documentan en la
[revisión del código](REVISION-STARK-CODIGO-2026-10-25.md). Los bits de seguridad
que calcula ese estimador son una **estimación de soundness en su modelo
clásico y bajo sus hipótesis**, no una certificación de bits postcuánticos.
No demuestran por sí solos seguridad de Fiat-Shamir frente a consultas
cuánticas, conocimiento cero de la composición completa ni seguridad de toda
la aplicación. Tampoco ampliar la salida de un hash aumenta automáticamente
su resistencia interna.

Se puede implementar y probar hoy el protocolo y sus rechazos sin disponer de
una máquina cuántica. Lo implementado aquí no equivale a una auditoría externa
ni a una certificación de resistencia cuántica. La cadena, las wallets y el
transporte v2 descritos funcionan en cadenas locales: la regtest y la testnet v2
antes de que el lanzamiento le fije el génesis.
