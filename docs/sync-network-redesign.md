# Hoja de trabajo: red, sincronizacion y estabilidad ZOKA

Fecha de trabajo: 2026-07-02  
Alcance: analisis del codigo local del repositorio, con foco en conexion entre nodos, IBD/sync, propagacion de bloques, estabilidad operacional y tecnologia conveniente para el siguiente salto. No incluye logs vivos de VPS ni mediciones de red en produccion.

## 0. Tesis ejecutiva

ZOKA no esta debil porque use mala tecnologia base. La base elegida puede servir: Rust, RocksDB, RandomX, libp2p, PoW acumulado, snapshots, API RPC y wallet privada son piezas razonables. El problema real es de arquitectura de red:

- El P2P existe, pero no esta organizado como protocolo de sincronizacion blockchain.
- Gossipsub se usa como bus generico para pedir y responder bloques completos.
- HTTP RPC se volvio un segundo plano de consenso de facto: sync por rango, `/sync/block`, watchdog y autopush.
- La red depende demasiado de dos nodos core en la misma region/operador.
- El descubrimiento de peers no persiste ni alimenta de verdad al DHT.
- No hay separacion clara entre "anuncio", "descarga", "validacion", "fork-choice", "estado de sync" y "operacion".

El cambio de paradigma recomendado es:

> Mantener el consenso PoW y la logica ZK/privacidad de ZOKA, pero reconstruir la red como un stack P2P tipado: peerstore persistente + block inventory gossip + request/response para headers/bloques + headers-first IBD + snapshots firmados/chunkeados + health/metrics reales. RPC queda como conveniencia y fallback, no como columna vertebral de la red.

## 1. Evidencia del codigo actual

### 1.1 P2P existe, pero como transporte generico

Archivos clave:

- `src/network/p2p.rs`
- `src/network/message.rs`
- `src/main.rs`
- `src/state/sync.rs`

Hechos observados:

- `Node::new` monta libp2p con TCP + Noise + Yamux + Identify + Kademlia + Gossipsub.
  - Evidencia: `src/network/p2p.rs:980-1035`.
- Kademlia usa `MemoryStore`, por lo tanto no persiste tabla de routing entre reinicios.
  - Evidencia: `src/network/p2p.rs:1005-1008`.
- No aparece uso productivo de `kademlia.add_address`, `kademlia.bootstrap` ni `get_closest_peers` para descubrimiento general de nodos.
  - El DHT se usa para user records (`put_record`/`get_record`), no como peer discovery completo.
  - Evidencia: `src/network/p2p.rs:1722-1766`.
- `src/network/discovery.rs` y `PeerManager` tienen estructuras de reputacion/peer tracking, pero no gobiernan el runtime del nodo.
  - Evidencia: `src/network/discovery.rs`, `src/network/p2p.rs:213-288`.

Diagnostico:

- Hay libp2p, pero no hay una "capa de red blockchain" madura encima.
- Kademlia esta presente como dependencia, pero no sostiene la red.
- El nodo aprende peers por seeds/manual dial y conexiones activas, pero no construye memoria historica de red.

### 1.2 Mensajeria: un enum grande para todo

`ZokaMessage` mezcla:

- transacciones publicas,
- transacciones privadas,
- stem/mixer,
- `BlockRequest`,
- `BlockResponse`,
- `SyncState`,
- `Ping`,
- PBFT,
- user registry/query.

Evidencia: `src/network/message.rs:31-76`.

Problemas:

- El mismo topic general transporta mensajes con semanticas muy distintas.
- `BlockRequest` y `BlockResponse` son mensajes de gossip, no streams directos peer-to-peer.
- No hay correlation id de request/response.
- No hay peer target explicito.
- No hay control de backpressure por peer.
- No hay penalizacion de peer por timeout/respuesta mala.
- El campo `encrypted_block` en `BlockResponseMessage` realmente contiene un bloque serializado JSON, no un bloque cifrado.
  - Evidencia: `src/main.rs:6262-6273`.

Riesgo:

- Para una blockchain, pedir bloques por pubsub es fragil. Pubsub es bueno para anuncios y propagacion eventual; no es bueno como canal principal para descarga exacta de datos historicos.

### 1.3 IBD P2P sigue siendo secuencial

El path P2P puro:

- `run_sync_step` calcula el siguiente bloque requerido.
- Llama a `node.request_block_with_retry(...)`.
- Espera un unico `BlockResponse` que coincida.

Evidencia:

- `src/main.rs:6167-6241`
- `src/network/p2p.rs:1488-1553`
- `src/state/sync.rs:474-485`

Problemas:

- Descarga un bloque por paso en P2P.
- La respuesta puede venir de cualquier peer.
- Si la respuesta no llega, se re-publica el request al topic.
- Si llegan bloques de otras alturas, se difieren en memoria.
- No hay pipeline de multiples bloques.
- No hay seleccion de mejores peers.
- No hay headers-first.

Consecuencia:

- A baja altura puede funcionar. A miles o millones de bloques, el modelo no escala.
- Un peer lento o una malla gossipsub pobre frena el catch-up.
- El nodo queda mas cerca de "cliente que pregunta al aire" que de "full node sincronizando con protocolo de bloques".

### 1.4 RPC sync compensa las debilidades del P2P

El path RPC es mas eficiente que el P2P:

- `run_rpc_sync_batch` consulta alturas remotas.
- Usa `/blocks/range/:from/:to/raw`.
- Cachea hasta 128 respuestas de rango.
- Aplica hasta `max_blocks` por ciclo.

Evidencia:

- `src/main.rs:6353-6547`
- `src/main.rs:6420-6459`

Esto es pragmatico y probablemente salvo la red. Pero como arquitectura tiene un costo:

- La sincronizacion real depende de HTTP y de los RPC core.
- Los nodos publicos se vuelven fuente de verdad operacional.
- Si BOOT/RPC fallan, el P2P deberia cubrir, pero hoy no parece suficientemente fuerte.
- La red se comporta mas como "fullnodes siguiendo bootstrap RPC" que como red P2P autonoma.

### 1.5 `/sync/block` y autopush son muletas centrales

Existe un endpoint para que mineros externos empujen bloques por HTTP:

- `POST /sync/block`
- Habilitado por env `ZOKA_API_ENABLE_BLOCK_SUBMIT`.
- En deploy mainnet se habilita.

Evidencia:

- `src/api/mod.rs:4417-4525`
- `.github/workflows/deploy-mainnet.yml:123`

Problemas:

- Es util para mineros detras de NAT, pero convierte al RPC en carril de propagacion de bloques.
- Bypassea la negociacion de protocolo libp2p.
- El endpoint usa `MAINNET_CHAIN_ID` hardcodeado.
  - Evidencia: `src/api/mod.rs:4481-4484`.
- El mismo comentario del codigo reconoce que P2P gossip solo no alcanza para mineros NAT.
  - Evidencia: `src/main.rs:8246-8249`.

Conclusion:

- `/sync/block` debe quedar como fallback autenticado/rate-limited, no como estrategia primaria de propagacion.

### 1.6 Seeds y manifiesto de red: poca redundancia real

`docs/mainnet-network.v1.json` contiene:

- 2 seed nodes activos.
- Ambos en `nyc`.
- Ambos operador `zoka-core`.
- Transporte solo TCP.
- `snapshot_signers`: vacio.

Evidencia: `docs/mainnet-network.v1.json`.

Ademas, el deploy de mainnet arranca servicios con `--no-network-config`:

- Bootstrap: `--no-network-config`.
  - Evidencia: `.github/workflows/deploy-mainnet.yml:102-114`.
- RPC: `--no-network-config` + `--seed /ip4/${BOOTSTRAP_IP}/tcp/28080`.
  - Evidencia: `.github/workflows/deploy-mainnet.yml:207-217`.

Problemas:

- El manifiesto existe, pero la infraestructura core lo salta en deploy.
- El bootstrap primario no usa la lista de seeds porque el mismo es seed.
- El RPC secundario depende manualmente del bootstrap.
- No hay diversidad geografica ni de operador.
- No hay DNS seed ni dnsaddr.
- No hay peerstore persistente local.

Conclusion:

- La red no puede sobrevivir bien a caida de BOOT si el resto de nodos no conservan peers y no tienen varios seeds independientes.

### 1.7 Health endpoint demasiado pobre

`/health` responde solo:

```json
{"status":"healthy"}
```

Evidencia: `src/api/mod.rs:2058-2061`.

`HealthChecker` ya quedo integrado al API principal via `/health/checks` durante Fase 0.

Evidencia:

- `src/health/mod.rs:79-92`
- `src/health/mod.rs:111-185`

Problemas:

- No informa altura local.
- No informa target remoto.
- No informa peers conectados.
- No informa si esta en IBD.
- No informa ultimo bloque recibido/minado.
- No informa RPC fallback activo.
- No informa errores recientes de sync.

Conclusion:

- Un proceso puede estar "healthy" y aun asi estar aislado, atrasado o congelado.

### 1.8 Watchdog actual revela el problema de fondo

`scripts/zoka-sync-watchdog.sh` compara BOOT vs local y empuja bloques via RPC si hay gap.

Evidencia:

- `scripts/zoka-sync-watchdog.sh`
- `deploy/systemd/zoka-sync-watchdog.timer`

Esto es bueno como apagafuegos, pero malo como arquitectura permanente:

- BOOT se vuelve fuente de recuperacion.
- El sistema externo corrige lo que el protocolo P2P deberia resolver.
- La salud de la red depende de `curl` + `/sync/block`.

### 1.9 Snapshots existen, pero no como state sync de red

`src/snapshot.rs` permite export/import de snapshots canonicos.

Evidencia:

- `src/snapshot.rs:1-27`
- CLI: `snapshot-export`, `snapshot-import`.

Limitaciones:

- El snapshot actual captura bloques, no un estado compacto verificable que evite replay completo.
- No hay protocolo P2P para listar snapshots.
- No hay chunks.
- No hay snapshot signers activos en mainnet manifest.
- No hay seed/RPC que sirva snapshots como parte del boot normal.

Conclusion:

- La pieza existe como herramienta offline, no como state sync superior integrado.

### 1.10 Fork-choice y reorg: avance real, pero aun con riesgo

`state::chain` valida PoW y calcula trabajo acumulado. Tambien guarda side branches y puede promover rama mas pesada.

Evidencia:

- `src/state/chain.rs:383-403`
- `src/state/chain.rs:680-694`

Pero hay senales de deuda:

- Existe comentario de "future fork-choice" y rollback de estado como migracion de mayor riesgo.
  - Evidencia: `src/state/chain.rs:697-710`.
- Hay dos caminos de aplicacion:
  - `StateManager::apply_block` aplica transacciones y persiste estado.
  - `state::chain::apply_block` valida PoW/metadatos y persiste cadena.
  - Evidencia: `src/state/mod.rs:294-324`, `src/state/chain.rs:588-679`.

Riesgo:

- Si se agregan mas transacciones publicas o mas estado no trivial, los dos caminos pueden divergir.
- Reorg profundo necesita rollback/replay completo de estado, no solo indices de altura/nullifiers.

### 1.11 ZSilent: el usuario-minero depende demasiado del nodo local

ZSilent agrega un problema de producto y red que no es menor: el usuario que quiere minar no esta usando solo una wallet. En la practica se le exige operar un full node local, mantenerlo sincronizado, mantenerlo actualizado, sostener el RPC local y ademas minar. Cuando cualquiera de esas piezas falla, la mineria se vuelve imposible para el usuario final.

Evidencia:

- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/node/NodeController.kt`
- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/mining/MiningController.kt`
- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/services/ZokaCli.kt`
- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/config/ZokaNetworkConfig.kt`

Flujo actual observado:

- ZSilent arranca `zoka fullnode`.
- Lo arranca con `--no-mine` y despues controla la mineria por RPC local.
- Habilita `ZOKA_API_ENABLE_MINING_CONTROL=true`.
- `MiningController` inicia mineria con `POST /mining/start` contra `http://127.0.0.1:3000` o el RPC local del manifest.
- Si ese RPC local no responde, la app no puede iniciar mineria.
- `NodeController` reintenta reiniciar el nodo hasta 5 veces; despues queda en error y el usuario debe intervenir.
- El nodo puede arrancar con `--allow-isolated-start`, lo cual ayuda al arranque, pero tambien permite quedar vivo sin estar realmente conectado a la red.
- Existe logica de version minima en configuracion, pero el arranque del nodo no queda claramente bloqueado por cliente obsoleto antes de que el usuario intente operar.

Esto genera una falla de experiencia:

- El usuario cree que "abre ZSilent y mina".
- En realidad necesita que:
  - el binario este actualizado,
  - el nodo local arranque,
  - el RPC local responda,
  - el nodo encuentre peers,
  - el nodo sincronice,
  - el nodo no quede aislado,
  - el nodo no caiga despues de 5 reintentos,
  - el minero no este sobre un tip viejo.

Y genera una falla de red mas seria:

- Los mineros no solo producen bloques; tambien terminan siendo parte de la disponibilidad de transacciones.
- Si pocos mineros/nodos reciben cuerpos de transacciones privadas, las tx pueden quedar varadas.
- Si ZSilent no puede mantener el nodo local vivo, ese minero no solo deja de producir: tambien deja de ayudar a empujar transacciones de otros usuarios.
- El resultado es una red donde cada caida de nodo de escritorio reduce hashpower, propagacion y disponibilidad de mempool al mismo tiempo.

Conclusion:

- No conviene seguir tratando a ZSilent como un simple launcher de nodo.
- ZSilent debe tener un supervisor de nodo/mineria con estado explicito, auto-reparacion y reglas duras de seguridad.
- La red debe separar mejor los roles de wallet, relay, full node, miner y archive.
- El usuario que quiere minar no deberia quedar bloqueado por detalles invisibles de sync; la app debe decir exactamente "syncing", "isolated", "outdated", "rpc unreachable", "snapshot needed" o "ready to mine".

### 1.12 Incidente P0 confirmado: arranque, bootstrap y gossip no disponibles

Los sintomas observados convierten el problema en disponibilidad critica, no solo en deuda tecnica.

Sintomas concretos reportados:

- IBD/arranque bloquea la API:
  - el nodo carga ~836 bloques desde RocksDB al arrancar,
  - Tokio no atiende conexiones durante ese periodo,
  - se acumulan ~115 conexiones pendientes,
  - hay timeouts externos durante 3-5 minutos.
- No hay peer store persistente:
  - cada arranque olvida peers historicos,
  - si BOOT esta caido o inaccesible en la ventana inicial, el nodo queda aislado hasta reinicio manual.
- `src/network/discovery.rs` es codigo muerto:
  - tiene estructura de peers/reputacion,
  - pero no esta conectado al flujo productivo de `Node`,
  - no salva peers ni alimenta redial.
- Gossip no esta propagando bloques en produccion:
  - se observan cero mensajes gossip reales,
  - los nodos descubren bloques por tick de sync,
  - no hay propagacion en tiempo real.
- Puerto P2P `28080` cerrado al publico en BOOT:
  - si UFW permite solo IPs internas/personales, mineros externos no pueden hacer bootstrap,
  - el manifest puede anunciar BOOT, pero la red real lo bloquea.

Lectura tecnica:

- La API se inicia tarde. En `cmd_fullnode_auto`, primero se abre/carga estado con `open_state_manager(db).await`, y recien despues se spawnea `start_api_on_bind_with_broadcaster`.
- `open_state_manager` llama `StateManager::load_from_storage`.
- `load_from_storage` hace lecturas sincronas de RocksDB y reconstruye estado antes de que exista una API viva.
- Si esa carga demora, `/health`, `/chain/height`, `/sync/status` y endpoints de control no existen todavia.
- La red no tiene memoria: Kademlia usa `MemoryStore`, no un peerstore persistido.
- La unica realidad de bootstrap son seeds conocidos; si el seed no acepta conexiones, no hay segunda linea.

Impacto:

- ZSilent puede fallar al iniciar mineria aunque el usuario no tenga ningun problema local real.
- Los RPC externos ven timeouts y acumulan cola.
- La red queda centralizada de facto en BOOT.
- Si BOOT tiene `28080` cerrado, el bootstrap publico esta roto aunque el servicio este "running".
- Si gossip no propaga bloques, los mineros siempre trabajan con informacion tardia.

Decision:

- Esto debe tratarse como P0 operacional.
- No conviene esperar a headers-first, snapshots o request/response completo para resolverlo.
- Primero hay que restaurar disponibilidad basica: API viva temprano, BOOT alcanzable, peerstore minimo, metricas de gossip y redial.

## 2. Diagnostico duro por area

### 2.1 Conexion entre nodos

Estado actual:

- Conecta a seeds manuales o candidatos del manifest.
- Hace reconnect periodico con `maintain_seed_connections`.
- No persiste peers.
- No usa DHT como red de descubrimiento general.
- No hay DNS seed.
- No hay peer scoring productivo.
- Si BOOT tiene `28080/tcp` cerrado al publico, el bootstrap anunciado no es realmente publico.

Fallas probables:

- Reinicio = perdida de memoria de red.
- Si BOOT cae antes de que el nodo conozca otro peer, queda aislado.
- Si un seed vuelve, el nodo intenta redial, pero solo contra lista conocida inicial.
- Si los seeds son pocos y misma region, hay punto de falla operativo.
- Si UFW/firewall bloquea P2P, los mineros externos no pueden entrar a la red aunque el manifest este correcto.

Lo conveniente:

- Abrir/verificar `28080/tcp` publico en seeds que figuran como publicos.
- `PeerStore` persistente en `{ZOKA_HOME}/peers.json` o RocksDB.
- Guardar multiaddr, peer_id, ultimo exito, ultimo fallo, score, protocolos soportados.
- Cargar peers al boot junto con seeds.
- Agregar Kademlia bootstrap real: `add_address` + `bootstrap`.
- Agregar dnsaddr/DNS seeds para no hardcodear IPs.
- Implementar peer scoring y quarantine por comportamiento malo.

### 2.2 Sincronizacion

Estado actual:

- P2P: un bloque por request, via gossipsub.
- RPC: rango secuencial por HTTP.
- No headers-first.
- No validacion previa de cadena de headers antes de descargar cuerpos.
- No paralelismo de descarga P2P.
- No hay descarga por chunks.
- No control de proveedores por calidad.

Fallas probables:

- Sync lento al crecer la cadena.
- Peers con mala conectividad hacen perder ciclos.
- El nodo no sabe si el tip remoto es consistente hasta aplicar cuerpos.
- Mineros pueden quedar minando sobre altura vieja si se enteran tarde.

Lo conveniente:

- Nuevo protocolo `/zoka/headers/1`:
  - `GetHeaders { locator, stop_hash, max }`
  - `Headers { headers }`
- Nuevo protocolo `/zoka/blocks/1`:
  - `GetBlocks { hashes }`
  - `Blocks { bodies }`
- `BlockAnnounce` por gossipsub:
  - solo `height`, `hash`, `prev_hash`, `work`, `difficulty_bits`.
- IBD headers-first:
  - construir cadena de headers por trabajo acumulado.
  - luego descargar cuerpos en paralelo.
- Pipeline:
  - headers worker,
  - body fetch workers,
  - validation/apply worker,
  - orphan/fork manager.

### 2.3 Propagacion de bloques

Estado actual:

- Cuando se mina, se publica `BlockResponse` completo por gossipsub.
- Tambien se postea por HTTP a RPCs configurados.
- Hay rebroadcast de bloques recientes configurable.
- En produccion se observan cero mensajes gossip utiles para bloques.

Problemas:

- Se gossipea payload pesado en vez de inventario liviano.
- No hay separacion entre "me entere de un bloque" y "descargue/valide el bloque".
- Rebroadcast de cuerpos puede saturar API/gossip.
- HTTP autopush se volvio necesario para NAT.
- Si gossip no entrega nada, el loop de sync se convierte en el unico mecanismo de descubrimiento de bloques.

Lo conveniente:

- Instrumentar gossip antes de redisenarlo:
  - mensajes publicados por tipo,
  - mensajes recibidos por tipo,
  - peers mesh por topic,
  - publish failures,
  - first-seen block latency.
- Gossip solo de anuncios:
  - `BlockAnnounce`.
  - `TxAnnounce`.
  - `HaveBlocks` opcional.
- Descarga de bodies por request/response.
- Nodos NAT pueden publicar anuncios por outbound P2P o fallback RPC autenticado, pero el resto de la red recupera por P2P.
- Retener los ultimos N bloques en memoria para servir rapido.

### 2.4 Estabilidad operacional

Estado actual:

- Servicios con nombres distintos (`zoka-bootstrap`, `zoka-rpc`).
- Watchdog externo repara gaps por HTTP.
- Rate limits se corrigen con workflow ad hoc.
- `/health` es superficial.
- La API se inicia despues de cargar estado desde RocksDB, asi que durante arranque pesado no hay health real.

Problemas:

- La operacion sabe que la red se desincroniza, pero la correccion es externa.
- El operador necesita workflows manuales para rate limits y sync.
- Ya existen `/ready`, `/startup/status`, `/sync/status` y `/health/checks` como base de operacion.
- Aun faltan `/peers`, `/net/diagnostics` y metricas Prometheus completas.
- Si cargar estado tarda 3-5 minutos, los balanceadores/clientes ven timeout en vez de `starting/loading_state`.

Lo conveniente:

- Arranque API-first:
  - bind temprano,
  - `/health` responde `starting`,
  - `/ready` responde 503 hasta terminar carga,
  - `/startup/status` expone fase y progreso.
- Mover hidratacion pesada de RocksDB a `spawn_blocking` o worker dedicado.
- Unificar servicio: `zoka-node.service`.
- Roles por config: `seed`, `rpc`, `miner`, `archive`, no por binarios/servicios distintos.
- Health real:
  - `/health`: proceso vivo.
  - `/ready`: puede servir API.
  - `/sync/status`: `{local_height,target_height,mode,lag,peers,last_block_at,last_sync_error}`.
  - `/peers`: peer count, connected, inbound/outbound, best score.
  - `/metrics`: Prometheus.
- Watchdog basado en salud local, no en BOOT como autoridad unica.

### 2.5 Seguridad de red

Estado actual bueno:

- Identify usa `PROTOCOL_FREEZE_ID` y desconecta peers incompatibles.
- Mensajes inbound tienen validaciones basicas.
- Bloques pasan por validacion PoW al aplicar.

Evidencia:

- `src/network/p2p.rs:689-812`
- `src/state/sync.rs:407-472`

Debilidades:

- No hay reputacion productiva por peer.
- No hay scoring gossipsub configurado.
- No hay rate limiting P2P por tipo de mensaje, salvo mixer.
- No hay ban/quarantine persistente.
- `/sync/block` acepta bloques por HTTP si esta habilitado, sin identidad P2P ni firma de peer.
- SyncState anuncia altura y state root, pero el state_root no se usa para una verificacion fuerte de progreso.

Lo conveniente:

- Peer scoring integrado.
- Limites por mensaje:
  - max block size,
  - max requests por peer/minuto,
  - max in-flight,
  - max bad responses antes de quarantine.
- `BlockAnnounce` firmado por peer libp2p.
- RPC block submit con token/operator allowlist o mTLS si queda habilitado en nodos publicos.

### 2.6 Observabilidad y tests

Estado actual:

- Mucha cobertura unitaria en modulos.
- Poca evidencia de tests integrados multi-nodo reales.
- No hay harness que simule:
  - seed down,
  - partition,
  - peer lento,
  - block storm,
  - fork corta,
  - RPC caido,
  - NAT miner.

Lo conveniente:

- Test harness `zoka-net-sim` con 3-5 nodos locales.
- Tests de caos:
  - apagar BOOT y verificar que siguen sincronizando.
  - iniciar nodo desde cero con solo peerstore.
  - minar dos ramas y verificar fork-choice.
  - limitar ancho de banda artificialmente.
  - medir tiempo IBD a 1k/10k/100k bloques sinteticos.

### 2.7 ZSilent, mineria y disponibilidad de transacciones

Estado actual:

- La mineria de escritorio depende del nodo local.
- El nodo local debe estar sincronizado y con RPC sano para que `POST /mining/start` funcione.
- El usuario no tiene un modo claro de "wallet liviana + miner conectado a relays".
- Si el nodo cae varias veces, ZSilent deja de reintentar despues de un umbral corto.
- Si el nodo queda aislado, puede parecer iniciado, pero no necesariamente esta listo para minar.
- Los mineros terminan empujando transacciones de todos los usuarios, sobre todo transacciones privadas que necesitan disponibilidad de cuerpo.

Fallas probables:

- Usuarios con buen hardware no pueden minar por problemas de red, no por problemas de PoW.
- Mineros atrasados producen bloques stale o directamente quedan bloqueados.
- Transacciones de usuarios dependen demasiado de que ciertos RPC/mineros esten vivos.
- Una caida de nodo de escritorio degrada tres cosas al mismo tiempo:
  - hashpower util,
  - propagacion de bloques,
  - disponibilidad de transacciones.

Lo conveniente:

- Separar roles:
  - `wallet`: firma, consulta balance, envia tx.
  - `relay`: recibe/anuncia tx y bloques, sirve cuerpos.
  - `fullnode`: valida y sincroniza cadena.
  - `miner`: pide trabajo, valida tip, produce bloque.
  - `archive`: conserva historia completa y snapshots.
- ZSilent debe poder operar en dos perfiles:
  - `wallet mode`: no exige full node local.
  - `miner mode`: exige nodo sano, pero con supervisor fuerte y auto-reparacion.
- Mineria debe tener gating explicito:
  - no minar si el nodo esta atrasado,
  - no minar si no hay peers suficientes,
  - no minar si la version esta obsoleta,
  - no minar si no se pudo verificar tip contra al menos N fuentes.
- El mempool privado necesita una capa de disponibilidad:
  - anuncio liviano de tx,
  - fetch por request/response,
  - cache temporal en relays,
  - multiples fuentes por tx,
  - fallback RPC solo como degradacion.

## 3. Cambio de paradigma recomendado

### 3.1 Principio rector

La red debe sobrevivir sin BOOT.

Regla nueva:

- Si BOOT cae 30 minutos, la red no debe detenerse.
- Si RPC cae, los nodos deben seguir descubriendo, anunciando y descargando bloques por P2P.
- Si un nodo reinicia, debe recordar peers historicos.
- Si un minero esta detras de NAT, debe poder publicar salida por P2P o por fallback RPC sin convertir RPC en consenso.
- Si el nodo esta atrasado, debe poder explicar exactamente por que.

### 3.2 Arquitectura nueva por planos

#### Plano A: Peer plane

Responsabilidad:

- Descubrir, recordar, clasificar y reconectar peers.

Componentes:

- `src/network/peer_store.rs`
- `src/network/peer_score.rs`
- `src/network/bootstrap.rs`

Datos persistidos:

```json
{
  "version": 1,
  "peers": [
    {
      "peer_id": "12D3Koo...",
      "addrs": ["/ip4/1.2.3.4/tcp/28080"],
      "protocols": ["/zoka/1.0.2-mainnet-zenith", "/zoka/headers/1"],
      "last_connected_at": 1782940100,
      "last_failed_at": 0,
      "score": 42,
      "source": "seed|dht|identify|manual|peer-exchange"
    }
  ]
}
```

Reglas:

- Redial con backoff + jitter.
- Max peers por subnet / ASN cuando sea posible.
- Quarantine persistente para peers que mandan basura.
- DHT bootstrap real.
- Seeds por DNS + manifest + peerstore.

#### Plano B: Protocol plane

Responsabilidad:

- Protocolos P2P tipados, versionados y negociados.

Mantener:

- libp2p TCP/Noise/Yamux.
- Identify.
- Gossipsub.
- Kademlia.

Agregar:

- `libp2p-request-response` para sync de headers/bloques.
- Protocol IDs:
  - `/zoka/headers/1`
  - `/zoka/blocks/1`
  - `/zoka/snapshots/1`
  - `/zoka/peer-exchange/1`

Uso correcto:

- Gossipsub:
  - anuncios livianos,
  - transacciones,
  - eventos de red.
- Request/response:
  - headers,
  - block bodies,
  - snapshot manifests,
  - chunks.

Por que:

- Gossipsub da propagacion.
- Request/response da exactitud, timeout, peer target y control de backpressure.

#### Plano C: Sync engine

Responsabilidad:

- Saber que falta, a quien pedirlo, como validarlo y cuando aplicar.

Estados:

- `DiscoveringPeers`
- `FindingTip`
- `HeaderSync`
- `BodySync`
- `Applying`
- `CaughtUp`
- `Degraded`
- `Isolated`

Pipeline:

1. Obtener peers.
2. Consultar tips.
3. Pedir headers por locator.
4. Elegir rama por trabajo acumulado.
5. Descargar cuerpos en paralelo.
6. Aplicar en orden.
7. Mantener mempool/private bodies.
8. Publicar estado y metricas.

Reglas:

- No minar si:
  - `sync_state != CaughtUp`,
  - peers < minimo,
  - RPC muestra altura mayor y cadena local no coincide,
  - hay backfill activo.
- No confiar en altura anunciada sin headers.
- No descargar cuerpos de una rama cuyo header chain no supera trabajo local.

#### Plano D: State sync / snapshots

Responsabilidad:

- Arranque rapido de nodos nuevos sin replay eterno.

Nuevo modelo:

- Snapshot manifest firmado:
  - chain id,
  - height,
  - block hash,
  - app/state root,
  - chunks,
  - checksum por chunk,
  - firma ed25519 de signer del manifest.
- Snapshot chunks:
  - servidos por HTTP y P2P.
  - verificados por checksum.
- Despues del import:
  - validar checkpoint/header,
  - continuar por headers/bodies desde snapshot height + 1.

Importante:

- Para no cambiar consenso inmediatamente, se puede iniciar con snapshot de bloques como hoy, pero servido por red y firmado.
- El salto superior real es snapshot de estado verificable, porque evita replay completo.

#### Plano E: Private tx / mempool plane

Responsabilidad:

- Que transacciones privadas lleguen a mineros sin depender de un RPC central.
- Que los mineros tengan disponibilidad de cuerpos de transaccion aunque algunos nodos de escritorio esten caidos.

Cambios:

- `PrivateTxAnnounce { tx_hash, size, fee_commitment? }`.
- Fetch body por request/response si falta.
- Mantener Dandelion/mixer para privacidad, pero no mezclarlo con block sync.
- RPC pull queda como fallback para nodos NAT.
- Bloques canonicos deben incluir cuerpos privados cuando aplique la regla de bundle canonico.
- Cada tx privada debe poder tener varios proveedores:
  - nodo que la recibio,
  - relay publico,
  - minero que ya la descargo,
  - RPC fallback.
- Los mineros no deberian depender de "ver la tx justo por gossip"; deben poder reconstruir mempool desde anuncios + fetch.
- El sistema debe medir:
  - tx anunciadas,
  - tx fetch success/failure,
  - tx sin cuerpo disponible,
  - peers que anuncian pero no sirven cuerpos,
  - latencia desde envio de usuario hasta presencia en mempool minero.

#### Plano F: Operations plane

Responsabilidad:

- Saber si la red esta viva y si el nodo cumple su rol.

Endpoints:

- `/health`: vivo.
- `/ready`: listo para API.
- `/sync/status`: estado de sync.
- `/network/runtime`: ya existe, extender.
- `/peers`: peer table resumida.
- `/metrics`: Prometheus.

Alertas:

- peers = 0 por mas de 60s.
- lag > 3 bloques por mas de 2 target block times.
- no block progress en X minutos pero peers > 0.
- RPC fallback activo demasiado tiempo.
- publish failures > umbral.
- rejected blocks por peer > umbral.

#### Plano G: ZSilent miner/user plane

Responsabilidad:

- Que el usuario de escritorio pueda minar sin convertirse en operador manual de infraestructura.
- Que ZSilent sea capaz de distinguir "nodo iniciado" de "nodo listo para minar".
- Que la app mantenga actualizado y reparable el nodo local.

Estados visibles que ZSilent debe exponer:

- `starting_node`
- `checking_version`
- `outdated_client`
- `syncing`
- `isolated`
- `rpc_unreachable`
- `repairing_from_snapshot`
- `ready_to_mine`
- `mining`
- `paused_stale_tip`
- `paused_low_peers`
- `paused_sync_error`

Cambios:

- Reemplazar el limite duro de 5 reinicios por supervisor de larga vida:
  - backoff exponencial con maximo razonable,
  - contador visible,
  - diagnostico de ultima causa,
  - reparacion automatica si la base local queda corrupta o demasiado atrasada.
- Bloquear mineria si el cliente esta por debajo de `minimum_client_version`.
- Antes de iniciar mineria, validar:
  - `/sync/status` en estado `CaughtUp`,
  - peers >= minimo,
  - ultimo bloque reciente,
  - altura/tip coherente contra al menos dos fuentes si hay RPCs disponibles.
- Si el nodo local cae mientras mina:
  - pausar mineria,
  - conservar intencion del usuario,
  - reiniciar nodo,
  - reanudar solo cuando vuelva a `ready_to_mine`.
- Agregar modo de reparacion:
  - importar snapshot firmado,
  - delta sync,
  - revalidar tip,
  - reactivar mineria.
- Separar UI/estado de:
  - "nodo corriendo",
  - "nodo sincronizado",
  - "nodo conectado",
  - "mineria permitida",
  - "mineria activa".

## 4. Tecnologia recomendada

### 4.1 Recomendacion principal: evolucionar sobre libp2p, no reemplazar todo

Por que conviene:

- Ya esta en Rust y en el repo.
- No requiere abandonar RandomX ni privacidad actual.
- No exige hard fork si se introduce como protocolo de red compatible.
- Permite migracion por etapas.
- Es tecnologia adecuada si se usa con protocolos correctos.

Stack recomendado:

- `libp2p`:
  - `identify`
  - `kad`
  - `gossipsub`
  - `request-response`
  - `ping`
  - `autonat`/relay en fase posterior si se necesita NAT traversal real.
- `serde` al inicio para compatibilidad; considerar `prost`/protobuf o `postcard` para wire format estable y compacto en v2.
- RocksDB para chain/peerstore inicial.
- Prometheus metrics.
- Snapshot manifests firmados con ed25519.

### 4.2 Alternativa CometBFT

Cuando conviene:

- Si ZOKA decide dejar de ser red PoW abierta.
- Si se quiere BFT con finality rapida y set de validadores.
- Si el objetivo es app-chain con ABCI y gobernanza de validadores.

Por que no conviene ahora:

- Cambia el modelo de seguridad.
- Reduce o elimina el caracter de mineria abierta RandomX.
- Requiere migracion grande de consenso, bloques, mempool y operacion.
- La parte ZK/wallet se puede portar, pero no es un cambio incremental.

Decision:

- No recomiendo CometBFT como reemplazo inmediato.
- Si se evalua, debe ser un proyecto paralelo "ZOKA v2 validator network", no un parche sobre mainnet actual.

### 4.3 Alternativa Substrate / Polkadot SDK

Cuando conviene:

- Si se quiere entrar a un ecosistema completo con runtime, pallets, networking, block production/finality y tooling.

Por que no conviene ahora:

- Reescritura profunda.
- El modelo BABE/GRANDPA/PoS no coincide con RandomX PoW abierto.
- ZOKA tiene privacidad y wallet custom que requeriria mucha integracion.

Decision:

- No recomendable para resolver el problema actual de conexion/sync.

### 4.4 Mejor opcion realista

La tecnologia superior no es "tirar libp2p". Es usar libp2p como lo usan las redes maduras:

- gossip para avisar,
- request/response para descargar,
- peerstore para recordar,
- headers-first para decidir,
- snapshots para arrancar,
- metricas para operar.

## 5. Roadmap por fases

### Fase -1 - Mitigacion P0 de disponibilidad (hoy / 24 horas)

Objetivo:

- Que el nodo deje de parecer muerto durante el arranque.
- Que BOOT sea realmente alcanzable por mineros externos.
- Que un reinicio no olvide toda la red.
- Que sepamos si gossip esta vivo o muerto con datos.

Acciones:

- [x] Abrir/verificar `28080/tcp` publico en BOOT y en cada seed publico:
  - [x] deploy mainnet abre `28080/tcp` si `ufw` esta instalado,
  - `ufw allow 28080/tcp`,
  - verificado desde red externa el 2026-07-02 con `scripts/check-node-readiness.ps1`: `159.223.119.216:28080` y `157.230.5.103:28080` aceptan TCP,
  - `docs/mainnet-network.v1.json` mantiene ambos seeds como publicos porque aceptan conexiones P2P.
- [x] Cambiar arranque a API-first:
  - iniciar listener HTTP antes de la carga pesada,
  - `/health` debe responder aunque el estado este cargando,
  - `/ready` debe responder 503 hasta terminar,
  - `/startup/status` debe exponer `opening_db`, `loading_state`, `starting_p2p`, `ready`.
- [x] Mover carga pesada de RocksDB fuera del camino que bloquea el runtime:
  - usar `tokio::task::spawn_blocking` para hydrate de estado,
  - evitar reconstruir estructuras completas si existen metadatos `chain:height`, `chain:last_hash`, `chain:state_root`,
  - cargar indices pesados lazy/on-demand cuando sea posible.
- [x] Agregar peerstore minimo viable:
  - guardar peers conectados en `{ZOKA_HOME}/peers.json`,
  - cargar peers al inicio,
  - redial a peerstore aunque BOOT falle,
  - persistir `last_connected_at`, `last_failed_at`, `score`, `addr`.
- [x] Instrumentar gossip minimo viable:
  - contador global de `publish_ok`/`publish_err`,
  - contador global de mensajes recibidos,
  - contador de `BlockResponse` recibido por gossip,
  - timestamp del ultimo bloque recibido por gossip,
  - pendiente para Fase 1: peers mesh por topic y alerta automatica si `gossip_rx_blocks = 0` mientras la cadena avanza por RPC.
- [x] Agregar check operacional post-deploy:
  - agregado `scripts/check-node-readiness.ps1` para verificar TCP P2P, `/health`, `/ready`, `/sync/status` y `/network/runtime` desde fuera,
  - comando estricto post-deploy: `powershell -ExecutionPolicy Bypass -File scripts/check-node-readiness.ps1 -RequireReady -ReadyWaitSeconds 180`,
  - hallazgo 2026-07-02: P2P `28080` esta abierto, pero `http://159.223.119.216:3000` y `http://157.230.5.103:3000` timeoutean desde fuera,
  - hallazgo 2026-07-02: `boot-rpc.zoka.network` no resuelve DNS; publicar DNS/TLS o corregir `public_rpc` antes de considerar RPC publico release-ready,
  - prueba manual pendiente para caos: reiniciar nodo con BOOT caido y verificar redial desde peerstore en entorno multi-nodo.

Archivos:

- `src/main.rs`
- `src/api/mod.rs`
- `src/network/p2p.rs`
- `src/network/peer_store.rs`
- `.github/workflows/deploy-mainnet.yml`
- `docs/mainnet-network.v1.json`

Criterio de aceptacion:

- Durante carga de RocksDB, la API no timeoutea: responde `health=starting`.
- Un nodo reiniciado puede reconectar con peers previos aunque BOOT no responda.
- Un minero externo puede abrir conexion TCP a `BOOT:28080`.
- El operador puede demostrar si gossip esta propagando o no con metricas.

### Fase 0 - Congelar diagnostico e instrumentar (1-2 dias)

- [x] Documentar estado actual de red en este archivo.
  - Incluye diagnostico de IBD/API, peerstore, discovery, gossip, BOOT, manifest/deploy y hallazgos operativos 2026-07-02.
- [x] Agregar `/sync/status` con:
  - local height,
  - target height,
  - sync mode,
  - connected peers,
  - last sync error,
  - rpc fallback active,
  - last block received time.
  - extra implementado: `ready_to_mine`, startup phase, lag, gossip block counters.
- [x] Cambiar `/health` para incluir version minima:
  - status,
  - chain id,
  - height,
  - peers,
  - uptime.
- [x] Integrar `HealthChecker` o eliminarlo si no se va a usar.
- [x] Agregar logs estructurados para:
  - seed dial success/failure,
  - block request timeout,
  - block apply reject,
  - rpc sync fallback.

Archivos:

- `src/api/mod.rs`
- `src/health/mod.rs`
- `src/main.rs`

Criterio de aceptacion:

- Un operador puede ver si el nodo esta aislado, atrasado o sincronizado sin leer journal completo.

### Fase 0.5 - Supervisor ZSilent y gating de mineria (2-5 dias)

- [x] Conectar ZSilent a `/sync/status` cuando exista; mientras tanto, derivar estado desde:
  - `/health`,
  - `mine-status --json`,
  - altura local/remota,
  - peers,
  - ultimo bloque recibido.
- [x] Cambiar el modelo de reinicio:
  - no abandonar permanentemente despues de 5 intentos,
  - mantener backoff,
  - mostrar causa visible,
  - permitir recuperacion automatica.
- [x] Bloquear `POST /mining/start` si:
  - RPC local no responde,
  - nodo esta aislado,
  - nodo esta atrasado,
  - cliente/binario esta obsoleto,
  - tip local no coincide con fuentes remotas confiables.
  - Implementado ahora: ZSilent consulta `/sync/status` y no envia el POST si `ready_to_mine=false`; cliente obsoleto/tip invalido deben quedar reflejados por el core como `mode/status=error`.
- [x] Mantener la intencion del usuario:
  - si el usuario activo mineria y el nodo cae, ZSilent debe reparar y reanudar solo cuando sea seguro.
  - Implementado ahora: ZSilent conserva `integratedMiningRequested`, deja mineria en cola con razon visible y reintenta solo cuando `/sync/status.ready_to_mine=true`.
- [x] Agregar mensaje de estado accionable:
  - `actualizando cliente`,
  - `sincronizando`,
  - `sin peers`,
  - `RPC local caido`,
  - `reparando base local`,
  - `listo para minar`.
- [x] Preparar modo `wallet mode`:
  - ZSilent puede enviar/consultar sin exigir full node local.
  - Solo `miner mode` exige nodo local sano.
  - Implementado: `ZSILENT_APP_MODE=wallet|miner` (`miner` conserva comportamiento actual).
  - En `wallet` mode, lecturas de wallet usan `--rpc-only`, el refresh de balance puede usar RPC remoto, no se auto-sincroniza DB local y la mineria queda bloqueada con mensaje accionable.

Archivos:

- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/node/NodeController.kt`
- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/mining/MiningController.kt`
- `zsilent-core-release-1.5.0/src/main/kotlin/com/zsilent/core/core/config/ZokaNetworkConfig.kt`
- `src/api/mod.rs`

Criterio de aceptacion:

- Un usuario de ZSilent entiende por que no puede minar y la app intenta reparar sin requerir que el usuario reinicie manualmente.
- ZSilent nunca inicia mineria sobre un nodo aislado, obsoleto o atrasado.

### Fase 1 - PeerStore y bootstrap real (2-5 dias)

- [x] Crear `src/network/peer_store.rs`.
- [x] Persistir peers conectados al menos cada 60s.
- [x] Cargar peerstore al boot.
- [x] Redial paralelo a:
  - manual seeds,
  - manifest seeds,
  - peerstore peers.
- [x] Agregar jitter/backoff.
- [x] Registrar protocolos soportados recibidos por Identify.
- [x] Usar Kademlia para bootstrap real:
  - agregar direcciones de seeds,
  - llamar bootstrap,
  - guardar peers descubiertos.
- [x] Convertir `PeerManager`/`PeerDiscovery` en codigo productivo o eliminarlos.

Archivos:

- `src/network/p2p.rs`
- `src/network/peer_store.rs`
- `src/network/discovery.rs`
- `src/main.rs`

Criterio de aceptacion:

- Nodo A arranca, se conecta a BOOT, aprende nodo B, se apaga BOOT, reinicia A y reconecta con B sin seed.

### Fase 2 - BlockAnnounce e inventario liviano (3-7 dias)

- [x] Agregar `ZokaMessage::BlockAnnounce`.
- [x] Al minar/aplicar bloque nuevo, publicar anuncio liviano.
- [x] Al recibir anuncio:
  - validar forma,
  - comparar altura/trabajo,
  - pedir headers/bloque si falta.
- [x] Dejar de rebroadcastear bloques completos salvo modo compat.
- [x] Mantener `BlockResponse` legacy durante transicion.

Archivos:

- `src/network/message.rs`
- `src/network/p2p.rs`
- `src/main.rs`
- `src/state/sync.rs`

Criterio de aceptacion:

- Un bloque minado se anuncia en menos de 2s y los peers lo solicitan sin esperar loop de sync.

### Fase 3 - Request/Response para bloques (1-2 semanas)

- [x] Agregar dependencia/feature `libp2p request-response`.
- [x] Crear codec:
  - `GetBlock { height, hash? }`
  - `BlockData { block, private_tx_bodies, difficulty_bits }`
  - `NotFound`
  - `Rejected`
- [x] Reemplazar `request_block_with_retry` basado en gossip por request/response.
- [x] Medir latencia por peer.
- [x] Penalizar timeouts y respuestas invalidas.
- [x] Mantener path gossip legacy solo para compatibilidad durante 1 release.

Implementado ahora:

- `request_block_with_retry` intenta primero `libp2p request-response` directo contra peers conectados.
- `BlockBodyResponse` soporta `Found`, `NotFound` y `Rejected`; los rechazos operativos se penalizan sin confundirse con bloque ausente.
- Se mantienen `BlockRequest`/`BlockResponse` por gossip como fallback legacy durante la transicion.
- Se registran contadores directos, latencia media, respuestas invalidas y score por peer; `/network/runtime` expone las metricas globales de request/response.

Archivos:

- `Cargo.toml`
- `src/network/p2p.rs`
- `src/network/message.rs`
- `src/state/sync.rs`

Criterio de aceptacion:

- Un nodo puede pedir bloque N a peer especifico y obtener respuesta directa con timeout controlado.

### Fase 3.5 - Tx availability para mineros (1-2 semanas)

- [x] Agregar `PrivateTxAnnounce` liviano.
  - Implementado como `ZokaMessage::PrivateTxAnnounce` con `tx_hash`, `body_size_bytes`, `private_proof_profile` y `timestamp`.
  - El fullnode lo valida, lo publica cuando recibe tx privada local/API, y reanuncia disponibilidad al persistir un cuerpo gossipeado.
  - Compatibilidad: se mantiene gossip del cuerpo completo hasta completar fetch directo por request/response.
- [x] Agregar fetch de cuerpo privado por request/response:
  - `GetPrivateTx { tx_hash }`,
  - `PrivateTxBody { tx_hash, body }`,
  - `NotFound`,
  - `RateLimited`.
  - Implementado como `src/network/private_tx_request_response.rs` con protocolo `/zoka/private-tx/reqresp/1`.
  - El nodo sirve cuerpos desde RocksDB, pide cuerpos al recibir `PrivateTxAnnounce` si no los tiene, valida hash/body y marca pending para el minero.
- [x] Cachear cuerpos recientes en relays/mineros por ventana corta.
  - `Node` mantiene cache in-memory de cuerpos privados recientes con TTL de 10 minutos y maximo 2048 entradas.
  - El cache se alimenta al recibir tx local/API, al importar gossip y al completar fetch directo; los requests entrantes se sirven primero desde cache y luego desde RocksDB.
- [x] Permitir que un minero busque cuerpo en multiples peers antes de descartar tx.
  - `request_private_tx_body_with_retry` envia requests directos a varios peers conectados con timeout controlado y acepta la primera respuesta valida.
- [x] Penalizar peers que anuncian tx pero nunca sirven cuerpo.
  - Ya existia `penalize_private_tx_announcers()` para el caso de fallo total. Se agrego penalizacion tambien cuando un peer especifico consultado nunca responde (ni Body ni NotFound ni OutboundFailure) aunque otro peer sirva el cuerpo con exito: `src/network/p2p.rs` (`request_private_tx_body_with_retry`, rama de timeout).
- [x] Medir tx varadas:
  - anunciadas sin cuerpo,
  - cuerpo recibido tarde (>2s),
  - incluidas en bloque,
  - descartadas por expiracion: no implementado — no existe mecanismo de expiracion/TTL para el pool de tx privadas pendientes en este codebase; agregarlo es un cambio de comportamiento sobre mempool en mainnet, fuera de alcance de este pase de metricas.
  - Contadores globales en `NetworkRuntimeState` (`src/api/mod.rs`), expuestos via `GET` del snapshot de red: `private_tx_announced_without_body`, `private_tx_body_received_late`, `private_tx_included_in_block`, `private_tx_rpc_fallback_pulled`.
- [x] Mantener RPC pull solo como fallback y marcarlo en metricas.
  - El pull por RPC ya era NAT-friendly/secundario por diseno (comentario existente en `src/main.rs`); se agrego el contador `private_tx_rpc_fallback_pulled` en ambos call sites (pull de pendientes y backfill de cuerpos committeados).

Archivos:

- `src/network/message.rs`
- `src/network/p2p.rs`
- `src/state/sync.rs`
- `src/api/mod.rs`

Criterio de aceptacion:

- Una tx enviada por un usuario llega al mempool de al menos N mineros/relays aunque un RPC o un nodo de escritorio caiga.
- Un minero puede construir bloque sin depender de un unico RPC para recuperar cuerpos de tx privadas.

### Fase 4 - Headers-first IBD y descarga paralela (2-3 semanas)

- [x] Definir tipo de resumen: se usa `HeaderSummary { header, difficulty_bits, cumulative_work }` en lugar de un `BlockHeaderLite` separado.
- [x] Agregar protocolo `/zoka/headers/1`:
  - `GetHeaders { locator: Vec<String>, max: u32, stop_hash: Option<String> }`
  - `HeadersResponse::Headers { headers: Vec<HeaderSummary>, common_ancestor: String }`
  - `HeadersResponse::NotFound { reason: String }`
- [x] Implementar chain locator Bitcoin-style:
  - tip, saltos exponenciales (paso duplica en cada iteracion), genesis; maximo 32 entradas.
  - `build_chain_locator(storage)` en `src/state/sync.rs`.
- [x] Validar headers:
  - linkage (prev_hash),
  - alturas contiguas,
  - trabajo acumulado creciente.
  - `validate_header_chain(headers, common_ancestor_hash)` en `src/state/sync.rs`.
- [x] Descargar bodies con 4-16 workers en paralelo.
  - `Node::request_blocks_batch_direct` (`src/network/p2p.rs`) dispara un request directo por altura (round-robin entre peers) sin esperar entre uno y otro, y drena las respuestas conforme llegan.
  - `run_ibd_body_prefetch_step` (`src/main.rs`, `IBD_PARALLEL_BODY_WORKERS = 8`) pide una ventana de hasta 8 alturas en paralelo y guarda cada respuesta en el cache existente de `PeerChainView` (`stash_prefetched_block_response`, nuevo helper en `src/state/sync.rs`) sin tocar el camino de aplicacion: `run_sync_step` sigue aplicando de a un bloque, en orden, via `apply_block` sin cambios - el paralelismo es solo en la descarga, no en la aplicacion. Se desactiva durante branch-backfill (esa ruta sigue siendo secuencial y con hash fijado a proposito).
  - Nota: en BOOT/VPS2 este camino queda en segundo plano hoy porque el fallback RPC (`run_rpc_sync_batch`, ya paralelo/batched por HTTP) corre primero cuando hay endpoints RPC configurados; el beneficio es mas visible para nodos que solo tienen P2P.
- [x] Aplicar en orden (manejado por la logica existente de `apply_block_inner`).
- [x] Manejar orphan/side branch por hash: `SyncOutcome::GapAhead`, `MissingParent`, `BehindOrSibling`.

Implementado en el commit `c981fa7`:

- `src/network/header_request_response.rs` — codec con JSON, protocolo `/zoka/headers/1`.
- `src/network/p2p.rs` — `NodeBehaviour` con behaviour de headers; `PendingHeaderRequest`, `ReceivedHeadersResponse`; metodos `take_pending_header_request()`, `respond_header_request()`, `send_header_request()`.
- `src/state/sync.rs` — `build_chain_locator()`, `validate_header_chain()`.
- `src/main.rs` — `headers_response_from_storage()`, `serve_pending_header_requests()`, ambos conectados al event loop.

Archivos:

- `src/network/header_request_response.rs` (nuevo)
- `src/state/sync.rs`
- `src/network/p2p.rs`
- `src/main.rs`

Criterio de aceptacion:

- Un nodo puede pedir al peer los headers a partir de su locator y recibir la cadena diferencial para saber donde diverge.

### Fase 5 - Snapshots firmados y servidos por red (1-2 semanas iniciales)

- [x] Activar `snapshot_signers` en manifest.
  - Se generaron dos claves ed25519 independientes (una por operador/nodo, sin compartir secreto entre servidores): una en BOOT y otra en VPS2, cada una en `/root/.zoka/snapshot-signer.key` (permisos 600, root-only) con su publica en `snapshot-signer.pub`. Ambas publicas quedaron registradas en `docs/mainnet-network.v1.json` -> `snapshot_signers`. Cualquiera de los dos nodos puede firmar snapshots de forma independiente si el otro esta offline.
  - Nota operativa: BOOT y VPS2 corren hoy con `--no-network-config` (config vieja, pre-unificacion de la Fase 6), asi que este manifest recien se lee cuando se redeploye con `zoka-node.service`.
- [x] Publicar snapshot firmado por release/operador.
  - `cmd_snapshot_export` no escribia el sidecar `.snap.manifest` (la funcion `write_manifest_sidecar` existia pero no se llamaba desde ningun lado); se corrigio en `src/main.rs` para que lo escriba automaticamente despues de exportar.
  - Se genero y firmo un snapshot real en BOOT (altura 1092, `/data/zoka/snapshots/mainnet-1092.snap` + `.manifest`) con la clave de BOOT. El binario actualmente desplegado en BOOT es anterior a las rutas HTTP de la Fase 5 (`GET /snapshots` devuelve 404 hoy), asi que el snapshot queda listo en disco pero se sirve por HTTP recien despues del proximo redeploy binario (pendiente, decision del operador).
- [x] Agregar endpoints HTTP:
  - `GET /snapshots` — lista todos los manifests disponibles para el chain_id activo.
  - `GET /snapshots/latest` — retorna el manifest mas alto disponible.
  - `GET /snapshots/:height/download` — descarga el archivo `.snap` directamente.
- [x] Agregar protocolo P2P `/zoka/snapshots/1`:
  - `GetSnapshotManifests { chain_id, max_height? }`
  - `SnapshotManifestsResponse::Manifests { manifests: Vec<SnapshotManifest> }`
  - `SnapshotManifestsResponse::NotAvailable { reason }`
- [x] Agregar `SnapshotFileManifest` como sidecar `.snap.manifest` JSON (evita deserializar el `.snap` para listar disponibles).
- [x] Agregar `scan_snapshot_manifests(dir)` y `write_manifest_sidecar(snapshot, path, ...)` en `src/snapshot.rs`.
- [x] Servir inbound `GetSnapshotManifests` desde el event loop del fullnode.
- [x] Importar snapshot y luego continuar con headers/bodies.
  - En `cmd_fullnode_auto` (`src/main.rs`), justo despues de abrir la DB y antes de cargar el `StateManager`: si hay un snapshot local (`scan_snapshot_manifests` sobre `default_snapshot_dir()`) para el `chain_id` activo con `head_height` al menos 100 bloques por delante de la altura local, se verifica sha256 contra el manifest, se verifica la firma contra cualquiera de las claves en `snapshot_signers` del network-config (archivo o embebido), y si valida se importa via `import_into_storage` (cada bloque sigue pasando por `apply_block`, mismas reglas de consenso que la red). Sin snapshot local, sin signers configurados, o sin firma valida: no hace nada y sigue el flujo normal de headers/bodies. Solo lee un archivo ya presente en disco, no descarga nada de la red.
- [ ] En fase posterior, evolucionar de snapshot de bloques a snapshot de estado verificable.

Implementado en el commit `082e5ca`:

- `src/network/snapshot_request_response.rs` (nuevo) — codec con JSON, protocolo `/zoka/snapshots/1`.
- `src/network/p2p.rs` — behaviour de snapshots; `PendingSnapshotManifestRequest`, `ReceivedSnapshotManifests`; metodos `take_pending_snapshot_manifest_request()`, `respond_snapshot_manifest_request()`, `send_snapshot_manifest_request()`.
- `src/snapshot.rs` — `SnapshotFileManifest`, `manifest_path_for()`, `write_manifest_sidecar()`, `scan_snapshot_manifests()`, `default_snapshot_dir()`.
- `src/api/mod.rs` — tres nuevos endpoints bajo `/snapshots`.
- `src/state/mod.rs` — `StateManager::chain_id()`.
- `src/main.rs` — `serve_pending_snapshot_manifest_requests()` conectado al event loop.

Archivos:

- `src/network/snapshot_request_response.rs` (nuevo)
- `src/snapshot.rs`
- `src/api/mod.rs`
- `src/state/mod.rs`
- `src/network/p2p.rs`
- `src/main.rs`

Criterio de aceptacion:

- Nodo nuevo puede pedir manifests a peers por P2P y descargar el snapshot via HTTP; la cadena completa no es necesaria para bootstrappear.

### Fase 6 - Operacion y descentralizacion (continuo)

- [x] Unificar `zoka-bootstrap.service` y `zoka-rpc.service` en `zoka-node.service`.
  - Deploy detiene y deshabilita los nombres legacy antes de crear el nuevo.
- [x] Configurar roles por flags en el ExecStart del servicio unificado (bootstrap vs rpc/full).
- [x] Quitar `--no-network-config` del deploy; ambos nodos usan `--network-config /opt/zoka/mainnet-network.v1.json`.
  - El workflow copia `docs/mainnet-network.v1.json` al servidor durante cada deploy.
- [ ] Agregar minimo 3 seeds de distinta region/operador (pendiente: no hay tercer servidor disponible aun).
  - Los 2 seeds actuales (159.223.119.216, 157.230.5.103) estan ambos en NYC/DigitalOcean.
- [x] Mantener RPCs como lectura/fallback: ningun seed/seed_node es requisito de consenso; el nodo funciona con `--allow-isolated-start`.
- [x] Watchdog con multiples fuentes de referencia:
  - `BOOT_RPC` acepta lista CSV; intenta cada endpoint en orden hasta que uno responda.
  - Watchdog timer se instala en el RPC node via deploy.
  - Referencia ambos seeds para no depender de BOOT como unica autoridad.

Implementado en el commit `91373af`:

- `.github/workflows/deploy-mainnet.yml` — servicio unificado, copia de network config y watchdog, stop/disable de nombres legacy.
- `deploy/systemd/zoka-node.service` (nuevo) — plantilla de referencia del servicio unificado.
- `deploy/systemd/zoka-sync-watchdog.service` — actualizado `After=` a `zoka-node.service`, `BOOT_RPC` con ambos seeds.
- `scripts/zoka-sync-watchdog.sh` — soporte de `BOOT_RPC` CSV con fallback entre seeds.

Archivos:

- `.github/workflows/deploy-mainnet.yml`
- `deploy/systemd/zoka-node.service` (nuevo)
- `deploy/systemd/zoka-sync-watchdog.service`
- `scripts/zoka-sync-watchdog.sh`

Criterio de aceptacion:

- BOOT apagado durante 30 minutos: red sigue minando/sincronizando entre peers restantes.
- El watchdog puede alcanzar al menos un seed de referencia cuando BOOT cae.

### Fase 7 - Simulacion multi-nodo y caos (1-2 semanas)

- [ ] Crear harness local de 5 nodos.
- [ ] Pruebas:
  - seed down,
  - RPC down,
  - peer lento,
  - mensajes invalidos,
  - fork corta,
  - reorg controlada,
  - miner NAT,
  - restart con peerstore.
- [ ] Medir:
  - tiempo hasta peer >= 1,
  - tiempo IBD,
  - propagacion de bloque,
  - bloques minados sobre stale tip,
  - uso de CPU/mem/red.

Archivos:

- `tests/`
- `scripts/`
- `.github/workflows/`

Criterio de aceptacion:

- Ningun cambio de red se mergea sin pasar simulacion multi-nodo.

### Fase 7.5 - Robustez de peer discovery y gossip (previo a v1.6.0)

Auditoria puntual del descubrimiento de pares (Kademlia + PeerStore) y gossip
(BlockAnnounce, gossipsub), hecha con evidencia de logs reales de produccion
la noche del 2026-07-02. Veredicto: nada roto, 5 puntos de deuda tecnica
concretos, cerrados todos antes de cortar v1.6.0 (agrupados con el bump de
`PROTOCOL_FREEZE_ID` de esa version).

- [x] Re-bootstrap periodico de Kademlia (antes solo se disparaba al arrancar
  y, condicionalmente, cuando el peerstore observaba direcciones nuevas).
  Timer independiente cada 15 min en `src/main.rs`.
- [x] Poda del PeerStore (`src/network/peer_store.rs::prune()`): cap de 500
  entradas, descarta fallos viejos (14+ dias, 10+ fallos) salvo que haya
  habido conexion exitosa reciente (30 dias de gracia).
- [x] Gossipsub sintonizado al tamano real de la red (`mesh_n=4,
  mesh_n_low=2, mesh_n_high=8, mesh_outbound_min=1` en vez de los defaults de
  libp2p pensados para 50+ peers).
- [x] Reintento con backoff para `BlockAnnounce`: antes se pedia el cuerpo
  del bloque anunciado una sola vez por gossip, sin reintento si se perdia.
  Ahora hay barrido cada 2s (`sweep_block_announce_retries` en
  `src/main.rs`) con backoff 2s/4s/8s, hasta 3 intentos via
  `request_block_with_retry` (directo + fallback gossip), auto-limpieza si
  el bloque ya llego por otra via.
- [x] Metrica de tamano real del mesh de gossip (`GossipStats.mesh_peer_count`,
  via `gossipsub::mesh_peers()` del topic principal, no `all_peers()` que
  puede verse sano con el mesh vacio) - expuesta en el snapshot de red.

Excluido a proposito: backoff por IP en conexiones P2P entrantes fallidas
("Failed to negotiate transport protocol(s)", visto en logs de BOOT). La
mitigacion correcta es rate-limiting a nivel de firewall/iptables en el
droplet al momento del deploy, no un `NetworkBehaviour` custom nuevo en
libp2p (cambio mas grande y riesgoso para el beneficio que da).

Verificado con `cargo check --bins` tras cada item y un arranque real
aislado (`--allow-isolated-start`, red/DB descartables) que llego a estado
`RUNNING` sin panics. Local unicamente - no desplegado a BOOT/VPS2 ni
publicado, sigue el mismo patron de esta sesion.

## 6. Tabla de prioridades

| Prioridad | Trabajo | Impacto | Riesgo |
|---|---|---|---|
| P0 | Abrir/verificar `28080/tcp` publico en BOOT | Mineros externos pueden bootstrappear | Bajo/operacional |
| P0 | API-first startup | No hay timeouts durante carga de RocksDB | Medio |
| P0 | Carga RocksDB en worker/lazy hydrate | Tokio no queda bloqueado por IBD/hydrate | Medio |
| P0 | PeerStore minimo viable | Reinicio no olvida la red | Medio |
| P0 | Telemetria gossip real | Se confirma si bloques viajan o no por gossip | Bajo |
| P0 | `/sync/status` real | Operacion deja de estar ciega | Bajo |
| P0 | Supervisor ZSilent + gating de mineria | Usuarios dejan de minar sobre nodos rotos y reciben reparacion automatica | Medio |
| P0 | Version minima obligatoria en ZSilent/nodo | Evita mineros obsoletos rompiendo sync/mineria | Bajo/medio |
| P0 | PeerStore persistente | Sobrevive caida/reinicio de seeds | Medio |
| P0 | Quitar `--no-network-config` del deploy core | Manifiesto pasa a ser verdad operacional | Bajo |
| P1 | BlockAnnounce | Propagacion mas rapida, menos stale mining | Medio |
| P1 | Request/response de bloques | Sync P2P confiable | Medio |
| P1 | Tx availability mesh | Transacciones llegan a mineros sin RPC unico | Medio/alto |
| P1 | Peer scoring/quarantine | Menos peers malos/timeouts | Medio |
| P2 | Headers-first IBD | Escala de verdad | Alto |
| P2 | Snapshots firmados | Onboarding rapido | Medio |
| P2 | 3+ seeds multi-region | Descentralizacion minima | Bajo/operacional |
| P3 | State snapshot verificable | Salto grande de performance | Alto |

## 7. Decisiones que NO conviene tomar ahora

- No migrar inmediatamente a CometBFT.
  - Motivo: cambia el modelo PoW abierto por set validador/BFT.
- No migrar inmediatamente a Substrate.
  - Motivo: resuelve muchas cosas, pero exige reescritura masiva.
- No cambiar RandomX ahora.
  - Motivo: no resuelve sync/conexion.
- No cambiar formato de bloque hasta estabilizar red.
  - Motivo: puede requerir hard fork.
- No depender de `/sync/block` como path primario.
  - Motivo: centraliza la red en RPC.

## 8. Definicion de exito

La nueva red se considera superior cuando cumple:

- Durante arranque pesado, `/health` responde y `/ready` explica que todavia no esta listo.
- Los seeds publicos aceptan conexiones P2P externas en `28080/tcp`.
- Un nodo conserva peers entre reinicios.
- Un nodo nuevo puede sincronizar sin RPC, solo P2P.
- Un nodo nuevo puede arrancar rapido con snapshot firmado + delta sync.
- BOOT puede caer y los nodos no quedan en `connected_peers = 0`.
- La propagacion de bloque ocurre por anuncio liviano + fetch directo.
- Se puede medir cuantos bloques llegaron por gossip y cuantos por sync tick/RPC.
- El operador ve lag, peers, errores y modo de sync por API.
- Los mineros no minan si estan atrasados o aislados.
- ZSilent repara/reintenta nodo local sin abandonar al usuario despues de pocos fallos.
- ZSilent muestra razon exacta cuando la mineria esta pausada.
- Una tx de usuario llega a multiples mineros/relays sin depender de un unico RPC.
- RPC es fallback, no autoridad.
- Hay tests multi-nodo antes de cada release.

## 9. Referencias tecnologicas consultadas

- libp2p protocolos y negociacion: https://libp2p.io/docs/protocols/
- rust-libp2p gossipsub: https://libp2p.github.io/rust-libp2p/libp2p_gossipsub/index.html
- rust-libp2p request-response: https://docs.rs/libp2p-request-response/latest/libp2p_request_response/
- CometBFT state sync: https://docs.cometbft.com/v0.38/spec/p2p/legacy-docs/messages/state-sync
- CometBFT proyecto: https://github.com/cometbft/cometbft
- Polkadot/Substrate consenso BABE/GRANDPA como referencia de separacion produccion/finalidad: https://docs.polkadot.com/reference/polkadot-hub/consensus-and-security/pos-consensus/

## 10. Proxima accion recomendada

Orden concreto para empezar sin romper mainnet:

1. Abrir y verificar `28080/tcp` publico en BOOT/seeds.
2. Cambiar arranque a API-first para que `/health` responda durante carga.
3. Mover hidratacion pesada de RocksDB fuera del camino que bloquea Tokio.
4. Agregar peerstore minimo viable y redial desde peerstore.
5. Instrumentar gossip y demostrar si hay `gossip_rx_blocks`.
6. Implementar `/sync/status` y health real.
7. Conectar ZSilent a ese estado y bloquear mineria si el nodo no esta listo.
8. Cambiar el supervisor de ZSilent para reparar/reintentar sin abandonar al usuario.
9. Activar redial a peerstore + seeds del manifest.
10. Agregar `BlockAnnounce`.
11. Agregar request/response para `GetBlock`.
12. Agregar disponibilidad de tx privadas por announce/fetch.
13. Recien despues, headers-first IBD.

Esto da mejora inmediata de estabilidad sin hard fork y prepara el terreno para el salto grande.
