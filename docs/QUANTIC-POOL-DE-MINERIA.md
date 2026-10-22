# Pool de minería de Zoka Quantic (borrador de diseño)

22/10/2026. Pedido del usuario: **diseñar el pool ahora**, para construirlo
después. Antes había preguntado si convenía un Stratum o seguir con minería
individual; la recomendación fue minería individual por ahora y, más adelante,
un pool sin custodia. Es un borrador: nada está construido.

## Por qué ZSilent 3.0 no tiene pool

El minado en equipo de Zenith (`src/team_mining`) viajaba en mensajes de Zenith
(`TeamAdvertise`, `TeamJoin`, `ShareSubmit`, `TurnUpdate`), con plantillas de
bloques de Zenith. En Quantic eso no existe, y desde el 22/10 un proceso de
Quantic descarta esos mensajes en la entrada (`quantic_message_gate`), para que
las redes no se mezclen. El **POOL OFF** de ZSilent es lo que quedó.

## Qué tiene que cumplir

- **Sin custodia:** nadie tiene las monedas de otro, ni por un rato.
- **Privado:** en la cadena no se ve quién cobra. El coinbase de Quantic ya es
  una nota blindada.
- **Sin servidores:** la red no tiene servidores y la PC del equipo no puede
  abrir puertos. Todo tiene que ir por gossip, Tor o iroh.
- **Que sirva:** que un minero chico cobre seguido en vez de esperar un bloque
  entero.

## Lo que pone el código

- **Coinbase:** el consenso admite hasta 64 acciones (`MAX_ACTIONS_PER_TX`); solo
  el génesis exige una (`check_coinbase` en `crates/zoka-quantic/src/state.rs`).
  El minero de hoy arma una sola salida (`build_coinbase`). Un coinbase que les
  pague a varios **no necesita fork**.
- **Costo de prueba:** cada salida del coinbase es una acción de Orchard con su
  prueba Halo 2. Un coinbase de muchas salidas tarda en probarse, y hay que
  rehacerlo cada vez que cambia la plantilla, porque entra en el `merkle_root`
  que hashea el PoW. Con 2 acciones es del orden de un segundo; con 64, a medir
  (decenas de segundos, estimado).
- **Direcciones:** 1.734 caracteres cada una. Con las direcciones cortas
  ([QUANTIC-DIRECCIONES-CORTAS.md](QUANTIC-DIRECCIONES-CORTAS.md)) se comparten
  fácil, pero para armar una salida hace falta la completa.
- **PoW:** RandomX sobre `zoka-quantic-pow-v1 ‖ chain_id ‖ prev_hash ‖ merkle_root
  ‖ state_root ‖ altura ‖ hora ‖ nonce`, con el nonce de 8 bytes al final
  (`build_quantic_pow_input` en `src/mining/pow.rs`). **XMRig no puede minar
  Zoka hoy:** solo cambia 4 bytes en la posición 39 de un blob con el formato de
  Monero.

## Opciones

**A. Pool Stratum clásico (con custodia).** El operador cobra cada bloque y les
paga después a los mineros. Es lo que usan Monero y XMRig. Descartada como
diseño de Zoka:

- el operador tiene la plata de todos y los registros de aportes;
- necesita un servidor prendido todo el día;
- XMRig igual no minaría sin cambiar el formato del PoW.

**B. Equipos sin custodia (lo de Zenith, llevado a Quantic).** Un nodo del
equipo coordina y otro queda de respaldo.

- Cada plantilla paga el coinbase entero, **en una sola salida**, al miembro al
  que le toca el turno.
- Los miembros minan *shares* de RandomX a la dificultad del equipo (uno cada
  15 s más o menos), y el coordinador los valida contra la plantilla exacta.
- Los turnos se reparten en proporción al trabajo aportado. Esa regla ya está
  escrita y probada en Zenith (`payout_turn_converges_by_contributed_work`).
- Los mensajes serían propios de Quantic, en los tópicos de cada red, con
  invitación por código.

| A favor | En contra |
| --- | --- |
| El coinbase sigue con una salida: el costo de prueba de hoy | El coordinador ve las direcciones y el hashrate de los miembros |
| No cambia el consenso | Podría hacer trampa con los turnos. Los contadores son del equipo, así que se nota, y los miembros se van o eligen otro coordinador |
| Reutiliza la lógica de Zenith y sus pruebas | Hace falta confianza dentro del equipo, no con toda la red |
| Funciona por gossip, sin servidores | Cada bloque le paga entero a uno: el reparto se empareja a lo largo de varios bloques |

**C. Cadena de shares estilo P2Pool (sin custodia y sin coordinador).** Los
mineros arman entre todos una cadena de shares, y cada bloque real les paga en
su coinbase a los que aportaron en la última ventana. Nadie coordina. En contra:

- el costo de prueba de un coinbase de muchas salidas en cada cambio de
  plantilla;
- las direcciones completas de los que cobran viajan en la cadena de shares;
- es otra cadena, con sus propias reorganizaciones.

Una variante con menos costo: cada bloque les paga solo a los N con más saldo
acumulado (por ejemplo N = 8).

## Recomendación

1. **Etapa 1: B, equipos sin custodia en Quantic.** Es lo que tenía Zenith,
   encaja con el coinbase de una salida y no cambia el consenso. Se prueba en la
   testnet.
2. **Etapa 2, después de la mainnet y si hay muchos mineros:** C con N salidas
   limitado.
3. **Decisión antes del génesis de la mainnet: ¿un formato de PoW compatible
   con XMRig?** Habría que poner un nonce de 4 bytes en la posición 39 de un
   blob de largo fijo, como Monero. Es consenso: o se fija antes del génesis, o
   después hace falta un fork.
   - A favor: los mineros usan XMRig, que es rápido y conocido, y cualquier
     proxy de Stratum.
   - En contra: hay que cambiar el encabezado del PoW, y con 4 bytes de nonce la
     plantilla se agota más rápido (se renueva con un extranonce en el coinbase).
   - Antes de decidir, medir el minero propio contra XMRig en la misma PC.

## Plan de la etapa 1

1. **Red:** mensajes de equipo propios de Quantic, en los tópicos de cada red,
   que `quantic_message_gate` deje pasar solo en Quantic. Estado del equipo
   reutilizando `src/team_mining`, sin lo de Zenith.
2. **Plantillas:** el runtime de Quantic arma el bloque con el coinbase al
   miembro del turno. Shares de RandomX contra el objetivo del equipo, validados
   en el coordinador.
3. **Respaldo del coordinador**, como en Zenith: toma el lugar a los 90 s sin
   noticias.
4. **API y línea de comandos:** crear equipo, invitar, unirse, salir, ver
   turnos.
5. **ZSilent:** vuelve la pantalla del pool para Quantic (POOL ON), con equipo,
   invitación, aportes y turnos.
6. **Pruebas:** red local de 3 nodos en regtest, con un coordinador que se cae y
   un miembro que miente, y después la testnet.

## Mientras tanto

La minería individual funciona en ZSilent 3.0.1. La píldora POOL OFF podría
decir "próximamente" hasta la etapa 1.

## Preguntas abiertas

- ¿Cuántos miembros puede tener un equipo, y cómo se elige al coordinador?
- Las comisiones de las transacciones del bloque van al del turno: ¿se cuentan
  en el reparto?
- ¿Qué pasa con el bloque que encuentra un miembro justo cuando cambia el turno?
  En Zenith la plantilla vieja seguía valiendo hasta la siguiente.
