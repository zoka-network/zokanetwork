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
