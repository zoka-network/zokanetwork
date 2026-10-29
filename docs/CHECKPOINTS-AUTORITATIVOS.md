# Checkpoints autoritativos

Pedido del usuario (13/10/2026): *"mi nodo, es decir mi PC, debe ser un nodo
autoritario: cuando otros nodos se conecten, ya sea en Zenith o en Quantic,
deberán obedecer las reglas de mi nodo para evitar forks o ramas distintas."*

## Qué es

El nodo del equipo (la PC del usuario) firma cada tanto un **checkpoint**: "en la
altura H de esta cadena el bloque canónico es este hash". Los demás nodos traen
compilada la clave pública del equipo, verifican la firma y **no siguen ninguna
rama que contradiga el último checkpoint firmado**. Una rama que lo contiene
gana aunque tenga menos trabajo acumulado; una que no lo contiene se rechaza.

No es un mecanismo nuevo: el nodo ya hace exactamente eso con los checkpoints
compilados de Zenith (296, 297, 298, 1460, 3127, 10887, 14500), que obligaron a
sacar un binario por cada uno. Los checkpoints firmados son la misma regla con
la lista ampliada en vivo, sin binario nuevo.

## Qué protege y qué no

- **Evita forks y ramas distintas** por debajo del último checkpoint: clientes
  viejos o con errores, una partición de la red, o alguien con más hashrate que
  intenta reescribir el pasado.
- **No cambia las reglas de validación.** Cada nodo sigue validando cada bloque
  (prueba de trabajo, transacciones, emisión). El equipo elige *cuál* cadena
  válida es la canónica; no puede volver válido un bloque inválido, crear monedas
  fuera de la emisión ni gastar fondos ajenos.
- **Es una confianza explícita en el equipo.** Quien tenga la clave decide la
  cadena canónica. Hay que declararlo públicamente, igual que el premine.
- **Si la clave se pierde**, la red sigue funcionando por trabajo acumulado desde
  el último checkpoint; rotar la clave exige un binario nuevo.
- **Si la clave se filtra**, el atacante puede congelar la red en una rama propia
  válida, pero no falsificar bloques. La clave vive solo en la PC del equipo.
- **Ed25519 no es post-cuántico.** La versión 2 de la firma será híbrida
  (Ed25519 + ML-DSA-65), junto con los snapshots firmados (etapa 5). El campo
  `version` del checkpoint existe para eso.

## Formato

```json
{"version":1,"chain_id":"zoka-mainnet-zenith-v1","height":25300,
 "block_hash":"…64 hex…","signature":"…128 hex (Ed25519)…"}
```

Mensaje firmado: `"zoka-signed-checkpoint-v1" ‖ 0x1f ‖ chain_id ‖ 0x1f ‖
height (u64 little-endian) ‖ 0x1f ‖ block_hash`. El `chain_id` dentro de la firma
impide usar un checkpoint de Zenith en Quantic o al revés.

### Versión 2: firma híbrida (etapa 6, 16/10/2026)

```json
{"version":2,"chain_id":"…","height":25300,"block_hash":"…64 hex…",
 "signature":"…128 hex (mitad Ed25519)…","mldsa_signature":"…6.618 hex (mitad ML-DSA-65)…"}
```

- Las dos firmas son obligatorias y cubren el mismo mensaje:
  `"zoka-signed-checkpoint-v2" ‖ 0x1f ‖ chain_id ‖ 0x1f ‖ height ‖ 0x1f ‖
  block_hash`; ML-DSA usa además el dominio como contexto. Implementación en
  `src/hybrid_signature.rs` (con `pq-experimental`, que llevan los releases).
- **Política:** un binario que tiene compilada la mitad ML-DSA de la clave de una
  cadena (`economics::CHECKPOINT_AUTHORITY_MLDSA_KEYS`) solo acepta la versión 2
  de esa cadena; uno que no la tiene, solo la 1. Aceptar las dos dejaría entrar a
  quien rompa Ed25519.
- La mitad Ed25519 de una v2 no sirve como v1: el dominio es otro.
- Un nodo viejo recibe la v2 (el campo nuevo se ignora en el JSON), la rechaza
  por versión y sigue funcionando. La v1 se serializa igual que antes.
- **La autoridad** firma la v2 si tiene el archivo `checkpoint-authority-mldsa.key`
  al lado de la clave Ed25519 y coincide con la compilada. En Zenith publica
  también la v1 para los nodos de antes de la etapa 6 (la guarda aparte para
  reenviarla); en Quantic, solo la v2. Si la cadena exige la v2 y falta la mitad
  ML-DSA, no firma nada.
- La versión que no es la de la política de un nodo no lo ensucia: la capa P2P
  la deja pasar y el nodo la descarta sin mensaje.
- Crear la mitad ML-DSA: `zoka checkpoint-keygen --mldsa` en la PC de la
  autoridad (nunca sobrescribe). **Hoy Zenith no tiene ninguna compilada: no
  cambia nada hasta que el usuario la cree y salga un binario con ella.**

## Cómo lo aplican los nodos

1. **Aceptación** (al recibirlo por la red, por la API o del propio firmador):
   firma válida con la clave de esa cadena; `height` mayor que el último
   aceptado (un checkpoint viejo no puede reemplazar a uno nuevo); el mismo
   `height` con otro hash se rechaza y se avisa en el log (sería la clave
   comprometida o un error del firmador).
2. **Persistencia:** el último aceptado queda en la base (`checkpoint:signed:latest`)
   y todos en `checkpoint:signed:<altura>` para auditoría.
3. **Aplicación:** el último aceptado se suma a la lista de checkpoints de
   `state/chain.rs`. Desde ahí rigen las tres reglas que ya existen: el historial
   local que lo contradice pone al nodo en cuarentena (deja de minar), una rama
   candidata que no lo contiene se rechaza, y una que lo contiene fuerza la
   reorganización aunque tenga menos trabajo.

## Distribución

- **Gossip:** tópico propio `zoka/checkpoints/v1`. Los nodos viejos no se suscriben,
  así que no les llega nada que no entiendan. El nodo valida la firma antes de
  reenviar, para no propagar basura.
- **El firmador lo reenvía cada pocos minutos**, para que un nodo que se conecta
  tarde lo reciba sin esperar al próximo.
- **API:** `GET /checkpoint/latest` (lectura pública) y `POST /checkpoint` (se
  verifica como cualquier otro: permite inyectarlo a mano si la red falla).

## Modo autoridad (la PC del equipo)

- `zoka checkpoint-keygen --network testnet` crea la clave en
  `<ZOKA_HOME>/.zoka/<red>/checkpoint-authority.key` (no sobrescribe) e imprime la
  clave pública que se compila en los binarios.
- Si ese archivo existe al arrancar, el nodo entra en modo autoridad: cada
  **30 bloques** firma el bloque **6 por debajo de la punta**, lo guarda y lo
  publica.
- **Solo firma si la red le confirma la punta:** al menos un par conectado, una
  punta anunciada que no supera la propia y ninguna rama competidora en descarga.
  Con `--min-mine-peers 0` un nodo sin pares figura «al día»; sin esta condición,
  una PC aislada que sigue minando sola firmaría su propia rama y quedaría atada
  a ella. El checkpoint vigente se reenvía igual, aunque no toque firmar.
- **Tampoco firma si su cadena no contiene el último checkpoint aceptado** (uno
  firmado a mano para corregir la rama, o uno por encima de su punta): firmaría
  más arriba la misma rama que ese checkpoint vino a corregir y la red volvería
  a ella. Retoma cuando se reorganiza.
- `zoka checkpoint-status --network testnet` muestra la clave pública y el último
  checkpoint aceptado.
- **Respaldo:** el archivo de la clave se guarda fuera de la PC (sin conexión). Es
  la única copia.

## Zenith y Quantic

- **Zenith (testnet):** su propia clave. Se activa con el próximo binario.
- **Zoka Quantic:** otra clave, activa desde el génesis y declarada en el
  whitepaper junto al premine.

## Corregir a mano

Si la autoridad firmara una rama equivocada, quedaría atada a ella. Un
checkpoint más alto sobre la rama correcta la reemplaza en todos los nodos, que
se reorganizan hacia esa rama aunque no esté en la propia:

1. Elegir una altura `H` mayor que la del último checkpoint
   (`zoka checkpoint-status`) en la que no se haya firmado nada.
2. Tomar el hash del bloque `H` de la rama correcta en un nodo que la siga
   (`GET /blocks/H`).
3. En la PC de la autoridad:
   `zoka checkpoint-sign --network testnet --height H --hash <hash> --post http://127.0.0.1:3000`.
   El comando firma solo si la clave local es la compilada y verifica el
   resultado antes de entregarlo.
4. El nodo de la autoridad lo acepta, queda en cuarentena hasta bajar esa rama
   y se reorganiza; lo reenvía a la red en el ciclo siguiente (5 minutos como
   mucho). Los demás nodos lo aceptan porque es más alto.

**Nunca firmar un hash que no sea de un bloque real:** todos los nodos que
obedecen quedarían en cuarentena esperando una rama que no existe.

## Prueba en red local (13/10/2026)

Tres nodos en la PC, cada uno con su `ZOKA_HOME`, sin manifiesto de red ni iroh,
con una clave de prueba (`ZOKA_CHECKPOINT_AUTHORITY_PUBKEY`) y dificultad de
ensayo. Binario de `e142e349`. Resultado:

1. **La autoridad firma**: A minaba sola y firmó el checkpoint 16 en cuanto B se
   conectó (punta 22: la punta menos 6).
2. **Llega por gossip**: B lo tuvo en el mismo segundo, aun antes de tener ese
   bloque (estaba en el 12).
3. **Corrección a mano**: `checkpoint-sign --post` le entregó a B un checkpoint en
   el 17 y B lo aceptó.
4. **Una rama más pesada no arrastra a la autoridad**: C minó sola hasta el 64 una
   rama distinta desde el génesis (A estaba en 24) y se conectó a A. A la rechazó
   ("checkpoint mismatch at height 16") y siguió en su rama.
5. **El nodo desviado obedece**: C recibió el checkpoint a los pocos segundos,
   dejó de minar (cuarentena), entró en modo de recuperación y en menos de dos
   minutos tenía en el 16 el bloque firmado, bajando la rama de A.

## Testnet v2 (STARK, 28/10/2026)

Decisión del usuario (28/10): la PC sigue siendo el nodo autoritario en la
testnet v2, que reemplaza a la v1. Rama `quantic-v2-checkpoints`.

- **Claves:** la testnet v2 (`zoka-quantic-testnet-v2`) tiene compiladas las
  mismas dos mitades que la v1 de Quantic. Cada firma lleva el chain id, así
  que un checkpoint de una cadena no sirve en la otra, y la PC no necesita
  claves nuevas (el usuario lo confirmó el 28/10). La versión 2 (híbrida) es
  la única que se acepta.
- **Modo autoridad:** `quantic-pq-node serve --authority DIR`. `DIR` es la
  carpeta con `checkpoint-authority.key` y `checkpoint-authority-mldsa.key`, la
  de la red de la v1. Si alguna de las dos no es la compilada, el nodo no
  arranca, en vez de firmar algo que nadie aceptaría. Las reglas de firma son
  las de la v1: cada 30 bloques, el bloque 6 por debajo de la punta, con algún
  par en sesión (entrante o saliente) y ningún par saliente que informe una
  punta más alta. Si su cadena no contiene el último checkpoint aceptado, no
  firma.
- **Distribución:** `HEAD` informa la altura del último checkpoint aceptado.
  Cada sesión, antes de mover bloques, trae el del otro lado si es más nuevo
  (`CHECKPOINT`) o le entrega el propio (`SUBMIT_CHECKPOINT`). Así el pull que
  sigue ya queda atado al checkpoint. `sync` también lo trae. Cada nodo lo
  verifica contra las claves compiladas antes de guardarlo. Un checkpoint
  rechazado no se vuelve a pedir en esa sesión.
- **API:** `GET /checkpoint` y `POST /checkpoint`. `/status` muestra el vigente
  y si la cadena canónica lo contiene (`followed`).
- **Cuarentena:** no se arma una plantilla de minería (API, Stratum ni minero
  interno) sobre una rama que contradice un checkpoint aceptado. El error dice
  `mining paused`. Se mina de nuevo cuando el nodo se reorganiza.
- **Corregir a mano:**
  `quantic-pq-node checkpoint-sign --authority DIR --network testnet --height H
  --hash <hash> --out cp.json` firma solo con las claves compiladas y verifica
  el resultado antes de imprimirlo. Se entrega con
  `Invoke-RestMethod -Method Post -ContentType application/json -InFile cp.json
  http://127.0.0.1:9140/checkpoint` y los nodos se lo pasan entre sí.
- **Probado** con tres procesos reales y claves descartables de
  `zoka checkpoint-keygen`:
  - la autoridad, sola durante 25 s, no firmó nada;
  - con un seguidor conectado firmó el 4;
  - un nodo que había minado solo una cadena de 30 bloques aceptó el
    checkpoint y pasó a la de 11 de la autoridad;
  - un checkpoint firmado a mano y entregado al seguidor llegó a los tres;
  - uno falsificado se rechazó (422).

## Pendiente

- Crear la mitad ML-DSA de la clave de Zenith (la crea el usuario en la PC) y
  compilarla; la de Quantic, antes del génesis.
- **Mientras un par anuncia una punta más alta, la autoridad no firma** (figura
  sincronizando). Es lo correcto si esa punta es real; un par que mienta su
  altura puede frenar las firmas nuevas, no las viejas (el vigente se reenvía
  igual). Revisar antes de Quantic. En la v2 pasa lo mismo: un par saliente que
  informa una punta más alta, aunque sea de una rama que el checkpoint ya
  descarta, frena las firmas mientras dure la sesión.
