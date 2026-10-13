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

## Pendiente

- **Firma híbrida** (versión 2: Ed25519 + ML-DSA-65), con la etapa 5.
- **Mientras un par anuncia una punta más alta, la autoridad no firma** (figura
  sincronizando). Es lo correcto si esa punta es real; un par que mienta su
  altura puede frenar las firmas nuevas, no las viejas (el vigente se reenvía
  igual). Revisar antes de Quantic.
