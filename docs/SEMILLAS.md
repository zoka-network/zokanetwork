# Semillas de la testnet v2: cómo se suma una

Una **semilla** es un nodo que viene fijado dentro de `zoka`, en
[quantic-testnet-v2-network.v1.json](quantic-testnet-v2-network.v1.json): un
nodo nuevo la marca primero y por ella conoce al resto (intercambio de pares).
Se fija por dos datos públicos: el endpoint id de iroh y la huella (blake3) de
su clave ML-DSA-65. La dirección IP no va: la resuelve el servicio de
direcciones de n0.

## Por qué hace falta una segunda

Al 30/10/2026 hay una sola, `seed1`: la PC del usuario, que además firma los
checkpoints.

- Si la PC está apagada, un nodo **nuevo** no encuentra la red.
- Los nodos que ya corrieron recuerdan los pares que conocieron (el
  `PeerBook` se guarda en su base) y se reconectan entre ellos.
- Hoy casi no hay otros nodos: la red es la PC.
- Una semilla no puede inventar bloques, porque cada nodo valida todo. Lo que
  sí podría hacer es aislar a un nodo nuevo, sin pasarle otros pares. Por eso
  conviene tener varias y de operadores distintos.

## Pasos

1. **El operador** deja un nodo prendido todo el día. Puede usar el minero
   CLI (`2-start-mining.cmd` / `./zoka-miner.sh mine`; sin XMRig el nodo
   igual corre) o ZSilent. Después corre `show-node-id.cmd` /
   `./zoka-miner.sh node-id` (minero CLI 4.0.3 en adelante) y manda los dos
   ids. Con ZSilent, los muestra la página Node: "Endpoint id" y "ML-DSA key
   id".
2. **Probarla antes de fijarla.** En una carpeta temporal, con un manifiesto de
   prueba que lleve solo la semilla nueva (mismo formato que el oficial):

   ```text
   set ZOKA_PQ_PASSWORD=<cualquiera, identidad descartable>
   zoka quantic-pq-node serve --network testnet --db <tmp>\db --identity <tmp>\id ^
       --api 127.0.0.1:9199 --seeds <tmp>\manifiesto-prueba.json
   ```

   En `/status` (`http://127.0.0.1:9199/status`) tiene que aparecer al menos 1
   par saliente, y la altura tiene que llegar a la de la red. Repetirlo con la
   PC apagada, si se puede, es la prueba de verdad.
3. **Fijarla.** Agregar la entrada a `seed_nodes`, con `label` `seed2`,
   `operator`, `endpoint_id`, `ml_dsa_key_id`, `status` `active` y `note`. El
   máximo es 16 (`MAX_SEEDS`). Actualizar la prueba
   `manifests_pin_seeds_by_id_and_key_and_the_builtin_one_parses`
   (`src/quantic/stark_peers.rs`), que hoy espera una sola semilla.
4. **Publicarla.** Hace falta un nodo nuevo (el manifiesto va compilado): el
   minero CLI y ZSilent con ese nodo, con el procedimiento de siempre (ver la
   hoja de ruta, 4.0.2 y 4.0.4) y el OK del usuario.

## Sacar una semilla

Poner `status` en `retired`, en vez de borrarla: el historial queda en el
archivo. Los nodos viejos la siguen marcando hasta que se actualizan, y no pasa
nada si no responde.
