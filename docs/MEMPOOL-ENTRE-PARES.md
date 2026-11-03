# Pagos que tardan en minarse: el mempool entre pares

Caso real del 3/11/2026 (testnet v2) y su arreglo, en la rama
`arreglo-mempool-entre-pares`.

## Qué pasó

Un pago de ZSilent a ZKAPriv (`ebfdbf1a…`, 1.938.495 bytes, casi todo prueba
STARK) se armó a las 19:07:55 con la cadena en el bloque ~4001 y se minó recién
en el bloque 4012, a las 19:41:37: 11 bloques y 34 minutos. Vencía en el ~4021.

1. El nodo de la PC lo aceptó y en segundos lo empujó a seed2 y seed3. Las tres
   lo tuvieron en el mempool todo el tiempo (`mempool_transactions: 1`).
2. El único que minaba era un minero externo (~900 H/s, par `a39d882d93`)
   detrás de NAT. Él llama a las semillas; las semillas lo llaman a él por el
   relay de n0.
3. El protocolo solo tenía `SUBMIT_TX`: una transacción viaja del nodo que llama
   al nodo llamado, nunca al revés. Para llegar al minero, seed2 tenía que
   empujársela por la sesión que ella le abre a él.
4. Esa sesión venía estable desde las 16:16 (una sola falla en tres horas).
   Desde las 19:09:58, justo al tener el pago para mandarle, se cortó 11 veces
   seguidas, cada ~3 min: `STARK request timeout` o `connection lost`. Las dos
   puntas cortan un pedido a los 120 s (`REQUEST_TIMEOUT` en el cliente y en
   `serve_requests`). Mandar casi 2 MB por un relay lento y verificar la prueba
   en una CPU que mina no entraba en ese tiempo. Un intento pasó cerca de las
   19:38 y el minero lo incluyó en el 4012.

Ver registros: `journalctl -u zoka-seed | grep a39d882d93` en el VPS 1 (la hora
del VPS es CET, 4 h más que Argentina).

## El arreglo

El nodo que llama también **pide** el mempool del par:

- `MEMPOOL` (13): los txid del mempool, 32 bytes cada uno, como mucho 256.
- `GET_TX` (14): una transacción por txid; vacío si ya no está.
- `Head` lleva `"mempool": true`. Un nodo viejo no lo manda (y cierra la
  conexión ante un pedido desconocido), así que a ese par no se le pide nada.
  Los clientes viejos ignoran el campo.
- En cada ciclo de la sesión saliente, con las puntas iguales, después de
  empujar lo propio: lista los txid del par y baja hasta 4 que no tenga. Los
  verifica como cualquier `SUBMIT_TX` y recuerda los que rechazó mientras el par
  los siga ofreciendo (`TxRelay` en `stark_node.rs`).

Así el minero baja el pago por su propia conexión a la semilla (IP fija,
directa), unos segundos después de que llega a la semilla, sin depender de que
la semilla logre llamarlo.

**Para que sirva, el minero tiene que actualizar su nodo.** Las semillas ya lo
ofrecen apenas corran esta versión; un minero viejo sigue dependiendo del
empuje de la semilla, como hoy.

## Buscar un pago por su id

Mientras el pago esperaba, ZSilent no lo encontraba en su explorador: en la v2,
`blockByHash`, `mempoolTransaction` y `privateTransaction` eran stubs que
devolvían null, y el texto prometía encontrar "los enviados desde este nodo".
Ahora la API local y la pública tienen `GET /transactions/<txid>`:

- `{"status":"pending"}` si está en el mempool del nodo;
- `{"status":"confirmed","height","block_hash","confirmations"}` si está en uno
  de los últimos 720 bloques canónicos (`TX_LOOKUP_BLOCKS`, un día). El nodo no
  indexa transacciones: recorre los bloques hacia atrás, uno por vez, tomando el
  candado solo para leer cada uno;
- 404 si no está. En la API pública cuenta en el cupo de las páginas de escaneo.

ZSilent (`1354cdf`) y ZokaScan (`056eea8`) la usan; con un nodo anterior la
ruta no existe y la búsqueda dice que no lo encontró.

## Lo que mostraba mal ZSilent mientras tanto

Aparte del nodo (zsilent-core, rama `arreglo-reserva-y-nonce`):

- El envío reserva sus notas hasta minarse o vencer, y elige primero la más
  grande: acá, el vuelto del premine (4.983.022,9996 ZKA). El nodo informa esa
  reserva en `reserved_atoms`, pero ZSilent no lo leía y el total bajó a
  36.776,9998. ZokaScan, que escanea con la clave de vista, mostraba bien
  5.019.799,9994. Ahora el total suma la reserva y la tarjeta Pending la muestra.
- `BlockSummary.nonce` era `Long`: los nonces u64 de Quantic lo desbordan y
  "Latest blocks" fallaba entero. Ahora es `ULong`.
- ZokaScan (rama `nonce-exacto`) mostraba el nonce redondeado (número de
  JavaScript); ahora lo lee como texto.
