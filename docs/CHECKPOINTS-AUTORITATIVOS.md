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
  el último checkpoint; rotar la clave exige un binario nuevo. Con una
  autoridad de k de n (versión 3, más abajo) perder una clave no frena nada
  mientras queden k.
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

## Claves cifradas en disco (10/11/2026)

Rama `endurecimiento-sin-consenso`. Los dos archivos de la autoridad son 32
bytes en hex, sin cifrar: quien copia el archivo (o un respaldo) tiene la
clave. Ahora el nodo también los lee cifrados con una contraseña.

- **Formato:** un JSON con `schema` (`zoka-authority-key-v1`), `key`
  (`ed25519` o `ml-dsa-65`, para no tomar una mitad por la otra) y la semilla
  cifrada con Argon2id y AES-256-GCM, igual que la identidad del nodo.
- **Lectura:** si el archivo empieza con `{` está cifrado y el nodo toma la
  contraseña de `ZOKA_AUTHORITY_PASSWORD`. Sin ella, o con otra, no arranca
  como autoridad y lo dice. Un archivo en hex se lee como siempre: nada
  cambia para quien no cifra.
- **Cifrar:** con la contraseña en esa variable,
  `quantic-pq-node authority-encrypt --authority DIR`. Antes de tocar nada
  comprueba que las dos mitades estén y se lean. Escribe cada archivo
  cifrado aparte, lo vuelve a leer con la contraseña y recién entonces lo
  pone en lugar del original.
- **No borra nada.** La clave en claro queda al lado como
  `checkpoint-authority.key.plain-backup` (y la ML-DSA igual). La borra quien
  cifró, después de ver al nodo firmar un checkpoint con los archivos nuevos.
  Los respaldos fuera de la PC también hay que cambiarlos por los cifrados.
- **Sirve para los dos nodos:** `load_signing_key` y `load_mldsa_seed` son
  las mismas funciones que usan `zoka checkpoint-sign` y el nodo de la v1.

La contraseña quedó como opción para quien firme a mano o fuera de ZSilent.
En la PC autoridad manda lo de abajo.

## Protección automática con la cuenta de Windows (11/11/2026)

Decisión del usuario (11/11, a Codex): **«todo lo que sea ZSilent y ZKAPriv
debe ser automático, que el usuario haga lo menos posible»**. Entonces la PC
autoridad no pide contraseña: las claves se protegen con la cuenta de
Windows. Lo empezó Codex y lo terminó Claude, en la misma rama (nodo en
`src/checkpoint_authority/protection.rs`; ZSilent en su rama
`endurecimiento-sin-consenso`).

- **Al arrancar:** si encuentra las claves, ZSilent lanza el nodo con
  `--authority --authority-protection windows-user`. El nodo valida las dos
  mitades contra las compiladas y las reescribe protegidas con DPAPI para el
  usuario actual (`zoka-authority-windows-user-v1`). No queda copia en claro.
  Cada mitad se escribe aparte, se vuelve a abrir y recién entonces
  reemplaza al original (`ReplaceFileW`, que conserva los permisos del
  archivo). Un corte entre las dos mitades se retoma solo en el arranque
  siguiente. Si ya están protegidas, no se toca nada.
- **Lo que protege:** una copia suelta de los archivos (otra PC, otro
  usuario, un disco robado) no abre. **Lo que no:** un programa que corra
  como el mismo usuario de Windows puede pedirle a DPAPI que la abra.
- **Si falta una mitad**, el nodo no arranca como autoridad: falla en vez de
  dejar de firmar sin avisar.
- **Respaldo:** «Export encrypted backup» de ZSilent (Settings > Security),
  con la contraseña que elige el usuario. En la PC autoridad el archivo lleva
  además la recuperación de la autoridad (`quantic-pq-node authority-backup`),
  cifrada aparte con Argon2id y AES-256-GCM y la misma contraseña. Nada de
  la clave pasa por Kotlin.
- **Restaurar:** «Restore encrypted backup» primero comprueba que la
  recuperación sea de esta cadena y no choque con claves que ya haya
  (`authority-restore --check-only yes`), importa la billetera y después
  devuelve las claves, protegidas para el usuario actual. La billetera se
  importa sola: la recuperación de la autoridad no queda adentro del archivo
  de la billetera ni de sus respaldos.

**Riesgo, y qué hacer antes de instalarlo:** DPAPI ata la clave al perfil de
Windows. Si el perfil se pierde (Windows reinstalado, cuenta nueva o una
contraseña reseteada por un administrador), la copia protegida no se abre.
Entonces la recuperación es el respaldo cifrado de ZSilent o el respaldo en
D:. **Antes de instalar en la PC autoridad una ZSilent con esto**, conviene
confirmar que el respaldo en D: está al día o exportar el respaldo cifrado
apenas arranque.

**No está instalado.** Las claves de la PC siguen en hex hasta que el usuario
instale esa versión.

Lo que esto no resuelve: una sola clave y una sola PC. Repartir la firma
(dos de tres firmantes) es un cambio de la regla de aceptación: lo de abajo.

## Versión 3: k de n firmantes (protocolo v3, 11/11/2026)

Punto 9 de la revisión del 10/11, rama `protocolo-v3`. Con una sola clave,
quien la tenga (o quien entre a la PC) mueve los checkpoints de toda la red, y
si la clave se pierde, no hay cómo firmar. Ahora una cadena puede tener una
autoridad de **k de n claves**: ninguna sola alcanza, y perder una no frena
nada mientras queden k.

- **Formato:**

  ```json
  {"version":3,"chain_id":"…","height":180,"block_hash":"…64 hex…",
   "cosignatures":[{"signer":0,"ed25519":"…128 hex…","mldsa":"…6.618 hex…"},
                   {"signer":2,"ed25519":"…","mldsa":"…"}]}
  ```

  - Cada firma es híbrida (las dos mitades obligatorias) y cubre
    `"zoka-signed-checkpoint-v3" ‖ 0x1f ‖ chain_id ‖ 0x1f ‖ height ‖ 0x1f ‖
    block_hash`. El dominio es otro que el de la v2: una firma de una no sirve
    en la otra.
  - `signer` es el lugar de la clave en la lista compilada. Las firmas van en
    orden de clave y sin repetir. Hasta 8 claves.
  - No lleva `signature` ni `mldsa_signature`.
- **Política:** una cadena de `economics::CHECKPOINT_AUTHORITY_COSIGNERS` (y
  de la misma lista en `zoka-light`) solo acepta la versión 3, y una de una
  sola clave, la 2. En el conjunto ninguna mitad se repite entre dos claves:
  la misma persona no cuenta dos veces.
- **Firmas parciales:** cada clave firma por su lado, en su propio nodo. Una
  versión 3 con menos firmas que el umbral es parcial:
  - el nodo verifica cada firma y la guarda (las de los 8 bloques más altos);
  - se suma a las que ya tenía de ese bloque;
  - en cada sesión empuja sus parciales a los pares que no tienen un
    checkpoint igual o más alto, una vez cada una;
  - el nodo donde se juntan k firmas acepta el checkpoint entero, y desde ahí
    viaja como cualquier otro (`HEAD`, `CHECKPOINT`, `SUBMIT_CHECKPOINT`).
  - `SUBMIT_CHECKPOINT` contesta `partial`, y `POST /checkpoint`,
    `"partial": true`.
- **Alturas alineadas:** dos firmas solo se juntan si son del mismo bloque,
  así que todas las claves firman las mismas alturas: los múltiplos de 30 que
  quedan 6 bloques por debajo de la punta (`aligned_height_to_sign`), con las
  mismas condiciones de siempre (la red confirma la punta y la cadena contiene
  el último checkpoint).
- **Una clave no firma dos bloques a la misma altura.** Cada nodo firmante
  anota la última altura que firmó (`checkpoint:signed:own`) antes de soltar
  la firma, y nunca firma esa ni una más baja. Si una reorganización le
  cambiara el bloque, dos hashes con su firma podrían completar dos
  checkpoints distintos.
- **Firmar a mano:** `checkpoint-sign` con una autoridad de k de n da la firma
  de esa clave (versión 3, parcial). Cada uno firma la misma altura y el
  mismo hash y la entrega con `POST /checkpoint`; el nodo las junta.
- **Claves del firmante:** `--authority DIR` acepta cualquiera de las claves
  del conjunto. El nodo sabe con eso qué número de clave es. El respaldo y la
  restauración (`authority-backup` / `authority-restore`) aceptan también
  cualquiera.
- **Red local de prueba:** `ZOKA_CHECKPOINT_AUTHORITY_THRESHOLD` (k) y
  `ZOKA_CHECKPOINT_AUTHORITY_COSIGNERS` (las otras claves, `ed25519:ml-dsa`
  en hex, separadas por comas), junto con las dos de la primera clave. Si
  algo de eso no es válido, la cadena no acepta ningún checkpoint.
- **Billeteras:** `zoka-light` (billetera web y ZokaScan) y el motor de
  ZKAPriv verifican la versión 3 con el mismo umbral. Solo aceptan el
  checkpoint entero.
- **Probado** (`checkpoint_authority`, `stark_node`, `zoka_light_equivalence`):
  - una firma sola no alcanza;
  - la misma firma dos veces no suma;
  - con la segunda clave se completa;
  - una clave ajena, repetida, fuera de orden o con otro número no cuenta;
  - otro bloque a la misma altura es un conflicto;
  - lo que junta el nodo vale en `zoka-light`, y una parcial no vale en
    ninguno de los dos;
  - con tres nodos en loopback y dos de tres: la primera clave sola no fijó
    nada; con la segunda, en otro nodo, los tres tuvieron el checkpoint con
    las dos firmas, y la tercera clave no hizo falta.

**La testnet v3 arranca con uno de uno**: la clave de siempre (la PC), ya en
la versión 3. Pasar a dos de tres es agregar las otras dos claves y subir el
umbral, en el nodo y en `zoka-light`, con nodos y billeteras nuevos. **Lo
decide el usuario**, porque es dónde vive cada clave. Una propuesta:

- la PC (la clave de hoy);
- el VPS 1, con su propia clave;
- una tercera sin conexión, en D: o en un pendrive, que solo firma a mano
  si falta una de las otras dos.

Con eso, ni la PC sola ni el VPS solo mueven la red, y perder cualquiera de
los tres no frena las firmas. Conviene decidirlo antes del génesis de la v3:
cambiar el conjunto después obliga a actualizar todas las billeteras.

## Pendiente

- Crear la mitad ML-DSA de la clave de Zenith (la crea el usuario en la PC) y
  compilarla; la de Quantic, antes del génesis.
- **Mientras un par anuncia una punta más alta, la autoridad no firma** (figura
  sincronizando). Es lo correcto si esa punta es real; un par que mienta su
  altura puede frenar las firmas nuevas, no las viejas (el vigente se reenvía
  igual). Revisar antes de Quantic. En la v2 pasa lo mismo: un par saliente que
  informa una punta más alta, aunque sea de una rama que el checkpoint ya
  descarta, frena las firmas mientras dure la sesión.
