# Etapa 5 (2ª parte): Zoka Quantic dentro del nodo

16/10/2026. Diseño de trabajo para llevar `crates/zoka-quantic` (la biblioteca de
la [etapa 5](ETAPA5-diseno-quantic.md)) a una red que funcione. Decisiones
técnicas tomadas por Claude con la autonomía que dio el usuario; revisables.

## Decisión: un perfil del mismo nodo, no un nodo nuevo

El nodo de Zenith ya resolvió lo difícil de la red: Tor y Arti, iroh, semillas
DNS, sincronización y reorganizaciones, checkpoints de la autoridad, snapshots,
la API que usan ZSilent y ZKAPriv. Un nodo nuevo tendría que rehacer todo eso.
`NetworkProfile` ya lo anticipa ("Zoka Quantic va a tener su propio perfil y sus
propias rutas, que ningún cliente viejo conoce").

- Perfil `quantic-testnet`: chain id `zoka-quantic-testnet-v1`, carpeta
  `.zoka/quantic-testnet`, su manifiesto y su génesis. Mainnet, en la etapa 8.
- Feature `quantic` (arrastra `orchard` y Halo 2). Los releases se compilan con
  `pq-experimental,quantic`. Sin ella, el binario es el de hoy.
- **Zenith no cambia en nada**: cada regla nueva mira el chain id.

## Bloques

Se reutiliza `Block` con un campo nuevo, `quantic_transactions` (cada una en el
formato binario de la biblioteca, en hex dentro del JSON; vacío y omitido en
Zenith, así sus bloques y sus hashes quedan byte por byte iguales).

En un bloque de Quantic:

- `quantic_transactions[0]` es el coinbase; no hay transacciones públicas ni
  ningún campo privado de Zenith, salvo `private_nullifiers`.
- `private_nullifiers` = los nullifiers de todas sus transacciones, ordenados y
  sin repetir. El consenso exige que coincidan con los de las transacciones. Así
  el índice de nullifiers gastados de Zenith, que ya resuelve bien las
  reorganizaciones (incidente del 9/7), protege también a Quantic.
- `state_root` = `quantic-v1:<raíz de txid>:<raíz de autorización>:<raíz del
  árbol de notas>`. Entra en el hash del bloque, así la prueba de trabajo
  compromete las tres. Una wallet liviana usa la primera para verificar los
  bloques compactos y la tercera para verificar su árbol.
- `merkle_root` suma cada transacción con el prefijo `quantic-tx-v1:` (solo si
  hay; en Zenith no cambia nada).
- Prueba de trabajo, hash del bloque, dificultad y checkpoints: los de siempre
  (con la firma híbrida de la etapa 6). El tiempo de bloque y la dificultad
  inicial se fijan en la etapa 7.2.

## Estado de consenso

- **Nullifiers:** el índice de siempre (ver arriba).
- **Árbol de notas:** su frontera después de cada bloque, guardada por hash de
  bloque. Aplicar un bloque parte de la frontera de su padre, sea o no la rama
  canónica: las reorganizaciones no necesitan deshacer nada.
- **Anclas:** una transacción tiene que anclar en la raíz final de alguno de los
  **100 bloques anteriores de su propia rama** (unas 3 horas y media). Zcash
  acepta cualquier raíz pasada; acotarla simplifica las reorganizaciones y no
  cambia nada para la wallet, que usa la última.
- **Maduración del coinbase:** al aplicar el bloque de altura h se insertan
  primero las notas del coinbase del ancestro de altura h − 60 de la misma rama.
- **Altura del génesis:** el nodo numera desde 1 (la altura 0 es sintética), así
  que el bloque 1 de Quantic es el génesis con el premine público. La biblioteca
  recibe la altura del génesis como parámetro.

## Mempool, red y minería

- Mempool propio de Quantic: verificación de la prueba y las firmas al entrar,
  ancla dentro de la ventana, nullifiers sin gastar ni repetidos en el mempool.
- Mensaje P2P nuevo con la transacción entera (unos 11 KB con 2 acciones).
- El minero arma el coinbase con su prueba Halo 2 (2 s en la PC) una vez por
  plantilla, no por nonce. La clave de prueba se arma al arrancar (2,8 s).

## API y wallet

- `GET /quantic/info`, `POST /quantic/tx`, `GET /quantic/tx/:txid`,
  `GET /quantic/compact?from=&to=` (bloques compactos de la etapa 5).
- CLI `zoka quantic-wallet` (crear, dirección, saldo, enviar), con la frase de
  siempre y escaneo sobre bloques compactos del propio nodo.
- ZSilent y ZKAPriv después, sobre lo mismo.

## Orden

1. Biblioteca: altura del génesis y ventana de anclas.
2. Perfil `quantic-testnet` y feature `quantic` (sin consenso todavía).
3. Formato de bloque y validación completa con la biblioteca, estado persistido.
4. Mempool, mensaje P2P, minería con coinbase Quantic.
5. API y wallet de línea de comandos.
6. Red local de prueba (como la de la etapa 3): tres nodos, minería, envíos,
   una reorganización.
7. Génesis de la testnet de Quantic con el premine (dirección del equipo de la
   testnet) y su semilla en la PC.
8. ZSilent y ZKAPriv.

## Estado (16/10/2026, noche)

Hechos los pasos 1 a 6 en la rama `etapa5-quantic-nodo`:

- Biblioteca: altura del génesis, ventana de anclas, reglas de consenso como
  funciones puras y la frontera del árbol serializable.
- Nodo (feature `quantic`): campo `quantic_transactions` en `Block` (Zenith no
  cambia: 952 pruebas pasan), validación con `crate::quantic`, estado por hash de
  bloque, mempool, minero, perfiles `quantic-testnet` y `quantic-regtest`,
  protocolo y tópicos propios (no se mezcla con Zenith), mensaje
  `QuanticTransaction`, API `/quantic/*` y `zoka quantic-wallet`.
- **Prueba de trabajo:** la de Quantic compromete `merkle_root` y `state_root`.
  La de Zenith no cubre el cuerpo del bloque (ver abajo).
- **Regtest:** dificultad fija de 8 bits (como el regtest de Bitcoin),
  maduración de 2 bloques y premine a una clave de prueba pública.

### Prueba en red local (16/10/2026)

Tres nodos en la PC, cada uno con su `ZOKA_HOME` en la carpeta temporal, su API
(3211–3213) y su puerto P2P (29211–29213), sin semillas ni nada de Zenith. Binario
de depuración con `--features quantic`.

1. A mina desde el génesis: el bloque 1 paga el premine público a la wallet del
   equipo de regtest; la wallet de línea de comandos ve 5.000.000 ZKA disponibles
   y el minero 24 ZKA por madurar.
2. B se conecta a A y valida toda la cadena (pruebas Halo 2 incluidas).
3. El equipo le paga 100 ZKA a Bob por la API de A (17 s en la build de
   depuración, con el armado de la clave). Bob los ve consultando a **B**.
4. Bob le paga 40 ZKA al minero por la API de **B**, que no mina: la
   transacción viaja por la red a A, que la mina. Saldos exactos: Bob 59,9998;
   minero 160,0004 = 10 × 12 + 2 comisiones + 40.
5. **Reorganización:** C se sincroniza con A, se aísla y mina su propia rama, en
   la que el equipo le paga 7 ZKA a Bob. Al reconectarse, C adopta la rama más
   pesada de A: los dos quedan con la misma punta, Bob vuelve a 0 y la nota del
   equipo queda sin gastar.

### Hallazgo en Zenith (sin tocar)

La prueba de trabajo de Zenith (`build_pow_input`) cubre cadena, padre,
dirección del minero, altura, hora y nonce, **pero no el cuerpo del bloque**: se
puede tomar el sello de un bloque recién minado y ponerle otro cuerpo (otras
transacciones privadas, otros nullifiers) con el mismo trabajo. El hash del
bloque cambia, así que la cadena sigue enlazada, pero dos bloques distintos
compiten con el mismo trabajo y un nodo que los reenvía puede cambiar qué
transacciones entran. Arreglarlo en Zenith exige un fork con altura de
activación; decidirlo con el usuario.

### Génesis de la testnet (preparado, 16/10/2026)

- `zoka quantic-genesis --network quantic-testnet --premine-to <zqtest1…>` arma el
  bloque 1 con el premine público (77 s en la build de depuración, con RandomX a
  12 bits) y lo escribe en `docs/quantic-testnet-genesis.v1.json`.
- Su hash y la dirección van a `economics::QUANTIC_TESTNET_GENESIS`; el bloque
  queda embebido en el binario y cada nodo nuevo lo importa al arrancar. Otro
  bloque 1 no vale aunque pague lo mismo.
- **Ensayo con una wallet descartable** (sin commitear): el nodo importó el
  génesis, la wallet vio 5.000.000 ZKA, minó con RandomX y un segundo nodo se
  sincronizó con él. Después se revirtieron los archivos y se recompiló.

Lo que falta para abrir la testnet (pasos del usuario, al final):

1. Crear la wallet del equipo de la testnet en la PC:
   `zoka quantic-wallet new --network quantic-testnet --name equipo` (anotar la
   frase de 24 palabras fuera de la PC).
2. Pasarle la dirección a Claude, que arma el génesis, fija las constantes y
   compila.
3. Arrancar el nodo de la PC en `quantic-testnet` minando, y publicarlo como
   semilla en el manifiesto.

### Pendiente

7. Génesis de la testnet de Quantic con su premine y la PC como semilla (ver arriba).
8. ZSilent y ZKAPriv: el motor del teléfono y su puente JNI están hechos (17/10,
   [QUANTIC-MOTOR-MOVIL.md](QUANTIC-MOTOR-MOVIL.md)); faltan las pantallas, que
   decide el usuario.
9. ~~Devolver al mempool las transacciones de bloques que una reorganización
   descarta.~~ Hecho el 16/10 (`runtime::orphaned_transfers`).
10. Bloques de más de 96 acciones: el minero se queda en 96 para que el bloque
    entre en un mensaje de gossip (1 MB, con cada transacción en hex). Uno más
    grande igual llegaría (se anuncia y los pares lo bajan directo, hasta 16 MB),
    pero por Tor tardaría y aumentaría los bloques huérfanos. Antes de subir el
    tope: bloques compactos (encabezado + txids, el resto sale del mempool) o un
    cuerpo binario sin hex. 96 acciones por bloque son unas 0,4 transferencias
    por segundo.
11. Wallets livianas por P2P: hecho el 17/10 (`/zoka-quantic/light/1`, con
    presupuesto por par).
