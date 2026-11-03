# Semillas de la testnet v2: cómo se suma una

**La idea: cada nodo es una semilla.** Todo nodo v2 responde lo mismo que una
semilla (bloques, transacciones, páginas para los teléfonos, direcciones
cortas, checkpoints), se anuncia solo (`--announce yes` por defecto) y comparte
los pares que probó. Guarda hasta 256 en su base y al reiniciar vuelve a ellos
aunque las semillas estén caídas. Desde el 3/11 el teléfono (ZKAPriv) hace lo
mismo con su propia lista (`stark_mobile::PhonePeers`): prueba primero los
pares que le respondieron, después las semillas y al final el resto.

Una **semilla del manifiesto** es solo la primera puerta para quien no conoce
a nadie todavía: un nodo fijado dentro de `zoka`, en
[quantic-testnet-v2-network.v1.json](quantic-testnet-v2-network.v1.json). Se
fija por dos datos públicos: el endpoint id de iroh y la huella (blake3) de su
clave ML-DSA-65. Las semillas no tienen permisos especiales: no firman nada ni
deciden nada, y pueden ser de cualquiera de la comunidad (hasta 16). Los VPS
del equipo entran como una semilla más, igual que la de un usuario.

Sin `direct_addresses`, la IP la resuelve el servicio de direcciones de n0 y,
si el nodo está detrás de un router, se llega por los relays de n0. Una
semilla con IP pública y `--bind` (ver abajo) se alcanza directo, sin n0.

## Por qué hace falta una fuera de la PC

Al 3/11/2026 hay dos semillas y **las dos están en la PC del usuario**:

- `seed1`: el nodo de ZSilent, que además firma los checkpoints;
- `seed2`: el nodo del explorador, también detrás de la .onion
  (`stream_address`), la del Modo Tor.

Si la PC se apaga, un nodo **nuevo** no encuentra la red, y en Modo Tor no hay
a dónde llegar. Los nodos que ya corrieron recuerdan sus pares (el `PeerBook`
se guarda en su base) y se reconectan entre ellos. El 1/11 se cayó XMRig de la
PC y la cadena siguió dos días gracias a un minero de afuera; cuando ese minero
paró, quedó 4 horas sin bloques. Una semilla de otro operador, en otra red,
es lo que hace que la testnet no dependa de una sola computadora.

Una semilla no puede inventar bloques, porque cada nodo valida todo. Lo que sí
podría hacer es aislar a un nodo nuevo, sin pasarle otros pares. Por eso
conviene tener varias y de operadores distintos.

## Semilla en un servidor Linux (VPS)

**Qué servidor:** Linux x86-64 con glibc 2.31 o más nuevo (Ubuntu 20.04+ o
Debian 11+), 2 núcleos, 2 GB de RAM (4 GB si además mina con XMRig), 20 GB de
disco, prendido todo el día. No hace falta abrir puertos: el nodo llega a la
red por los relays de n0.

**Minar en el servidor es opcional.** Una semilla sirve aunque no mine. Muchos
proveedores prohíben minar en sus servidores (Contabo, por ejemplo, permite
nodos pero no minar con prueba de trabajo en sus VPS): revisar los términos
antes de poner XMRig.

**Con IP pública, puerto fijo.** En un VPS conviene abrir un puerto UDP en su
firewall (el del proveedor y `ufw`; no es el router de nadie) y arrancar con
`--bind 0.0.0.0:PUERTO`. Al fijarla, el equipo agrega
`"direct_addresses": ["IP:PUERTO"]` a su entrada: los nodos y los teléfonos la
alcanzan directo, aunque el servicio de direcciones o los relays de n0 no
anden. Sin `--bind`, el puerto cambia en cada arranque y la dirección no sirve
para el manifiesto.

En el servidor, con un usuario común (no root):

```bash
# 1. Bajar el minero CLI y verificarlo (el hash está en las notas de la release).
wget https://github.com/zoka-network/zokanetwork/releases/download/v4.0.4/zoka-cli-miner-4.0.4-linux-x64.tar.gz
sha256sum zoka-cli-miner-4.0.4-linux-x64.tar.gz
tar xzf zoka-cli-miner-4.0.4-linux-x64.tar.gz
cd zoka-cli-miner-4.0.4-linux-x64

# 2. Una wallet (cobra lo que mine, si mina). Anotar las 24 palabras.
./zoka-miner.sh create-wallet

# 3. Que el nodo corra siempre, también después de un reinicio. Con IP
#    pública, con el puerto UDP fijo y abierto (ejemplo: 4433).
sudo ufw allow 4433/udp
./zoka-miner.sh autostart --bind 0.0.0.0:4433
sudo loginctl enable-linger "$USER"

# 4. Los dos ids públicos de la semilla, para mandarlos al equipo.
./zoka-miner.sh node-id
./zoka-miner.sh status
```

Sin XMRig al lado del script, `mine` deja el nodo sirviendo la cadena sin
minar, que es justo lo que necesita una semilla. La carpeta `data/.../node`
guarda la identidad: si se borra, la semilla cambia de ids y hay que fijarla
de nuevo.

## Pasos del lado del equipo

1. **Recibir los dos ids** (`node-id`: "Endpoint id" y "ML-DSA key id"; en
   ZSilent están en la página Node).
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
3. **Fijarla.** Agregar la entrada a `seed_nodes`, con `label` (`seed3`),
   `operator`, `endpoint_id`, `ml_dsa_key_id`, `status` `active` y `note`, y
   `direct_addresses` si corre con `--bind` en una IP pública. El máximo es 16
   (`MAX_SEEDS`). Conviene que sean de operadores y redes distintos. Actualizar la prueba
   `manifests_pin_seeds_by_id_and_key_and_the_builtin_one_parses`
   (`src/quantic/stark_peers.rs`), que cuenta las semillas.
4. **Publicarla.** El manifiesto va compilado: hace falta un nodo nuevo (el
   minero CLI, ZSilent y el motor de ZKAPriv con ese nodo), con el
   procedimiento de siempre y el OK del usuario. Si la semilla llega antes de
   publicar una versión, conviene meterla en esa misma.

## Lo que todavía depende de alguien (3/11/2026)

- **n0.** Un par aprendido viaja solo con su id (para que un anuncio no pueda
  mandar a los nodos a marcar la IP de un tercero), y el id se resuelve en el
  servicio de direcciones de n0. Si n0 no anda, solo se alcanzan las semillas
  con `direct_addresses` y las .onion. Alternativas a estudiar: la DHT de
  BitTorrent (`iroh-mainline-address-lookup`, dependencia nueva) o un relay
  propio en un VPS como segunda opción.
- **Las .onion no se comparten.** En modo relay el anuncio lleva solo el id, y
  la libreta borra las direcciones: un nodo o teléfono en Modo Tor solo conoce
  las .onion del manifiesto. Compartirlas (solo `.onion`, que por Tor no llevan
  a ningún host de afuera) necesita probar cómo trata iroh una dirección de
  transporte propio en un endpoint que no lo tiene.
- **Los checkpoints** los firma solo la PC del equipo y todos los nodos los
  siguen. Para la mainnet hay que decidir si quedan solo para que las
  billeteras livianas verifiquen o si se retiran.
- **La minería:** casi todos los bloques los mina el XMRig de la PC.

## Sacar una semilla

Poner `status` en `retired`, en vez de borrarla: el historial queda en el
archivo. Los nodos viejos la siguen marcando hasta que se actualizan, y no pasa
nada si no responde.
